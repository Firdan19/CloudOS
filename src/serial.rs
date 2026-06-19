use crate::stats;
use x86_64::instructions::interrupts as cpu_interrupts;
use x86_64::instructions::port::Port;

const COM1: u16 = 0x3f8;

pub fn init() {
    cpu_interrupts::without_interrupts(|| unsafe {
        let mut interrupt_enable = Port::<u8>::new(COM1 + 1);
        let mut line_control = Port::<u8>::new(COM1 + 3);
        let mut data = Port::<u8>::new(COM1);
        let mut fifo_control = Port::<u8>::new(COM1 + 2);
        let mut modem_control = Port::<u8>::new(COM1 + 4);

        interrupt_enable.write(0x00);
        line_control.write(0x80);
        data.write(0x03);
        interrupt_enable.write(0x00);
        line_control.write(0x03);
        fifo_control.write(0xc7);
        modem_control.write(0x0b);
    });
}

pub fn serial_print(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}

pub fn serial_print_bytes(bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        write_byte(byte);
    }
}

pub fn serial_println(s: &str) {
    serial_print(s);
    serial_print("\n");
}

pub fn log(tag: &str, message: &str) {
    serial_print("[");
    serial_print(tag);
    serial_print("] ");
    serial_println(message);
}

pub fn log_bytes(tag: &str, label: &str, bytes: &[u8]) {
    serial_print("[");
    serial_print(tag);
    serial_print("] ");
    serial_print(label);
    serial_print(": ");
    serial_print_bytes(bytes);
    serial_print("\n");
}

pub fn log_u64(tag: &str, label: &str, value: u64) {
    serial_print("[");
    serial_print(tag);
    serial_print("] ");
    serial_print(label);
    serial_print(": ");
    serial_print_u64(value);
    serial_print("\n");
}

pub fn log_hex_u64(tag: &str, label: &str, value: u64) {
    serial_print("[");
    serial_print(tag);
    serial_print("] ");
    serial_print(label);
    serial_print(": ");
    serial_print_hex_u64(value);
    serial_print("\n");
}

pub fn log_bool(tag: &str, label: &str, enabled: bool) {
    serial_print("[");
    serial_print(tag);
    serial_print("] ");
    serial_print(label);
    serial_print(": ");
    if enabled {
        serial_print("on");
    } else {
        serial_print("off");
    }
    serial_print("\n");
}

pub fn write_byte(byte: u8) {
    cpu_interrupts::without_interrupts(|| match byte {
        b'\n' => {
            write_raw_byte(b'\r');
            write_raw_byte(b'\n');
        }
        byte => write_raw_byte(byte),
    });
}

fn serial_print_u64(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();

    if value == 0 {
        write_byte(b'0');
        return;
    }

    while value > 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for byte in digits[index..].iter().copied() {
        write_byte(byte);
    }
}

fn serial_print_hex_u64(value: u64) {
    serial_print("0x");

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
            write_byte(byte);
        }

        if shift == 0 {
            break;
        }

        shift -= 4;
    }
}

fn write_raw_byte(byte: u8) {
    unsafe {
        let mut data = Port::<u8>::new(COM1);
        let mut line_status = Port::<u8>::new(COM1 + 5);

        while line_status.read() & 0x20 == 0 {}

        data.write(byte);
        stats::inc_serial_byte();
    }
}
