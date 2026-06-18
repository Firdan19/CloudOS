use core::ptr::NonNull;
use volatile::VolatilePtr;
use x86_64::instructions::interrupts as cpu_interrupts;

const WHITE_ON_BLUE: u8 = 0x1f;
const CURSOR_ON_BLUE: u8 = 0x71;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;
const INPUT_COLUMNS_PER_ROW: usize = VGA_WIDTH - 2;
const PROMPT: &[u8] = b"> ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: u8,
}

struct Writer {
    row: usize,
    column: usize,
    input_row: usize,
    color_code: u8,
    cursor_visible: bool,
    cursor_saved: ScreenChar,
    buffer: *mut ScreenChar,
}

impl Writer {
    const fn new() -> Self {
        Self {
            row: 0,
            column: 0,
            input_row: 0,
            color_code: WHITE_ON_BLUE,
            cursor_visible: false,
            cursor_saved: ScreenChar {
                ascii_character: b' ',
                color_code: WHITE_ON_BLUE,
            },
            buffer: 0xb8000 as *mut ScreenChar,
        }
    }

    fn clear_screen(&mut self) {
        self.hide_cursor();

        for offset in 0..(VGA_WIDTH * VGA_HEIGHT) {
            self.write_cell(offset, b' ', self.color_code);
        }

        self.row = 0;
        self.column = 0;
        self.input_row = 0;
    }

    fn show_splash(&mut self) {
        self.clear_screen();
        self.write_centered(10, "CloudOS");
        self.write_centered(12, "Kernel v0.0.2 - Booted");
        self.write_string_at(16, 4, "Keyboard input ready:");
        self.set_cursor(18, 0);
    }

    fn start_prompt(&mut self) {
        self.hide_cursor();

        if self.column != 0 {
            self.new_line();
        }

        self.input_row = self.row;
        self.render_input(&[]);
    }

    fn render_input(&mut self, input: &[u8]) {
        self.hide_cursor();

        let rows_needed = (input.len() / INPUT_COLUMNS_PER_ROW) + 1;
        if self.input_row + rows_needed > VGA_HEIGHT {
            let scroll_count = self.input_row + rows_needed - VGA_HEIGHT;
            for _ in 0..scroll_count {
                self.scroll_up();
            }
            self.input_row = self.input_row.saturating_sub(scroll_count);
        }

        for row in self.input_row..VGA_HEIGHT {
            self.clear_row(row);
        }

        let mut remaining = input;
        let mut row = self.input_row;

        loop {
            self.write_prompt_at(row);

            let take = remaining.len().min(INPUT_COLUMNS_PER_ROW);
            for (index, byte) in remaining.iter().copied().take(take).enumerate() {
                self.write_cell(
                    row * VGA_WIDTH + PROMPT.len() + index,
                    vga_byte(byte),
                    self.color_code,
                );
            }

            remaining = &remaining[take..];

            if remaining.is_empty() {
                if take == INPUT_COLUMNS_PER_ROW {
                    row += 1;
                    if row >= VGA_HEIGHT {
                        self.scroll_up();
                        row = VGA_HEIGHT - 1;
                        self.input_row = self.input_row.saturating_sub(1);
                    }
                    self.write_prompt_at(row);
                    self.set_cursor(row, PROMPT.len());
                } else {
                    self.set_cursor(row, PROMPT.len() + take);
                }
                break;
            }

            row += 1;
            if row >= VGA_HEIGHT {
                self.scroll_up();
                row = VGA_HEIGHT - 1;
                self.input_row = self.input_row.saturating_sub(1);
            }
        }
    }

    fn write_centered(&mut self, row: usize, s: &str) {
        let width = s.bytes().count().min(VGA_WIDTH);
        let column = (VGA_WIDTH - width) / 2;

        self.write_string_at(row, column, s);
    }

    fn write_string_at(&mut self, row: usize, column: usize, s: &str) {
        if row >= VGA_HEIGHT || column >= VGA_WIDTH {
            return;
        }

        let start = row * VGA_WIDTH + column;
        let max_len = VGA_WIDTH - column;

        for (offset, byte) in s.bytes().take(max_len).enumerate() {
            self.write_cell(start + offset, vga_byte(byte), self.color_code);
        }
    }

    fn write_prompt_at(&mut self, row: usize) {
        for (index, byte) in PROMPT.iter().copied().enumerate() {
            self.write_cell(row * VGA_WIDTH + index, byte, self.color_code);
        }
    }

    fn set_cursor(&mut self, row: usize, column: usize) {
        self.hide_cursor();
        self.row = row.min(VGA_HEIGHT - 1);
        self.column = column.min(VGA_WIDTH - 1);
    }

    fn write_string(&mut self, s: &str) {
        self.hide_cursor();

        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }

    fn write_line(&mut self, s: &str) {
        self.write_string(s);
        self.write_byte(b'\n');
    }

    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            8 => self.backspace(),
            byte => {
                if self.column >= VGA_WIDTH {
                    self.new_line();
                }

                let offset = self.row * VGA_WIDTH + self.column;
                self.write_cell(offset, vga_byte(byte), self.color_code);
                self.column += 1;

                if self.column >= VGA_WIDTH {
                    self.new_line();
                }
            }
        }
    }

    fn new_line(&mut self) {
        self.column = 0;
        if self.row + 1 >= VGA_HEIGHT {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    fn backspace(&mut self) {
        if self.column > 0 {
            self.column -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.column = VGA_WIDTH - 1;
        } else {
            return;
        }

        let offset = self.row * VGA_WIDTH + self.column;
        self.write_cell(offset, b' ', self.color_code);
    }

    fn scroll_up(&mut self) {
        self.hide_cursor();

        for row in 1..VGA_HEIGHT {
            for column in 0..VGA_WIDTH {
                let from = row * VGA_WIDTH + column;
                let to = (row - 1) * VGA_WIDTH + column;
                let character = self.read_cell(from);
                self.write_cell(to, character.ascii_character, character.color_code);
            }
        }

        self.clear_row(VGA_HEIGHT - 1);
        self.row = VGA_HEIGHT - 1;
        self.column = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let start = row * VGA_WIDTH;
        for column in 0..VGA_WIDTH {
            self.write_cell(start + column, b' ', self.color_code);
        }
    }

    fn toggle_cursor(&mut self) {
        if self.cursor_visible {
            self.hide_cursor();
        } else {
            self.show_cursor();
        }
    }

    fn show_cursor(&mut self) {
        if self.cursor_visible {
            return;
        }

        let offset = self.row * VGA_WIDTH + self.column;
        self.cursor_saved = self.read_cell(offset);
        self.write_cell(offset, b'_', CURSOR_ON_BLUE);
        self.cursor_visible = true;
    }

    fn hide_cursor(&mut self) {
        if !self.cursor_visible {
            return;
        }

        let offset = self.row * VGA_WIDTH + self.column;
        self.write_cell(
            offset,
            self.cursor_saved.ascii_character,
            self.cursor_saved.color_code,
        );
        self.cursor_visible = false;
    }

    fn read_cell(&self, offset: usize) -> ScreenChar {
        let cell = unsafe { VolatilePtr::new(NonNull::new_unchecked(self.buffer.add(offset))) };
        cell.read()
    }

    fn write_cell(&self, offset: usize, byte: u8, color_code: u8) {
        let cell = unsafe { VolatilePtr::new(NonNull::new_unchecked(self.buffer.add(offset))) };

        cell.write(ScreenChar {
            ascii_character: byte,
            color_code,
        });
    }
}

static mut WRITER: Writer = Writer::new();

pub fn init() {
    clear_screen();
}

pub fn show_splash() {
    with_writer(|writer| writer.show_splash());
}

pub fn start_prompt() {
    with_writer(|writer| writer.start_prompt());
}

pub fn render_input(input: &[u8]) {
    with_writer(|writer| writer.render_input(input));
}

pub fn toggle_cursor() {
    with_writer(|writer| writer.toggle_cursor());
}

pub fn write_string(s: &str) {
    with_writer(|writer| writer.write_string(s));
}

pub fn write_line(s: &str) {
    with_writer(|writer| writer.write_line(s));
}

pub fn write_byte(byte: u8) {
    with_writer(|writer| {
        writer.hide_cursor();
        writer.write_byte(byte);
    });
}

pub fn clear_screen() {
    with_writer(|writer| writer.clear_screen());
}

fn with_writer<R>(f: impl FnOnce(&mut Writer) -> R) -> R {
    cpu_interrupts::without_interrupts(|| unsafe {
        let writer = core::ptr::addr_of_mut!(WRITER);
        f(&mut *writer)
    })
}

fn vga_byte(byte: u8) -> u8 {
    match byte {
        0x20..=0x7e => byte,
        b'\n' => byte,
        8 => byte,
        _ => b'?',
    }
}
