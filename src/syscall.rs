use crate::{interrupts, scheduler, serial, stats, user};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const SYSCALL_LOG: u64 = 1;
pub const SYSCALL_UPTIME: u64 = 2;
pub const SYSCALL_EXIT: u64 = 3;
pub const SYSCALL_YIELD: u64 = 4;

pub const RET_OK: u64 = 0;
pub const RET_UNKNOWN_SYSCALL: u64 = u64::MAX;

type SyscallHandler = fn(u64, &mut user::SyscallFrame) -> u64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReturnCode {
    Zero,
    Dynamic,
    Never,
}

#[derive(Clone, Copy)]
pub struct SyscallEntry {
    pub number: u64,
    pub name: &'static str,
    pub arg_count: u8,
    pub return_code: ReturnCode,
    pub logging: bool,
    handler: SyscallHandler,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub entries: u64,
    pub dispatches: u64,
    pub unknown_syscalls: u64,
    pub last_number: u64,
    pub last_return: u64,
}

const SYSCALLS: [SyscallEntry; 4] = [
    SyscallEntry {
        number: SYSCALL_LOG,
        name: "log",
        arg_count: 1,
        return_code: ReturnCode::Zero,
        logging: true,
        handler: syscall_log,
    },
    SyscallEntry {
        number: SYSCALL_UPTIME,
        name: "uptime",
        arg_count: 0,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_uptime,
    },
    SyscallEntry {
        number: SYSCALL_EXIT,
        name: "exit",
        arg_count: 1,
        return_code: ReturnCode::Never,
        logging: true,
        handler: syscall_exit,
    },
    SyscallEntry {
        number: SYSCALL_YIELD,
        name: "yield",
        arg_count: 0,
        return_code: ReturnCode::Zero,
        logging: true,
        handler: syscall_yield,
    },
];

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static DISPATCHES: AtomicU64 = AtomicU64::new(0);
static UNKNOWN_SYSCALLS: AtomicU64 = AtomicU64::new(0);
static LAST_NUMBER: AtomicU64 = AtomicU64::new(0);
static LAST_RETURN: AtomicU64 = AtomicU64::new(0);

pub fn init() -> Snapshot {
    INITIALIZED.store(true, Ordering::Release);
    serial::log("syscall", "table ready");
    serial::log_u64("syscall", "table entries", SYSCALLS.len() as u64);
    snapshot()
}

pub fn dispatch(number: u64, arg0: u64, frame: &mut user::SyscallFrame) {
    user::record_syscall();
    stats::inc_syscall();

    DISPATCHES.fetch_add(1, Ordering::Relaxed);
    LAST_NUMBER.store(number, Ordering::Release);

    let Some(entry) = lookup(number) else {
        UNKNOWN_SYSCALLS.fetch_add(1, Ordering::Relaxed);
        LAST_RETURN.store(RET_UNKNOWN_SYSCALL, Ordering::Release);
        serial::log_u64("syscall", "unknown syscall", number);
        frame.rax = RET_UNKNOWN_SYSCALL;
        return;
    };

    if entry.logging {
        serial::log_u64("syscall", "dispatch", entry.number);
    }

    if entry.return_code == ReturnCode::Never {
        LAST_RETURN.store(RET_OK, Ordering::Release);
    }

    let return_value = (entry.handler)(arg0, frame);
    LAST_RETURN.store(return_value, Ordering::Release);

    if entry.logging && entry.return_code != ReturnCode::Never {
        serial::log_u64("syscall", "return", return_value);
    }
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        initialized: INITIALIZED.load(Ordering::Acquire),
        entries: SYSCALLS.len() as u64,
        dispatches: DISPATCHES.load(Ordering::Acquire),
        unknown_syscalls: UNKNOWN_SYSCALLS.load(Ordering::Acquire),
        last_number: LAST_NUMBER.load(Ordering::Acquire),
        last_return: LAST_RETURN.load(Ordering::Acquire),
    }
}

pub fn table_len() -> usize {
    SYSCALLS.len()
}

pub fn table_entry(index: usize) -> Option<SyscallEntry> {
    if index >= SYSCALLS.len() {
        return None;
    }

    Some(SYSCALLS[index])
}

pub fn lookup(number: u64) -> Option<&'static SyscallEntry> {
    SYSCALLS.iter().find(|entry| entry.number == number)
}

pub fn selftest() -> bool {
    INITIALIZED.load(Ordering::Acquire)
        && SYSCALLS.len() == 4
        && lookup(SYSCALL_LOG).is_some()
        && lookup(SYSCALL_UPTIME).is_some()
        && lookup(SYSCALL_EXIT).is_some()
        && lookup(SYSCALL_YIELD).is_some()
        && table_numbers_unique()
        && SYSCALLS[0].arg_count == 1
        && SYSCALLS[1].arg_count == 0
        && SYSCALLS[2].return_code == ReturnCode::Never
        && SYSCALLS[3].return_code == ReturnCode::Zero
}

pub fn return_code_name(code: ReturnCode) -> &'static str {
    match code {
        ReturnCode::Zero => "zero",
        ReturnCode::Dynamic => "dynamic",
        ReturnCode::Never => "never",
    }
}

fn syscall_log(arg0: u64, frame: &mut user::SyscallFrame) -> u64 {
    serial::log_u64("syscall", "user log id", arg0);
    frame.rax = RET_OK;
    RET_OK
}

fn syscall_uptime(_arg0: u64, frame: &mut user::SyscallFrame) -> u64 {
    let ticks = interrupts::ticks();
    user::record_uptime_return(ticks);
    serial::log_u64("syscall", "uptime ticks", ticks);
    frame.rax = ticks;
    ticks
}

fn syscall_exit(arg0: u64, _frame: &mut user::SyscallFrame) -> u64 {
    serial::log_u64("syscall", "exit", arg0);
    unsafe { user::exit_to_kernel(arg0) }
}

fn syscall_yield(_arg0: u64, frame: &mut user::SyscallFrame) -> u64 {
    let current = scheduler::yield_current();
    serial::log_u64("syscall", "yield", current);
    frame.rax = RET_OK;
    RET_OK
}

fn table_numbers_unique() -> bool {
    for left in 0..SYSCALLS.len() {
        for right in (left + 1)..SYSCALLS.len() {
            if SYSCALLS[left].number == SYSCALLS[right].number {
                return false;
            }
        }
    }

    true
}
