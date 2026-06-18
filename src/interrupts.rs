use crate::keyboard::KeyEvent;
use crate::{keyboard, serial, vga};
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::interrupts as cpu_interrupts;
use x86_64::instructions::port::Port;
use x86_64::instructions::tables::lidt;
use x86_64::structures::DescriptorTablePointer;
use x86_64::VirtAddr;

const IDT_ENTRIES: usize = 256;
const CODE_SELECTOR: u16 = 0x08;
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
    fn default_exception_with_error_stub();
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
        let address = handler as usize as u64;

        self.offset_low = address as u16;
        self.selector = CODE_SELECTOR;
        self.options = INTERRUPT_GATE;
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
    serial::serial_println("[irq] idt, pic, pit ready");
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

    for vector in [8usize, 10, 11, 12, 13, 14, 17, 21, 29, 30] {
        unsafe {
            (*idt.add(vector)).set_handler(default_exception_with_error_stub);
        }
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
    PIT_TICKS.fetch_add(1, Ordering::Relaxed);
    vga::toggle_cursor();

    unsafe {
        send_eoi(TIMER_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn keyboard_interrupt_handler() {
    keyboard::handle_interrupt();

    unsafe {
        send_eoi(KEYBOARD_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn default_irq_handler() {
    unsafe {
        send_eoi(TIMER_IRQ);
    }
}

#[no_mangle]
pub extern "C" fn exception_handler() {
    cpu_interrupts::disable();
    serial::serial_println("[panic] CPU fault captured");
    vga::write_string("\nCPU fault captured. System halted.");
    loop {
        x86_64::instructions::hlt();
    }
}
