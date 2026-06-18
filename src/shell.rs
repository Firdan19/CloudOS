use crate::{interrupts, serial, vga};
use x86_64::instructions::interrupts as cpu_interrupts;

const INPUT_BUFFER_SIZE: usize = 512;

pub fn run() -> ! {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;

    prompt();

    loop {
        cpu_interrupts::disable();
        interrupts::poll_keyboard();

        if let Some(byte) = interrupts::pop_key() {
            cpu_interrupts::enable();

            match byte {
                b'\n' => {
                    serial::serial_println("");
                    vga::write_byte(b'\n');
                    execute(&input[..input_len]);
                    input_len = 0;
                    prompt();
                }
                8 => {
                    if input_len > 0 {
                        input_len -= 1;
                        serial::serial_print("\x08 \x08");
                        vga::render_input(&input[..input_len]);
                    }
                }
                b'\t' => {
                    if input_len < INPUT_BUFFER_SIZE {
                        input[input_len] = b' ';
                        input_len += 1;
                        serial::write_byte(b' ');
                        vga::render_input(&input[..input_len]);
                    }
                }
                0x20..=0x7e => {
                    if input_len < INPUT_BUFFER_SIZE {
                        input[input_len] = byte;
                        input_len += 1;
                        serial::write_byte(byte);
                        vga::render_input(&input[..input_len]);
                    }
                }
                _ => {}
            }
        } else {
            cpu_interrupts::enable_and_hlt();
        }
    }
}

fn prompt() {
    serial::serial_print("> ");
    vga::start_prompt();
}

fn execute(input: &[u8]) {
    let command = trim_ascii(input);

    if command.is_empty() {
        return;
    }

    if command == b"help" {
        println("help clear version about");
    } else if command == b"clear" {
        serial::serial_println("clear");
        vga::show_splash();
    } else if command == b"version" {
        println("CloudOS v0.0.2");
    } else if command == b"about" {
        println("CloudOS: Sistem operasi untuk semua, tanpa perlu perangkat mahal.");
    } else {
        println("Perintah tidak dikenal. Ketik help.");
    }
}

fn println(s: &str) {
    vga::write_line(s);
    serial::serial_println(s);
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
