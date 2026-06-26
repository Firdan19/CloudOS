use crate::{gdt, interrupts, paging, serial, stats, syscall};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::instructions::interrupts as cpu_interrupts;

const USER_CODE_BYTES: [u8; 71] = [
    0x48, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xcd, 0x80, 0x48, 0xb8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8,
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xcd, 0x80, 0xf4, 0xeb, 0xfd,
];

const USER_FAULT_CODE_OFFSET: u64 = 128;
const USER_FAULT_ADDRESS: u64 = paging::USER_SPACE_BASE + 0x3000;
pub const USER_FAULT_EXIT_BASE: u64 = 0x1000;
pub const PROBE_EXIT_CODE: u64 = 42;

#[repr(C, align(4096))]
struct Page {
    bytes: [u8; paging::PAGE_SIZE_4K as usize],
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub code_mapped: bool,
    pub stack_mapped: bool,
    pub syscall_gate_ready: bool,
    pub code_selector: u16,
    pub data_selector: u16,
    pub syscall_vector: u64,
    pub code_virtual: u64,
    pub stack_top: u64,
    pub run_count: u64,
    pub pass_count: u64,
    pub syscall_count: u64,
    pub fault_count: u64,
    pub last_fault_vector: u64,
    pub last_fault_address: u64,
    pub last_fault_exit_code: u64,
    pub last_exit_code: u64,
    pub last_uptime_return: u64,
}

#[derive(Clone, Copy)]
pub struct ProbeResult {
    pub ran: bool,
    pub passed: bool,
    pub exit_code: u64,
    pub syscalls_before: u64,
    pub syscalls_after: u64,
}

#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

static USER_READY: AtomicBool = AtomicBool::new(false);
static USER_CODE_MAPPED: AtomicBool = AtomicBool::new(false);
static USER_STACK_MAPPED: AtomicBool = AtomicBool::new(false);
static USER_RUNS: AtomicU64 = AtomicU64::new(0);
static USER_PASSES: AtomicU64 = AtomicU64::new(0);
static USER_SYSCALLS: AtomicU64 = AtomicU64::new(0);
static USER_FAULTS: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_VECTOR: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_EXIT_CODE: AtomicU64 = AtomicU64::new(0);
static LAST_EXIT_CODE: AtomicU64 = AtomicU64::new(0);
static LAST_UPTIME_RETURN: AtomicU64 = AtomicU64::new(0);

static mut USER_CODE_PAGE: Page = Page {
    bytes: [0; paging::PAGE_SIZE_4K as usize],
};
static mut USER_STACK_PAGE: Page = Page {
    bytes: [0; paging::PAGE_SIZE_4K as usize],
};

unsafe extern "C" {
    fn user_enter(entry: u64, stack_top: u64, data_selector: u64, code_selector: u64) -> u64;
    fn user_return_to_kernel(exit_code: u64) -> !;
}

pub fn init() -> Snapshot {
    write_probe_program();

    let code_mapped = map_probe_page(
        paging::USER_PROBE_CODE_PAGE,
        page_address(core::ptr::addr_of!(USER_CODE_PAGE)),
    );
    let stack_mapped = map_probe_page(
        paging::USER_PROBE_STACK_PAGE,
        page_address(core::ptr::addr_of!(USER_STACK_PAGE)),
    );

    USER_CODE_MAPPED.store(code_mapped, Ordering::Release);
    USER_STACK_MAPPED.store(stack_mapped, Ordering::Release);
    USER_READY.store(code_mapped && stack_mapped, Ordering::Release);

    let snapshot = snapshot();
    let code = paging::translate(paging::USER_PROBE_CODE_PAGE);
    let stack = paging::translate(paging::USER_PROBE_STACK_PAGE);
    serial::log_bool("user", "ring3 pages", snapshot.initialized);
    serial::log_bool(
        "user",
        "code user page",
        code.mapped && code.user_accessible,
    );
    serial::log_bool(
        "user",
        "stack user page",
        stack.mapped && stack.user_accessible,
    );
    serial::log_hex_u64("user", "code virt", snapshot.code_virtual);
    serial::log_hex_u64("user", "stack top", snapshot.stack_top);

    snapshot
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        initialized: USER_READY.load(Ordering::Acquire),
        code_mapped: USER_CODE_MAPPED.load(Ordering::Acquire),
        stack_mapped: USER_STACK_MAPPED.load(Ordering::Acquire),
        syscall_gate_ready: interrupts::syscall_gate_ready(),
        code_selector: gdt::USER_CODE_SELECTOR,
        data_selector: gdt::USER_DATA_SELECTOR,
        syscall_vector: interrupts::SYSCALL_VECTOR as u64,
        code_virtual: paging::USER_PROBE_CODE_PAGE,
        stack_top: paging::USER_PROBE_STACK_TOP,
        run_count: USER_RUNS.load(Ordering::Acquire),
        pass_count: USER_PASSES.load(Ordering::Acquire),
        syscall_count: USER_SYSCALLS.load(Ordering::Acquire),
        fault_count: USER_FAULTS.load(Ordering::Acquire),
        last_fault_vector: LAST_FAULT_VECTOR.load(Ordering::Acquire),
        last_fault_address: LAST_FAULT_ADDRESS.load(Ordering::Acquire),
        last_fault_exit_code: LAST_FAULT_EXIT_CODE.load(Ordering::Acquire),
        last_exit_code: LAST_EXIT_CODE.load(Ordering::Acquire),
        last_uptime_return: LAST_UPTIME_RETURN.load(Ordering::Acquire),
    }
}

pub fn run_entry(entry_point: u64, stack_top: u64) -> ProbeResult {
    let before = USER_SYSCALLS.load(Ordering::Acquire);

    if !USER_READY.load(Ordering::Acquire) || !interrupts::syscall_gate_ready() {
        return ProbeResult {
            ran: false,
            passed: false,
            exit_code: 0,
            syscalls_before: before,
            syscalls_after: before,
        };
    }

    USER_RUNS.fetch_add(1, Ordering::Relaxed);
    stats::inc_user_probe();
    serial::log("user", "entering ring3 probe");

    let exit_code = unsafe {
        user_enter(
            entry_point,
            stack_top,
            gdt::USER_DATA_SELECTOR as u64,
            gdt::USER_CODE_SELECTOR as u64,
        )
    };
    cpu_interrupts::enable();

    LAST_EXIT_CODE.store(exit_code, Ordering::Release);
    let after = USER_SYSCALLS.load(Ordering::Acquire);
    let passed = exit_code == PROBE_EXIT_CODE && after >= before.saturating_add(4);

    if passed {
        USER_PASSES.fetch_add(1, Ordering::Relaxed);
        stats::inc_user_probe_pass();
        serial::log("user", "ring3 probe passed");
    } else {
        serial::log("user", "ring3 probe failed");
    }

    ProbeResult {
        ran: true,
        passed,
        exit_code,
        syscalls_before: before,
        syscalls_after: after,
    }
}

pub fn run_probe() -> ProbeResult {
    run_entry(probe_entry_point(), probe_stack_top())
}

pub fn probe_entry_point() -> u64 {
    paging::USER_PROBE_CODE_PAGE
}

pub fn fault_entry_point() -> u64 {
    paging::USER_PROBE_CODE_PAGE + USER_FAULT_CODE_OFFSET
}

pub fn fault_address() -> u64 {
    USER_FAULT_ADDRESS
}

pub fn probe_stack_top() -> u64 {
    paging::USER_PROBE_STACK_TOP
}

pub fn probe_expected_exit_code() -> u64 {
    PROBE_EXIT_CODE
}

pub fn fault_exit_code(vector: u64) -> u64 {
    USER_FAULT_EXIT_BASE.saturating_add(vector)
}

pub fn record_syscall() {
    USER_SYSCALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_fault(vector: u64, fault_address: u64, exit_code: u64) {
    USER_FAULTS.fetch_add(1, Ordering::Relaxed);
    LAST_FAULT_VECTOR.store(vector, Ordering::Release);
    LAST_FAULT_ADDRESS.store(fault_address, Ordering::Release);
    LAST_FAULT_EXIT_CODE.store(exit_code, Ordering::Release);
}

pub fn record_uptime_return(ticks: u64) {
    LAST_UPTIME_RETURN.store(ticks, Ordering::Release);
}

pub unsafe fn exit_to_kernel(exit_code: u64) -> ! {
    unsafe { user_return_to_kernel(exit_code) }
}

#[no_mangle]
pub extern "C" fn syscall_dispatch_handler(number: u64, arg0: u64, frame: *mut SyscallFrame) {
    let frame = unsafe { &mut *frame };
    syscall::dispatch(number, arg0, frame);
}

fn write_probe_program() {
    unsafe {
        let page = core::ptr::addr_of_mut!(USER_CODE_PAGE).cast::<u8>();
        core::ptr::write_bytes(page, 0, paging::PAGE_SIZE_4K as usize);

        for (index, byte) in USER_CODE_BYTES.iter().copied().enumerate() {
            page.add(index).write_volatile(byte);
        }

        write_fault_program(page.add(USER_FAULT_CODE_OFFSET as usize));
    }
}

unsafe fn write_fault_program(destination: *mut u8) {
    unsafe {
        destination.add(0).write_volatile(0x48);
        destination.add(1).write_volatile(0xb8);

        for (index, byte) in USER_FAULT_ADDRESS.to_le_bytes().iter().copied().enumerate() {
            destination.add(2 + index).write_volatile(byte);
        }

        destination.add(10).write_volatile(0x48);
        destination.add(11).write_volatile(0x8b);
        destination.add(12).write_volatile(0x00);
        destination.add(13).write_volatile(0xf4);
        destination.add(14).write_volatile(0xeb);
        destination.add(15).write_volatile(0xfd);
    }
}

fn map_probe_page(virt: u64, phys: u64) -> bool {
    let translation = paging::translate(virt);
    if translation.mapped {
        return translation.phys == phys && translation.user_accessible;
    }

    paging::map_user_page(virt, phys).is_ok() && paging::translate(virt).user_accessible
}

fn page_address<T>(pointer: *const T) -> u64 {
    (pointer as u64) & !(paging::PAGE_SIZE_4K - 1)
}
