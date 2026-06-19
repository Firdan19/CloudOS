use crate::stats;
use core::ptr::NonNull;
use volatile::VolatilePtr;
use x86_64::instructions::interrupts as cpu_interrupts;

const TEXT_ON_LIGHT: u8 = 0x70;
const MUTED_ON_LIGHT: u8 = 0x78;
const ACCENT_ON_LIGHT: u8 = 0x71;
const BAR_COLOR: u8 = 0x17;
const CURSOR_COLOR: u8 = 0x07;
const PANIC_BACKGROUND: u8 = 0x4f;
const PANIC_PANEL: u8 = 0x0f;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;
const CONSOLE_TOP: usize = 8;
const CONSOLE_BOTTOM: usize = 23;
const PANEL_LEFT: usize = 1;
const PANEL_RIGHT: usize = 78;
const CONSOLE_LEFT: usize = 3;
const CONSOLE_RIGHT: usize = 76;
const CONSOLE_WIDTH: usize = CONSOLE_RIGHT - CONSOLE_LEFT + 1;
const INPUT_COLUMNS_PER_ROW: usize = CONSOLE_WIDTH - 2;
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
            color_code: TEXT_ON_LIGHT,
            cursor_visible: false,
            cursor_saved: ScreenChar {
                ascii_character: b' ',
                color_code: TEXT_ON_LIGHT,
            },
            buffer: 0xb8000 as *mut ScreenChar,
        }
    }

    fn clear_screen(&mut self) {
        stats::inc_vga_clear();
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
        self.draw_status_bar();
        self.write_centered_color(2, "Tobacco", ACCENT_ON_LIGHT);
        self.write_centered_color(3, "Kernel v0.0.5 - Booted", TEXT_ON_LIGHT);
        self.write_centered_color(
            5,
            "Type help for commands  |  Up/Down history  |  Esc clears input",
            MUTED_ON_LIGHT,
        );
        self.draw_panel();
        self.set_cursor(CONSOLE_TOP, CONSOLE_LEFT);
    }

    fn show_panic_screen(&mut self, title: &str, detail: &str) {
        self.hide_cursor();

        for offset in 0..(VGA_WIDTH * VGA_HEIGHT) {
            self.write_cell(offset, b' ', PANIC_BACKGROUND);
        }

        self.write_centered_color(2, "Tobacco", PANIC_BACKGROUND);
        self.write_centered_color(3, "KERNEL PANIC", PANIC_BACKGROUND);

        self.fill_rect(5, 6, 19, 73, PANIC_PANEL);
        self.draw_box(5, 6, 19, 73, PANIC_PANEL);

        self.write_centered_in_range(7, 6, 73, title, PANIC_PANEL);
        self.write_string_at_color(9, 10, "error   : ", PANIC_PANEL);
        self.write_string_at_color(9, 20, detail, PANIC_PANEL);
        self.write_string_at_color(11, 10, "status  : halted", PANIC_PANEL);
        self.write_string_at_color(12, 10, "serial  : active", PANIC_PANEL);
        self.write_string_at_color(13, 10, "action  : close QEMU or restart VM", PANIC_PANEL);
        self.write_string_at_color(
            16,
            10,
            "Tobacco stopped to protect kernel state.",
            PANIC_PANEL,
        );
        self.write_centered_color(21, "No host disk was touched.", PANIC_BACKGROUND);

        self.row = 18;
        self.column = 10;
        self.input_row = 0;
    }

    fn show_exception_screen(
        &mut self,
        name: &str,
        vector: u64,
        error_code: u64,
        instruction_pointer: u64,
        fault_address: u64,
        cpu_flags: u64,
    ) {
        self.hide_cursor();

        for offset in 0..(VGA_WIDTH * VGA_HEIGHT) {
            self.write_cell(offset, b' ', PANIC_BACKGROUND);
        }

        self.write_centered_color(2, "Tobacco", PANIC_BACKGROUND);
        self.write_centered_color(3, "CPU EXCEPTION", PANIC_BACKGROUND);

        self.fill_rect(5, 4, 21, 75, PANIC_PANEL);
        self.draw_box(5, 4, 21, 75, PANIC_PANEL);

        self.write_centered_in_range(7, 4, 75, name, PANIC_PANEL);
        self.write_string_at_color(9, 8, "vector  : ", PANIC_PANEL);
        self.write_u64_at_color(9, 18, vector, PANIC_PANEL);
        self.write_string_at_color(10, 8, "error   : ", PANIC_PANEL);
        self.write_hex_u64_at_color(10, 18, error_code, PANIC_PANEL);
        self.write_string_at_color(11, 8, "rip     : ", PANIC_PANEL);
        self.write_hex_u64_at_color(11, 18, instruction_pointer, PANIC_PANEL);
        self.write_string_at_color(12, 8, "cr2     : ", PANIC_PANEL);
        self.write_hex_u64_at_color(12, 18, fault_address, PANIC_PANEL);
        self.write_string_at_color(13, 8, "rflags  : ", PANIC_PANEL);
        self.write_hex_u64_at_color(13, 18, cpu_flags, PANIC_PANEL);
        self.write_string_at_color(15, 8, "status  : halted", PANIC_PANEL);
        self.write_string_at_color(16, 8, "serial  : active", PANIC_PANEL);
        self.write_string_at_color(17, 8, "action  : close QEMU or restart VM", PANIC_PANEL);
        self.write_string_at_color(
            19,
            8,
            "Tobacco stopped to protect kernel state.",
            PANIC_PANEL,
        );
        self.write_centered_color(23, "No host disk was touched.", PANIC_BACKGROUND);

        self.row = 20;
        self.column = 8;
        self.input_row = 0;
    }

    fn start_prompt(&mut self) {
        self.hide_cursor();

        if self.column != CONSOLE_LEFT {
            self.new_line();
        }

        self.input_row = self.row;
        self.render_input(&[]);
    }

    fn render_input(&mut self, input: &[u8]) {
        self.render_input_with_cursor(input, input.len());
    }

    fn render_input_with_cursor(&mut self, input: &[u8], cursor_index: usize) {
        self.hide_cursor();

        let rows_needed = (input.len() / INPUT_COLUMNS_PER_ROW) + 1;
        if self.input_row + rows_needed - 1 > CONSOLE_BOTTOM {
            let scroll_count = self.input_row + rows_needed - 1 - CONSOLE_BOTTOM;
            for _ in 0..scroll_count {
                self.scroll_console_up();
            }
            self.input_row = self.input_row.saturating_sub(scroll_count).max(CONSOLE_TOP);
        }

        for row in self.input_row..=CONSOLE_BOTTOM {
            self.clear_console_row(row);
        }

        for visual_row in 0..rows_needed {
            let row = self.input_row + visual_row;
            let start = visual_row * INPUT_COLUMNS_PER_ROW;
            let end = start.saturating_add(INPUT_COLUMNS_PER_ROW).min(input.len());

            self.write_prompt_at(row);

            if start < input.len() {
                for (index, byte) in input[start..end].iter().copied().enumerate() {
                    self.write_cell(
                        row * VGA_WIDTH + CONSOLE_LEFT + PROMPT.len() + index,
                        vga_byte(byte),
                        self.color_code,
                    );
                }
            }
        }

        let cursor_index = cursor_index.min(input.len());
        let cursor_row = self.input_row + (cursor_index / INPUT_COLUMNS_PER_ROW);
        let cursor_column = CONSOLE_LEFT + PROMPT.len() + (cursor_index % INPUT_COLUMNS_PER_ROW);

        self.set_cursor(cursor_row, cursor_column);
    }

    fn write_centered_color(&mut self, row: usize, s: &str, color_code: u8) {
        let width = s.bytes().count().min(VGA_WIDTH);
        let column = (VGA_WIDTH - width) / 2;

        self.write_string_at_color(row, column, s, color_code);
    }

    fn write_centered_in_range(
        &mut self,
        row: usize,
        left: usize,
        right: usize,
        s: &str,
        color_code: u8,
    ) {
        if left > right || right >= VGA_WIDTH {
            return;
        }

        let range_width = right - left + 1;
        let width = s.bytes().count().min(range_width);
        let column = left + ((range_width - width) / 2);

        self.write_string_at_color(row, column, s, color_code);
    }

    fn write_string_at_color(&mut self, row: usize, column: usize, s: &str, color_code: u8) {
        if row >= VGA_HEIGHT || column >= VGA_WIDTH {
            return;
        }

        let start = row * VGA_WIDTH + column;
        let max_len = VGA_WIDTH - column;

        for (offset, byte) in s.bytes().take(max_len).enumerate() {
            self.write_cell(start + offset, vga_byte(byte), color_code);
        }
    }

    fn write_u64_at_color(&mut self, row: usize, column: usize, mut value: u64, color_code: u8) {
        let mut digits = [0u8; 20];
        let mut index = digits.len();

        if value == 0 {
            self.write_cell(row * VGA_WIDTH + column, b'0', color_code);
            return;
        }

        while value > 0 {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }

        for (offset, byte) in digits[index..].iter().copied().enumerate() {
            if column + offset >= VGA_WIDTH {
                break;
            }
            self.write_cell(row * VGA_WIDTH + column + offset, byte, color_code);
        }
    }

    fn write_hex_u64_at_color(&mut self, row: usize, column: usize, value: u64, color_code: u8) {
        if column + 2 >= VGA_WIDTH {
            return;
        }

        self.write_cell(row * VGA_WIDTH + column, b'0', color_code);
        self.write_cell(row * VGA_WIDTH + column + 1, b'x', color_code);

        let mut started = false;
        let mut shift = 60u64;
        let mut output_column = column + 2;

        loop {
            let nibble = ((value >> shift) & 0x0f) as u8;

            if nibble != 0 || started || shift == 0 {
                started = true;
                let byte = if nibble < 10 {
                    b'0' + nibble
                } else {
                    b'a' + (nibble - 10)
                };

                if output_column < VGA_WIDTH {
                    self.write_cell(row * VGA_WIDTH + output_column, byte, color_code);
                    output_column += 1;
                }
            }

            if shift == 0 {
                break;
            }

            shift -= 4;
        }
    }

    fn write_prompt_at(&mut self, row: usize) {
        for (index, byte) in PROMPT.iter().copied().enumerate() {
            self.write_cell(
                row * VGA_WIDTH + CONSOLE_LEFT + index,
                byte,
                ACCENT_ON_LIGHT,
            );
        }
    }

    fn draw_status_bar(&mut self) {
        self.clear_row_with_color(0, BAR_COLOR);
        self.write_string_at_color(0, 2, "Tobacco Terminal", BAR_COLOR);
        self.write_centered_color(0, "Phase 1 Console", BAR_COLOR);
        self.write_string_at_color(0, 68, "v0.0.5", BAR_COLOR);
    }

    fn draw_panel(&mut self) {
        for column in PANEL_LEFT..=PANEL_RIGHT {
            self.write_cell(7 * VGA_WIDTH + column, b'-', MUTED_ON_LIGHT);
            self.write_cell(24 * VGA_WIDTH + column, b'-', MUTED_ON_LIGHT);
        }

        self.write_cell(7 * VGA_WIDTH + PANEL_LEFT, b'+', MUTED_ON_LIGHT);
        self.write_cell(7 * VGA_WIDTH + PANEL_RIGHT, b'+', MUTED_ON_LIGHT);
        self.write_cell(24 * VGA_WIDTH + PANEL_LEFT, b'+', MUTED_ON_LIGHT);
        self.write_cell(24 * VGA_WIDTH + PANEL_RIGHT, b'+', MUTED_ON_LIGHT);

        for row in CONSOLE_TOP..=CONSOLE_BOTTOM {
            self.write_cell(row * VGA_WIDTH + PANEL_LEFT, b'|', MUTED_ON_LIGHT);
            self.write_cell(row * VGA_WIDTH + PANEL_RIGHT, b'|', MUTED_ON_LIGHT);
            self.clear_console_row(row);
        }
    }

    fn fill_rect(&self, top: usize, left: usize, bottom: usize, right: usize, color_code: u8) {
        for row in top..=bottom {
            for column in left..=right {
                self.write_cell(row * VGA_WIDTH + column, b' ', color_code);
            }
        }
    }

    fn draw_box(&self, top: usize, left: usize, bottom: usize, right: usize, color_code: u8) {
        for column in left..=right {
            self.write_cell(top * VGA_WIDTH + column, b'-', color_code);
            self.write_cell(bottom * VGA_WIDTH + column, b'-', color_code);
        }

        for row in top..=bottom {
            self.write_cell(row * VGA_WIDTH + left, b'|', color_code);
            self.write_cell(row * VGA_WIDTH + right, b'|', color_code);
        }

        self.write_cell(top * VGA_WIDTH + left, b'+', color_code);
        self.write_cell(top * VGA_WIDTH + right, b'+', color_code);
        self.write_cell(bottom * VGA_WIDTH + left, b'+', color_code);
        self.write_cell(bottom * VGA_WIDTH + right, b'+', color_code);
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
                if self.column < CONSOLE_LEFT {
                    self.column = CONSOLE_LEFT;
                }

                if self.column > CONSOLE_RIGHT {
                    self.new_line();
                }

                let offset = self.row * VGA_WIDTH + self.column;
                self.write_cell(offset, vga_byte(byte), self.color_code);
                self.column += 1;

                if self.column > CONSOLE_RIGHT {
                    self.new_line();
                }
            }
        }
    }

    fn new_line(&mut self) {
        self.column = CONSOLE_LEFT;
        if self.row >= CONSOLE_BOTTOM {
            self.scroll_console_up();
        } else {
            self.row += 1;
        }

        if self.row < CONSOLE_TOP {
            self.row = CONSOLE_TOP;
        }
    }

    fn backspace(&mut self) {
        if self.column > CONSOLE_LEFT {
            self.column -= 1;
        } else if self.row > CONSOLE_TOP {
            self.row -= 1;
            self.column = CONSOLE_RIGHT;
        } else {
            return;
        }

        let offset = self.row * VGA_WIDTH + self.column;
        self.write_cell(offset, b' ', self.color_code);
    }

    fn scroll_console_up(&mut self) {
        stats::inc_vga_scroll();
        self.hide_cursor();

        for row in (CONSOLE_TOP + 1)..=CONSOLE_BOTTOM {
            for column in CONSOLE_LEFT..=CONSOLE_RIGHT {
                let from = row * VGA_WIDTH + column;
                let to = (row - 1) * VGA_WIDTH + column;
                let character = self.read_cell(from);
                self.write_cell(to, character.ascii_character, character.color_code);
            }
        }

        self.clear_console_row(CONSOLE_BOTTOM);
        self.row = CONSOLE_BOTTOM;
        self.column = CONSOLE_LEFT;
    }

    fn clear_row(&mut self, row: usize) {
        self.clear_row_with_color(row, self.color_code);
    }

    fn clear_row_with_color(&mut self, row: usize, color_code: u8) {
        let start = row * VGA_WIDTH;
        for column in 0..VGA_WIDTH {
            self.write_cell(start + column, b' ', color_code);
        }
    }

    fn clear_console_row(&mut self, row: usize) {
        for column in CONSOLE_LEFT..=CONSOLE_RIGHT {
            self.write_cell(row * VGA_WIDTH + column, b' ', self.color_code);
        }
    }

    fn toggle_cursor(&mut self) {
        stats::inc_cursor_toggle();
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
        self.write_cell(offset, b'_', CURSOR_COLOR);
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
        stats::inc_vga_cell_write();
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

pub fn show_panic_screen(title: &str, detail: &str) {
    with_writer(|writer| writer.show_panic_screen(title, detail));
}

pub fn show_exception_screen(
    name: &str,
    vector: u64,
    error_code: u64,
    instruction_pointer: u64,
    fault_address: u64,
    cpu_flags: u64,
) {
    with_writer(|writer| {
        writer.show_exception_screen(
            name,
            vector,
            error_code,
            instruction_pointer,
            fault_address,
            cpu_flags,
        )
    });
}

pub fn start_prompt() {
    with_writer(|writer| writer.start_prompt());
}

pub fn render_input(input: &[u8]) {
    with_writer(|writer| writer.render_input(input));
}

pub fn render_input_with_cursor(input: &[u8], cursor_index: usize) {
    with_writer(|writer| writer.render_input_with_cursor(input, cursor_index));
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
