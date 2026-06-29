pub const INIT_EXPECTED_EXIT_CODE: u64 = 42;
pub const INIT_MINIMUM_SYSCALLS: u64 = 7;
pub const INIT_TEXT_SIGNATURE: [u8; 10] =
    [0x48, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
pub const INIT_DATA_SIGNATURE: [u8; 12] = *b"Tobacco init";
pub const INIT_BSS_PROBE_OFFSET: u64 = 128;
