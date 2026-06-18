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

pub fn write_byte(byte: u8) {
    cpu_interrupts::without_interrupts(|| match byte {
        b'\n' => {
            write_raw_byte(b'\r');
            write_raw_byte(b'\n');
        }
        byte => write_raw_byte(byte),
    });
}

fn write_raw_byte(byte: u8) {
    unsafe {
        let mut data = Port::<u8>::new(COM1);
        let mut line_status = Port::<u8>::new(COM1 + 5);

        while line_status.read() & 0x20 == 0 {}

        data.write(byte);
    }
}
