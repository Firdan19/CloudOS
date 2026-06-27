use std::env;
use std::fs;
use std::path::Path;

const USER_ELF_BASE: u64 = 0x4010_0000;
const PAGE_SIZE: u64 = 4096;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const PROGRAM_HEADER_COUNT: usize = 2;
const TEXT_OFFSET: usize = 0x1000;
const DATA_OFFSET: usize = 0x2000;
const INIT_DATA: [u8; 16] = *b"Tobacco init\0\0\0\0";
const INIT_ELF_SIZE: usize = DATA_OFFSET + INIT_DATA.len();

const INIT_CODE: [u8; 71] = [
    0x48, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xcd, 0x80, 0x48, 0xb8, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x48, 0xb8,
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xcd, 0x80, 0xf4, 0xeb, 0xfd,
];

fn main() {
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "target/initramfs.cpio".to_string());
    let init_elf = build_init_elf();
    let archive = build_initramfs(&init_elf);

    let output_path = Path::new(&output);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("failed to create initramfs output directory");
    }
    fs::write(output_path, archive).expect("failed to write initramfs archive");
}

fn build_init_elf() -> Vec<u8> {
    let mut image = vec![0u8; INIT_ELF_SIZE];

    image[0..4].copy_from_slice(b"\x7fELF");
    image[4] = 2;
    image[5] = 1;
    image[6] = 1;

    write_u16(&mut image, 16, 2);
    write_u16(&mut image, 18, 62);
    write_u32(&mut image, 20, 1);
    write_u64(&mut image, 24, USER_ELF_BASE);
    write_u64(&mut image, 32, ELF_HEADER_SIZE as u64);
    write_u16(&mut image, 52, ELF_HEADER_SIZE as u16);
    write_u16(&mut image, 54, PROGRAM_HEADER_SIZE as u16);
    write_u16(&mut image, 56, PROGRAM_HEADER_COUNT as u16);

    let text_header = ELF_HEADER_SIZE;
    write_u32(&mut image, text_header, 1);
    write_u32(&mut image, text_header + 4, 5);
    write_u64(&mut image, text_header + 8, TEXT_OFFSET as u64);
    write_u64(&mut image, text_header + 16, USER_ELF_BASE);
    write_u64(&mut image, text_header + 32, INIT_CODE.len() as u64);
    write_u64(&mut image, text_header + 40, INIT_CODE.len() as u64);
    write_u64(&mut image, text_header + 48, PAGE_SIZE);

    let data_header = ELF_HEADER_SIZE + PROGRAM_HEADER_SIZE;
    write_u32(&mut image, data_header, 1);
    write_u32(&mut image, data_header + 4, 6);
    write_u64(&mut image, data_header + 8, DATA_OFFSET as u64);
    write_u64(&mut image, data_header + 16, USER_ELF_BASE + PAGE_SIZE);
    write_u64(&mut image, data_header + 32, INIT_DATA.len() as u64);
    write_u64(&mut image, data_header + 40, PAGE_SIZE);
    write_u64(&mut image, data_header + 48, PAGE_SIZE);

    image[TEXT_OFFSET..TEXT_OFFSET + INIT_CODE.len()].copy_from_slice(&INIT_CODE);
    image[DATA_OFFSET..DATA_OFFSET + INIT_DATA.len()].copy_from_slice(&INIT_DATA);
    image
}

fn build_initramfs(init_elf: &[u8]) -> Vec<u8> {
    let mut archive = Vec::new();
    append_entry(&mut archive, 1, "bin", 0o040755, 2, &[]);
    append_entry(&mut archive, 2, "bin/init", 0o100755, 1, init_elf);
    append_entry(&mut archive, 3, "TRAILER!!!", 0, 1, &[]);
    archive
}

fn append_entry(archive: &mut Vec<u8>, inode: u32, name: &str, mode: u32, links: u32, data: &[u8]) {
    let name_size = name.len() + 1;
    let header = format!(
        "070701{inode:08x}{mode:08x}{uid:08x}{gid:08x}{links:08x}{mtime:08x}{file_size:08x}{dev_major:08x}{dev_minor:08x}{rdev_major:08x}{rdev_minor:08x}{name_size:08x}{check:08x}",
        uid = 0,
        gid = 0,
        mtime = 0,
        file_size = data.len(),
        dev_major = 0,
        dev_minor = 0,
        rdev_major = 0,
        rdev_minor = 0,
        check = 0,
    );
    assert_eq!(header.len(), 110);

    archive.extend_from_slice(header.as_bytes());
    archive.extend_from_slice(name.as_bytes());
    archive.push(0);
    pad_to_four(archive);
    archive.extend_from_slice(data);
    pad_to_four(archive);
}

fn pad_to_four(bytes: &mut Vec<u8>) {
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

fn write_u16(destination: &mut [u8], offset: usize, value: u16) {
    destination[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(destination: &mut [u8], offset: usize, value: u32) {
    destination[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(destination: &mut [u8], offset: usize, value: u64) {
    destination[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
