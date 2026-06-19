use crate::interrupts;
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

const KIND_CAPACITY: usize = 32;
const DETAIL_CAPACITY: usize = 96;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub present: bool,
    pub tick: u64,
    pub vector: u64,
    pub error_code: u64,
    pub instruction_pointer: u64,
    pub fault_address: u64,
    pub cpu_flags: u64,
    kind: [u8; KIND_CAPACITY],
    kind_len: u8,
    detail: [u8; DETAIL_CAPACITY],
    detail_len: u8,
}

impl Snapshot {
    pub const fn empty() -> Self {
        Self {
            present: false,
            tick: 0,
            vector: 0,
            error_code: 0,
            instruction_pointer: 0,
            fault_address: 0,
            cpu_flags: 0,
            kind: [0; KIND_CAPACITY],
            kind_len: 0,
            detail: [0; DETAIL_CAPACITY],
            detail_len: 0,
        }
    }

    pub fn kind(&self) -> &[u8] {
        &self.kind[..self.kind_len as usize]
    }

    pub fn detail(&self) -> &[u8] {
        &self.detail[..self.detail_len as usize]
    }
}

struct PanicStore {
    value: UnsafeCell<Snapshot>,
}

unsafe impl Sync for PanicStore {}

static LAST_PANIC: PanicStore = PanicStore {
    value: UnsafeCell::new(Snapshot::empty()),
};

pub fn record_rust_panic(detail: &str) {
    record("Rust panic", detail.as_bytes(), 0, 0, 0, 0, 0);
}

pub fn record_exception(
    kind: &str,
    vector: u64,
    error_code: u64,
    instruction_pointer: u64,
    fault_address: u64,
    cpu_flags: u64,
) {
    record(
        kind,
        b"CPU exception captured",
        vector,
        error_code,
        instruction_pointer,
        fault_address,
        cpu_flags,
    );
}

pub fn snapshot() -> Snapshot {
    let mut snapshot = Snapshot::empty();
    cpu_interrupts::without_interrupts(|| {
        snapshot = unsafe { *LAST_PANIC.value.get() };
    });
    snapshot
}

fn record(
    kind: &str,
    detail: &[u8],
    vector: u64,
    error_code: u64,
    instruction_pointer: u64,
    fault_address: u64,
    cpu_flags: u64,
) {
    cpu_interrupts::without_interrupts(|| {
        let record = unsafe { &mut *LAST_PANIC.value.get() };
        *record = Snapshot::empty();

        record.present = true;
        record.tick = interrupts::ticks();
        record.vector = vector;
        record.error_code = error_code;
        record.instruction_pointer = instruction_pointer;
        record.fault_address = fault_address;
        record.cpu_flags = cpu_flags;
        record.kind_len = copy_ascii(kind.as_bytes(), &mut record.kind) as u8;
        record.detail_len = copy_ascii(detail, &mut record.detail) as u8;
    });
}

fn copy_ascii(source: &[u8], destination: &mut [u8]) -> usize {
    let mut len = 0usize;

    for byte in source.iter().copied() {
        if len >= destination.len() {
            break;
        }

        destination[len] = match byte {
            0x20..=0x7e => byte,
            _ => b'?',
        };
        len += 1;
    }

    len
}
