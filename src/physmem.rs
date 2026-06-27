use crate::{multiboot, serial};
use core::cell::UnsafeCell;

pub const FRAME_SIZE: u64 = 4096;

const MAX_ALLOC_REGIONS: usize = 16;
const MAX_RECYCLED_FRAMES: usize = 128;
const MIN_ALLOC_ADDR: u64 = 0x0010_0000;

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

#[derive(Clone, Copy)]
struct AllocRegion {
    start: u64,
    end: u64,
}

impl AllocRegion {
    const fn empty() -> Self {
        Self { start: 0, end: 0 }
    }

    fn frame_count(&self) -> u64 {
        self.end.saturating_sub(self.start) / FRAME_SIZE
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub exhausted: bool,
    pub region_count: u32,
    pub current_region: u32,
    pub total_usable_frames: u64,
    pub allocatable_frames: u64,
    pub allocated_frames: u64,
    pub recycled_frames: u64,
    pub recycled_capacity: u64,
    pub skipped_frames: u64,
    pub free_frames: u64,
    pub next_frame: u64,
    pub last_allocated_frame: u64,
    pub kernel_start: u64,
    pub kernel_end: u64,
    pub protected_until: u64,
}

impl Snapshot {
    const fn empty() -> Self {
        Self {
            initialized: false,
            exhausted: false,
            region_count: 0,
            current_region: 0,
            total_usable_frames: 0,
            allocatable_frames: 0,
            allocated_frames: 0,
            recycled_frames: 0,
            recycled_capacity: MAX_RECYCLED_FRAMES as u64,
            skipped_frames: 0,
            free_frames: 0,
            next_frame: 0,
            last_allocated_frame: 0,
            kernel_start: 0,
            kernel_end: 0,
            protected_until: 0,
        }
    }
}

struct FrameAllocator {
    initialized: bool,
    exhausted: bool,
    regions: [AllocRegion; MAX_ALLOC_REGIONS],
    region_count: usize,
    current_region: usize,
    next_frame: u64,
    total_usable_frames: u64,
    allocatable_frames: u64,
    allocated_frames: u64,
    recycled_frames: [u64; MAX_RECYCLED_FRAMES],
    recycled_count: usize,
    skipped_frames: u64,
    last_allocated_frame: u64,
    kernel_start: u64,
    kernel_end: u64,
    protected_until: u64,
}

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            initialized: false,
            exhausted: false,
            regions: [AllocRegion::empty(); MAX_ALLOC_REGIONS],
            region_count: 0,
            current_region: 0,
            next_frame: 0,
            total_usable_frames: 0,
            allocatable_frames: 0,
            allocated_frames: 0,
            recycled_frames: [0; MAX_RECYCLED_FRAMES],
            recycled_count: 0,
            skipped_frames: 0,
            last_allocated_frame: 0,
            kernel_start: 0,
            kernel_end: 0,
            protected_until: 0,
        }
    }

    fn init(&mut self) {
        *self = Self::new();

        self.kernel_start = kernel_start();
        self.kernel_end = align_up(kernel_end(), FRAME_SIZE);
        self.protected_until = self.kernel_end.max(MIN_ALLOC_ADDR);

        let boot_info = multiboot::summary();
        if boot_info.address != 0 && boot_info.total_size != 0 {
            let boot_info_end = align_up(
                boot_info
                    .address
                    .saturating_add(boot_info.total_size as u64),
                FRAME_SIZE,
            );
            self.protected_until = self.protected_until.max(boot_info_end);
        }

        let module_end = align_up(multiboot::highest_module_end(), FRAME_SIZE);
        self.protected_until = self.protected_until.max(module_end);

        for index in 0..multiboot::stored_region_count() {
            if let Some(region) = multiboot::region(index) {
                self.add_memory_region(region);
            }
        }

        if self.region_count > 0 {
            self.next_frame = self.regions[0].start;
        } else {
            self.exhausted = true;
        }

        self.initialized = true;
    }

    fn add_memory_region(&mut self, region: multiboot::MemoryRegion) {
        if !region.is_available() || region.length < FRAME_SIZE {
            return;
        }

        let region_start = align_up(region.base_addr, FRAME_SIZE);
        let region_end = align_down(region.end_addr(), FRAME_SIZE);

        if region_end <= region_start {
            return;
        }

        self.total_usable_frames += (region_end - region_start) / FRAME_SIZE;

        let alloc_start = if region_end <= self.protected_until {
            self.skipped_frames += (region_end - region_start) / FRAME_SIZE;
            return;
        } else {
            let start = region_start.max(self.protected_until);
            self.skipped_frames += start.saturating_sub(region_start) / FRAME_SIZE;
            start
        };

        if alloc_start >= region_end {
            return;
        }

        if self.region_count < MAX_ALLOC_REGIONS {
            let alloc_region = AllocRegion {
                start: alloc_start,
                end: region_end,
            };
            self.allocatable_frames += alloc_region.frame_count();
            self.regions[self.region_count] = alloc_region;
            self.region_count += 1;
        } else {
            self.skipped_frames += (region_end - alloc_start) / FRAME_SIZE;
        }
    }

    fn allocate_frame(&mut self) -> Option<u64> {
        if !self.initialized {
            self.init();
        }

        if self.recycled_count > 0 {
            self.recycled_count -= 1;
            let frame = self.recycled_frames[self.recycled_count];
            self.recycled_frames[self.recycled_count] = 0;
            self.allocated_frames += 1;
            self.last_allocated_frame = frame;
            self.exhausted = false;
            return Some(frame);
        }

        while self.current_region < self.region_count {
            let region = self.regions[self.current_region];

            if self.next_frame < region.start {
                self.next_frame = region.start;
            }

            if self.next_frame < region.end {
                let frame = self.next_frame;
                self.next_frame = self.next_frame.saturating_add(FRAME_SIZE);
                self.allocated_frames += 1;
                self.last_allocated_frame = frame;
                return Some(frame);
            }

            self.current_region += 1;
            if self.current_region < self.region_count {
                self.next_frame = self.regions[self.current_region].start;
            }
        }

        self.exhausted = true;
        None
    }

    fn free_frame(&mut self, frame: u64) -> bool {
        if !self.initialized {
            self.init();
        }

        if !is_frame_aligned(frame) || frame < self.protected_until || !self.is_issued_frame(frame)
        {
            return false;
        }

        if self.recycled_count >= MAX_RECYCLED_FRAMES {
            return false;
        }

        for index in 0..self.recycled_count {
            if self.recycled_frames[index] == frame {
                return false;
            }
        }

        self.recycled_frames[self.recycled_count] = frame;
        self.recycled_count += 1;
        self.allocated_frames = self.allocated_frames.saturating_sub(1);
        self.exhausted = false;

        true
    }

    fn is_issued_frame(&self, frame: u64) -> bool {
        for index in 0..self.region_count {
            let region = self.regions[index];
            if frame >= region.start && frame < region.end {
                if index < self.current_region {
                    return true;
                }

                if index == self.current_region {
                    return frame < self.next_frame;
                }

                return false;
            }
        }

        false
    }

    fn snapshot(&self) -> Snapshot {
        let free_frames = self
            .allocatable_frames
            .saturating_sub(self.allocated_frames);

        Snapshot {
            initialized: self.initialized,
            exhausted: self.exhausted,
            region_count: self.region_count as u32,
            current_region: self.current_region as u32,
            total_usable_frames: self.total_usable_frames,
            allocatable_frames: self.allocatable_frames,
            allocated_frames: self.allocated_frames,
            recycled_frames: self.recycled_count as u64,
            recycled_capacity: MAX_RECYCLED_FRAMES as u64,
            skipped_frames: self.skipped_frames,
            free_frames,
            next_frame: self.next_frame,
            last_allocated_frame: self.last_allocated_frame,
            kernel_start: self.kernel_start,
            kernel_end: self.kernel_end,
            protected_until: self.protected_until,
        }
    }
}

struct AllocatorStore {
    value: UnsafeCell<FrameAllocator>,
}

unsafe impl Sync for AllocatorStore {}

static ALLOCATOR: AllocatorStore = AllocatorStore {
    value: UnsafeCell::new(FrameAllocator::new()),
};

pub fn init() -> Snapshot {
    let allocator = allocator_mut();
    allocator.init();
    let snapshot = allocator.snapshot();

    serial::log_u64(
        "mem",
        "frame allocator regions",
        snapshot.region_count as u64,
    );
    serial::log_u64("mem", "allocatable frames", snapshot.allocatable_frames);

    snapshot
}

pub fn allocate_frame() -> Option<u64> {
    allocator_mut().allocate_frame()
}

pub fn free_frame(frame: u64) -> bool {
    allocator_mut().free_frame(frame)
}

pub fn snapshot() -> Snapshot {
    allocator_mut().snapshot()
}

fn allocator_mut() -> &'static mut FrameAllocator {
    unsafe { &mut *ALLOCATOR.value.get() }
}

fn kernel_start() -> u64 {
    unsafe { core::ptr::addr_of!(__kernel_start) as u64 }
}

fn kernel_end() -> u64 {
    unsafe { core::ptr::addr_of!(__kernel_end) as u64 }
}

const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

const fn is_frame_aligned(value: u64) -> bool {
    value & (FRAME_SIZE - 1) == 0
}

const fn align_up(value: u64, alignment: u64) -> u64 {
    align_down(value.saturating_add(alignment - 1), alignment)
}
