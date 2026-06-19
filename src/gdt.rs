use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const TSS_SELECTOR: u16 = 0x18;
pub const DOUBLE_FAULT_IST_INDEX: u16 = 1;

const GDT_ENTRY_COUNT: usize = 5;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const KERNEL_CODE_DESCRIPTOR: u64 = 0x00af_9a00_0000_ffff;
const KERNEL_DATA_DESCRIPTOR: u64 = 0x00cf_9200_0000_ffff;
const TSS_AVAILABLE_DESCRIPTOR: u64 = 0x89;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub loaded: bool,
    pub code_selector: u16,
    pub data_selector: u16,
    pub tss_selector: u16,
    pub double_fault_ist_index: u16,
    pub gdt_base: u64,
    pub gdt_limit: u16,
    pub tss_base: u64,
    pub tss_limit: u16,
    pub double_fault_stack_top: u64,
    pub double_fault_stack_bytes: u64,
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct TablePointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct TaskStateSegment {
    reserved_1: u32,
    privilege_stack_table: [u64; 3],
    reserved_2: u64,
    interrupt_stack_table: [u64; 7],
    reserved_3: u64,
    reserved_4: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved_1: 0,
            privilege_stack_table: [0; 3],
            reserved_2: 0,
            interrupt_stack_table: [0; 7],
            reserved_3: 0,
            reserved_4: 0,
            iomap_base: size_of::<TaskStateSegment>() as u16,
        }
    }
}

#[repr(C, align(16))]
struct InterruptStack {
    bytes: [u8; DOUBLE_FAULT_STACK_SIZE],
}

static GDT_LOADED: AtomicBool = AtomicBool::new(false);
static mut GDT: [u64; GDT_ENTRY_COUNT] = [0; GDT_ENTRY_COUNT];
static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut DOUBLE_FAULT_STACK: InterruptStack = InterruptStack {
    bytes: [0; DOUBLE_FAULT_STACK_SIZE],
};

pub fn init() -> Snapshot {
    let tss_base = tss_base();
    let tss_limit = (size_of::<TaskStateSegment>() - 1) as u64;
    let (tss_low, tss_high) = tss_descriptor(tss_base, tss_limit);

    unsafe {
        let tss = core::ptr::addr_of_mut!(TSS);
        (*tss).interrupt_stack_table[(DOUBLE_FAULT_IST_INDEX - 1) as usize] =
            double_fault_stack_top();
        (*tss).iomap_base = size_of::<TaskStateSegment>() as u16;

        let gdt = core::ptr::addr_of_mut!(GDT).cast::<u64>();
        *gdt.add(0) = 0;
        *gdt.add(1) = KERNEL_CODE_DESCRIPTOR;
        *gdt.add(2) = KERNEL_DATA_DESCRIPTOR;
        *gdt.add(3) = tss_low;
        *gdt.add(4) = tss_high;

        load_gdt();
        load_data_segments();
        load_tss();
    }

    GDT_LOADED.store(true, Ordering::Release);
    snapshot()
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        loaded: GDT_LOADED.load(Ordering::Acquire),
        code_selector: KERNEL_CODE_SELECTOR,
        data_selector: KERNEL_DATA_SELECTOR,
        tss_selector: TSS_SELECTOR,
        double_fault_ist_index: DOUBLE_FAULT_IST_INDEX,
        gdt_base: gdt_base(),
        gdt_limit: gdt_limit(),
        tss_base: tss_base(),
        tss_limit: (size_of::<TaskStateSegment>() - 1) as u16,
        double_fault_stack_top: double_fault_stack_top(),
        double_fault_stack_bytes: DOUBLE_FAULT_STACK_SIZE as u64,
    }
}

fn tss_descriptor(base: u64, limit: u64) -> (u64, u64) {
    let low = (limit & 0xffff)
        | ((base & 0x00ff_ffff) << 16)
        | (TSS_AVAILABLE_DESCRIPTOR << 40)
        | (((limit >> 16) & 0x0f) << 48)
        | (((base >> 24) & 0xff) << 56);
    let high = (base >> 32) & 0xffff_ffff;

    (low, high)
}

unsafe fn load_gdt() {
    let pointer = TablePointer {
        limit: gdt_limit(),
        base: gdt_base(),
    };

    unsafe {
        core::arch::asm!(
            "lgdt [{gdt_pointer}]",
            gdt_pointer = in(reg) &pointer,
            options(readonly, nostack, preserves_flags)
        );
    }
}

unsafe fn load_data_segments() {
    unsafe {
        core::arch::asm!(
            "mov ax, {data_selector}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov fs, ax",
            "mov gs, ax",
            data_selector = const KERNEL_DATA_SELECTOR,
            out("ax") _,
            options(nostack, preserves_flags)
        );
    }
}

unsafe fn load_tss() {
    unsafe {
        core::arch::asm!(
            "mov ax, {tss_selector}",
            "ltr ax",
            tss_selector = const TSS_SELECTOR,
            out("ax") _,
            options(nostack, preserves_flags)
        );
    }
}

fn gdt_base() -> u64 {
    unsafe { core::ptr::addr_of!(GDT) as u64 }
}

fn gdt_limit() -> u16 {
    (size_of::<[u64; GDT_ENTRY_COUNT]>() - 1) as u16
}

fn tss_base() -> u64 {
    unsafe { core::ptr::addr_of!(TSS) as u64 }
}

fn double_fault_stack_top() -> u64 {
    unsafe { core::ptr::addr_of!(DOUBLE_FAULT_STACK) as u64 + DOUBLE_FAULT_STACK_SIZE as u64 }
}
