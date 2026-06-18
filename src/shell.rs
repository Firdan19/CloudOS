use crate::keyboard::KeyEvent;
use crate::{interrupts, serial, vga};
use x86_64::instructions::interrupts as cpu_interrupts;

const INPUT_BUFFER_SIZE: usize = 512;
const PIT_HZ: u64 = 18;

pub fn run() -> ! {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;

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
                    execute(&input[..input_len]);
                    input_len = 0;
                    cursor = 0;
                    prompt();
                }
                KeyEvent::Backspace => {
                    if delete_previous_input_byte(&mut input, &mut input_len, &mut cursor) {
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::Escape => {
                    input_len = 0;
                    cursor = 0;
                    serial::serial_println("^esc");
                    vga::render_input_with_cursor(&input[..input_len], cursor);
                }
                KeyEvent::Tab => {
                    if insert_input_byte(&mut input, &mut input_len, &mut cursor, b' ') {
                        vga::render_input_with_cursor(&input[..input_len], cursor);
                    }
                }
                KeyEvent::Char(byte) if (0x20..=0x7e).contains(&byte) => {
                    if insert_input_byte(&mut input, &mut input_len, &mut cursor, byte) {
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
                KeyEvent::ArrowUp
                | KeyEvent::ArrowDown
                | KeyEvent::ShiftPressed
                | KeyEvent::ShiftReleased => {}
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

    if eq_ignore_ascii_case(command, b"help") {
        println("help clear version about echo uptime");
    } else if eq_ignore_ascii_case(command, b"clear") {
        serial::serial_println("clear");
        vga::show_splash();
    } else if eq_ignore_ascii_case(command, b"version") {
        println("CloudOS v0.0.5");
    } else if eq_ignore_ascii_case(command, b"about") {
        println("CloudOS: Sistem operasi untuk semua, tanpa perlu perangkat mahal.");
    } else if eq_ignore_ascii_case(command, b"echo") {
        print_ascii_line(arguments);
    } else if eq_ignore_ascii_case(command, b"uptime") {
        print("uptime ticks: ");
        print_u64(interrupts::ticks());
        print(" (~");
        print_u64(interrupts::ticks() / PIT_HZ);
        println("s)");
    } else {
        println("Perintah tidak dikenal. Ketik help.");
    }
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
