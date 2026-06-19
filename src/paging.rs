use crate::serial;
use core::sync::atomic::{AtomicBool, Ordering};

pub const PAGE_SIZE_4K: u64 = 4096;
pub const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
pub const BOOT_IDENTITY_MAP_BYTES: u64 = 1024 * 1024 * 1024;
pub const PAGE_TABLE_MEMORY_BYTES: u64 = PAGE_SIZE_4K * 3;

const PAGE_TABLE_ENTRIES: usize = 512;
const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_HUGE_PAGE: u64 = 1 << 7;
const ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const HUGE_ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffe0_0000;

unsafe extern "C" {
    static boot_p4_table: u64;
    static boot_p3_table: u64;
    static boot_p2_table: u64;
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
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
}

#[derive(Clone, Copy)]
pub struct TranslateResult {
    pub virt: u64,
    pub phys: u64,
    pub mapped: bool,
    pub huge_page: bool,
    pub page_size: u64,
}

static PAGING_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn init() -> Snapshot {
    PAGING_INITIALIZED.store(true, Ordering::Release);
    let snapshot = snapshot();

    serial::log("paging", "boot page tables ready");
    serial::log_hex_u64("paging", "cr3", snapshot.cr3);
    serial::log_hex_u64("paging", "p4 table", snapshot.p4_addr);
    serial::log_hex_u64("paging", "p3 table", snapshot.p3_addr);
    serial::log_hex_u64("paging", "p2 table", snapshot.p2_addr);
    serial::log_u64("paging", "huge pages", snapshot.huge_pages);

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

    Snapshot {
        initialized: PAGING_INITIALIZED.load(Ordering::Acquire),
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
    }
}

pub fn translate(virt: u64) -> TranslateResult {
    let p4 = boot_p4();
    let p3 = boot_p3();
    let p2 = boot_p2();

    let p4_index = ((virt >> 39) & 0x1ff) as usize;
    let p3_index = ((virt >> 30) & 0x1ff) as usize;
    let p2_index = ((virt >> 21) & 0x1ff) as usize;
    let huge_offset = virt & (HUGE_PAGE_SIZE - 1);

    if p4_index != 0 || p3_index != 0 {
        return unmapped(virt);
    }

    let p4_entry = p4[p4_index];
    if !entry_present(p4_entry) || entry_addr(p4_entry) != p3_addr() {
        return unmapped(virt);
    }

    let p3_entry = p3[p3_index];
    if !entry_present(p3_entry) || entry_addr(p3_entry) != p2_addr() {
        return unmapped(virt);
    }

    let p2_entry = p2[p2_index];
    if !entry_present(p2_entry) {
        return unmapped(virt);
    }

    if entry_huge(p2_entry) {
        return TranslateResult {
            virt,
            phys: huge_entry_addr(p2_entry).saturating_add(huge_offset),
            mapped: true,
            huge_page: true,
            page_size: HUGE_PAGE_SIZE,
        };
    }

    TranslateResult {
        virt,
        phys: entry_addr(p2_entry).saturating_add(virt & (PAGE_SIZE_4K - 1)),
        mapped: true,
        huge_page: false,
        page_size: PAGE_SIZE_4K,
    }
}

fn unmapped(virt: u64) -> TranslateResult {
    TranslateResult {
        virt,
        phys: 0,
        mapped: false,
        huge_page: false,
        page_size: 0,
    }
}

fn boot_p4() -> &'static [u64; PAGE_TABLE_ENTRIES] {
    unsafe { &*(p4_addr() as *const [u64; PAGE_TABLE_ENTRIES]) }
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

fn entry_present(entry: u64) -> bool {
    entry & ENTRY_PRESENT != 0
}

fn entry_huge(entry: u64) -> bool {
    entry & ENTRY_HUGE_PAGE != 0
}

fn entry_addr(entry: u64) -> u64 {
    entry & ENTRY_ADDR_MASK
}

fn huge_entry_addr(entry: u64) -> u64 {
    entry & HUGE_ENTRY_ADDR_MASK
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
