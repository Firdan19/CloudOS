use crate::{interrupts, ipc, process, scheduler, serial, stats, user};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const SYSCALL_LOG: u64 = 1;
pub const SYSCALL_UPTIME: u64 = 2;
pub const SYSCALL_EXIT: u64 = 3;
pub const SYSCALL_YIELD: u64 = 4;
pub const SYSCALL_IPC_SEND: u64 = 5;
pub const SYSCALL_IPC_RECEIVE: u64 = 6;
pub const SYSCALL_GETPID: u64 = 7;
pub const SYSCALL_IPC_SELF: u64 = 8;
pub const SYSCALL_IPC_RECEIVE_TIMEOUT: u64 = 9;
pub const SYSCALL_IPC_CANCEL: u64 = 10;
pub const SYSCALL_IPC_SEND_CAPABILITY: u64 = 11;
pub const SYSCALL_IPC_REVOKE: u64 = 12;

pub const RET_OK: u64 = 0;
pub const RET_UNKNOWN_SYSCALL: u64 = u64::MAX;
pub const RET_INVALID_USER_BUFFER: u64 = u64::MAX - 1;
pub const RET_IPC_ERROR_BASE: u64 = u64::MAX - 32;

type SyscallHandler = fn(u64, &mut user::SyscallFrame) -> SyscallOutcome;

#[derive(Clone, Copy)]
struct SyscallOutcome {
    return_value: u64,
    switch_root: u64,
}

impl SyscallOutcome {
    const fn complete(return_value: u64) -> Self {
        Self {
            return_value,
            switch_root: 0,
        }
    }

    const fn switch(switch_root: u64) -> Self {
        Self {
            return_value: RET_OK,
            switch_root,
        }
    }
}

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

const SYSCALLS: [SyscallEntry; 12] = [
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
    SyscallEntry {
        number: SYSCALL_IPC_SEND,
        name: "ipc_send",
        arg_count: 3,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_send,
    },
    SyscallEntry {
        number: SYSCALL_IPC_RECEIVE,
        name: "ipc_receive",
        arg_count: 2,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_receive,
    },
    SyscallEntry {
        number: SYSCALL_GETPID,
        name: "getpid",
        arg_count: 0,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_getpid,
    },
    SyscallEntry {
        number: SYSCALL_IPC_SELF,
        name: "ipc_self",
        arg_count: 0,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_self,
    },
    SyscallEntry {
        number: SYSCALL_IPC_RECEIVE_TIMEOUT,
        name: "ipc_receive_timeout",
        arg_count: 3,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_receive_timeout,
    },
    SyscallEntry {
        number: SYSCALL_IPC_CANCEL,
        name: "ipc_cancel",
        arg_count: 1,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_cancel,
    },
    SyscallEntry {
        number: SYSCALL_IPC_SEND_CAPABILITY,
        name: "ipc_send_capability",
        arg_count: 5,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_send_capability,
    },
    SyscallEntry {
        number: SYSCALL_IPC_REVOKE,
        name: "ipc_revoke",
        arg_count: 1,
        return_code: ReturnCode::Dynamic,
        logging: true,
        handler: syscall_ipc_revoke,
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

pub fn dispatch(number: u64, arg0: u64, frame: &mut user::SyscallFrame) -> u64 {
    user::record_syscall();
    stats::inc_syscall();

    DISPATCHES.fetch_add(1, Ordering::Relaxed);
    LAST_NUMBER.store(number, Ordering::Release);

    let Some(entry) = lookup(number) else {
        UNKNOWN_SYSCALLS.fetch_add(1, Ordering::Relaxed);
        LAST_RETURN.store(RET_UNKNOWN_SYSCALL, Ordering::Release);
        serial::log_u64("syscall", "unknown syscall", number);
        frame.rax = RET_UNKNOWN_SYSCALL;
        return 0;
    };

    if entry.logging {
        serial::log_u64("syscall", "dispatch", entry.number);
    }

    if entry.return_code == ReturnCode::Never {
        LAST_RETURN.store(RET_OK, Ordering::Release);
    }

    let outcome = (entry.handler)(arg0, frame);
    LAST_RETURN.store(outcome.return_value, Ordering::Release);
    if outcome.switch_root != 0 {
        serial::log_hex_u64("syscall", "context switch root", outcome.switch_root);
        return outcome.switch_root;
    }
    if entry.return_code != ReturnCode::Never {
        frame.rax = outcome.return_value;
    }

    if entry.logging && entry.return_code != ReturnCode::Never {
        serial::log_u64("syscall", "return", outcome.return_value);
    }
    0
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
        && SYSCALLS.len() == 12
        && lookup(SYSCALL_LOG).is_some()
        && lookup(SYSCALL_UPTIME).is_some()
        && lookup(SYSCALL_EXIT).is_some()
        && lookup(SYSCALL_YIELD).is_some()
        && lookup(SYSCALL_IPC_SEND).is_some()
        && lookup(SYSCALL_IPC_RECEIVE).is_some()
        && lookup(SYSCALL_GETPID).is_some()
        && lookup(SYSCALL_IPC_SELF).is_some()
        && lookup(SYSCALL_IPC_RECEIVE_TIMEOUT).is_some()
        && lookup(SYSCALL_IPC_CANCEL).is_some()
        && lookup(SYSCALL_IPC_SEND_CAPABILITY).is_some()
        && lookup(SYSCALL_IPC_REVOKE).is_some()
        && table_numbers_unique()
        && SYSCALLS[0].arg_count == 1
        && SYSCALLS[1].arg_count == 0
        && SYSCALLS[2].return_code == ReturnCode::Never
        && SYSCALLS[3].return_code == ReturnCode::Zero
        && SYSCALLS[4].arg_count == 3
        && SYSCALLS[5].arg_count == 2
        && SYSCALLS[6].arg_count == 0
        && SYSCALLS[7].arg_count == 0
        && SYSCALLS[8].arg_count == 3
        && SYSCALLS[9].arg_count == 1
        && SYSCALLS[10].arg_count == 5
        && SYSCALLS[11].arg_count == 1
}

pub fn return_code_name(code: ReturnCode) -> &'static str {
    match code {
        ReturnCode::Zero => "zero",
        ReturnCode::Dynamic => "dynamic",
        ReturnCode::Never => "never",
    }
}

fn syscall_log(arg0: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    serial::log_u64("syscall", "user log id", arg0);
    frame.rax = RET_OK;
    SyscallOutcome::complete(RET_OK)
}

fn syscall_uptime(_arg0: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let ticks = interrupts::ticks();
    user::record_uptime_return(ticks);
    serial::log_u64("syscall", "uptime ticks", ticks);
    frame.rax = ticks;
    SyscallOutcome::complete(ticks)
}

fn syscall_exit(arg0: u64, _frame: &mut user::SyscallFrame) -> SyscallOutcome {
    serial::log_u64("syscall", "exit", arg0);
    unsafe { user::exit_to_kernel(arg0) }
}

fn syscall_yield(_arg0: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let current = scheduler::yield_current();
    serial::log_u64("syscall", "yield", current);
    frame.rax = RET_OK;
    SyscallOutcome::complete(RET_OK)
}

fn syscall_ipc_send(handle: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let sender = scheduler::snapshot().current_task;
    let length = frame.rdx as usize;
    if sender == 0 || length > ipc::MAX_MESSAGE_BYTES {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidTask));
    }

    let mut message = [0u8; ipc::MAX_MESSAGE_BYTES];
    if !process::copy_from_user(sender, frame.rsi, &mut message[..length]) {
        return SyscallOutcome::complete(RET_INVALID_USER_BUFFER);
    }
    match ipc::send_capability(sender, handle, &message[..length]) {
        Ok(sequence) => {
            serial::log_u64("ipc", "syscall send bytes", length as u64);
            frame.rax = sequence;
            SyscallOutcome::complete(sequence)
        }
        Err(error) => SyscallOutcome::complete(ipc_error_return(error)),
    }
}

fn syscall_ipc_receive(buffer: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    syscall_ipc_receive_common(buffer, frame, None)
}

fn syscall_ipc_receive_timeout(buffer: u64, frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let timeout_ticks = frame.rdx;
    syscall_ipc_receive_common(buffer, frame, Some(timeout_ticks))
}

fn syscall_ipc_receive_common(
    buffer: u64,
    frame: &mut user::SyscallFrame,
    timeout_ticks: Option<u64>,
) -> SyscallOutcome {
    let receiver = scheduler::snapshot().current_task;
    let capacity = (frame.rsi as usize).min(ipc::MAX_MESSAGE_BYTES);
    if receiver == 0 {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidTask));
    }
    if !process::validate_user_buffer(receiver, buffer, capacity as u64, true).valid {
        return SyscallOutcome::complete(RET_INVALID_USER_BUFFER);
    }

    match process::take_ipc_wake_reason(receiver) {
        process::IpcWakeReason::Timeout => {
            return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::Timeout));
        }
        process::IpcWakeReason::Cancelled => {
            return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::Cancelled));
        }
        process::IpcWakeReason::None | process::IpcWakeReason::Message => {}
    }

    let mut message = [0u8; ipc::MAX_MESSAGE_BYTES];
    match ipc::receive(receiver, &mut message[..capacity], false) {
        Ok(ipc::ReceiveOutcome::Message(delivery)) => {
            if !process::copy_to_user(receiver, buffer, &message[..delivery.length as usize]) {
                return SyscallOutcome::complete(RET_INVALID_USER_BUFFER);
            }
            frame.rdx = delivery.sender;
            frame.rcx = delivery.sequence;
            frame.r8 = delivery.capability_handle;
            frame.r9 = delivery.capability_rights as u64;
            frame.rax = delivery.length;
            serial::log_u64("ipc", "syscall receive bytes", delivery.length);
            let _ = process::complete_ipc_restart(receiver);
            SyscallOutcome::complete(delivery.length)
        }
        Ok(ipc::ReceiveOutcome::Blocked) => {
            SyscallOutcome::complete(ipc_error_return(ipc::IpcError::BlockFailed))
        }
        Err(ipc::IpcError::QueueEmpty) => {
            if timeout_ticks == Some(0) {
                return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::Timeout));
            }
            let deadline_tick = timeout_ticks
                .map(|ticks| interrupts::ticks().saturating_add(ticks))
                .unwrap_or(0);
            match ipc::block_syscall(receiver, frame, deadline_tick) {
                Ok(ipc::SyscallBlockOutcome::Switched(address_space_root)) => {
                    SyscallOutcome::switch(address_space_root)
                }
                Ok(ipc::SyscallBlockOutcome::MessageReady) => {
                    syscall_ipc_receive_common(buffer, frame, timeout_ticks)
                }
                Err(error) => SyscallOutcome::complete(ipc_error_return(error)),
            }
        }
        Err(error) => SyscallOutcome::complete(ipc_error_return(error)),
    }
}

fn syscall_getpid(_arg0: u64, _frame: &mut user::SyscallFrame) -> SyscallOutcome {
    SyscallOutcome::complete(scheduler::snapshot().current_task)
}

fn syscall_ipc_self(_arg0: u64, _frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let task_id = scheduler::snapshot().current_task;
    let result = if task_id == 0 {
        ipc_error_return(ipc::IpcError::InvalidTask)
    } else {
        match ipc::self_capability(task_id) {
            Ok(handle) => {
                serial::log_hex_u64("ipc-cap", "self capability issued", handle);
                handle
            }
            Err(error) => ipc_error_return(error),
        }
    };
    SyscallOutcome::complete(result)
}

fn syscall_ipc_cancel(handle: u64, _frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let requester = scheduler::snapshot().current_task;
    let result = if requester == 0 {
        ipc_error_return(ipc::IpcError::InvalidTask)
    } else {
        match ipc::cancel_wait(requester, handle) {
            Ok(target) => target,
            Err(error) => ipc_error_return(error),
        }
    };
    SyscallOutcome::complete(result)
}

fn syscall_ipc_send_capability(
    destination_handle: u64,
    frame: &mut user::SyscallFrame,
) -> SyscallOutcome {
    let sender = scheduler::snapshot().current_task;
    let length = frame.rdx as usize;
    if sender == 0 || length > ipc::MAX_MESSAGE_BYTES {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidTask));
    }
    if frame.r8 > u8::MAX as u64 {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidRights));
    }
    let Some(requested_rights) = ipc::CapabilityRights::from_bits(frame.r8 as u8) else {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidRights));
    };

    let mut message = [0u8; ipc::MAX_MESSAGE_BYTES];
    if !process::copy_from_user(sender, frame.rsi, &mut message[..length]) {
        return SyscallOutcome::complete(RET_INVALID_USER_BUFFER);
    }
    match ipc::send_with_capability(
        sender,
        destination_handle,
        frame.rcx,
        requested_rights,
        &message[..length],
    ) {
        Ok(sequence) => {
            serial::log_u64("ipc-cap", "syscall transfer bytes", length as u64);
            SyscallOutcome::complete(sequence)
        }
        Err(error) => SyscallOutcome::complete(ipc_error_return(error)),
    }
}

fn syscall_ipc_revoke(handle: u64, _frame: &mut user::SyscallFrame) -> SyscallOutcome {
    let owner = scheduler::snapshot().current_task;
    if owner == 0 {
        return SyscallOutcome::complete(ipc_error_return(ipc::IpcError::InvalidTask));
    }
    match ipc::revoke_capability_tree(owner, handle) {
        Ok(report) => {
            serial::log_u64("ipc-cap", "revoked tree size", report.revoked);
            serial::log_u64("ipc-cap", "revoked descendants", report.descendants);
            serial::log_u64(
                "ipc-cap",
                "queued capabilities stripped",
                report.queued_stripped,
            );
            SyscallOutcome::complete(report.revoked)
        }
        Err(error) => SyscallOutcome::complete(ipc_error_return(error)),
    }
}

pub fn ipc_error_return(error: ipc::IpcError) -> u64 {
    RET_IPC_ERROR_BASE.saturating_add(ipc::error_code(error))
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
