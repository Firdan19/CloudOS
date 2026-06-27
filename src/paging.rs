use crate::{physmem, serial};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::instructions::interrupts as cpu_interrupts;

pub const PAGE_SIZE_4K: u64 = 4096;
pub const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
pub const BOOT_IDENTITY_MAP_BYTES: u64 = 1024 * 1024 * 1024;
pub const PAGE_TABLE_MEMORY_BYTES: u64 = PAGE_SIZE_4K * 3;
pub const KERNEL_VIRTUAL_BASE: u64 = 0xffff_8000_0000_0000;
pub const KERNEL_VIRTUAL_SIZE: u64 = 1024 * 1024 * 1024;
pub const KERNEL_VIRTUAL_END: u64 = KERNEL_VIRTUAL_BASE + KERNEL_VIRTUAL_SIZE;
pub const KERNEL_HEAP_GUARD_LOW: u64 = KERNEL_VIRTUAL_BASE + 0x0100_0000;
pub const KERNEL_HEAP_BASE: u64 = KERNEL_HEAP_GUARD_LOW + PAGE_SIZE_4K;
pub const KERNEL_HEAP_SIZE: u64 = 64 * 1024;
pub const KERNEL_HEAP_GUARD_HIGH: u64 = KERNEL_HEAP_BASE + KERNEL_HEAP_SIZE;
pub const KERNEL_VM_TEST_PAGE: u64 = KERNEL_VIRTUAL_BASE + 0x0200_0000;
pub const USER_SPACE_BASE: u64 = BOOT_IDENTITY_MAP_BYTES;
pub const USER_SPACE_SIZE: u64 = 16 * 1024 * 1024;
pub const USER_SPACE_END: u64 = USER_SPACE_BASE + USER_SPACE_SIZE;
pub const USER_PROBE_CODE_PAGE: u64 = USER_SPACE_BASE;
pub const USER_PROBE_STACK_PAGE: u64 = USER_SPACE_BASE + PAGE_SIZE_4K;
pub const USER_PROBE_STACK_TOP: u64 = USER_PROBE_STACK_PAGE + PAGE_SIZE_4K;
pub const USER_ELF_BASE: u64 = USER_SPACE_BASE + 0x0010_0000;
pub const USER_ELF_SIZE: u64 = 0x0010_0000;
pub const USER_ELF_END: u64 = USER_ELF_BASE + USER_ELF_SIZE;
pub const USER_ELF_STACK_GUARD: u64 = USER_ELF_END;
pub const USER_ELF_STACK_PAGE: u64 = USER_ELF_STACK_GUARD + PAGE_SIZE_4K;
pub const USER_ELF_STACK_TOP: u64 = USER_ELF_STACK_PAGE + PAGE_SIZE_4K;

const PAGE_TABLE_ENTRIES: usize = 512;
const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_HUGE_PAGE: u64 = 1 << 7;
const ENTRY_NO_EXECUTE: u64 = 1 << 63;
const DEFAULT_PAGE_FLAGS: u64 = ENTRY_PRESENT | ENTRY_WRITABLE;
const USER_DATA_PAGE_FLAGS: u64 = DEFAULT_PAGE_FLAGS | ENTRY_USER | ENTRY_NO_EXECUTE;
const ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const HUGE_ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffe0_0000;
pub const MAX_TRACKED_MAPPINGS: usize = 128;

unsafe extern "C" {
    static boot_p4_table: u64;
    static boot_p3_table: u64;
    static boot_p2_table: u64;
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub mapper_initialized: bool,
    pub cr3: u64,
    pub p4_addr: u64,
    pub p3_addr: u64,
    pub p2_addr: u64,
    pub p4_present_entries: u64,
    pub p3_present_entries: u64,
    pub p2_present_entries: u64,
    pub huge_pages: u64,
    pub identity_mapped_bytes: u64,
    pub page_table_bytes: u64,
    pub mapped_pages: u64,
    pub unmapped_pages: u64,
    pub page_table_frames: u64,
    pub guard_pages: u64,
    pub tracked_mappings: u64,
    pub tracking_capacity: u64,
    pub tracking_overflows: u64,
    pub tracking_misses: u64,
    pub kernel_owned_pages: u64,
    pub heap_owned_pages: u64,
    pub user_owned_pages: u64,
    pub test_owned_pages: u64,
    pub permission_violations: u64,
}

#[derive(Clone, Copy)]
pub struct TranslateResult {
    pub virt: u64,
    pub phys: u64,
    pub mapped: bool,
    pub huge_page: bool,
    pub user_accessible: bool,
    pub writable: bool,
    pub executable: bool,
    pub page_size: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UserPagePermissions {
    pub writable: bool,
    pub executable: bool,
}

impl UserPagePermissions {
    pub const READ_EXECUTE: Self = Self {
        writable: false,
        executable: true,
    };

    pub const READ_WRITE: Self = Self {
        writable: true,
        executable: false,
    };

    fn flags(self) -> u64 {
        let mut flags = ENTRY_PRESENT | ENTRY_USER;
        if self.writable {
            flags |= ENTRY_WRITABLE;
        }
        if !self.executable {
            flags |= ENTRY_NO_EXECUTE;
        }
        flags
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    VirtualUnaligned,
    PhysicalUnaligned,
    OutsideManagedRange,
    PhysicalNotIdentityMapped,
    AlreadyMapped,
    HugePageConflict,
    OutOfFrames,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnmapError {
    VirtualUnaligned,
    OutsideManagedRange,
    NotMapped,
    HugePageConflict,
    PageTableNotReachable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageOwner {
    Kernel,
    Heap,
    User,
    Test,
}

impl PageOwner {
    fn default_flags(self) -> u64 {
        match self {
            PageOwner::User => USER_DATA_PAGE_FLAGS,
            PageOwner::Kernel | PageOwner::Heap | PageOwner::Test => DEFAULT_PAGE_FLAGS,
        }
    }
}

#[derive(Clone, Copy)]
pub struct MappingInfo {
    pub virt: u64,
    pub phys: u64,
    pub owner: PageOwner,
    pub user_accessible: bool,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Clone, Copy)]
pub struct PermissionAudit {
    pub checked_pages: u64,
    pub violations: u64,
    pub kernel_pages: u64,
    pub heap_pages: u64,
    pub user_pages: u64,
    pub test_pages: u64,
    pub guard_pages_intact: bool,
    pub tracking_consistent: bool,
}

#[derive(Clone, Copy)]
struct MappingRecord {
    virt: u64,
    phys: u64,
    flags: u64,
    owner: PageOwner,
    active: bool,
}

impl MappingRecord {
    const fn empty() -> Self {
        Self {
            virt: 0,
            phys: 0,
            flags: 0,
            owner: PageOwner::Kernel,
            active: false,
        }
    }

    fn info(&self) -> MappingInfo {
        MappingInfo {
            virt: self.virt,
            phys: self.phys,
            owner: self.owner,
            user_accessible: self.flags & ENTRY_USER != 0,
            writable: self.flags & ENTRY_WRITABLE != 0,
            executable: self.flags & ENTRY_NO_EXECUTE == 0,
        }
    }
}

struct MapperState {
    initialized: bool,
    mapped_pages: u64,
    unmapped_pages: u64,
    page_table_frames: u64,
    guard_pages: u64,
    records: [MappingRecord; MAX_TRACKED_MAPPINGS],
    tracking_overflows: u64,
    tracking_misses: u64,
}

impl MapperState {
    const fn new() -> Self {
        Self {
            initialized: false,
            mapped_pages: 0,
            unmapped_pages: 0,
            page_table_frames: 0,
            guard_pages: 0,
            records: [MappingRecord::empty(); MAX_TRACKED_MAPPINGS],
            tracking_overflows: 0,
            tracking_misses: 0,
        }
    }

    fn record_mapping(&mut self, virt: u64, phys: u64, flags: u64, owner: PageOwner) {
        for record in self.records.iter_mut() {
            if record.active && record.virt == virt {
                record.phys = phys;
                record.flags = flags;
                record.owner = owner;
                return;
            }
        }

        for record in self.records.iter_mut() {
            if !record.active {
                *record = MappingRecord {
                    virt,
                    phys,
                    flags,
                    owner,
                    active: true,
                };
                return;
            }
        }

        self.tracking_overflows = self.tracking_overflows.saturating_add(1);
    }

    fn forget_mapping(&mut self, virt: u64) -> Option<MappingRecord> {
        for record in self.records.iter_mut() {
            if record.active && record.virt == virt {
                let removed = *record;
                *record = MappingRecord::empty();
                return Some(removed);
            }
        }

        self.tracking_misses = self.tracking_misses.saturating_add(1);
        None
    }

    fn mapping_info(&self, virt: u64) -> Option<MappingInfo> {
        for record in self.records.iter() {
            if record.active && record.virt == align_down(virt, PAGE_SIZE_4K) {
                return Some(record.info());
            }
        }

        None
    }

    fn active_mapping_count(&self) -> u64 {
        let mut count = 0;
        for record in self.records.iter() {
            if record.active {
                count += 1;
            }
        }

        count
    }

    fn owner_counts(&self) -> (u64, u64, u64, u64) {
        let mut kernel = 0;
        let mut heap = 0;
        let mut user = 0;
        let mut test = 0;

        for record in self.records.iter() {
            if !record.active {
                continue;
            }

            match record.owner {
                PageOwner::Kernel => kernel += 1,
                PageOwner::Heap => heap += 1,
                PageOwner::User => user += 1,
                PageOwner::Test => test += 1,
            }
        }

        (kernel, heap, user, test)
    }
}

struct MapperStore {
    value: UnsafeCell<MapperState>,
}

unsafe impl Sync for MapperStore {}

static PAGING_INITIALIZED: AtomicBool = AtomicBool::new(false);
static MAPPER_STATE: MapperStore = MapperStore {
    value: UnsafeCell::new(MapperState::new()),
};

pub fn init() -> Snapshot {
    PAGING_INITIALIZED.store(true, Ordering::Release);
    let state = mapper_state_mut();
    state.initialized = true;
    state.guard_pages = 2;

    let snapshot = snapshot();

    serial::log("paging", "boot page tables ready");
    serial::log_hex_u64("paging", "cr3", snapshot.cr3);
    serial::log_hex_u64("paging", "p4 table", snapshot.p4_addr);
    serial::log_hex_u64("paging", "p3 table", snapshot.p3_addr);
    serial::log_hex_u64("paging", "p2 table", snapshot.p2_addr);
    serial::log_u64("paging", "huge pages", snapshot.huge_pages);
    serial::log("paging", "virtual mapper ready");
    serial::log("paging", "page ownership tracking ready");

    snapshot
}

pub fn snapshot() -> Snapshot {
    let p4 = boot_p4();
    let p3 = boot_p3();
    let p2 = boot_p2();

    let p4_present_entries = count_present_entries(p4);
    let p3_present_entries = count_present_entries(p3);
    let p2_present_entries = count_present_entries(p2);
    let huge_pages = count_huge_entries(p2);
    let state = mapper_state();
    let (kernel_owned_pages, heap_owned_pages, user_owned_pages, test_owned_pages) =
        state.owner_counts();
    let audit = permission_audit_inner(state);

    Snapshot {
        initialized: PAGING_INITIALIZED.load(Ordering::Acquire),
        mapper_initialized: state.initialized,
        cr3: read_cr3(),
        p4_addr: p4_addr(),
        p3_addr: p3_addr(),
        p2_addr: p2_addr(),
        p4_present_entries,
        p3_present_entries,
        p2_present_entries,
        huge_pages,
        identity_mapped_bytes: huge_pages.saturating_mul(HUGE_PAGE_SIZE),
        page_table_bytes: PAGE_TABLE_MEMORY_BYTES,
        mapped_pages: state.mapped_pages,
        unmapped_pages: state.unmapped_pages,
        page_table_frames: state.page_table_frames,
        guard_pages: state.guard_pages,
        tracked_mappings: state.active_mapping_count(),
        tracking_capacity: MAX_TRACKED_MAPPINGS as u64,
        tracking_overflows: state.tracking_overflows,
        tracking_misses: state.tracking_misses,
        kernel_owned_pages,
        heap_owned_pages,
        user_owned_pages,
        test_owned_pages,
        permission_violations: audit.violations,
    }
}

pub fn translate(virt: u64) -> TranslateResult {
    let p4 = boot_p4();

    let p4_index = ((virt >> 39) & 0x1ff) as usize;
    let p3_index = ((virt >> 30) & 0x1ff) as usize;
    let p2_index = ((virt >> 21) & 0x1ff) as usize;
    let p1_index = ((virt >> 12) & 0x1ff) as usize;
    let huge_offset = virt & (HUGE_PAGE_SIZE - 1);

    let p4_entry = p4[p4_index];
    if !entry_present(p4_entry) {
        return unmapped(virt);
    }
    let mut user_accessible = entry_user(p4_entry);
    let mut writable = entry_writable(p4_entry);
    let mut executable = !entry_no_execute(p4_entry);

    let Some(p3) = table_from_entry(p4_entry) else {
        return unmapped(virt);
    };

    let p3_entry = p3[p3_index];
    if !entry_present(p3_entry) {
        return unmapped(virt);
    }
    user_accessible &= entry_user(p3_entry);
    writable &= entry_writable(p3_entry);
    executable &= !entry_no_execute(p3_entry);

    let Some(p2) = table_from_entry(p3_entry) else {
        return unmapped(virt);
    };

    let p2_entry = p2[p2_index];
    if !entry_present(p2_entry) {
        return unmapped(virt);
    }
    user_accessible &= entry_user(p2_entry);
    writable &= entry_writable(p2_entry);
    executable &= !entry_no_execute(p2_entry);

    if entry_huge(p2_entry) {
        return TranslateResult {
            virt,
            phys: huge_entry_addr(p2_entry).saturating_add(huge_offset),
            mapped: true,
            huge_page: true,
            user_accessible: user_accessible && entry_user(p2_entry),
            writable: writable && entry_writable(p2_entry),
            executable: executable && !entry_no_execute(p2_entry),
            page_size: HUGE_PAGE_SIZE,
        };
    }

    let Some(p1) = table_from_entry(p2_entry) else {
        return unmapped(virt);
    };

    let p1_entry = p1[p1_index];
    if !entry_present(p1_entry) {
        return unmapped(virt);
    }
    user_accessible &= entry_user(p1_entry);
    writable &= entry_writable(p1_entry);
    executable &= !entry_no_execute(p1_entry);

    TranslateResult {
        virt,
        phys: entry_addr(p1_entry).saturating_add(virt & (PAGE_SIZE_4K - 1)),
        mapped: true,
        huge_page: false,
        user_accessible,
        writable,
        executable,
        page_size: PAGE_SIZE_4K,
    }
}

pub fn map_new_page(virt: u64) -> Result<u64, MapError> {
    map_new_page_owned(virt, PageOwner::Kernel)
}

pub fn map_new_page_owned(virt: u64, owner: PageOwner) -> Result<u64, MapError> {
    map_new_page_with_flags(virt, owner.default_flags(), owner)
}

pub fn map_new_user_page(virt: u64, permissions: UserPagePermissions) -> Result<u64, MapError> {
    map_new_page_with_flags(virt, permissions.flags(), PageOwner::User)
}

fn map_new_page_with_flags(virt: u64, flags: u64, owner: PageOwner) -> Result<u64, MapError> {
    cpu_interrupts::without_interrupts(|| {
        if !is_page_aligned(virt) {
            return Err(MapError::VirtualUnaligned);
        }

        if !in_mappable_virtual_range(virt, flags) {
            return Err(MapError::OutsideManagedRange);
        }

        if translate(virt).mapped {
            return Err(MapError::AlreadyMapped);
        }

        let phys = physmem::allocate_frame().ok_or(MapError::OutOfFrames)?;
        if !identity_reachable(phys) {
            let _ = physmem::free_frame(phys);
            return Err(MapError::PhysicalNotIdentityMapped);
        }

        zero_frame(phys);
        if let Err(error) = map_page_inner_with_flags(virt, phys, flags, owner) {
            let _ = physmem::free_frame(phys);
            return Err(error);
        }

        Ok(phys)
    })
}

pub fn map_page(virt: u64, phys: u64) -> Result<(), MapError> {
    cpu_interrupts::without_interrupts(|| map_page_inner_with_owner(virt, phys, PageOwner::Kernel))
}

pub fn map_user_page(virt: u64, phys: u64) -> Result<(), MapError> {
    map_user_page_with_permissions(virt, phys, UserPagePermissions::READ_WRITE)
}

pub fn map_user_page_with_permissions(
    virt: u64,
    phys: u64,
    permissions: UserPagePermissions,
) -> Result<(), MapError> {
    cpu_interrupts::without_interrupts(|| {
        map_page_inner_with_flags(virt, phys, permissions.flags(), PageOwner::User)
    })
}

pub fn unmap_page(virt: u64) -> Result<u64, UnmapError> {
    cpu_interrupts::without_interrupts(|| unmap_page_inner(virt))
}

pub fn probe_map_unmap(virt: u64) -> bool {
    if translate(virt).mapped {
        return false;
    }

    let Ok(phys) = map_new_page_owned(virt, PageOwner::Test) else {
        return false;
    };

    let mapped = translate(virt);
    let owner = mapping_info(virt);
    if !mapped.mapped
        || mapped.phys != phys
        || mapped.page_size != PAGE_SIZE_4K
        || owner.map(|info| info.owner) != Some(PageOwner::Test)
    {
        if let Ok(unmapped_phys) = unmap_page(virt) {
            let _ = physmem::free_frame(unmapped_phys);
        }
        return false;
    }

    let Ok(unmapped_phys) = unmap_page(virt) else {
        return false;
    };

    unmapped_phys == phys
        && physmem::free_frame(unmapped_phys)
        && !translate(virt).mapped
        && mapping_info(virt).is_none()
}

pub fn mapping_info(virt: u64) -> Option<MappingInfo> {
    mapper_state().mapping_info(virt)
}

pub fn permission_audit() -> PermissionAudit {
    cpu_interrupts::without_interrupts(|| permission_audit_inner(mapper_state()))
}

pub fn guard_page_test() -> bool {
    !translate(KERNEL_HEAP_GUARD_LOW).mapped
        && !translate(KERNEL_HEAP_GUARD_HIGH).mapped
        && matches!(
            map_new_page_owned(KERNEL_HEAP_GUARD_LOW, PageOwner::Test),
            Err(MapError::OutsideManagedRange)
        )
        && matches!(
            map_new_page_owned(KERNEL_HEAP_GUARD_HIGH, PageOwner::Test),
            Err(MapError::OutsideManagedRange)
        )
}

pub fn fault_policy(address: u64, _error_code: u64) -> &'static str {
    if in_page(address, KERNEL_HEAP_GUARD_LOW) || in_page(address, KERNEL_HEAP_GUARD_HIGH) {
        "heap guard page violation"
    } else if in_user_virtual_range(address) && !translate(address).mapped {
        "user virtual address not mapped"
    } else if in_user_virtual_range(address) {
        "user virtual address protection fault"
    } else if in_managed_virtual_range(address) && !translate(address).mapped {
        "managed virtual address not mapped"
    } else if in_managed_virtual_range(address) {
        "managed virtual address protection fault"
    } else if address < BOOT_IDENTITY_MAP_BYTES {
        "low identity-map page fault"
    } else {
        "outside Tobacco managed virtual memory"
    }
}

pub fn map_error_name(error: MapError) -> &'static str {
    match error {
        MapError::VirtualUnaligned => "virtual address is not 4 KiB aligned",
        MapError::PhysicalUnaligned => "physical address is not 4 KiB aligned",
        MapError::OutsideManagedRange => "virtual address outside managed range",
        MapError::PhysicalNotIdentityMapped => "physical frame is not identity reachable",
        MapError::AlreadyMapped => "virtual page is already mapped",
        MapError::HugePageConflict => "mapping conflicts with a huge page",
        MapError::OutOfFrames => "physical frame allocator exhausted",
    }
}

pub fn unmap_error_name(error: UnmapError) -> &'static str {
    match error {
        UnmapError::VirtualUnaligned => "virtual address is not 4 KiB aligned",
        UnmapError::OutsideManagedRange => "virtual address outside managed range",
        UnmapError::NotMapped => "virtual page is not mapped",
        UnmapError::HugePageConflict => "mapping conflicts with a huge page",
        UnmapError::PageTableNotReachable => "page table is not identity reachable",
    }
}

fn unmapped(virt: u64) -> TranslateResult {
    TranslateResult {
        virt,
        phys: 0,
        mapped: false,
        huge_page: false,
        user_accessible: false,
        writable: false,
        executable: false,
        page_size: 0,
    }
}

fn boot_p4() -> &'static [u64; PAGE_TABLE_ENTRIES] {
    unsafe { &*(p4_addr() as *const [u64; PAGE_TABLE_ENTRIES]) }
}

fn boot_p4_mut() -> &'static mut [u64; PAGE_TABLE_ENTRIES] {
    unsafe { &mut *(p4_addr() as *mut [u64; PAGE_TABLE_ENTRIES]) }
}

fn boot_p3() -> &'static [u64; PAGE_TABLE_ENTRIES] {
    unsafe { &*(p3_addr() as *const [u64; PAGE_TABLE_ENTRIES]) }
}

fn boot_p2() -> &'static [u64; PAGE_TABLE_ENTRIES] {
    unsafe { &*(p2_addr() as *const [u64; PAGE_TABLE_ENTRIES]) }
}

fn p4_addr() -> u64 {
    unsafe { core::ptr::addr_of!(boot_p4_table) as u64 }
}

fn p3_addr() -> u64 {
    unsafe { core::ptr::addr_of!(boot_p3_table) as u64 }
}

fn p2_addr() -> u64 {
    unsafe { core::ptr::addr_of!(boot_p2_table) as u64 }
}

fn count_present_entries(table: &[u64; PAGE_TABLE_ENTRIES]) -> u64 {
    let mut count = 0;

    for entry in table.iter().copied() {
        if entry_present(entry) {
            count += 1;
        }
    }

    count
}

fn count_huge_entries(table: &[u64; PAGE_TABLE_ENTRIES]) -> u64 {
    let mut count = 0;

    for entry in table.iter().copied() {
        if entry_present(entry) && entry_huge(entry) {
            count += 1;
        }
    }

    count
}

fn map_page_inner_with_owner(virt: u64, phys: u64, owner: PageOwner) -> Result<(), MapError> {
    map_page_inner_with_flags(virt, phys, owner.default_flags(), owner)
}

fn map_page_inner_with_flags(
    virt: u64,
    phys: u64,
    flags: u64,
    owner: PageOwner,
) -> Result<(), MapError> {
    if !is_page_aligned(virt) {
        return Err(MapError::VirtualUnaligned);
    }

    if !is_page_aligned(phys) {
        return Err(MapError::PhysicalUnaligned);
    }

    if !in_mappable_virtual_range(virt, flags) {
        return Err(MapError::OutsideManagedRange);
    }

    if !identity_reachable(phys) {
        return Err(MapError::PhysicalNotIdentityMapped);
    }

    let p4_index = ((virt >> 39) & 0x1ff) as usize;
    let p3_index = ((virt >> 30) & 0x1ff) as usize;
    let p2_index = ((virt >> 21) & 0x1ff) as usize;
    let p1_index = ((virt >> 12) & 0x1ff) as usize;

    let p4 = boot_p4_mut();
    let p3 = ensure_next_table(&mut p4[p4_index], flags)?;
    let p2 = ensure_next_table(&mut p3[p3_index], flags)?;

    if entry_huge(p2[p2_index]) {
        return Err(MapError::HugePageConflict);
    }

    let p1 = ensure_next_table(&mut p2[p2_index], flags)?;

    if entry_present(p1[p1_index]) {
        return Err(MapError::AlreadyMapped);
    }

    p1[p1_index] = phys | flags;
    let state = mapper_state_mut();
    state.mapped_pages = state.mapped_pages.saturating_add(1);
    state.record_mapping(virt, phys, flags, owner);
    invlpg(virt);

    Ok(())
}

fn unmap_page_inner(virt: u64) -> Result<u64, UnmapError> {
    if !is_page_aligned(virt) {
        return Err(UnmapError::VirtualUnaligned);
    }

    if !in_managed_virtual_range(virt) && !in_user_virtual_range(virt) {
        return Err(UnmapError::OutsideManagedRange);
    }

    let p4_index = ((virt >> 39) & 0x1ff) as usize;
    let p3_index = ((virt >> 30) & 0x1ff) as usize;
    let p2_index = ((virt >> 21) & 0x1ff) as usize;
    let p1_index = ((virt >> 12) & 0x1ff) as usize;

    let p4 = boot_p4_mut();
    if !entry_present(p4[p4_index]) {
        return Err(UnmapError::NotMapped);
    }

    let p3 = table_from_entry_mut(p4[p4_index]).ok_or(UnmapError::PageTableNotReachable)?;
    if !entry_present(p3[p3_index]) {
        return Err(UnmapError::NotMapped);
    }

    let p2 = table_from_entry_mut(p3[p3_index]).ok_or(UnmapError::PageTableNotReachable)?;
    if entry_huge(p2[p2_index]) {
        return Err(UnmapError::HugePageConflict);
    }

    if !entry_present(p2[p2_index]) {
        return Err(UnmapError::NotMapped);
    }

    let p1 = table_from_entry_mut(p2[p2_index]).ok_or(UnmapError::PageTableNotReachable)?;
    let entry = p1[p1_index];
    if !entry_present(entry) {
        return Err(UnmapError::NotMapped);
    }

    p1[p1_index] = 0;
    let state = mapper_state_mut();
    state.unmapped_pages = state.unmapped_pages.saturating_add(1);
    state.forget_mapping(virt);
    invlpg(virt);

    Ok(entry_addr(entry))
}

fn permission_audit_inner(state: &MapperState) -> PermissionAudit {
    let mut audit = PermissionAudit {
        checked_pages: 0,
        violations: 0,
        kernel_pages: 0,
        heap_pages: 0,
        user_pages: 0,
        test_pages: 0,
        guard_pages_intact: !translate(KERNEL_HEAP_GUARD_LOW).mapped
            && !translate(KERNEL_HEAP_GUARD_HIGH).mapped,
        tracking_consistent: true,
    };

    for record in state.records.iter() {
        if !record.active {
            continue;
        }

        audit.checked_pages = audit.checked_pages.saturating_add(1);

        match record.owner {
            PageOwner::Kernel => audit.kernel_pages = audit.kernel_pages.saturating_add(1),
            PageOwner::Heap => audit.heap_pages = audit.heap_pages.saturating_add(1),
            PageOwner::User => audit.user_pages = audit.user_pages.saturating_add(1),
            PageOwner::Test => audit.test_pages = audit.test_pages.saturating_add(1),
        }

        let translated = translate(record.virt);
        let mut ok = translated.mapped
            && translated.phys == record.phys
            && translated.page_size == PAGE_SIZE_4K
            && translated.user_accessible == (record.flags & ENTRY_USER != 0)
            && translated.writable == (record.flags & ENTRY_WRITABLE != 0)
            && translated.executable == (record.flags & ENTRY_NO_EXECUTE == 0);

        match record.owner {
            PageOwner::User => {
                ok &= in_user_virtual_range(record.virt) && translated.user_accessible;
            }
            PageOwner::Kernel | PageOwner::Heap | PageOwner::Test => {
                ok &= in_managed_virtual_range(record.virt) && !translated.user_accessible;
            }
        }

        if !ok {
            audit.violations = audit.violations.saturating_add(1);
        }
    }

    if audit.checked_pages != state.active_mapping_count() || state.tracking_overflows != 0 {
        audit.tracking_consistent = false;
        audit.violations = audit.violations.saturating_add(1);
    }

    if !audit.guard_pages_intact {
        audit.violations = audit.violations.saturating_add(1);
    }

    audit
}

fn ensure_next_table(
    entry: &mut u64,
    flags: u64,
) -> Result<&'static mut [u64; PAGE_TABLE_ENTRIES], MapError> {
    if entry_present(*entry) {
        if entry_huge(*entry) {
            return Err(MapError::HugePageConflict);
        }

        *entry |= flags & (ENTRY_WRITABLE | ENTRY_USER);
        return table_from_entry_mut(*entry).ok_or(MapError::PhysicalNotIdentityMapped);
    }

    let frame = physmem::allocate_frame().ok_or(MapError::OutOfFrames)?;
    if !identity_reachable(frame) {
        let _ = physmem::free_frame(frame);
        return Err(MapError::PhysicalNotIdentityMapped);
    }

    zero_frame(frame);
    *entry = frame | ENTRY_PRESENT | (flags & (ENTRY_WRITABLE | ENTRY_USER));
    let state = mapper_state_mut();
    state.page_table_frames = state.page_table_frames.saturating_add(1);

    table_from_entry_mut(*entry).ok_or(MapError::PhysicalNotIdentityMapped)
}

fn table_from_entry(entry: u64) -> Option<&'static [u64; PAGE_TABLE_ENTRIES]> {
    let address = entry_addr(entry);

    if !identity_reachable(address) {
        return None;
    }

    Some(unsafe { &*(address as *const [u64; PAGE_TABLE_ENTRIES]) })
}

fn table_from_entry_mut(entry: u64) -> Option<&'static mut [u64; PAGE_TABLE_ENTRIES]> {
    let address = entry_addr(entry);

    if !identity_reachable(address) {
        return None;
    }

    Some(unsafe { &mut *(address as *mut [u64; PAGE_TABLE_ENTRIES]) })
}

fn zero_frame(phys: u64) {
    unsafe {
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE_4K as usize);
    }
}

fn in_managed_virtual_range(virt: u64) -> bool {
    virt >= KERNEL_VIRTUAL_BASE && virt < KERNEL_VIRTUAL_END
}

fn in_user_virtual_range(virt: u64) -> bool {
    (USER_SPACE_BASE..USER_SPACE_END).contains(&virt)
}

fn in_mappable_virtual_range(virt: u64, flags: u64) -> bool {
    if flags & ENTRY_USER != 0 {
        in_user_virtual_range(virt)
    } else {
        in_managed_virtual_range(virt) && !is_guard_page(virt)
    }
}

fn identity_reachable(phys: u64) -> bool {
    phys < BOOT_IDENTITY_MAP_BYTES
}

fn is_page_aligned(value: u64) -> bool {
    value & (PAGE_SIZE_4K - 1) == 0
}

const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

fn in_page(address: u64, page_start: u64) -> bool {
    address >= page_start && address < page_start.saturating_add(PAGE_SIZE_4K)
}

fn is_guard_page(virt: u64) -> bool {
    in_page(virt, KERNEL_HEAP_GUARD_LOW) || in_page(virt, KERNEL_HEAP_GUARD_HIGH)
}

fn entry_present(entry: u64) -> bool {
    entry & ENTRY_PRESENT != 0
}

fn entry_huge(entry: u64) -> bool {
    entry & ENTRY_HUGE_PAGE != 0
}

fn entry_user(entry: u64) -> bool {
    entry & ENTRY_USER != 0
}

fn entry_writable(entry: u64) -> bool {
    entry & ENTRY_WRITABLE != 0
}

fn entry_no_execute(entry: u64) -> bool {
    entry & ENTRY_NO_EXECUTE != 0
}

fn entry_addr(entry: u64) -> u64 {
    entry & ENTRY_ADDR_MASK
}

fn huge_entry_addr(entry: u64) -> u64 {
    entry & HUGE_ENTRY_ADDR_MASK
}

fn mapper_state() -> &'static MapperState {
    unsafe { &*MAPPER_STATE.value.get() }
}

fn mapper_state_mut() -> &'static mut MapperState {
    unsafe { &mut *MAPPER_STATE.value.get() }
}

fn invlpg(virt: u64) {
    unsafe {
        core::arch::asm!(
            "invlpg [{address}]",
            address = in(reg) virt,
            options(nostack, preserves_flags)
        );
    }
}

fn read_cr3() -> u64 {
    let value: u64;

    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }

    value
}
