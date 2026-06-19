use crate::keyboard::{self, KeyEvent};
use crate::{interrupts, serial, vga};
use x86_64::instructions::interrupts as cpu_interrupts;

const INPUT_BUFFER_SIZE: usize = 512;
const HISTORY_SIZE: usize = 16;
const PIT_HZ: u64 = 18;
const KERNEL_LOAD_BASE: u64 = 0x0010_0000;
const IDENTITY_MAP_BYTES: u64 = 1024 * 1024 * 1024;
const PAGE_TABLE_BYTES: u64 = 4096 * 3;
const STACK_BYTES: u64 = 16 * 1024;
const VGA_BUFFER_ADDRESS: u64 = 0x000b_8000;

struct Command {
    name: &'static str,
    description: &'static str,
    handler: fn(&[u8]),
}

const COMMANDS: [Command; 10] = [
    Command {
        name: "help",
        description: "tampilkan daftar command",
        handler: command_help,
    },
    Command {
        name: "clear",
        description: "bersihkan layar terminal",
        handler: command_clear,
    },
    Command {
        name: "version",
        description: "tampilkan versi CloudOS",
        handler: command_version,
    },
    Command {
        name: "about",
        description: "tampilkan visi singkat CloudOS",
        handler: command_about,
    },
    Command {
        name: "echo",
        description: "cetak ulang teks",
        handler: command_echo,
    },
    Command {
        name: "uptime",
        description: "tampilkan tick PIT sejak boot",
        handler: command_uptime,
    },
    Command {
        name: "sysinfo",
        description: "ringkasan subsistem kernel",
        handler: command_sysinfo,
    },
    Command {
        name: "mem",
        description: "ringkasan layout memori awal",
        handler: command_mem,
    },
    Command {
        name: "ticks",
        description: "tampilkan tick timer mentah",
        handler: command_ticks,
    },
    Command {
        name: "keyboard",
        description: "status input keyboard PS/2",
        handler: command_keyboard,
    },
];

struct CommandHistory {
    entries: [[u8; INPUT_BUFFER_SIZE]; HISTORY_SIZE],
    lengths: [usize; HISTORY_SIZE],
    len: usize,
}

impl CommandHistory {
    const fn new() -> Self {
        Self {
            entries: [[0; INPUT_BUFFER_SIZE]; HISTORY_SIZE],
            lengths: [0; HISTORY_SIZE],
            len: 0,
        }
    }

    fn push(&mut self, input: &[u8]) {
        let input = trim_ascii(input);

        if input.is_empty() {
            return;
        }

        if self.is_same_as_latest(input) {
            return;
        }

        if self.len == HISTORY_SIZE {
            for index in 1..HISTORY_SIZE {
                self.entries[index - 1] = self.entries[index];
                self.lengths[index - 1] = self.lengths[index];
            }
            self.len -= 1;
        }

        let index = self.len;
        self.entries[index][..input.len()].copy_from_slice(input);
        self.lengths[index] = input.len();
        self.len += 1;
    }

    fn latest_index(&self) -> Option<usize> {
        if self.len == 0 {
            None
        } else {
            Some(self.len - 1)
        }
    }

    fn previous_index(&self, selected: Option<usize>) -> Option<usize> {
        match selected {
            Some(index) if index > 0 => Some(index - 1),
            Some(index) => Some(index),
            None => self.latest_index(),
        }
    }

    fn next_index(&self, selected: Option<usize>) -> Option<usize> {
        match selected {
            Some(index) if index + 1 < self.len => Some(index + 1),
            Some(_) | None => None,
        }
    }

    fn load(
        &self,
        index: usize,
        input: &mut [u8; INPUT_BUFFER_SIZE],
        input_len: &mut usize,
        cursor: &mut usize,
    ) -> bool {
        if index >= self.len {
            return false;
        }

        let len = self.lengths[index];
        input[..len].copy_from_slice(&self.entries[index][..len]);
        *input_len = len;
        *cursor = len;

        true
    }

    fn is_same_as_latest(&self, input: &[u8]) -> bool {
        if self.len == 0 {
            return false;
        }

        let index = self.len - 1;
        self.lengths[index] == input.len() && &self.entries[index][..input.len()] == input
    }
}

pub fn run() -> ! {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;
    let mut history = CommandHistory::new();
    let mut history_selected = None;

    prompt();

    loop {
        cpu_interrupts::disable();
        interrupts::poll_keyboard();

        if let Some(event) = interrupts::pop_key_event() {
            cpu_interrupts::enable();

            match event {
                KeyEvent::Enter => {
                    serial::serial_println("");
                    vga::write_byte(b'\n');
                    history.push(&input[..input_len]);
                    execute(&input[..input_len]);
                    input_len = 0;
                    cursor = 0;
                    history_selected = None;
                    prompt();
                }
                KeyEvent::Backspace => {
                    if delete_previous_input_byte(&mut input, &mut input_len, &mut cursor) {
                        history_selected = None;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::Escape => {
                    input_len = 0;
                    cursor = 0;
                    history_selected = None;
                    serial::serial_println("^esc");
                    vga::render_input_with_cursor(&input[..input_len], cursor);
                }
                KeyEvent::Tab => {
                    if insert_input_byte(&mut input, &mut input_len, &mut cursor, b' ') {
                        history_selected = None;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::Char(byte) if (0x20..=0x7e).contains(&byte) => {
                    if insert_input_byte(&mut input, &mut input_len, &mut cursor, byte) {
                        history_selected = None;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::ArrowLeft => {
                    if cursor > 0 {
                        cursor -= 1;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::ArrowRight => {
                    if cursor < input_len {
                        cursor += 1;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::ArrowUp => {
                    if let Some(index) = history.previous_index(history_selected) {
                        history_selected = Some(index);
                        if history.load(index, &mut input, &mut input_len, &mut cursor) {
                            vga::render_input_with_cursor(&input[..input_len], cursor);
                        }
                    }
                }
                KeyEvent::ArrowDown => {
                    if let Some(index) = history.next_index(history_selected) {
                        history_selected = Some(index);
                        if history.load(index, &mut input, &mut input_len, &mut cursor) {
                            vga::render_input_with_cursor(&input[..input_len], cursor);
                        }
                    } else if history_selected.is_some() {
                        history_selected = None;
                        input_len = 0;
                        cursor = 0;
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::ShiftPressed | KeyEvent::ShiftReleased => {}
                KeyEvent::CapsLockToggled(enabled) => {
                    if enabled {
                        serial::serial_println("[keyboard] caps lock on");
                    } else {
                        serial::serial_println("[keyboard] caps lock off");
                    }
                }
                KeyEvent::Char(_) => {}
            }
        } else {
            cpu_interrupts::enable_and_hlt();
        }
    }
}

fn insert_input_byte(
    input: &mut [u8; INPUT_BUFFER_SIZE],
    input_len: &mut usize,
    cursor: &mut usize,
    byte: u8,
) -> bool {
    if *input_len >= INPUT_BUFFER_SIZE {
        return false;
    }

    if *cursor > *input_len {
        *cursor = *input_len;
    }

    let mut index = *input_len;
    while index > *cursor {
        input[index] = input[index - 1];
        index -= 1;
    }

    input[*cursor] = byte;
    *input_len += 1;
    *cursor += 1;

    true
}

fn delete_previous_input_byte(
    input: &mut [u8; INPUT_BUFFER_SIZE],
    input_len: &mut usize,
    cursor: &mut usize,
) -> bool {
    if *cursor == 0 || *input_len == 0 {
        return false;
    }

    if *cursor > *input_len {
        *cursor = *input_len;
    }

    let mut index = *cursor;
    while index < *input_len {
        input[index - 1] = input[index];
        index += 1;
    }

    *cursor -= 1;
    *input_len -= 1;

    true
}

fn prompt() {
    serial::serial_print("> ");
    vga::start_prompt();
}

fn execute(input: &[u8]) {
    let command_line = trim_ascii(input);

    if command_line.is_empty() {
        return;
    }

    let (command, arguments) = split_command(command_line);
    serial::serial_print("[shell] command: ");
    serial::serial_print_bytes(command);
    serial::serial_println("");

    for command_entry in COMMANDS.iter() {
        if eq_ignore_ascii_case(command, command_entry.name.as_bytes()) {
            (command_entry.handler)(arguments);
            return;
        }
    }

    println("Perintah tidak dikenal. Ketik help.");
}

fn command_help(_arguments: &[u8]) {
    println("Commands:");

    for command in COMMANDS.iter() {
        print("  ");
        print(command.name);
        print_spaces(10usize.saturating_sub(command.name.len()));
        print("- ");
        println(command.description);
    }
}

fn command_clear(_arguments: &[u8]) {
    serial::serial_println("clear");
    vga::show_splash();
}

fn command_version(_arguments: &[u8]) {
    println("CloudOS v0.0.5");
}

fn command_about(_arguments: &[u8]) {
    println("CloudOS: Sistem operasi untuk semua, tanpa perlu perangkat mahal.");
}

fn command_echo(arguments: &[u8]) {
    print_ascii_line(arguments);
}

fn command_uptime(_arguments: &[u8]) {
    print("uptime ticks: ");
    print_u64(interrupts::ticks());
    print(" (~");
    print_u64(interrupts::ticks() / PIT_HZ);
    println("s)");
}

fn command_sysinfo(_arguments: &[u8]) {
    println("CloudOS system info:");
    println("  version   : v0.0.5");
    println("  arch      : x86_64 long mode");
    println("  boot      : GRUB Multiboot2 ISO");
    println("  console   : VGA text mode 80x25");
    println("  irq       : IDT 256, PIC 8259 remap, PIT timer");
    println("  keyboard  : PS/2 IRQ1 event layer");
    println("  shell     : line editor, history, command table");
}

fn command_mem(_arguments: &[u8]) {
    println("CloudOS memory info:");
    print("  kernel base       : ");
    print_hex_u64(KERNEL_LOAD_BASE);
    newline();
    print("  identity map      : ");
    print_u64(IDENTITY_MAP_BYTES / 1024 / 1024);
    println(" MiB");
    print("  page tables       : ");
    print_u64(PAGE_TABLE_BYTES / 1024);
    println(" KiB");
    print("  boot stack        : ");
    print_u64(STACK_BYTES / 1024);
    println(" KiB");
    print("  vga buffer        : ");
    print_hex_u64(VGA_BUFFER_ADDRESS);
    newline();
    println("  allocator         : none");
    println("  multiboot mem map : not parsed yet");
}

fn command_ticks(_arguments: &[u8]) {
    print("timer ticks: ");
    print_u64(interrupts::ticks());
    print(" at ~");
    print_u64(PIT_HZ);
    println(" Hz");
}

fn command_keyboard(_arguments: &[u8]) {
    println("Keyboard status:");
    println("  controller : PS/2");
    println("  scancode   : set 1");
    println("  irq        : IRQ1 / vector 33");
    print("  shift      : ");
    print_on_off(keyboard::shift_pressed());
    newline();
    print("  caps lock  : ");
    print_on_off(keyboard::caps_lock_enabled());
    newline();
    print("  queued     : ");
    print_u64(keyboard::pending_events() as u64);
    println(" event(s)");
}

fn split_command(input: &[u8]) -> (&[u8], &[u8]) {
    for index in 0..input.len() {
        if input[index] == b' ' || input[index] == b'\t' {
            let command = &input[..index];
            let arguments = trim_ascii(&input[(index + 1)..]);
            return (command, arguments);
        }
    }

    (input, &[])
}

fn println(s: &str) {
    print(s);
    newline();
}

fn print(s: &str) {
    vga::write_string(s);
    serial::serial_print(s);
}

fn print_spaces(count: usize) {
    for _ in 0..count {
        vga::write_byte(b' ');
        serial::write_byte(b' ');
    }
}

fn print_ascii_line(bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        vga::write_byte(byte);
        serial::write_byte(byte);
    }

    newline();
}

fn newline() {
    vga::write_byte(b'\n');
    serial::serial_println("");
}

fn print_u64(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();

    if value == 0 {
        vga::write_byte(b'0');
        serial::write_byte(b'0');
        return;
    }

    while value > 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for byte in digits[index..].iter().copied() {
        vga::write_byte(byte);
        serial::write_byte(byte);
    }
}

fn print_hex_u64(value: u64) {
    print("0x");

    let mut started = false;
    let mut shift = 60u64;

    loop {
        let nibble = ((value >> shift) & 0x0f) as u8;

        if nibble != 0 || started || shift == 0 {
            started = true;
            let byte = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            vga::write_byte(byte);
            serial::write_byte(byte);
        }

        if shift == 0 {
            break;
        }

        shift -= 4;
    }
}

fn print_on_off(enabled: bool) {
    if enabled {
        print("on");
    } else {
        print("off");
    }
}

fn trim_ascii(mut input: &[u8]) -> &[u8] {
    while let Some((first, rest)) = input.split_first() {
        if *first == b' ' || *first == b'\t' {
            input = rest;
        } else {
            break;
        }
    }

    while let Some((last, rest)) = input.split_last() {
        if *last == b' ' || *last == b'\t' {
            input = rest;
        } else {
            break;
        }
    }

    input
}

fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    for index in 0..left.len() {
        if to_ascii_lower(left[index]) != to_ascii_lower(right[index]) {
            return false;
        }
    }

    true
}

fn to_ascii_lower(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte + 32
    } else {
        byte
    }
}
