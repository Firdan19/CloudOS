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
const BLOCK_MAGIC: u64 = 0x4842_4c4f_434b_3031;
const ALLOC_CANARY_HEAD: u64 = 0x4845_4150_4845_4144;
const ALLOC_CANARY_TAIL: u64 = 0x4845_4150_5441_494c;
const ALLOC_CANARY_BYTES: u64 = 8;
const MIN_SPLIT_BYTES: u64 = 32;
const MAX_BLOCKS: usize = 96;
const RECENT_FREE_COUNT: usize = 16;
const MAX_ALIGNMENT: u64 = paging::PAGE_SIZE_4K;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub base: u64,
    pub size: u64,
    pub used: u64,
    pub remaining: u64,
    pub mapped_pages: u64,
    pub allocations: u64,
    pub frees: u64,
    pub failed_allocations: u64,
    pub failed_frees: u64,
    pub double_frees: u64,
    pub invalid_frees: u64,
    pub coalesces: u64,
    pub active_allocations: u64,
    pub free_blocks: u64,
    pub metadata_blocks: u64,
    pub metadata_capacity: u64,
    pub allocated_bytes: u64,
    pub free_bytes: u64,
    pub largest_free_block: u64,
    pub high_watermark: u64,
    pub corruption_checks: u64,
    pub corruption_failures: u64,
    pub corruption_detections: u64,
    pub metadata_ok: bool,
    pub sentinel_ok: bool,
    pub allocation_canaries_ok: bool,
    pub guard_low: u64,
    pub guard_high: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FreeError {
    NotInitialized,
    InvalidPointer,
    DoubleFree,
    Corruption,
}

#[derive(Clone, Copy)]
struct HeapBlock {
    magic: u64,
    offset: u64,
    size: u64,
    payload_offset: u64,
    requested_size: u64,
    alignment: u64,
    allocated: bool,
    active: bool,
}

impl HeapBlock {
    const fn empty() -> Self {
        Self {
            magic: 0,
            offset: 0,
            size: 0,
            payload_offset: 0,
            requested_size: 0,
            alignment: 0,
            allocated: false,
            active: false,
        }
    }

    const fn free(offset: u64, size: u64) -> Self {
        Self {
            magic: BLOCK_MAGIC,
            offset,
            size,
            payload_offset: 0,
            requested_size: 0,
            alignment: 0,
            allocated: false,
            active: true,
        }
    }

    fn end(&self) -> u64 {
        self.offset.saturating_add(self.size)
    }

    fn payload_end(&self) -> u64 {
        self.payload_offset.saturating_add(self.requested_size)
    }
}

#[derive(Clone, Copy)]
struct HeapStats {
    allocated_bytes: u64,
    free_bytes: u64,
    largest_free_block: u64,
    active_allocations: u64,
    free_blocks: u64,
    metadata_blocks: u64,
    metadata_ok: bool,
    allocation_canaries_ok: bool,
}

struct KernelHeap {
    canary_head: u64,
    initialized: bool,
    mapped_pages: u64,
    allocations: u64,
    frees: u64,
    failed_allocations: u64,
    failed_frees: u64,
    double_frees: u64,
    invalid_frees: u64,
    coalesces: u64,
    high_watermark: u64,
    corruption_checks: u64,
    corruption_failures: u64,
    corruption_detections: u64,
    blocks: [HeapBlock; MAX_BLOCKS],
    recent_frees: [u64; RECENT_FREE_COUNT],
    recent_free_index: usize,
    canary_tail: u64,
}

impl KernelHeap {
    const fn new() -> Self {
        Self {
            canary_head: HEAP_CANARY_HEAD,
            initialized: false,
            mapped_pages: 0,
            allocations: 0,
            frees: 0,
            failed_allocations: 0,
            failed_frees: 0,
            double_frees: 0,
            invalid_frees: 0,
            coalesces: 0,
            high_watermark: 0,
            corruption_checks: 0,
            corruption_failures: 0,
            corruption_detections: 0,
            blocks: [HeapBlock::empty(); MAX_BLOCKS],
            recent_frees: [0; RECENT_FREE_COUNT],
            recent_free_index: 0,
            canary_tail: HEAP_CANARY_TAIL,
        }
    }

    fn snapshot(&self) -> Snapshot {
        let stats = self.compute_stats();

        Snapshot {
            initialized: self.initialized,
            base: HEAP_BASE,
            size: HEAP_SIZE,
            used: stats.allocated_bytes,
            remaining: stats.free_bytes,
            mapped_pages: self.mapped_pages,
            allocations: self.allocations,
            frees: self.frees,
            failed_allocations: self.failed_allocations,
            failed_frees: self.failed_frees,
            double_frees: self.double_frees,
            invalid_frees: self.invalid_frees,
            coalesces: self.coalesces,
            active_allocations: stats.active_allocations,
            free_blocks: stats.free_blocks,
            metadata_blocks: stats.metadata_blocks,
            metadata_capacity: MAX_BLOCKS as u64,
            allocated_bytes: stats.allocated_bytes,
            free_bytes: stats.free_bytes,
            largest_free_block: stats.largest_free_block,
            high_watermark: self.high_watermark,
            corruption_checks: self.corruption_checks,
            corruption_failures: self.corruption_failures,
            corruption_detections: self.corruption_detections,
            metadata_ok: self.metadata_ok(stats),
            sentinel_ok: self.sentinel_ok(),
            allocation_canaries_ok: stats.allocation_canaries_ok,
            guard_low: GUARD_LOW,
            guard_high: GUARD_HIGH,
        }
    }

    fn reset_allocator(&mut self) {
        self.blocks = [HeapBlock::empty(); MAX_BLOCKS];
        self.blocks[0] = HeapBlock::free(
            HEAP_SENTINEL_BYTES,
            HEAP_SIZE.saturating_sub(HEAP_SENTINEL_BYTES),
        );
        self.recent_frees = [0; RECENT_FREE_COUNT];
        self.recent_free_index = 0;
        self.high_watermark = 0;
        self.write_sentinel();
    }

    fn alloc(&mut self, size: u64, align: u64) -> Option<u64> {
        if !self.initialized || !self.metadata_ok(self.compute_stats()) || size == 0 {
            self.failed_allocations = self.failed_allocations.saturating_add(1);
            return None;
        }

        if align == 0 || !align.is_power_of_two() || align > MAX_ALIGNMENT {
            self.failed_allocations = self.failed_allocations.saturating_add(1);
            return None;
        }

        let effective_align = align.max(ALLOC_CANARY_BYTES);

        for index in 0..MAX_BLOCKS {
            if !self.blocks[index].active || self.blocks[index].allocated {
                continue;
            }

            let block = self.blocks[index];
            let payload_offset = align_up(
                block.offset.saturating_add(ALLOC_CANARY_BYTES),
                effective_align,
            );
            let used_end = payload_offset
                .saturating_add(size)
                .saturating_add(ALLOC_CANARY_BYTES);

            if used_end > block.end() {
                continue;
            }

            let remainder = block.end().saturating_sub(used_end);
            let allocated_size = if remainder >= MIN_SPLIT_BYTES {
                used_end.saturating_sub(block.offset)
            } else {
                block.size
            };

            self.blocks[index] = HeapBlock {
                magic: BLOCK_MAGIC,
                offset: block.offset,
                size: allocated_size,
                payload_offset,
                requested_size: size,
                alignment: effective_align,
                allocated: true,
                active: true,
            };

            if remainder >= MIN_SPLIT_BYTES {
                if let Some(slot) = self.free_metadata_slot() {
                    self.blocks[slot] = HeapBlock::free(used_end, remainder);
                } else {
                    self.blocks[index].size = block.size;
                }
            }

            self.write_allocation_canaries(index);
            self.allocations = self.allocations.saturating_add(1);
            let used = self.compute_stats().allocated_bytes;
            self.high_watermark = self.high_watermark.max(used);

            return Some(HEAP_BASE.saturating_add(payload_offset));
        }

        self.failed_allocations = self.failed_allocations.saturating_add(1);
        None
    }

    fn free(&mut self, address: u64) -> Result<(), FreeError> {
        if !self.initialized {
            self.failed_frees = self.failed_frees.saturating_add(1);
            return Err(FreeError::NotInitialized);
        }

        if let Some(index) = self.find_allocated_block(address) {
            if !self.allocation_canaries_ok(index) {
                self.failed_frees = self.failed_frees.saturating_add(1);
                self.corruption_detections = self.corruption_detections.saturating_add(1);
                return Err(FreeError::Corruption);
            }

            self.blocks[index].allocated = false;
            self.blocks[index].payload_offset = 0;
            self.blocks[index].requested_size = 0;
            self.blocks[index].alignment = 0;
            self.frees = self.frees.saturating_add(1);
            self.add_recent_free(address);
            self.coalesce_free_blocks();

            return Ok(());
        }

        self.failed_frees = self.failed_frees.saturating_add(1);
        if self.was_recently_freed(address) {
            self.double_frees = self.double_frees.saturating_add(1);
            return Err(FreeError::DoubleFree);
        }

        self.invalid_frees = self.invalid_frees.saturating_add(1);
        Err(FreeError::InvalidPointer)
    }

    fn corruption_check(&mut self) -> bool {
        self.corruption_checks = self.corruption_checks.saturating_add(1);

        let stats = self.compute_stats();
        let ok = self.metadata_ok(stats)
            && self.sentinel_ok()
            && stats.allocation_canaries_ok
            && paging::guard_page_test();
        if !ok {
            self.corruption_failures = self.corruption_failures.saturating_add(1);
        }

        ok
    }

    fn metadata_ok(&self, stats: HeapStats) -> bool {
        self.canary_head == HEAP_CANARY_HEAD
            && self.canary_tail == HEAP_CANARY_TAIL
            && self.mapped_pages <= HEAP_PAGES
            && (!self.initialized || self.mapped_pages == HEAP_PAGES)
            && stats.metadata_ok
    }

    fn compute_stats(&self) -> HeapStats {
        let mut stats = HeapStats {
            allocated_bytes: 0,
            free_bytes: 0,
            largest_free_block: 0,
            active_allocations: 0,
            free_blocks: 0,
            metadata_blocks: 0,
            metadata_ok: true,
            allocation_canaries_ok: true,
        };

        let mut covered_bytes = 0u64;

        for index in 0..MAX_BLOCKS {
            let block = self.blocks[index];
            if !block.active {
                continue;
            }

            stats.metadata_blocks = stats.metadata_blocks.saturating_add(1);
            covered_bytes = covered_bytes.saturating_add(block.size);

            if block.magic != BLOCK_MAGIC
                || block.size == 0
                || block.offset < HEAP_SENTINEL_BYTES
                || block.end() > HEAP_SIZE
            {
                stats.metadata_ok = false;
            }

            for other_index in (index + 1)..MAX_BLOCKS {
                let other = self.blocks[other_index];
                if other.active
                    && ranges_overlap(block.offset, block.end(), other.offset, other.end())
                {
                    stats.metadata_ok = false;
                }
            }

            if block.allocated {
                stats.active_allocations = stats.active_allocations.saturating_add(1);
                stats.allocated_bytes = stats.allocated_bytes.saturating_add(block.requested_size);

                let shape_ok = block.payload_offset
                    >= block.offset.saturating_add(ALLOC_CANARY_BYTES)
                    && block.payload_end().saturating_add(ALLOC_CANARY_BYTES) <= block.end()
                    && block.alignment >= ALLOC_CANARY_BYTES
                    && block.alignment.is_power_of_two()
                    && block.payload_offset % block.alignment == 0;

                if !shape_ok {
                    stats.metadata_ok = false;
                    stats.allocation_canaries_ok = false;
                } else if !self.allocation_canaries_ok(index) {
                    stats.allocation_canaries_ok = false;
                }
            } else {
                stats.free_blocks = stats.free_blocks.saturating_add(1);
                stats.free_bytes = stats.free_bytes.saturating_add(block.size);
                stats.largest_free_block = stats.largest_free_block.max(block.size);
            }
        }

        if self.initialized && covered_bytes != HEAP_SIZE.saturating_sub(HEAP_SENTINEL_BYTES) {
            stats.metadata_ok = false;
        }

        if stats.metadata_blocks > MAX_BLOCKS as u64
            || stats.allocated_bytes.saturating_add(stats.free_bytes) > HEAP_SIZE
        {
            stats.metadata_ok = false;
        }

        stats
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

    fn write_allocation_canaries(&self, index: usize) {
        let block = self.blocks[index];
        unsafe {
            write_u64(
                HEAP_BASE
                    .saturating_add(block.payload_offset)
                    .saturating_sub(ALLOC_CANARY_BYTES),
                ALLOC_CANARY_HEAD,
            );
            write_u64(
                HEAP_BASE.saturating_add(block.payload_end()),
                ALLOC_CANARY_TAIL,
            );
        }
    }

    fn allocation_canaries_ok(&self, index: usize) -> bool {
        let block = self.blocks[index];
        if !block.active || !block.allocated {
            return false;
        }

        unsafe {
            read_u64(
                HEAP_BASE
                    .saturating_add(block.payload_offset)
                    .saturating_sub(ALLOC_CANARY_BYTES),
            ) == ALLOC_CANARY_HEAD
                && read_u64(HEAP_BASE.saturating_add(block.payload_end())) == ALLOC_CANARY_TAIL
        }
    }

    fn find_allocated_block(&self, address: u64) -> Option<usize> {
        let offset = address.checked_sub(HEAP_BASE)?;

        for index in 0..MAX_BLOCKS {
            let block = self.blocks[index];
            if block.active && block.allocated && block.payload_offset == offset {
                return Some(index);
            }
        }

        None
    }

    fn free_metadata_slot(&self) -> Option<usize> {
        for index in 0..MAX_BLOCKS {
            if !self.blocks[index].active {
                return Some(index);
            }
        }

        None
    }

    fn add_recent_free(&mut self, address: u64) {
        self.recent_frees[self.recent_free_index] = address;
        self.recent_free_index = (self.recent_free_index + 1) % RECENT_FREE_COUNT;
    }

    fn was_recently_freed(&self, address: u64) -> bool {
        if address == 0 {
            return false;
        }

        for freed in self.recent_frees.iter().copied() {
            if freed == address {
                return true;
            }
        }

        false
    }

    fn coalesce_free_blocks(&mut self) {
        let mut changed = true;

        while changed {
            changed = false;

            for left in 0..MAX_BLOCKS {
                if !self.blocks[left].active || self.blocks[left].allocated {
                    continue;
                }

                for right in 0..MAX_BLOCKS {
                    if left == right || !self.blocks[right].active || self.blocks[right].allocated {
                        continue;
                    }

                    if self.blocks[left].end() == self.blocks[right].offset {
                        self.blocks[left].size = self.blocks[left]
                            .size
                            .saturating_add(self.blocks[right].size);
                        self.blocks[right] = HeapBlock::empty();
                        self.coalesces = self.coalesces.saturating_add(1);
                        changed = true;
                        break;
                    }

                    if self.blocks[right].end() == self.blocks[left].offset {
                        self.blocks[left].offset = self.blocks[right].offset;
                        self.blocks[left].size = self.blocks[left]
                            .size
                            .saturating_add(self.blocks[right].size);
                        self.blocks[right] = HeapBlock::empty();
                        self.coalesces = self.coalesces.saturating_add(1);
                        changed = true;
                        break;
                    }
                }

                if changed {
                    break;
                }
            }
        }
    }

    fn repair_canaries_for(&self, address: u64) -> bool {
        let Some(index) = self.find_allocated_block(address) else {
            return false;
        };

        self.write_allocation_canaries(index);
        true
    }

    fn corrupt_tail_canary_for(&self, address: u64) -> bool {
        let Some(index) = self.find_allocated_block(address) else {
            return false;
        };

        let block = self.blocks[index];
        unsafe {
            write_u64(HEAP_BASE.saturating_add(block.payload_end()), 0);
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
        heap.reset_allocator();
    }

    serial::log_u64("heap", "mapped pages", heap.mapped_pages);
    if heap.initialized {
        serial::log("heap", "kernel heap ready");
        serial::log("heap", "free list allocator ready");
        serial::log("heap", "allocator corruption guard ready");
    } else {
        serial::log("heap", "kernel heap partial");
    }

    heap.snapshot()
}

pub fn alloc(size: u64, align: u64) -> Option<u64> {
    cpu_interrupts::without_interrupts(|| heap_mut().alloc(size, align))
}

pub fn free(address: u64) -> Result<(), FreeError> {
    cpu_interrupts::without_interrupts(|| heap_mut().free(address))
}

pub fn corruption_check() -> bool {
    cpu_interrupts::without_interrupts(|| heap_mut().corruption_check())
}

pub fn probe() -> bool {
    let Some(address) = alloc(32, 8) else {
        return false;
    };

    let ok = write_and_verify_pattern(address, 32, 0xa5);
    let freed = free(address).is_ok();

    ok && freed
}

pub fn selftest() -> bool {
    let before = snapshot();

    let Some(first) = alloc(64, 16) else {
        return false;
    };
    let Some(second) = alloc(128, 64) else {
        let _ = free(first);
        return false;
    };
    let Some(third) = alloc(32, 8) else {
        let _ = free(second);
        let _ = free(first);
        return false;
    };

    let align_ok = first % 16 == 0 && second % 64 == 0 && third % 8 == 0;
    let patterns_ok = write_and_verify_pattern(first, 64, 0x11)
        && write_and_verify_pattern(second, 128, 0x22)
        && write_and_verify_pattern(third, 32, 0x33);

    let freed_second = free(second).is_ok();
    let double_free_detected = matches!(free(second), Err(FreeError::DoubleFree));

    let Some(reused) = alloc(96, 32) else {
        let _ = free(first);
        let _ = free(third);
        return false;
    };

    let reuse_ok = reused % 32 == 0 && write_and_verify_pattern(reused, 96, 0x44);

    let Some(corrupt) = alloc(24, 8) else {
        let _ = free(reused);
        let _ = free(third);
        let _ = free(first);
        return false;
    };

    let corrupted = corrupt_tail_canary(corrupt);
    let corruption_was_detected = matches!(free(corrupt), Err(FreeError::Corruption));
    let repaired = repair_canaries(corrupt);
    let corrupt_freed = free(corrupt).is_ok();
    let corruption_detected = corrupted && corruption_was_detected && repaired && corrupt_freed;

    let reused_freed = free(reused).is_ok();
    let third_freed = free(third).is_ok();
    let first_freed = free(first).is_ok();
    let cleanup = reused_freed && third_freed && first_freed;
    let after = snapshot();

    align_ok
        && patterns_ok
        && freed_second
        && double_free_detected
        && reuse_ok
        && corruption_detected
        && cleanup
        && after.active_allocations == before.active_allocations
        && after.allocated_bytes == before.allocated_bytes
        && after.metadata_ok
        && after.allocation_canaries_ok
        && corruption_check()
}

pub fn stress() -> bool {
    let before = snapshot();
    let mut addresses = [0u64; 8];
    let sizes = [24u64, 40, 56, 72, 88, 104, 120, 136];
    let aligns = [8u64, 16, 32, 64, 8, 16, 32, 64];

    for index in 0..addresses.len() {
        let Some(address) = alloc(sizes[index], aligns[index]) else {
            cleanup_addresses(&addresses);
            return false;
        };

        addresses[index] = address;
        if address % aligns[index] != 0
            || !write_and_verify_pattern(address, sizes[index], index as u8)
        {
            cleanup_addresses(&addresses);
            return false;
        }
    }

    if free(addresses[1]).is_err() || free(addresses[3]).is_err() || free(addresses[5]).is_err() {
        cleanup_addresses(&addresses);
        return false;
    }
    addresses[1] = 0;
    addresses[3] = 0;
    addresses[5] = 0;

    let Some(extra_one) = alloc(48, 16) else {
        cleanup_addresses(&addresses);
        return false;
    };
    let Some(extra_two) = alloc(96, 32) else {
        let _ = free(extra_one);
        cleanup_addresses(&addresses);
        return false;
    };

    let extra_ok = extra_one % 16 == 0
        && extra_two % 32 == 0
        && write_and_verify_pattern(extra_one, 48, 0x77)
        && write_and_verify_pattern(extra_two, 96, 0x88);

    let extra_two_freed = free(extra_two).is_ok();
    let extra_one_freed = free(extra_one).is_ok();
    let addresses_freed = cleanup_addresses(&addresses);
    let cleanup = extra_two_freed && extra_one_freed && addresses_freed;
    let after = snapshot();

    extra_ok
        && cleanup
        && after.active_allocations == before.active_allocations
        && after.allocated_bytes == before.allocated_bytes
        && after.metadata_ok
        && after.allocation_canaries_ok
        && corruption_check()
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

fn cleanup_addresses(addresses: &[u64]) -> bool {
    let mut ok = true;

    for address in addresses.iter().copied() {
        if address != 0 && free(address).is_err() {
            ok = false;
        }
    }

    ok
}

fn write_and_verify_pattern(address: u64, size: u64, seed: u8) -> bool {
    unsafe {
        let ptr = address as *mut u8;
        for index in 0..size as usize {
            ptr.add(index)
                .write_volatile(seed.wrapping_add(index as u8).rotate_left(1));
        }

        for index in 0..size as usize {
            let expected = seed.wrapping_add(index as u8).rotate_left(1);
            if ptr.add(index).read_volatile() != expected {
                return false;
            }
        }
    }

    true
}

fn corrupt_tail_canary(address: u64) -> bool {
    cpu_interrupts::without_interrupts(|| heap_mut().corrupt_tail_canary_for(address))
}

fn repair_canaries(address: u64) -> bool {
    cpu_interrupts::without_interrupts(|| heap_mut().repair_canaries_for(address))
}

fn align_up(value: u64, align: u64) -> u64 {
    value.saturating_add(align - 1) & !(align - 1)
}

fn ranges_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start < right_end && right_start < left_end
}

unsafe fn write_u64(address: u64, value: u64) {
    unsafe {
        (address as *mut u64).write_unaligned(value);
    }
}

unsafe fn read_u64(address: u64) -> u64 {
    unsafe { (address as *const u64).read_unaligned() }
}
