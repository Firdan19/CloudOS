use crate::{multiboot, serial};
use core::cell::UnsafeCell;

const CPIO_NEWC_MAGIC: &[u8; 6] = b"070701";
const CPIO_HEADER_SIZE: usize = 110;
const CPIO_TRAILER: &[u8] = b"TRAILER!!!";
const INIT_PATH: &[u8] = b"bin/init";
const MAX_ARCHIVE_SIZE: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_PATH_LEN: usize = 96;
const MODE_TYPE_MASK: u32 = 0o170000;
const MODE_REGULAR: u32 = 0o100000;
const MODE_DIRECTORY: u32 = 0o040000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArchiveError {
    MissingModule,
    InvalidModule,
    ArchiveTooLarge,
    TruncatedHeader,
    InvalidMagic,
    InvalidHexField,
    InvalidNameSize,
    NameTooLong,
    NameMissingTerminator,
    InvalidPath,
    EntryOutsideArchive,
    ChecksumUnsupported,
    MissingTrailer,
    InvalidTrailer,
    TrailingGarbage,
    TooManyEntries,
    MissingInit,
    InitNotRegular,
}

#[derive(Clone, Copy)]
pub struct PathField {
    bytes: [u8; MAX_PATH_LEN],
    len: usize,
}

impl PathField {
    const fn empty() -> Self {
        Self {
            bytes: [0; MAX_PATH_LEN],
            len: 0,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, ArchiveError> {
        if bytes.is_empty() || bytes.len() > MAX_PATH_LEN || !valid_path(bytes) {
            return Err(if bytes.len() > MAX_PATH_LEN {
                ArchiveError::NameTooLong
            } else {
                ArchiveError::InvalidPath
            });
        }

        let mut field = Self::empty();
        field.bytes[..bytes.len()].copy_from_slice(bytes);
        field.len = bytes.len();
        Ok(field)
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
pub struct EntryInfo {
    pub path: PathField,
    pub size: u64,
    pub mode: u32,
    pub regular: bool,
    pub directory: bool,
}

impl EntryInfo {
    const fn empty() -> Self {
        Self {
            path: PathField::empty(),
            size: 0,
            mode: 0,
            regular: false,
            directory: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub module_found: bool,
    pub valid: bool,
    pub module_start: u64,
    pub module_end: u64,
    pub archive_size: u64,
    pub entry_count: u64,
    pub regular_files: u64,
    pub directories: u64,
    pub total_file_bytes: u64,
    pub init_found: bool,
    pub init_size: u64,
    pub last_error: Option<ArchiveError>,
}

#[derive(Clone, Copy)]
struct ArchiveState {
    initialized: bool,
    module_found: bool,
    valid: bool,
    module_start: u64,
    module_end: u64,
    entry_count: usize,
    regular_files: u64,
    directories: u64,
    total_file_bytes: u64,
    init_found: bool,
    init_size: u64,
    entries: [EntryInfo; MAX_ENTRIES],
    last_error: Option<ArchiveError>,
}

impl ArchiveState {
    const fn empty() -> Self {
        Self {
            initialized: false,
            module_found: false,
            valid: false,
            module_start: 0,
            module_end: 0,
            entry_count: 0,
            regular_files: 0,
            directories: 0,
            total_file_bytes: 0,
            init_found: false,
            init_size: 0,
            entries: [EntryInfo::empty(); MAX_ENTRIES],
            last_error: None,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            initialized: self.initialized,
            module_found: self.module_found,
            valid: self.valid,
            module_start: self.module_start,
            module_end: self.module_end,
            archive_size: self.module_end.saturating_sub(self.module_start),
            entry_count: self.entry_count as u64,
            regular_files: self.regular_files,
            directories: self.directories,
            total_file_bytes: self.total_file_bytes,
            init_found: self.init_found,
            init_size: self.init_size,
            last_error: self.last_error,
        }
    }
}

struct ArchiveStore {
    value: UnsafeCell<ArchiveState>,
}

unsafe impl Sync for ArchiveStore {}

static ARCHIVE: ArchiveStore = ArchiveStore {
    value: UnsafeCell::new(ArchiveState::empty()),
};

pub struct File<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
    pub mode: u32,
}

struct ParsedEntry<'a> {
    name: &'a [u8],
    data: &'a [u8],
    mode: u32,
    next_offset: usize,
    trailer: bool,
}

pub fn init() -> Snapshot {
    let mut state = ArchiveState::empty();
    state.initialized = true;

    let result = match multiboot::find_module("initramfs") {
        Some(module) => {
            state.module_found = true;
            state.module_start = module.start;
            state.module_end = module.end;

            if !module.is_valid() {
                Err(ArchiveError::InvalidModule)
            } else if module.size() > MAX_ARCHIVE_SIZE {
                Err(ArchiveError::ArchiveTooLarge)
            } else {
                let bytes = unsafe {
                    core::slice::from_raw_parts(module.start as *const u8, module.size() as usize)
                };
                parse_archive(bytes, &mut state)
            }
        }
        None => Err(ArchiveError::MissingModule),
    };

    match result {
        Ok(()) => {
            state.valid = true;
            serial::log("initramfs", "CPIO newc archive ready");
            serial::log_hex_u64("initramfs", "module start", state.module_start);
            serial::log_u64(
                "initramfs",
                "archive bytes",
                state.module_end.saturating_sub(state.module_start),
            );
            serial::log_u64("initramfs", "entries", state.entry_count as u64);
            serial::log_u64("initramfs", "/bin/init bytes", state.init_size);
            serial::log("initramfs", "/bin/init found");
        }
        Err(error) => {
            state.last_error = Some(error);
            serial::log_bytes(
                "initramfs",
                "initialization failed",
                error_name(error).as_bytes(),
            );
        }
    }

    unsafe {
        *ARCHIVE.value.get() = state;
    }
    state.snapshot()
}

pub fn snapshot() -> Snapshot {
    unsafe { (*ARCHIVE.value.get()).snapshot() }
}

pub fn entry(index: usize) -> Option<EntryInfo> {
    let state = unsafe { &*ARCHIVE.value.get() };
    if index >= state.entry_count {
        return None;
    }

    Some(state.entries[index])
}

pub fn find(path: &str) -> Option<File<'static>> {
    let requested = normalize_requested_path(path.as_bytes())?;
    let bytes = archive_bytes()?;
    let mut offset = 0usize;

    while offset < bytes.len() {
        let parsed = parse_entry(bytes, offset).ok()?;
        if parsed.trailer {
            break;
        }
        if parsed.name == requested {
            let path = core::str::from_utf8(parsed.name).ok()?;
            return Some(File {
                path,
                data: parsed.data,
                mode: parsed.mode,
            });
        }
        offset = parsed.next_offset;
    }

    None
}

pub fn selftest() -> bool {
    let snapshot = snapshot();
    let Some(init) = find("/bin/init") else {
        return false;
    };

    snapshot.initialized
        && snapshot.module_found
        && snapshot.valid
        && snapshot.last_error.is_none()
        && snapshot.entry_count >= 2
        && snapshot.regular_files >= 1
        && snapshot.directories >= 1
        && snapshot.init_found
        && snapshot.init_size == init.data.len() as u64
        && init.path == "bin/init"
        && init.mode & MODE_TYPE_MASK == MODE_REGULAR
        && init.data.starts_with(b"\x7fELF")
        && find("/missing").is_none()
        && find("../bin/init").is_none()
}

pub fn error_name(error: ArchiveError) -> &'static str {
    match error {
        ArchiveError::MissingModule => "Multiboot2 initramfs module is missing",
        ArchiveError::InvalidModule => "initramfs module range is invalid",
        ArchiveError::ArchiveTooLarge => "initramfs exceeds the size limit",
        ArchiveError::TruncatedHeader => "CPIO header is truncated",
        ArchiveError::InvalidMagic => "CPIO newc magic is invalid",
        ArchiveError::InvalidHexField => "CPIO hexadecimal field is invalid",
        ArchiveError::InvalidNameSize => "CPIO name size is invalid",
        ArchiveError::NameTooLong => "CPIO path exceeds the kernel limit",
        ArchiveError::NameMissingTerminator => "CPIO path is not NUL terminated",
        ArchiveError::InvalidPath => "CPIO path is unsafe or invalid",
        ArchiveError::EntryOutsideArchive => "CPIO entry extends outside the archive",
        ArchiveError::ChecksumUnsupported => "CPIO checksum field is unsupported",
        ArchiveError::MissingTrailer => "CPIO trailer is missing",
        ArchiveError::InvalidTrailer => "CPIO trailer contains file data",
        ArchiveError::TrailingGarbage => "CPIO has non-zero data after its trailer",
        ArchiveError::TooManyEntries => "CPIO contains too many entries",
        ArchiveError::MissingInit => "/bin/init is missing from initramfs",
        ArchiveError::InitNotRegular => "/bin/init is not a regular file",
    }
}

fn parse_archive(bytes: &[u8], state: &mut ArchiveState) -> Result<(), ArchiveError> {
    let mut offset = 0usize;
    let mut trailer_found = false;

    while offset < bytes.len() {
        let parsed = parse_entry(bytes, offset)?;
        offset = parsed.next_offset;

        if parsed.trailer {
            trailer_found = true;
            break;
        }

        if state.entry_count >= MAX_ENTRIES {
            return Err(ArchiveError::TooManyEntries);
        }

        let file_type = parsed.mode & MODE_TYPE_MASK;
        let regular = file_type == MODE_REGULAR;
        let directory = file_type == MODE_DIRECTORY;
        if parsed.name == INIT_PATH && !regular {
            return Err(ArchiveError::InitNotRegular);
        }

        let info = EntryInfo {
            path: PathField::from_bytes(parsed.name)?,
            size: parsed.data.len() as u64,
            mode: parsed.mode,
            regular,
            directory,
        };
        state.entries[state.entry_count] = info;
        state.entry_count += 1;

        if regular {
            state.regular_files = state.regular_files.saturating_add(1);
            state.total_file_bytes = state
                .total_file_bytes
                .saturating_add(parsed.data.len() as u64);
        }
        if directory {
            state.directories = state.directories.saturating_add(1);
        }
        if parsed.name == INIT_PATH {
            state.init_found = true;
            state.init_size = parsed.data.len() as u64;
        }
    }

    if !trailer_found {
        return Err(ArchiveError::MissingTrailer);
    }
    if bytes[offset..].iter().any(|byte| *byte != 0) {
        return Err(ArchiveError::TrailingGarbage);
    }
    if !state.init_found {
        return Err(ArchiveError::MissingInit);
    }

    Ok(())
}

fn parse_entry(bytes: &[u8], offset: usize) -> Result<ParsedEntry<'_>, ArchiveError> {
    let header_end = offset
        .checked_add(CPIO_HEADER_SIZE)
        .ok_or(ArchiveError::EntryOutsideArchive)?;
    let header = bytes
        .get(offset..header_end)
        .ok_or(ArchiveError::TruncatedHeader)?;
    if &header[..6] != CPIO_NEWC_MAGIC {
        return Err(ArchiveError::InvalidMagic);
    }

    let mut field_offset = 6usize;
    while field_offset < CPIO_HEADER_SIZE {
        parse_hex_u32(&header[field_offset..field_offset + 8])?;
        field_offset += 8;
    }

    let mode = parse_hex_u32(&header[14..22])?;
    let file_size = parse_hex_u32(&header[54..62])? as usize;
    let name_size = parse_hex_u32(&header[94..102])? as usize;
    let checksum = parse_hex_u32(&header[102..110])?;
    if checksum != 0 {
        return Err(ArchiveError::ChecksumUnsupported);
    }
    if name_size == 0 {
        return Err(ArchiveError::InvalidNameSize);
    }
    if name_size > MAX_PATH_LEN + 1 {
        return Err(ArchiveError::NameTooLong);
    }

    let name_end = header_end
        .checked_add(name_size)
        .ok_or(ArchiveError::EntryOutsideArchive)?;
    let name_with_nul = bytes
        .get(header_end..name_end)
        .ok_or(ArchiveError::EntryOutsideArchive)?;
    if name_with_nul.last() != Some(&0) {
        return Err(ArchiveError::NameMissingTerminator);
    }
    let name = &name_with_nul[..name_size - 1];

    let data_start = align_up(name_end, 4).ok_or(ArchiveError::EntryOutsideArchive)?;
    let data_end = data_start
        .checked_add(file_size)
        .ok_or(ArchiveError::EntryOutsideArchive)?;
    let data = bytes
        .get(data_start..data_end)
        .ok_or(ArchiveError::EntryOutsideArchive)?;
    let next_offset = align_up(data_end, 4).ok_or(ArchiveError::EntryOutsideArchive)?;
    if next_offset > bytes.len() {
        return Err(ArchiveError::EntryOutsideArchive);
    }

    let trailer = name == CPIO_TRAILER;
    if trailer && !data.is_empty() {
        return Err(ArchiveError::InvalidTrailer);
    }
    if !trailer && !valid_path(name) {
        return Err(ArchiveError::InvalidPath);
    }

    Ok(ParsedEntry {
        name,
        data,
        mode,
        next_offset,
        trailer,
    })
}

fn parse_hex_u32(bytes: &[u8]) -> Result<u32, ArchiveError> {
    if bytes.len() != 8 {
        return Err(ArchiveError::InvalidHexField);
    }

    let mut value = 0u32;
    for byte in bytes.iter().copied() {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as u32,
            b'a'..=b'f' => (byte - b'a' + 10) as u32,
            b'A'..=b'F' => (byte - b'A' + 10) as u32,
            _ => return Err(ArchiveError::InvalidHexField),
        };
        value = value
            .checked_mul(16)
            .and_then(|current| current.checked_add(digit))
            .ok_or(ArchiveError::InvalidHexField)?;
    }

    Ok(value)
}

fn valid_path(path: &[u8]) -> bool {
    if path.is_empty() || path[0] == b'/' || path.last() == Some(&b'/') {
        return false;
    }

    let mut component_start = 0usize;
    for index in 0..=path.len() {
        if index == path.len() || path[index] == b'/' {
            let component = &path[component_start..index];
            if component.is_empty() || component == b"." || component == b".." {
                return false;
            }
            component_start = index + 1;
            continue;
        }

        let byte = path[index];
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')) {
            return false;
        }
    }

    true
}

fn normalize_requested_path(path: &[u8]) -> Option<&[u8]> {
    let normalized = if path.first() == Some(&b'/') {
        &path[1..]
    } else {
        path
    };
    if valid_path(normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn archive_bytes() -> Option<&'static [u8]> {
    let state = unsafe { &*ARCHIVE.value.get() };
    if !state.valid || state.module_end <= state.module_start {
        return None;
    }

    Some(unsafe {
        core::slice::from_raw_parts(
            state.module_start as *const u8,
            (state.module_end - state.module_start) as usize,
        )
    })
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
