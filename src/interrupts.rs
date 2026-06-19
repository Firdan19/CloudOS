use crate::keyboard::KeyEvent;
use crate::{gdt, keyboard, paging, paniclog, serial, stats, vga};
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::interrupts as cpu_interrupts;
use x86_64::instructions::port::Port;
use x86_64::instructions::tables::lidt;
use x86_64::structures::DescriptorTablePointer;
use x86_64::VirtAddr;

const IDT_ENTRIES: usize = 256;
const INTERRUPT_GATE: u16 = 0x8e00;

const PIC_1_COMMAND: u16 = 0x20;
const PIC_1_DATA: u16 = 0x21;
const PIC_2_COMMAND: u16 = 0xa0;
const PIC_2_DATA: u16 = 0xa1;
const PIC_EOI: u8 = 0x20;

const PIC_1_OFFSET: u8 = 32;
const PIC_2_OFFSET: u8 = 40;
const TIMER_IRQ: u8 = 0;
const KEYBOARD_IRQ: u8 = 1;
const TIMER_VECTOR: usize = (PIC_1_OFFSET + TIMER_IRQ) as usize;
const KEYBOARD_VECTOR: usize = (PIC_1_OFFSET + KEYBOARD_IRQ) as usize;

const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_CHANNEL_0_PORT: u16 = 0x40;
const PIT_MODE_3_BINARY: u8 = 0x36;
const PIT_DIVISOR_18HZ: u16 = 65535;

unsafe extern "C" {
    fn timer_interrupt_stub();
    fn keyboard_interrupt_stub();
    fn default_irq_stub();
    fn default_interrupt_stub();
    fn exception_00_divide_error_stub();
    fn exception_01_debug_stub();
    fn exception_02_nmi_stub();
    fn exception_03_breakpoint_stub();
    fn exception_04_overflow_stub();
    fn exception_05_bound_range_stub();
    fn exception_06_invalid_opcode_stub();
    fn exception_07_device_not_available_stub();
    fn exception_08_double_fault_stub();
    fn exception_10_invalid_tss_stub();
    fn exception_11_segment_not_present_stub();
    fn exception_12_stack_segment_fault_stub();
    fn exception_13_general_protection_fault_stub();
    fn exception_14_page_fault_stub();
    fn exception_16_x87_floating_point_stub();
    fn exception_17_alignment_check_stub();
    fn exception_18_machine_check_stub();
    fn exception_19_simd_floating_point_stub();
    fn exception_20_virtualization_stub();
    fn exception_21_control_protection_stub();
    fn exception_28_hypervisor_injection_stub();
    fn exception_29_vmm_communication_stub();
    fn exception_30_security_stub();
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ExceptionContext {
    vector: u64,
    error_code: u64,
    instruction_pointer: u64,
    code_segment: u64,
    cpu_flags: u64,
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    options: u16,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

const _: [(); 16] = [(); size_of::<IdtEntry>()];

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            options: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set_handler(&mut self, handler: unsafe extern "C" fn()) {
        self.set_handler_with_ist(handler, 0);
    }

    fn set_handler_with_ist(&mut self, handler: unsafe extern "C" fn(), ist_index: u16) {
        let address = handler as usize as u64;

        self.offset_low = address as u16;
        self.selector = gdt::KERNEL_CODE_SELECTOR;
        self.options = INTERRUPT_GATE | (ist_index & 0x0007);
        self.offset_mid = (address >> 16) as u16;
        self.offset_high = (address >> 32) as u32;
        self.reserved = 0;
    }
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::missing(); IDT_ENTRIES];
static PIT_TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    cpu_interrupts::disable();

    unsafe {
        init_idt();
        remap_pic();
        init_pit();
    }

    cpu_interrupts::enable();
    serial::log("irq", "idt, pic, pit ready");
}

pub fn pop_key_event() -> Option<KeyEvent> {
    keyboard::pop_event()
}

pub fn poll_keyboard() {
    keyboard::poll();
}

pub fn ticks() -> u64 {
    PIT_TICKS.load(Ordering::Acquire)
}

unsafe fn init_idt() {
    let idt = core::ptr::addr_of_mut!(IDT).cast::<IdtEntry>();

    for index in 0..IDT_ENTRIES {
        unsafe {
            (*idt.add(index)).set_handler(default_interrupt_stub);
        }
    }

    unsafe {
        (*idt.add(0)).set_handler(exception_00_divide_error_stub);
        (*idt.add(1)).set_handler(exception_01_debug_stub);
        (*idt.add(2)).set_handler(exception_02_nmi_stub);
        (*idt.add(3)).set_handler(exception_03_breakpoint_stub);
        (*idt.add(4)).set_handler(exception_04_overflow_stub);
        (*idt.add(5)).set_handler(exception_05_bound_range_stub);
        (*idt.add(6)).set_handler(exception_06_invalid_opcode_stub);
        (*idt.add(7)).set_handler(exception_07_device_not_available_stub);
        (*idt.add(8))
            .set_handler_with_ist(exception_08_double_fault_stub, gdt::DOUBLE_FAULT_IST_INDEX);
        (*idt.add(10)).set_handler(exception_10_invalid_tss_stub);
        (*idt.add(11)).set_handler(exception_11_segment_not_present_stub);
        (*idt.add(12)).set_handler(exception_12_stack_segment_fault_stub);
        (*idt.add(13)).set_handler(exception_13_general_protection_fault_stub);
        (*idt.add(14)).set_handler(exception_14_page_fault_stub);
        (*idt.add(16)).set_handler(exception_16_x87_floating_point_stub);
        (*idt.add(17)).set_handler(exception_17_alignment_check_stub);
        (*idt.add(18)).set_handler(exception_18_machine_check_stub);
        (*idt.add(19)).set_handler(exception_19_simd_floating_point_stub);
        (*idt.add(20)).set_handler(exception_20_virtualization_stub);
        (*idt.add(21)).set_handler(exception_21_control_protection_stub);
        (*idt.add(28)).set_handler(exception_28_hypervisor_injection_stub);
        (*idt.add(29)).set_handler(exception_29_vmm_communication_stub);
        (*idt.add(30)).set_handler(exception_30_security_stub);
    }

    for vector in PIC_1_OFFSET as usize..(PIC_2_OFFSET as usize + 8) {
        unsafe {
            (*idt.add(vector)).set_handler(default_irq_stub);
        }
    }

    unsafe {
        (*idt.add(TIMER_VECTOR)).set_handler(timer_interrupt_stub);
        (*idt.add(KEYBOARD_VECTOR)).set_handler(keyboard_interrupt_stub);
    }

    let idt_ptr = DescriptorTablePointer {
        limit: (size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: VirtAddr::from_ptr(core::ptr::addr_of!(IDT)),
    };

    unsafe {
        lidt(&idt_ptr);
    }
}

unsafe fn remap_pic() {
    let mut pic1_command = Port::<u8>::new(PIC_1_COMMAND);
    let mut pic1_data = Port::<u8>::new(PIC_1_DATA);
    let mut pic2_command = Port::<u8>::new(PIC_2_COMMAND);
    let mut pic2_data = Port::<u8>::new(PIC_2_DATA);

    unsafe {
        pic1_command.write(0x11);
        io_wait();
        pic2_command.write(0x11);
        io_wait();

        pic1_data.write(PIC_1_OFFSET);
        io_wait();
        pic2_data.write(PIC_2_OFFSET);
        io_wait();

        pic1_data.write(4);
        io_wait();
        pic2_data.write(2);
        io_wait();

        pic1_data.write(0x01);
        io_wait();
        pic2_data.write(0x01);
        io_wait();

        pic1_data.write(0b1111_1100);
        pic2_data.write(0xff);
    }
}

unsafe fn init_pit() {
    let mut command = Port::<u8>::new(PIT_COMMAND_PORT);
    let mut channel_0 = Port::<u8>::new(PIT_CHANNEL_0_PORT);
    let divisor = PIT_DIVISOR_18HZ;

    unsafe {
        command.write(PIT_MODE_3_BINARY);
        channel_0.write((divisor & 0x00ff) as u8);
        channel_0.write((divisor >> 8) as u8);
    }
}

unsafe fn io_wait() {
    let mut wait_port = Port::<u8>::new(0x80);
    unsafe {
        wait_port.write(0);
    }
}

unsafe fn send_eoi(irq: u8) {
    if irq >= 8 {
        let mut slave_command = Port::<u8>::new(PIC_2_COMMAND);
        unsafe {
            slave_command.write(PIC_EOI);
        }
    }

    let mut master_command = Port::<u8>::new(PIC_1_COMMAND);
    unsafe {
        master_command.write(PIC_EOI);
    }
}

#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    stats::inc_timer_irq();
    PIT_TICKS.fetch_add(1, Ordering::Relaxed);
    vga::toggle_cursor();

    unsafe {
        send_eoi(TIMER_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn keyboard_interrupt_handler() {
    stats::inc_keyboard_irq();
    keyboard::handle_interrupt();

    unsafe {
        send_eoi(KEYBOARD_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn default_irq_handler() {
    stats::inc_default_irq();
    unsafe {
        send_eoi(TIMER_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn exception_dispatch_handler(context: *const ExceptionContext) -> ! {
    stats::inc_exception();
    cpu_interrupts::disable();

    let context = unsafe { *context };
    let fault_address = if context.vector == 14 { read_cr2() } else { 0 };
    let name = exception_name(context.vector);

    paniclog::record_exception(
        name,
        context.vector,
        context.error_code,
        context.instruction_pointer,
        fault_address,
        context.cpu_flags,
    );

    serial::log("panic", "CPU exception captured");
    serial::log("panic", name);
    serial::log_u64("panic", "exception vector", context.vector);
    serial::log_hex_u64("panic", "error code", context.error_code);
    serial::log_hex_u64("panic", "rip", context.instruction_pointer);
    serial::log_hex_u64("panic", "cs", context.code_segment);
    serial::log_hex_u64("panic", "rflags", context.cpu_flags);

    if context.vector == 14 {
        serial::log_hex_u64("panic", "cr2", fault_address);
        serial::log(
            "panic",
            paging::fault_policy(fault_address, context.error_code),
        );
        log_page_fault_bits(context.error_code);
    }

    vga::show_exception_screen(
        name,
        context.vector,
        context.error_code,
        context.instruction_pointer,
        fault_address,
        context.cpu_flags,
    );
    serial::log("panic", "exception screen rendered");

    loop {
        x86_64::instructions::hlt();
    }
}

fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "Divide Error",
        1 => "Debug Exception",
        2 => "Non-Maskable Interrupt",
        3 => "Breakpoint",
        4 => "Overflow",
        5 => "BOUND Range Exceeded",
        6 => "Invalid Opcode",
        7 => "Device Not Available",
        8 => "Double Fault",
        10 => "Invalid TSS",
        11 => "Segment Not Present",
        12 => "Stack Segment Fault",
        13 => "General Protection Fault",
        14 => "Page Fault",
        16 => "x87 Floating-Point Exception",
        17 => "Alignment Check",
        18 => "Machine Check",
        19 => "SIMD Floating-Point Exception",
        20 => "Virtualization Exception",
        21 => "Control Protection Exception",
        28 => "Hypervisor Injection Exception",
        29 => "VMM Communication Exception",
        30 => "Security Exception",
        254 => "CPU Exception With Error Code",
        255 => "Unknown CPU Interrupt",
        _ => "CPU Exception",
    }
}

fn log_page_fault_bits(error_code: u64) {
    serial::log_bool("panic", "page present", error_code & (1 << 0) != 0);
    serial::log_bool("panic", "page write", error_code & (1 << 1) != 0);
    serial::log_bool("panic", "page user", error_code & (1 << 2) != 0);
    serial::log_bool("panic", "page reserved", error_code & (1 << 3) != 0);
    serial::log_bool("panic", "page instruction", error_code & (1 << 4) != 0);
}

fn read_cr2() -> u64 {
    let value: u64;

    unsafe {
        core::arch::asm!(
            "mov {}, cr2",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }

    value
}
