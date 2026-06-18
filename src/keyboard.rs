use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use x86_64::instructions::port::Port;

const KEYBOARD_DATA_PORT: u16 = 0x60;
const KEYBOARD_STATUS_PORT: u16 = 0x64;
const KEY_BUFFER_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ShiftPressed,
    ShiftReleased,
    CapsLockToggled(bool),
}

struct KeyBuffer {
    buffer: UnsafeCell<[KeyEvent; KEY_BUFFER_SIZE]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

unsafe impl Sync for KeyBuffer {}

impl KeyBuffer {
    const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new([KeyEvent::Escape; KEY_BUFFER_SIZE]),
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn push(&self, event: KeyEvent) {
        let write = self.write_index.load(Ordering::Relaxed);
        let next_write = (write + 1) % KEY_BUFFER_SIZE;

        if next_write == self.read_index.load(Ordering::Acquire) {
            return;
        }

        unsafe {
            let ptr = self.buffer.get().cast::<KeyEvent>().add(write);
            ptr.write(event);
        }

        self.write_index.store(next_write, Ordering::Release);
    }

    fn pop(&self) -> Option<KeyEvent> {
        let read = self.read_index.load(Ordering::Relaxed);

        if read == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        let event = unsafe {
            let ptr = self.buffer.get().cast::<KeyEvent>().add(read);
            ptr.read()
        };

        let next_read = (read + 1) % KEY_BUFFER_SIZE;
        self.read_index.store(next_read, Ordering::Release);

        Some(event)
    }
}

static KEY_BUFFER: KeyBuffer = KeyBuffer::new();
static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK: AtomicBool = AtomicBool::new(false);
static EXTENDED_SCANCODE: AtomicBool = AtomicBool::new(false);

pub fn init() {
    drain_output_buffer();
}

pub fn handle_interrupt() {
    let scancode = read_scancode();
    handle_scancode(scancode);
}

pub fn poll() {
    for _ in 0..16 {
        if !has_pending_scancode() {
            break;
        }

        handle_scancode(read_scancode());
    }
}

pub fn pop_event() -> Option<KeyEvent> {
    KEY_BUFFER.pop()
}

fn drain_output_buffer() {
    for _ in 0..32 {
        if !has_pending_scancode() {
            break;
        }

        let _ = read_scancode();
    }
}

fn has_pending_scancode() -> bool {
    let mut status_port = Port::<u8>::new(KEYBOARD_STATUS_PORT);
    let status = unsafe { status_port.read() };

    status & 0x01 != 0
}

fn read_scancode() -> u8 {
    let mut data_port = Port::<u8>::new(KEYBOARD_DATA_PORT);
    unsafe { data_port.read() }
}

fn handle_scancode(scancode: u8) {
    if scancode == 0xe0 {
        EXTENDED_SCANCODE.store(true, Ordering::Release);
        return;
    }

    if EXTENDED_SCANCODE.swap(false, Ordering::AcqRel) {
        handle_extended_scancode(scancode);
        return;
    }

    match scancode {
        0x2a | 0x36 => {
            SHIFT_PRESSED.store(true, Ordering::Release);
            KEY_BUFFER.push(KeyEvent::ShiftPressed);
            return;
        }
        0xaa | 0xb6 => {
            SHIFT_PRESSED.store(false, Ordering::Release);
            KEY_BUFFER.push(KeyEvent::ShiftReleased);
            return;
        }
        0x3a => {
            let enabled = !CAPS_LOCK.load(Ordering::Acquire);
            CAPS_LOCK.store(enabled, Ordering::Release);
            KEY_BUFFER.push(KeyEvent::CapsLockToggled(enabled));
            return;
        }
        _ => {}
    }

    if scancode & 0x80 != 0 {
        return;
    }

    if let Some(event) = scancode_to_event(scancode) {
        KEY_BUFFER.push(event);
    }
}

fn handle_extended_scancode(scancode: u8) {
    if scancode & 0x80 != 0 {
        return;
    }

    let event = match scancode {
        0x48 => Some(KeyEvent::ArrowUp),
        0x50 => Some(KeyEvent::ArrowDown),
        0x4b => Some(KeyEvent::ArrowLeft),
        0x4d => Some(KeyEvent::ArrowRight),
        _ => None,
    };

    if let Some(event) = event {
        KEY_BUFFER.push(event);
    }
}

fn scancode_to_event(scancode: u8) -> Option<KeyEvent> {
    let shifted = SHIFT_PRESSED.load(Ordering::Acquire);
    let caps = CAPS_LOCK.load(Ordering::Acquire);

    match scancode {
        0x01 => Some(KeyEvent::Escape),
        0x02 => Some(KeyEvent::Char(if shifted { b'!' } else { b'1' })),
        0x03 => Some(KeyEvent::Char(if shifted { b'@' } else { b'2' })),
        0x04 => Some(KeyEvent::Char(if shifted { b'#' } else { b'3' })),
        0x05 => Some(KeyEvent::Char(if shifted { b'$' } else { b'4' })),
        0x06 => Some(KeyEvent::Char(if shifted { b'%' } else { b'5' })),
        0x07 => Some(KeyEvent::Char(if shifted { b'^' } else { b'6' })),
        0x08 => Some(KeyEvent::Char(if shifted { b'&' } else { b'7' })),
        0x09 => Some(KeyEvent::Char(if shifted { b'*' } else { b'8' })),
        0x0a => Some(KeyEvent::Char(if shifted { b'(' } else { b'9' })),
        0x0b => Some(KeyEvent::Char(if shifted { b')' } else { b'0' })),
        0x0c => Some(KeyEvent::Char(if shifted { b'_' } else { b'-' })),
        0x0d => Some(KeyEvent::Char(if shifted { b'+' } else { b'=' })),
        0x0e => Some(KeyEvent::Backspace),
        0x0f => Some(KeyEvent::Tab),
        0x10 => Some(KeyEvent::Char(letter(b'q', shifted, caps))),
        0x11 => Some(KeyEvent::Char(letter(b'w', shifted, caps))),
        0x12 => Some(KeyEvent::Char(letter(b'e', shifted, caps))),
        0x13 => Some(KeyEvent::Char(letter(b'r', shifted, caps))),
        0x14 => Some(KeyEvent::Char(letter(b't', shifted, caps))),
        0x15 => Some(KeyEvent::Char(letter(b'y', shifted, caps))),
        0x16 => Some(KeyEvent::Char(letter(b'u', shifted, caps))),
        0x17 => Some(KeyEvent::Char(letter(b'i', shifted, caps))),
        0x18 => Some(KeyEvent::Char(letter(b'o', shifted, caps))),
        0x19 => Some(KeyEvent::Char(letter(b'p', shifted, caps))),
        0x1a => Some(KeyEvent::Char(if shifted { b'{' } else { b'[' })),
        0x1b => Some(KeyEvent::Char(if shifted { b'}' } else { b']' })),
        0x1c => Some(KeyEvent::Enter),
        0x1e => Some(KeyEvent::Char(letter(b'a', shifted, caps))),
        0x1f => Some(KeyEvent::Char(letter(b's', shifted, caps))),
        0x20 => Some(KeyEvent::Char(letter(b'd', shifted, caps))),
        0x21 => Some(KeyEvent::Char(letter(b'f', shifted, caps))),
        0x22 => Some(KeyEvent::Char(letter(b'g', shifted, caps))),
        0x23 => Some(KeyEvent::Char(letter(b'h', shifted, caps))),
        0x24 => Some(KeyEvent::Char(letter(b'j', shifted, caps))),
        0x25 => Some(KeyEvent::Char(letter(b'k', shifted, caps))),
        0x26 => Some(KeyEvent::Char(letter(b'l', shifted, caps))),
        0x27 => Some(KeyEvent::Char(if shifted { b':' } else { b';' })),
        0x28 => Some(KeyEvent::Char(if shifted { b'"' } else { b'\'' })),
        0x29 => Some(KeyEvent::Char(if shifted { b'~' } else { b'`' })),
        0x2b => Some(KeyEvent::Char(if shifted { b'|' } else { b'\\' })),
        0x2c => Some(KeyEvent::Char(letter(b'z', shifted, caps))),
        0x2d => Some(KeyEvent::Char(letter(b'x', shifted, caps))),
        0x2e => Some(KeyEvent::Char(letter(b'c', shifted, caps))),
        0x2f => Some(KeyEvent::Char(letter(b'v', shifted, caps))),
        0x30 => Some(KeyEvent::Char(letter(b'b', shifted, caps))),
        0x31 => Some(KeyEvent::Char(letter(b'n', shifted, caps))),
        0x32 => Some(KeyEvent::Char(letter(b'm', shifted, caps))),
        0x33 => Some(KeyEvent::Char(if shifted { b'<' } else { b',' })),
        0x34 => Some(KeyEvent::Char(if shifted { b'>' } else { b'.' })),
        0x35 => Some(KeyEvent::Char(if shifted { b'?' } else { b'/' })),
        0x39 => Some(KeyEvent::Char(b' ')),
        _ => None,
    }
}

fn letter(lowercase: u8, shifted: bool, caps: bool) -> u8 {
    if shifted ^ caps {
        lowercase - 32
    } else {
        lowercase
    }
}
