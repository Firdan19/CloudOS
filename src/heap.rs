use crate::{paging, serial};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const HEAP_BASE: u64 = paging::KERNEL_HEAP_BASE;
pub const HEAP_SIZE: u64 = paging::KERNEL_HEAP_SIZE;
pub const HEAP_PAGES: u64 = HEAP_SIZE / paging::PAGE_SIZE_4K;
pub const GUARD_LOW: u64 = paging::KERNEL_HEAP_GUARD_LOW;
pub const GUARD_HIGH: u64 = paging::KERNEL_HEAP_GUARD_HIGH;

const HEAP_SENTINEL_BYTES: u64 = 64;
const HEAP_CANARY_HEAD: u64 = 0x544f_4241_4343_4f48;
const HEAP_CANARY_TAIL: u64 = 0x4845_4150_5f4f_4b21;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub base: u64,
    pub size: u64,
    pub used: u64,
    pub remaining: u64,
    pub mapped_pages: u64,
    pub allocations: u64,
    pub failed_allocations: u64,
    pub high_watermark: u64,
    pub corruption_checks: u64,
    pub corruption_failures: u64,
    pub metadata_ok: bool,
    pub sentinel_ok: bool,
    pub guard_low: u64,
    pub guard_high: u64,
}

struct KernelHeap {
    canary_head: u64,
    initialized: bool,
    used: u64,
    high_watermark: u64,
    mapped_pages: u64,
    allocations: u64,
    failed_allocations: u64,
    corruption_checks: u64,
    corruption_failures: u64,
    canary_tail: u64,
}

impl KernelHeap {
    const fn new() -> Self {
        Self {
            canary_head: HEAP_CANARY_HEAD,
            initialized: false,
            used: 0,
            high_watermark: 0,
            mapped_pages: 0,
            allocations: 0,
            failed_allocations: 0,
            corruption_checks: 0,
            corruption_failures: 0,
            canary_tail: HEAP_CANARY_TAIL,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            initialized: self.initialized,
            base: HEAP_BASE,
            size: HEAP_SIZE,
            used: self.used,
            remaining: HEAP_SIZE.saturating_sub(self.used),
            mapped_pages: self.mapped_pages,
            allocations: self.allocations,
            failed_allocations: self.failed_allocations,
            high_watermark: self.high_watermark,
            corruption_checks: self.corruption_checks,
            corruption_failures: self.corruption_failures,
            metadata_ok: self.metadata_ok(),
            sentinel_ok: self.sentinel_ok(),
            guard_low: GUARD_LOW,
            guard_high: GUARD_HIGH,
        }
    }

    fn metadata_ok(&self) -> bool {
        self.canary_head == HEAP_CANARY_HEAD
            && self.canary_tail == HEAP_CANARY_TAIL
            && self.used <= HEAP_SIZE
            && self.high_watermark <= HEAP_SIZE
            && self.mapped_pages <= HEAP_PAGES
            && (!self.initialized || self.used >= HEAP_SENTINEL_BYTES)
    }

    fn write_sentinel(&self) {
        unsafe {
            let ptr = HEAP_BASE as *mut u8;
            for index in 0..HEAP_SENTINEL_BYTES as usize {
                ptr.add(index)
                    .write_volatile(0x5a ^ (index as u8).wrapping_mul(17));
            }
        }
    }

    fn sentinel_ok(&self) -> bool {
        if !self.initialized || self.mapped_pages != HEAP_PAGES {
            return false;
        }

        unsafe {
            let ptr = HEAP_BASE as *const u8;
            for index in 0..HEAP_SENTINEL_BYTES as usize {
                let expected = 0x5a ^ (index as u8).wrapping_mul(17);
                if ptr.add(index).read_volatile() != expected {
                    return false;
                }
            }
        }

        true
    }
}

struct HeapStore {
    value: UnsafeCell<KernelHeap>,
}

unsafe impl Sync for HeapStore {}

static KERNEL_HEAP: HeapStore = HeapStore {
    value: UnsafeCell::new(KernelHeap::new()),
};

pub fn init() -> Snapshot {
    let heap = heap_mut();
    if heap.initialized {
        return heap.snapshot();
    }

    let mut mapped_pages = 0u64;
    for page in 0..HEAP_PAGES {
        let virt = HEAP_BASE + page * paging::PAGE_SIZE_4K;
        match paging::map_new_page_owned(virt, paging::PageOwner::Heap) {
            Ok(_) => mapped_pages += 1,
            Err(error) => {
                serial::log("heap", paging::map_error_name(error));
                break;
            }
        }
    }

    heap.mapped_pages = mapped_pages;
    heap.initialized = mapped_pages == HEAP_PAGES;
    if heap.initialized {
        heap.used = HEAP_SENTINEL_BYTES;
        heap.high_watermark = HEAP_SENTINEL_BYTES;
        heap.write_sentinel();
    }

    serial::log_u64("heap", "mapped pages", heap.mapped_pages);
    if heap.initialized {
        serial::log("heap", "kernel heap ready");
        serial::log("heap", "allocator corruption guard ready");
    } else {
        serial::log("heap", "kernel heap partial");
    }

    heap.snapshot()
}

pub fn alloc(size: u64, align: u64) -> Option<u64> {
    cpu_interrupts::without_interrupts(|| {
        let heap = heap_mut();
        if !heap.initialized || !heap.metadata_ok() || size == 0 || !align.is_power_of_two() {
            heap.failed_allocations = heap.failed_allocations.saturating_add(1);
            return None;
        }

        let current = HEAP_BASE.saturating_add(heap.used);
        let aligned = align_up(current, align);
        let new_used = aligned.saturating_add(size).saturating_sub(HEAP_BASE);

        if new_used > HEAP_SIZE {
            heap.failed_allocations = heap.failed_allocations.saturating_add(1);
            return None;
        }

        heap.used = new_used;
        heap.high_watermark = heap.high_watermark.max(heap.used);
        heap.allocations = heap.allocations.saturating_add(1);

        Some(aligned)
    })
}

pub fn corruption_check() -> bool {
    cpu_interrupts::without_interrupts(|| {
        let heap = heap_mut();
        heap.corruption_checks = heap.corruption_checks.saturating_add(1);

        let ok = heap.metadata_ok() && heap.sentinel_ok() && paging::guard_page_test();
        if !ok {
            heap.corruption_failures = heap.corruption_failures.saturating_add(1);
        }

        ok
    })
}

pub fn probe() -> bool {
    let Some(address) = alloc(32, 8) else {
        return false;
    };

    unsafe {
        let ptr = address as *mut u8;
        for index in 0..32usize {
            ptr.add(index).write_volatile((index as u8) ^ 0xa5);
        }

        for index in 0..32usize {
            if ptr.add(index).read_volatile() != ((index as u8) ^ 0xa5) {
                return false;
            }
        }
    }

    true
}

pub fn snapshot() -> Snapshot {
    heap().snapshot()
}

fn heap() -> &'static KernelHeap {
    unsafe { &*KERNEL_HEAP.value.get() }
}

fn heap_mut() -> &'static mut KernelHeap {
    unsafe { &mut *KERNEL_HEAP.value.get() }
}

fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}
