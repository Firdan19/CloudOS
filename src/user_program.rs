use crate::paging;

pub const INIT_EXPECTED_EXIT_CODE: u64 = 42;
pub const INIT_MINIMUM_SYSCALLS: u64 = 4;
pub const INIT_TEXT_SIGNATURE: [u8; 10] =
    [0x48, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
pub const INIT_DATA_SIGNATURE: [u8; 12] = *b"Tobacco init";
pub const INIT_BSS_PROBE_OFFSET: u64 = 128;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const PROGRAM_HEADER_COUNT: usize = 2;
const TEXT_OFFSET: usize = 0x1000;
const DATA_OFFSET: usize = 0x2000;

const INIT_CODE: [u8; 71] = [
    0x48, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xcd, 0x80, 0x48, 0xb8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8,
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xcd, 0x80, 0xf4, 0xeb, 0xfd,
];

const INIT_DATA: [u8; 16] = *b"Tobacco init\0\0\0\0";
const INIT_ELF_SIZE: usize = DATA_OFFSET + INIT_DATA.len();

pub static INIT_ELF: [u8; INIT_ELF_SIZE] = build_init_elf();

const fn build_init_elf() -> [u8; INIT_ELF_SIZE] {
    let mut image = [0u8; INIT_ELF_SIZE];

    image[0] = 0x7f;
    image[1] = b'E';
    image[2] = b'L';
    image[3] = b'F';
    image[4] = 2;
    image[5] = 1;
    image[6] = 1;

    write_u16(&mut image, 16, 2);
    write_u16(&mut image, 18, 62);
    write_u32(&mut image, 20, 1);
    write_u64(&mut image, 24, paging::USER_ELF_BASE);
    write_u64(&mut image, 32, ELF_HEADER_SIZE as u64);
    write_u16(&mut image, 52, ELF_HEADER_SIZE as u16);
    write_u16(&mut image, 54, PROGRAM_HEADER_SIZE as u16);
    write_u16(&mut image, 56, PROGRAM_HEADER_COUNT as u16);

    let text_header = ELF_HEADER_SIZE;
    write_u32(&mut image, text_header, 1);
    write_u32(&mut image, text_header + 4, 5);
    write_u64(&mut image, text_header + 8, TEXT_OFFSET as u64);
    write_u64(&mut image, text_header + 16, paging::USER_ELF_BASE);
    write_u64(&mut image, text_header + 32, INIT_CODE.len() as u64);
    write_u64(&mut image, text_header + 40, INIT_CODE.len() as u64);
    write_u64(&mut image, text_header + 48, paging::PAGE_SIZE_4K);

    let data_header = ELF_HEADER_SIZE + PROGRAM_HEADER_SIZE;
    write_u32(&mut image, data_header, 1);
    write_u32(&mut image, data_header + 4, 6);
    write_u64(&mut image, data_header + 8, DATA_OFFSET as u64);
    write_u64(
        &mut image,
        data_header + 16,
        paging::USER_ELF_BASE + paging::PAGE_SIZE_4K,
    );
    write_u64(&mut image, data_header + 32, INIT_DATA.len() as u64);
    write_u64(&mut image, data_header + 40, paging::PAGE_SIZE_4K);
    write_u64(&mut image, data_header + 48, paging::PAGE_SIZE_4K);

    let mut index = 0;
    while index < INIT_CODE.len() {
        image[TEXT_OFFSET + index] = INIT_CODE[index];
        index += 1;
    }

    index = 0;
    while index < INIT_DATA.len() {
        image[DATA_OFFSET + index] = INIT_DATA[index];
        index += 1;
    }

    image
}

const fn write_u16(destination: &mut [u8], offset: usize, value: u16) {
    destination[offset] = value as u8;
    destination[offset + 1] = (value >> 8) as u8;
}

const fn write_u32(destination: &mut [u8], offset: usize, value: u32) {
    let mut index = 0;
    while index < 4 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}

const fn write_u64(destination: &mut [u8], offset: usize, value: u64) {
    let mut index = 0;
    while index < 8 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}
