use core::ptr::NonNull;
use volatile::VolatilePtr;

const WHITE_ON_BLUE: u8 = 0x1f;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: u8,
}

pub fn clear_screen(vga_buffer: VolatilePtr<'static, ScreenChar>) {
    for offset in 0..(VGA_WIDTH * VGA_HEIGHT) {
        write_cell(vga_buffer, offset, b' ');
    }
}

pub fn write_centered(vga_buffer: VolatilePtr<'static, ScreenChar>, row: usize, s: &str) {
    let width = s.chars().count().min(VGA_WIDTH);
    let column = (VGA_WIDTH - width) / 2;

    write_string_at(vga_buffer, row, column, s);
}

pub fn write_string_at(
    vga_buffer: VolatilePtr<'static, ScreenChar>,
    row: usize,
    column: usize,
    s: &str,
) {
    if row >= VGA_HEIGHT || column >= VGA_WIDTH {
        return;
    }

    let start = row * VGA_WIDTH + column;
    let max_len = VGA_WIDTH - column;

    for (offset, character) in s.chars().take(max_len).enumerate() {
        write_cell(vga_buffer, start + offset, vga_byte(character));
    }
}

fn vga_byte(character: char) -> u8 {
    match character {
        '—' => 0xc4,
        character if character.is_ascii() => character as u8,
        _ => b'?',
    }
}

fn write_cell(vga_buffer: VolatilePtr<'static, ScreenChar>, offset: usize, byte: u8) {
    let cell = unsafe {
        vga_buffer.map(|ptr| {
            let next = ptr.as_ptr().wrapping_add(offset);
            NonNull::new_unchecked(next)
        })
    };

    cell.write(ScreenChar {
        ascii_character: byte,
        color_code: WHITE_ON_BLUE,
    });
}
