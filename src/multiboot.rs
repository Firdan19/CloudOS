use core::cell::UnsafeCell;

pub const MULTIBOOT2_BOOTLOADER_MAGIC: u32 = 0x36d7_6289;

const MAX_BOOT_INFO_ADDRESS: u64 = 1024 * 1024 * 1024;
const MAX_BOOT_INFO_SIZE: u32 = 1024 * 1024;
const MAX_TEXT_LEN: usize = 96;
const MAX_REGIONS: usize = 16;
const MAX_MODULES: usize = 4;

const TAG_END: u32 = 0;
const TAG_COMMAND_LINE: u32 = 1;
const TAG_BOOTLOADER_NAME: u32 = 2;
const TAG_MODULE: u32 = 3;
const TAG_BASIC_MEMORY: u32 = 4;
const TAG_MEMORY_MAP: u32 = 6;

const MEMORY_AVAILABLE: u32 = 1;
const MEMORY_ACPI_RECLAIMABLE: u32 = 3;
const MEMORY_ACPI_NVS: u32 = 4;
const MEMORY_BAD: u32 = 5;

#[derive(Clone, Copy)]
pub struct TextField {
    bytes: [u8; MAX_TEXT_LEN],
    len: usize,
}

impl TextField {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; MAX_TEXT_LEN],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    fn set_from_c_string(&mut self, address: u64, max_len: usize) {
        self.len = 0;

        let copy_len = max_len.min(MAX_TEXT_LEN);
        for index in 0..copy_len {
            let byte = unsafe { core::ptr::read((address as usize + index) as *const u8) };

            if byte == 0 {
                break;
            }

            self.bytes[index] = if byte.is_ascii_graphic() || byte == b' ' {
                byte
            } else {
                b'?'
            };
            self.len += 1;
        }
    }
}

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub base_addr: u64,
    pub length: u64,
    pub region_type: u32,
}

#[derive(Clone, Copy)]
pub struct BootModule {
    pub start: u64,
    pub end: u64,
    pub command_line: TextField,
}

impl BootModule {
    pub const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            command_line: TextField::empty(),
        }
    }

    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_valid(&self) -> bool {
        self.start != 0
            && self.end > self.start
            && self.end <= MAX_BOOT_INFO_ADDRESS
            && self.size() <= MAX_BOOT_INFO_ADDRESS
    }
}

impl MemoryRegion {
    pub const fn empty() -> Self {
        Self {
            base_addr: 0,
            length: 0,
            region_type: 0,
        }
    }

    pub fn end_addr(&self) -> u64 {
        self.base_addr.saturating_add(self.length)
    }

    pub fn type_name(&self) -> &'static str {
        match self.region_type {
            MEMORY_AVAILABLE => "available",
            MEMORY_ACPI_RECLAIMABLE => "acpi reclaim",
            MEMORY_ACPI_NVS => "acpi nvs",
            MEMORY_BAD => "bad",
            _ => "reserved",
        }
    }

    pub fn is_available(&self) -> bool {
        self.region_type == MEMORY_AVAILABLE
    }
}

#[derive(Clone, Copy)]
pub struct MemorySummary {
    pub has_basic_memory: bool,
    pub has_memory_map: bool,
    pub mem_lower_kib: u32,
    pub mem_upper_kib: u32,
    pub entry_size: u32,
    pub entry_version: u32,
    pub region_count: u32,
    pub stored_region_count: u32,
    pub usable_region_count: u32,
    pub reserved_region_count: u32,
    pub acpi_region_count: u32,
    pub bad_region_count: u32,
    pub usable_bytes: u64,
    pub reserved_bytes: u64,
    pub acpi_bytes: u64,
    pub bad_bytes: u64,
    pub highest_address: u64,
    pub first_usable_base: u64,
    pub first_usable_length: u64,
    pub largest_usable_base: u64,
    pub largest_usable_length: u64,
    pub regions: [MemoryRegion; MAX_REGIONS],
}

impl MemorySummary {
    pub const fn empty() -> Self {
        Self {
            has_basic_memory: false,
            has_memory_map: false,
            mem_lower_kib: 0,
            mem_upper_kib: 0,
            entry_size: 0,
            entry_version: 0,
            region_count: 0,
            stored_region_count: 0,
            usable_region_count: 0,
            reserved_region_count: 0,
            acpi_region_count: 0,
            bad_region_count: 0,
            usable_bytes: 0,
            reserved_bytes: 0,
            acpi_bytes: 0,
            bad_bytes: 0,
            highest_address: 0,
            first_usable_base: 0,
            first_usable_length: 0,
            largest_usable_base: 0,
            largest_usable_length: 0,
            regions: [MemoryRegion::empty(); MAX_REGIONS],
        }
    }

    fn add_region(&mut self, region: MemoryRegion) {
        if region.length == 0 {
            return;
        }

        self.region_count += 1;
        self.highest_address = self.highest_address.max(region.end_addr());

        let stored_index = self.stored_region_count as usize;
        if stored_index < MAX_REGIONS {
            self.regions[stored_index] = region;
            self.stored_region_count += 1;
        }

        match region.region_type {
            MEMORY_AVAILABLE => {
                if self.usable_region_count == 0 {
                    self.first_usable_base = region.base_addr;
                    self.first_usable_length = region.length;
                }

                self.usable_region_count += 1;
                self.usable_bytes = self.usable_bytes.saturating_add(region.length);

                if region.length > self.largest_usable_length {
                    self.largest_usable_base = region.base_addr;
                    self.largest_usable_length = region.length;
                }
            }
            MEMORY_ACPI_RECLAIMABLE | MEMORY_ACPI_NVS => {
                self.acpi_region_count += 1;
                self.acpi_bytes = self.acpi_bytes.saturating_add(region.length);
            }
            MEMORY_BAD => {
                self.bad_region_count += 1;
                self.bad_bytes = self.bad_bytes.saturating_add(region.length);
            }
            _ => {
                self.reserved_region_count += 1;
                self.reserved_bytes = self.reserved_bytes.saturating_add(region.length);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct BootInfoSummary {
    pub magic: u32,
    pub address: u64,
    pub valid_magic: bool,
    pub parsed: bool,
    pub total_size: u32,
    pub reserved: u32,
    pub tag_count: u32,
    pub has_end_tag: bool,
    pub bootloader_name: TextField,
    pub command_line: TextField,
    pub module_count: u32,
    pub stored_module_count: u32,
    pub highest_module_end: u64,
    pub modules: [BootModule; MAX_MODULES],
    pub memory: MemorySummary,
}

impl BootInfoSummary {
    pub const fn empty() -> Self {
        Self {
            magic: 0,
            address: 0,
            valid_magic: false,
            parsed: false,
            total_size: 0,
            reserved: 0,
            tag_count: 0,
            has_end_tag: false,
            bootloader_name: TextField::empty(),
            command_line: TextField::empty(),
            module_count: 0,
            stored_module_count: 0,
            highest_module_end: 0,
            modules: [BootModule::empty(); MAX_MODULES],
            memory: MemorySummary::empty(),
        }
    }

    fn add_module(&mut self, module: BootModule) {
        if !module.is_valid() {
            return;
        }

        self.module_count = self.module_count.saturating_add(1);
        self.highest_module_end = self.highest_module_end.max(module.end);

        let index = self.stored_module_count as usize;
        if index < MAX_MODULES {
            self.modules[index] = module;
            self.stored_module_count = self.stored_module_count.saturating_add(1);
        }
    }
}

struct BootInfoStore {
    value: UnsafeCell<BootInfoSummary>,
}

unsafe impl Sync for BootInfoStore {}

static BOOT_INFO: BootInfoStore = BootInfoStore {
    value: UnsafeCell::new(BootInfoSummary::empty()),
};

pub fn init(magic: u32, address: u64) -> BootInfoSummary {
    let summary = unsafe { parse(magic, address) };

    unsafe {
        *BOOT_INFO.value.get() = summary;
    }

    summary
}

pub fn summary() -> BootInfoSummary {
    unsafe { *BOOT_INFO.value.get() }
}

pub fn region(index: usize) -> Option<MemoryRegion> {
    let summary = summary();

    if index >= summary.memory.stored_region_count as usize {
        return None;
    }

    Some(summary.memory.regions[index])
}

pub fn stored_region_count() -> usize {
    summary().memory.stored_region_count as usize
}

pub fn boot_module(index: usize) -> Option<BootModule> {
    let summary = summary();
    if index >= summary.stored_module_count as usize {
        return None;
    }

    Some(summary.modules[index])
}

pub fn find_module(command_line: &str) -> Option<BootModule> {
    let summary = summary();
    for index in 0..summary.stored_module_count as usize {
        let module = summary.modules[index];
        if module.command_line.as_str() == command_line {
            return Some(module);
        }
    }

    None
}

pub fn highest_module_end() -> u64 {
    summary().highest_module_end
}

unsafe fn parse(magic: u32, address: u64) -> BootInfoSummary {
    let mut summary = BootInfoSummary::empty();
    summary.magic = magic;
    summary.address = address;
    summary.valid_magic = magic == MULTIBOOT2_BOOTLOADER_MAGIC;

    if !summary.valid_magic || address == 0 || address >= MAX_BOOT_INFO_ADDRESS {
        return summary;
    }

    let total_size = unsafe { read_u32(address) };
    let reserved = unsafe { read_u32(address + 4) };

    summary.total_size = total_size;
    summary.reserved = reserved;

    if !(16..=MAX_BOOT_INFO_SIZE).contains(&total_size) {
        return summary;
    }

    if address.saturating_add(total_size as u64) >= MAX_BOOT_INFO_ADDRESS {
        return summary;
    }

    let mut offset = 8usize;
    let total_size = total_size as usize;

    while offset + 8 <= total_size {
        let tag_address = address + offset as u64;
        let tag_type = unsafe { read_u32(tag_address) };
        let tag_size = unsafe { read_u32(tag_address + 4) };

        if tag_size < 8 {
            break;
        }

        let tag_end = offset.saturating_add(tag_size as usize);
        if tag_end > total_size || tag_end < offset {
            break;
        }

        summary.tag_count += 1;

        match tag_type {
            TAG_END => {
                summary.has_end_tag = tag_size == 8;
                break;
            }
            TAG_COMMAND_LINE => {
                summary
                    .command_line
                    .set_from_c_string(tag_address + 8, tag_size as usize - 8);
            }
            TAG_BOOTLOADER_NAME => {
                summary
                    .bootloader_name
                    .set_from_c_string(tag_address + 8, tag_size as usize - 8);
            }
            TAG_MODULE => {
                parse_module(&mut summary, tag_address, tag_size);
            }
            TAG_BASIC_MEMORY => {
                parse_basic_memory(&mut summary, tag_address, tag_size);
            }
            TAG_MEMORY_MAP => {
                parse_memory_map(&mut summary, tag_address, tag_size);
            }
            _ => {}
        }

        let next_offset = align_up(offset.saturating_add(tag_size as usize), 8);
        if next_offset <= offset {
            break;
        }
        offset = next_offset;
    }

    summary.parsed = summary.has_end_tag;
    summary
}

fn parse_module(summary: &mut BootInfoSummary, tag_address: u64, tag_size: u32) {
    if tag_size < 17 {
        return;
    }
    if unsafe { core::ptr::read((tag_address + tag_size as u64 - 1) as *const u8) } != 0 {
        return;
    }

    let mut module = BootModule {
        start: unsafe { read_u32(tag_address + 8) } as u64,
        end: unsafe { read_u32(tag_address + 12) } as u64,
        command_line: TextField::empty(),
    };
    module
        .command_line
        .set_from_c_string(tag_address + 16, tag_size as usize - 16);
    summary.add_module(module);
}

fn parse_basic_memory(summary: &mut BootInfoSummary, tag_address: u64, tag_size: u32) {
    if tag_size < 16 {
        return;
    }

    summary.memory.has_basic_memory = true;
    summary.memory.mem_lower_kib = unsafe { read_u32(tag_address + 8) };
    summary.memory.mem_upper_kib = unsafe { read_u32(tag_address + 12) };
}

fn parse_memory_map(summary: &mut BootInfoSummary, tag_address: u64, tag_size: u32) {
    if tag_size < 16 {
        return;
    }

    let entry_size = unsafe { read_u32(tag_address + 8) };
    let entry_version = unsafe { read_u32(tag_address + 12) };

    if entry_size < 24 {
        return;
    }

    summary.memory.has_memory_map = true;
    summary.memory.entry_size = entry_size;
    summary.memory.entry_version = entry_version;

    let entries_start = tag_address + 16;
    let entries_len = tag_size as usize - 16;
    let mut offset = 0usize;

    while offset + 24 <= entries_len {
        let entry_address = entries_start + offset as u64;
        let region = MemoryRegion {
            base_addr: unsafe { read_u64(entry_address) },
            length: unsafe { read_u64(entry_address + 8) },
            region_type: unsafe { read_u32(entry_address + 16) },
        };

        summary.memory.add_region(region);
        offset = offset.saturating_add(entry_size as usize);
    }
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

unsafe fn read_u32(address: u64) -> u32 {
    unsafe { core::ptr::read_unaligned(address as usize as *const u32) }
}

unsafe fn read_u64(address: u64) -> u64 {
    unsafe { core::ptr::read_unaligned(address as usize as *const u64) }
}
