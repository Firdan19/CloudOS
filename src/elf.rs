use crate::{initramfs, paging, physmem, serial, user_program};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const MAX_PROGRAM_HEADERS: usize = 32;
const MAX_LOAD_SEGMENTS: usize = 8;
const MAX_LOAD_PAGES: usize = 64;
const MAX_MAPPED_PAGES: usize = MAX_LOAD_PAGES + 1;
const PT_LOAD: u32 = 1;
const PF_EXECUTE: u32 = 1;
const PF_WRITE: u32 = 2;
const PF_READ: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    AlreadyLoaded,
    InitProgramMissing,
    ImageTooSmall,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndianness,
    UnsupportedIdentVersion,
    UnsupportedType,
    UnsupportedMachine,
    UnsupportedVersion,
    InvalidHeaderSize,
    InvalidProgramHeaderSize,
    InvalidProgramHeaderCount,
    ProgramHeaderTableOutsideImage,
    NoLoadSegments,
    TooManyLoadSegments,
    SegmentFileLargerThanMemory,
    SegmentOutsideImage,
    SegmentAddressOverflow,
    SegmentOutsideUserRegion,
    SegmentAlignmentInvalid,
    SegmentMissingReadPermission,
    WritableExecutableSegment,
    SegmentPageOverlap,
    TooManyLoadPages,
    EntryOutsideExecutableSegment,
    TargetPageAlreadyMapped,
    PageMappingFailed(paging::MapError),
    StackMappingFailed(paging::MapError),
    AddressSpaceFailed(paging::AddressSpaceError),
    MappingLost,
}

#[derive(Clone, Copy)]
pub struct LoadedImage {
    pub entry_point: u64,
    pub image_start: u64,
    pub image_end: u64,
    pub stack_top: u64,
    pub load_segments: u64,
    pub mapped_pages: u64,
    pub writable_pages: u64,
    pub executable_pages: u64,
}

pub struct ProcessImage {
    pub address_space: paging::AddressSpace,
    pub entry_point: u64,
    pub stack_top: u64,
    pub mapped_pages: u64,
    pub table_frames: u64,
    pub first_user_frame: u64,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub loaded: bool,
    pub entry_point: u64,
    pub image_start: u64,
    pub image_end: u64,
    pub stack_top: u64,
    pub load_segments: u64,
    pub mapped_pages: u64,
    pub writable_pages: u64,
    pub executable_pages: u64,
    pub last_error: Option<LoadError>,
}

#[derive(Clone, Copy)]
struct SegmentPlan {
    file_offset: u64,
    virtual_address: u64,
    file_size: u64,
    memory_size: u64,
    writable: bool,
    executable: bool,
}

impl SegmentPlan {
    const fn empty() -> Self {
        Self {
            file_offset: 0,
            virtual_address: 0,
            file_size: 0,
            memory_size: 0,
            writable: false,
            executable: false,
        }
    }

    fn page_start(self) -> u64 {
        align_down(self.virtual_address, paging::PAGE_SIZE_4K)
    }

    fn memory_end(self) -> u64 {
        self.virtual_address + self.memory_size
    }

    fn page_end(self) -> u64 {
        align_up(self.memory_end(), paging::PAGE_SIZE_4K)
    }

    fn permissions(self) -> paging::UserPagePermissions {
        paging::UserPagePermissions {
            writable: self.writable,
            executable: self.executable,
        }
    }
}

struct ImagePlan {
    entry_point: u64,
    image_start: u64,
    image_end: u64,
    total_pages: usize,
    segment_count: usize,
    segments: [SegmentPlan; MAX_LOAD_SEGMENTS],
}

struct LoaderState {
    initialized: bool,
    loaded: Option<LoadedImage>,
    last_error: Option<LoadError>,
}

impl LoaderState {
    const fn new() -> Self {
        Self {
            initialized: false,
            loaded: None,
            last_error: None,
        }
    }
}

struct LoaderStore {
    value: UnsafeCell<LoaderState>,
}

unsafe impl Sync for LoaderStore {}

static LOADER: LoaderStore = LoaderStore {
    value: UnsafeCell::new(LoaderState::new()),
};

pub fn init() -> Snapshot {
    let result = cpu_interrupts::without_interrupts(|| {
        let state = loader_state_mut();
        if state.initialized {
            return state.loaded.ok_or(LoadError::AlreadyLoaded);
        }

        state.initialized = true;
        let result = initramfs::find("/bin/init")
            .ok_or(LoadError::InitProgramMissing)
            .and_then(|file| load_image(file.data));
        match result {
            Ok(image) => {
                state.loaded = Some(image);
                state.last_error = None;
                Ok(image)
            }
            Err(error) => {
                state.last_error = Some(error);
                Err(error)
            }
        }
    });

    match result {
        Ok(image) => {
            serial::log("elf", "ELF64 user image loaded");
            serial::log_hex_u64("elf", "entry point", image.entry_point);
            serial::log_u64("elf", "PT_LOAD segments", image.load_segments);
            serial::log_u64("elf", "mapped pages", image.mapped_pages);
            serial::log("elf", "W^X permissions active");
        }
        Err(error) => {
            serial::log_bytes("elf", "load failed", load_error_name(error).as_bytes());
        }
    }

    snapshot()
}

pub fn snapshot() -> Snapshot {
    cpu_interrupts::without_interrupts(|| {
        let state = loader_state();
        let loaded = state.loaded;
        Snapshot {
            initialized: state.initialized,
            loaded: loaded.is_some(),
            entry_point: loaded.map(|image| image.entry_point).unwrap_or(0),
            image_start: loaded.map(|image| image.image_start).unwrap_or(0),
            image_end: loaded.map(|image| image.image_end).unwrap_or(0),
            stack_top: loaded.map(|image| image.stack_top).unwrap_or(0),
            load_segments: loaded.map(|image| image.load_segments).unwrap_or(0),
            mapped_pages: loaded.map(|image| image.mapped_pages).unwrap_or(0),
            writable_pages: loaded.map(|image| image.writable_pages).unwrap_or(0),
            executable_pages: loaded.map(|image| image.executable_pages).unwrap_or(0),
            last_error: state.last_error,
        }
    })
}

pub fn loaded_image() -> Option<LoadedImage> {
    cpu_interrupts::without_interrupts(|| loader_state().loaded)
}

pub fn create_process_image() -> Result<ProcessImage, LoadError> {
    let file = initramfs::find("/bin/init").ok_or(LoadError::InitProgramMissing)?;
    let plan = validate_image(file.data)?;
    let mut address_space =
        paging::AddressSpace::create().map_err(LoadError::AddressSpaceFailed)?;

    let setup = (|| {
        for segment_index in 0..plan.segment_count {
            let segment = plan.segments[segment_index];
            let mut page = segment.page_start();
            while page < segment.page_end() {
                address_space
                    .map_user_page(page, segment.permissions())
                    .map_err(LoadError::AddressSpaceFailed)?;
                page += paging::PAGE_SIZE_4K;
            }
        }

        address_space
            .map_user_page(
                paging::USER_ELF_STACK_PAGE,
                paging::UserPagePermissions::READ_WRITE,
            )
            .map_err(LoadError::AddressSpaceFailed)?;

        for segment_index in 0..plan.segment_count {
            copy_segment_to_address_space(file.data, plan.segments[segment_index], &address_space)?;
        }

        let audit = address_space.audit();
        if !audit.valid_root
            || audit.user_pages != address_space.mapped_pages()
            || audit.writable_executable_pages != 0
            || !audit.stack_guard_intact
        {
            return Err(LoadError::MappingLost);
        }

        Ok(())
    })();

    if let Err(error) = setup {
        let _ = address_space.destroy();
        return Err(error);
    }

    Ok(ProcessImage {
        entry_point: plan.entry_point,
        stack_top: paging::USER_ELF_STACK_TOP,
        mapped_pages: address_space.mapped_pages(),
        table_frames: address_space.table_frames(),
        first_user_frame: address_space.first_user_frame(),
        address_space,
    })
}

pub fn selftest() -> bool {
    const INVALID_IMAGE: [u8; ELF_HEADER_SIZE] = [0; ELF_HEADER_SIZE];

    let snapshot = snapshot();
    let text = paging::translate(snapshot.entry_point);
    let data = paging::translate(paging::USER_ELF_BASE + paging::PAGE_SIZE_4K);
    let stack = paging::translate(paging::USER_ELF_STACK_PAGE);
    let init = initramfs::find("/bin/init");

    snapshot.initialized
        && snapshot.loaded
        && snapshot.last_error.is_none()
        && snapshot.load_segments == 2
        && snapshot.mapped_pages == 3
        && snapshot.writable_pages == 2
        && snapshot.executable_pages == 1
        && text.mapped
        && text.user_accessible
        && !text.writable
        && text.executable
        && data.mapped
        && data.user_accessible
        && data.writable
        && !data.executable
        && stack.mapped
        && stack.user_accessible
        && stack.writable
        && !stack.executable
        && !paging::translate(paging::USER_ELF_STACK_GUARD).mapped
        && mapped_bytes_equal(paging::USER_ELF_BASE, &user_program::INIT_TEXT_SIGNATURE)
        && mapped_bytes_equal(
            paging::USER_ELF_BASE + paging::PAGE_SIZE_4K,
            &user_program::INIT_DATA_SIGNATURE,
        )
        && mapped_byte(
            paging::USER_ELF_BASE + paging::PAGE_SIZE_4K + user_program::INIT_BSS_PROBE_OFFSET,
        ) == Some(0)
        && initramfs::selftest()
        && init
            .map(|file| validate_image(file.data).is_ok())
            .unwrap_or(false)
        && matches!(validate_image(&INVALID_IMAGE), Err(LoadError::BadMagic))
        && matches!(
            validate_image(&INVALID_IMAGE[..32]),
            Err(LoadError::ImageTooSmall)
        )
}

pub fn load_error_name(error: LoadError) -> &'static str {
    match error {
        LoadError::AlreadyLoaded => "ELF image is already loaded",
        LoadError::InitProgramMissing => "/bin/init is missing from initramfs",
        LoadError::ImageTooSmall => "ELF image is smaller than its header",
        LoadError::BadMagic => "ELF magic is invalid",
        LoadError::UnsupportedClass => "ELF class is not 64-bit",
        LoadError::UnsupportedEndianness => "ELF byte order is not little-endian",
        LoadError::UnsupportedIdentVersion => "ELF identification version is unsupported",
        LoadError::UnsupportedType => "ELF type is not executable",
        LoadError::UnsupportedMachine => "ELF machine is not x86_64",
        LoadError::UnsupportedVersion => "ELF version is unsupported",
        LoadError::InvalidHeaderSize => "ELF header size is invalid",
        LoadError::InvalidProgramHeaderSize => "ELF program header size is invalid",
        LoadError::InvalidProgramHeaderCount => "ELF program header count is invalid",
        LoadError::ProgramHeaderTableOutsideImage => "ELF program header table is truncated",
        LoadError::NoLoadSegments => "ELF has no PT_LOAD segments",
        LoadError::TooManyLoadSegments => "ELF has too many PT_LOAD segments",
        LoadError::SegmentFileLargerThanMemory => "ELF segment file size exceeds memory size",
        LoadError::SegmentOutsideImage => "ELF segment data is truncated",
        LoadError::SegmentAddressOverflow => "ELF segment address overflows",
        LoadError::SegmentOutsideUserRegion => "ELF segment is outside the user load region",
        LoadError::SegmentAlignmentInvalid => "ELF segment alignment is invalid",
        LoadError::SegmentMissingReadPermission => "ELF PT_LOAD segment is not readable",
        LoadError::WritableExecutableSegment => "ELF segment violates W^X",
        LoadError::SegmentPageOverlap => "ELF PT_LOAD pages overlap",
        LoadError::TooManyLoadPages => "ELF image exceeds loader page capacity",
        LoadError::EntryOutsideExecutableSegment => "ELF entry is outside executable memory",
        LoadError::TargetPageAlreadyMapped => "ELF target page is already mapped",
        LoadError::PageMappingFailed(error) => paging::map_error_name(error),
        LoadError::StackMappingFailed(error) => paging::map_error_name(error),
        LoadError::AddressSpaceFailed(error) => paging::address_space_error_name(error),
        LoadError::MappingLost => "ELF mapping disappeared during load",
    }
}

fn load_image(image: &[u8]) -> Result<LoadedImage, LoadError> {
    let plan = validate_image(image)?;
    let mut mapped = [0u64; MAX_MAPPED_PAGES];
    let mut mapped_count = 0usize;
    let mut writable_pages = 0u64;
    let mut executable_pages = 0u64;

    for segment_index in 0..plan.segment_count {
        let segment = plan.segments[segment_index];
        let mut page = segment.page_start();
        while page < segment.page_end() {
            if paging::translate(page).mapped {
                rollback_mappings(&mapped, mapped_count);
                return Err(LoadError::TargetPageAlreadyMapped);
            }

            match paging::map_new_user_page(page, segment.permissions()) {
                Ok(_) => {
                    mapped[mapped_count] = page;
                    mapped_count += 1;
                    if segment.writable {
                        writable_pages = writable_pages.saturating_add(1);
                    }
                    if segment.executable {
                        executable_pages = executable_pages.saturating_add(1);
                    }
                }
                Err(error) => {
                    rollback_mappings(&mapped, mapped_count);
                    return Err(LoadError::PageMappingFailed(error));
                }
            }
            page += paging::PAGE_SIZE_4K;
        }
    }

    if paging::translate(paging::USER_ELF_STACK_PAGE).mapped {
        rollback_mappings(&mapped, mapped_count);
        return Err(LoadError::TargetPageAlreadyMapped);
    }

    match paging::map_new_user_page(
        paging::USER_ELF_STACK_PAGE,
        paging::UserPagePermissions::READ_WRITE,
    ) {
        Ok(_) => {
            mapped[mapped_count] = paging::USER_ELF_STACK_PAGE;
            mapped_count += 1;
            writable_pages = writable_pages.saturating_add(1);
        }
        Err(error) => {
            rollback_mappings(&mapped, mapped_count);
            return Err(LoadError::StackMappingFailed(error));
        }
    }

    for segment_index in 0..plan.segment_count {
        if let Err(error) = copy_segment(image, plan.segments[segment_index]) {
            rollback_mappings(&mapped, mapped_count);
            return Err(error);
        }
    }

    Ok(LoadedImage {
        entry_point: plan.entry_point,
        image_start: plan.image_start,
        image_end: plan.image_end,
        stack_top: paging::USER_ELF_STACK_TOP,
        load_segments: plan.segment_count as u64,
        mapped_pages: mapped_count as u64,
        writable_pages,
        executable_pages,
    })
}

fn validate_image(image: &[u8]) -> Result<ImagePlan, LoadError> {
    if image.len() < ELF_HEADER_SIZE {
        return Err(LoadError::ImageTooSmall);
    }
    if image[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(LoadError::BadMagic);
    }
    if image[4] != 2 {
        return Err(LoadError::UnsupportedClass);
    }
    if image[5] != 1 {
        return Err(LoadError::UnsupportedEndianness);
    }
    if image[6] != 1 {
        return Err(LoadError::UnsupportedIdentVersion);
    }
    if read_u16(image, 16)? != 2 {
        return Err(LoadError::UnsupportedType);
    }
    if read_u16(image, 18)? != 62 {
        return Err(LoadError::UnsupportedMachine);
    }
    if read_u32(image, 20)? != 1 {
        return Err(LoadError::UnsupportedVersion);
    }
    if read_u16(image, 52)? as usize != ELF_HEADER_SIZE {
        return Err(LoadError::InvalidHeaderSize);
    }
    if read_u16(image, 54)? as usize != PROGRAM_HEADER_SIZE {
        return Err(LoadError::InvalidProgramHeaderSize);
    }

    let entry_point = read_u64(image, 24)?;
    let program_header_offset = read_u64(image, 32)?;
    let program_header_count = read_u16(image, 56)? as usize;
    if program_header_count == 0 || program_header_count > MAX_PROGRAM_HEADERS {
        return Err(LoadError::InvalidProgramHeaderCount);
    }

    let table_size = program_header_count
        .checked_mul(PROGRAM_HEADER_SIZE)
        .ok_or(LoadError::ProgramHeaderTableOutsideImage)?;
    let table_start = usize::try_from(program_header_offset)
        .map_err(|_| LoadError::ProgramHeaderTableOutsideImage)?;
    let table_end = table_start
        .checked_add(table_size)
        .ok_or(LoadError::ProgramHeaderTableOutsideImage)?;
    if table_end > image.len() {
        return Err(LoadError::ProgramHeaderTableOutsideImage);
    }

    let mut plan = ImagePlan {
        entry_point,
        image_start: u64::MAX,
        image_end: 0,
        total_pages: 0,
        segment_count: 0,
        segments: [SegmentPlan::empty(); MAX_LOAD_SEGMENTS],
    };
    let mut entry_is_executable = false;

    for index in 0..program_header_count {
        let offset = table_start + index * PROGRAM_HEADER_SIZE;
        if read_u32(image, offset)? != PT_LOAD {
            continue;
        }

        let flags = read_u32(image, offset + 4)?;
        let file_offset = read_u64(image, offset + 8)?;
        let virtual_address = read_u64(image, offset + 16)?;
        let file_size = read_u64(image, offset + 32)?;
        let memory_size = read_u64(image, offset + 40)?;
        let alignment = read_u64(image, offset + 48)?;

        if file_size > memory_size {
            return Err(LoadError::SegmentFileLargerThanMemory);
        }
        if memory_size == 0 {
            continue;
        }

        let file_end = file_offset
            .checked_add(file_size)
            .ok_or(LoadError::SegmentOutsideImage)?;
        if file_end > image.len() as u64 {
            return Err(LoadError::SegmentOutsideImage);
        }

        let memory_end = virtual_address
            .checked_add(memory_size)
            .ok_or(LoadError::SegmentAddressOverflow)?;
        if virtual_address < paging::USER_ELF_BASE || memory_end > paging::USER_ELF_END {
            return Err(LoadError::SegmentOutsideUserRegion);
        }

        if alignment > 1
            && (!alignment.is_power_of_two()
                || virtual_address % alignment != file_offset % alignment)
        {
            return Err(LoadError::SegmentAlignmentInvalid);
        }
        if flags & PF_READ == 0 {
            return Err(LoadError::SegmentMissingReadPermission);
        }

        let writable = flags & PF_WRITE != 0;
        let executable = flags & PF_EXECUTE != 0;
        if writable && executable {
            return Err(LoadError::WritableExecutableSegment);
        }

        let segment = SegmentPlan {
            file_offset,
            virtual_address,
            file_size,
            memory_size,
            writable,
            executable,
        };

        if plan.segment_count >= MAX_LOAD_SEGMENTS {
            return Err(LoadError::TooManyLoadSegments);
        }

        for existing_index in 0..plan.segment_count {
            let existing = plan.segments[existing_index];
            if segment.page_start() < existing.page_end()
                && existing.page_start() < segment.page_end()
            {
                return Err(LoadError::SegmentPageOverlap);
            }
        }

        let pages = ((segment.page_end() - segment.page_start()) / paging::PAGE_SIZE_4K) as usize;
        plan.total_pages = plan
            .total_pages
            .checked_add(pages)
            .ok_or(LoadError::TooManyLoadPages)?;
        if plan.total_pages > MAX_LOAD_PAGES {
            return Err(LoadError::TooManyLoadPages);
        }

        if executable && entry_point >= virtual_address && entry_point < memory_end {
            entry_is_executable = true;
        }

        plan.image_start = plan.image_start.min(segment.page_start());
        plan.image_end = plan.image_end.max(segment.page_end());
        plan.segments[plan.segment_count] = segment;
        plan.segment_count += 1;
    }

    if plan.segment_count == 0 {
        return Err(LoadError::NoLoadSegments);
    }
    if !entry_is_executable {
        return Err(LoadError::EntryOutsideExecutableSegment);
    }

    Ok(plan)
}

fn copy_segment(image: &[u8], segment: SegmentPlan) -> Result<(), LoadError> {
    let mut copied = 0u64;
    while copied < segment.file_size {
        let virtual_address = segment.virtual_address + copied;
        let translation = paging::translate(virtual_address);
        if !translation.mapped || !translation.user_accessible {
            return Err(LoadError::MappingLost);
        }

        let page_offset = virtual_address & (paging::PAGE_SIZE_4K - 1);
        let available = paging::PAGE_SIZE_4K - page_offset;
        let chunk = available.min(segment.file_size - copied);
        let source_offset = (segment.file_offset + copied) as usize;

        unsafe {
            core::ptr::copy_nonoverlapping(
                image.as_ptr().add(source_offset),
                translation.phys as *mut u8,
                chunk as usize,
            );
        }
        copied += chunk;
    }

    Ok(())
}

fn copy_segment_to_address_space(
    image: &[u8],
    segment: SegmentPlan,
    address_space: &paging::AddressSpace,
) -> Result<(), LoadError> {
    let mut copied = 0u64;
    while copied < segment.file_size {
        let virtual_address = segment.virtual_address + copied;
        let translation = address_space.translate(virtual_address);
        if !translation.mapped || !translation.user_accessible {
            return Err(LoadError::MappingLost);
        }

        let page_offset = virtual_address & (paging::PAGE_SIZE_4K - 1);
        let available = paging::PAGE_SIZE_4K - page_offset;
        let chunk = available.min(segment.file_size - copied);
        let source_offset = (segment.file_offset + copied) as usize;

        unsafe {
            core::ptr::copy_nonoverlapping(
                image.as_ptr().add(source_offset),
                translation.phys as *mut u8,
                chunk as usize,
            );
        }
        copied += chunk;
    }

    Ok(())
}

fn rollback_mappings(mapped: &[u64; MAX_MAPPED_PAGES], count: usize) {
    let mut remaining = count;
    while remaining > 0 {
        remaining -= 1;
        if let Ok(frame) = paging::unmap_page(mapped[remaining]) {
            let _ = physmem::free_frame(frame);
        }
    }
}

fn mapped_bytes_equal(virtual_address: u64, expected: &[u8]) -> bool {
    for (index, expected_byte) in expected.iter().copied().enumerate() {
        if mapped_byte(virtual_address + index as u64) != Some(expected_byte) {
            return false;
        }
    }

    true
}

fn mapped_byte(virtual_address: u64) -> Option<u8> {
    let translation = paging::translate(virtual_address);
    if !translation.mapped {
        return None;
    }

    Some(unsafe { core::ptr::read_volatile(translation.phys as *const u8) })
}

fn read_u16(image: &[u8], offset: usize) -> Result<u16, LoadError> {
    let bytes = image
        .get(offset..offset + 2)
        .ok_or(LoadError::ImageTooSmall)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(image: &[u8], offset: usize) -> Result<u32, LoadError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or(LoadError::ImageTooSmall)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(image: &[u8], offset: usize) -> Result<u64, LoadError> {
    let bytes = image
        .get(offset..offset + 8)
        .ok_or(LoadError::ImageTooSmall)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value
        .checked_add(alignment - 1)
        .map(|aligned| align_down(aligned, alignment))
        .unwrap_or(u64::MAX & !(alignment - 1))
}

fn loader_state() -> &'static LoaderState {
    unsafe { &*LOADER.value.get() }
}

fn loader_state_mut() -> &'static mut LoaderState {
    unsafe { &mut *LOADER.value.get() }
}
