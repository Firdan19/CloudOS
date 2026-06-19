use crate::keyboard::{self, KeyEvent};
use crate::{interrupts, multiboot, physmem, serial, stats, vga};
use x86_64::instructions::interrupts as cpu_interrupts;

const INPUT_BUFFER_SIZE: usize = 512;
const HISTORY_SIZE: usize = 16;
const PIT_HZ: u64 = 18;
const IDENTITY_MAP_BYTES: u64 = 1024 * 1024 * 1024;
const PAGE_TABLE_BYTES: u64 = 4096 * 3;
const STACK_BYTES: u64 = 16 * 1024;
const VGA_BUFFER_ADDRESS: u64 = 0x000b_8000;

struct Command {
    name: &'static str,
    description: &'static str,
    handler: fn(&[u8]),
}

const COMMANDS: [Command; 17] = [
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
        description: "tampilkan versi Tobacco",
        handler: command_version,
    },
    Command {
        name: "about",
        description: "tampilkan visi singkat Tobacco",
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
        description: "ringkasan memori dari Multiboot2",
        handler: command_mem,
    },
    Command {
        name: "memmap",
        description: "daftar region memory map",
        handler: command_memmap,
    },
    Command {
        name: "frames",
        description: "status physical frame allocator",
        handler: command_frames,
    },
    Command {
        name: "frame",
        description: "alokasi satu frame uji",
        handler: command_frame,
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
    Command {
        name: "perf",
        description: "tampilkan counter performa kernel",
        handler: command_perf,
    },
    Command {
        name: "irq",
        description: "tampilkan counter interrupt",
        handler: command_irq,
    },
    Command {
        name: "boot",
        description: "tampilkan status boot Phase 1",
        handler: command_boot,
    },
    Command {
        name: "bench",
        description: "benchmark ringan kernel",
        handler: command_bench,
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
                    serial::log("shell", "input cleared by escape");
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
                            stats::inc_shell_history_recall();
                            vga::render_input_with_cursor(&input[..input_len], cursor);
                        }
                    }
                }
                KeyEvent::ArrowDown => {
                    if let Some(index) = history.next_index(history_selected) {
                        history_selected = Some(index);
                        if history.load(index, &mut input, &mut input_len, &mut cursor) {
                            stats::inc_shell_history_recall();
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
                    serial::log_bool("keyboard", "caps lock", enabled);
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
        stats::inc_shell_empty_command();
        serial::log("shell", "empty command ignored");
        return;
    }

    let (command, arguments) = split_command(command_line);
    stats::inc_shell_command();
    serial::log_bytes("shell", "command", command);

    for command_entry in COMMANDS.iter() {
        if eq_ignore_ascii_case(command, command_entry.name.as_bytes()) {
            serial::log_bytes("shell", "handler", command_entry.name.as_bytes());
            (command_entry.handler)(arguments);
            return;
        }
    }

    serial::log_bytes("shell", "unknown command", command);
    stats::inc_shell_error();
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
    serial::log("shell", "clear screen requested");
    vga::show_splash();
}

fn command_version(_arguments: &[u8]) {
    println("Tobacco v0.0.5");
}

fn command_about(_arguments: &[u8]) {
    println("Tobacco: Sistem operasi untuk semua, tanpa perlu perangkat mahal.");
}

fn command_echo(arguments: &[u8]) {
    print_ascii_line(arguments);
}

fn command_uptime(_arguments: &[u8]) {
    serial::log_u64("shell", "uptime ticks", interrupts::ticks());
    print("uptime ticks: ");
    print_u64(interrupts::ticks());
    print(" (~");
    print_u64(interrupts::ticks() / PIT_HZ);
    println("s)");
}

fn command_sysinfo(_arguments: &[u8]) {
    println("Tobacco system info:");
    println("  version   : v0.0.5");
    println("  arch      : x86_64 long mode");
    println("  boot      : GRUB Multiboot2 ISO");
    println("  console   : VGA text mode 80x25");
    println("  irq       : IDT 256, PIC 8259 remap, PIT timer");
    println("  keyboard  : PS/2 IRQ1 event layer");
    println("  boot info : Multiboot2 parser + memory map");
    println("  memory    : physical frame allocator");
    println("  exceptions: vector-specific panic diagnostics");
    println("  shell     : line editor, history, command table");
    println("  metrics   : perf, irq, boot, bench");
}

fn command_mem(_arguments: &[u8]) {
    let boot_info = multiboot::summary();
    let memory = boot_info.memory;
    let frames = physmem::snapshot();

    println("Tobacco memory info:");
    print("  kernel start      : ");
    print_hex_u64(frames.kernel_start);
    newline();
    print("  kernel end        : ");
    print_hex_u64(frames.kernel_end);
    newline();
    print("  identity map      : ");
    print_u64(IDENTITY_MAP_BYTES / 1024 / 1024);
    println(" MiB");
    print("  multiboot parsed  : ");
    print_on_off(boot_info.parsed);
    newline();
    print("  boot info addr    : ");
    print_hex_u64(boot_info.address);
    newline();

    if memory.has_basic_memory {
        print("  lower memory      : ");
        print_u64(memory.mem_lower_kib as u64);
        println(" KiB");
        print("  upper memory      : ");
        print_u64(memory.mem_upper_kib as u64);
        println(" KiB");
    } else {
        println("  basic meminfo     : unavailable");
    }

    if memory.has_memory_map {
        print("  memory map        : ");
        print_u64(memory.region_count as u64);
        println(" region(s)");
        print("  usable RAM        : ");
        print_bytes(memory.usable_bytes);
        newline();
        print("  reserved RAM      : ");
        print_bytes(memory.reserved_bytes);
        newline();
        print("  ACPI RAM          : ");
        print_bytes(memory.acpi_bytes);
        newline();
        print("  bad RAM           : ");
        print_bytes(memory.bad_bytes);
        newline();
        print("  highest address   : ");
        print_hex_u64(memory.highest_address);
        newline();
        print("  first usable      : ");
        print_hex_u64(memory.first_usable_base);
        print(" / ");
        print_bytes(memory.first_usable_length);
        newline();
        print("  largest usable    : ");
        print_hex_u64(memory.largest_usable_base);
        print(" / ");
        print_bytes(memory.largest_usable_length);
        newline();
    } else {
        println("  memory map        : unavailable");
    }

    print("  page tables       : ");
    print_u64(PAGE_TABLE_BYTES / 1024);
    println(" KiB");
    print("  boot stack        : ");
    print_u64(STACK_BYTES / 1024);
    println(" KiB");
    print("  vga buffer        : ");
    print_hex_u64(VGA_BUFFER_ADDRESS);
    newline();
    print("  allocator         : ");
    print_on_off(frames.initialized);
    newline();
    print("  protected until   : ");
    print_hex_u64(frames.protected_until);
    newline();
    print("  allocatable       : ");
    print_u64(frames.allocatable_frames);
    println(" frame(s)");
    print("  allocated         : ");
    print_u64(frames.allocated_frames);
    println(" frame(s)");
    print("  free              : ");
    print_u64(frames.free_frames);
    println(" frame(s)");
}

fn command_memmap(_arguments: &[u8]) {
    let boot_info = multiboot::summary();
    let memory = boot_info.memory;

    if !memory.has_memory_map {
        println("Memory map tidak tersedia dari bootloader.");
        return;
    }

    println("Memory map:");

    for index in 0..multiboot::stored_region_count() {
        if let Some(region) = multiboot::region(index) {
            print("  ");
            print_u64(index as u64);
            print(": base ");
            print_hex_u64(region.base_addr);
            print(" len ");
            print_bytes(region.length);
            print(" ");
            println(region.type_name());
        }
    }

    if memory.region_count > memory.stored_region_count {
        print("  stored first ");
        print_u64(memory.stored_region_count as u64);
        print(" of ");
        print_u64(memory.region_count as u64);
        println(" region(s)");
    }
}

fn command_frames(_arguments: &[u8]) {
    let frames = physmem::snapshot();

    println("Frame allocator:");
    print("  initialized       : ");
    print_on_off(frames.initialized);
    newline();
    print("  exhausted         : ");
    print_on_off(frames.exhausted);
    newline();
    print_counter("regions", frames.region_count as u64);
    print_counter("current region", frames.current_region as u64);
    print_counter("total usable", frames.total_usable_frames);
    print_counter("allocatable", frames.allocatable_frames);
    print_counter("allocated", frames.allocated_frames);
    print_counter("free", frames.free_frames);
    print_counter("skipped", frames.skipped_frames);
    print("  next frame        : ");
    print_hex_u64(frames.next_frame);
    newline();
    print("  last allocated    : ");
    print_hex_u64(frames.last_allocated_frame);
    newline();
    print("  kernel start      : ");
    print_hex_u64(frames.kernel_start);
    newline();
    print("  kernel end        : ");
    print_hex_u64(frames.kernel_end);
    newline();
    print("  protected until   : ");
    print_hex_u64(frames.protected_until);
    newline();
}

fn command_frame(_arguments: &[u8]) {
    match physmem::allocate_frame() {
        Some(frame) => {
            serial::log_u64("mem", "allocated frame", frame);
            print("allocated frame: ");
            print_hex_u64(frame);
            newline();
        }
        None => {
            serial::log("mem", "frame allocator exhausted");
            println("Frame allocator habis.");
        }
    }
}

fn command_ticks(_arguments: &[u8]) {
    serial::log_u64("shell", "timer ticks", interrupts::ticks());
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

fn command_perf(_arguments: &[u8]) {
    let snapshot = stats::snapshot();

    println("Performance counters:");
    print_counter("vga cell writes", snapshot.vga_cell_writes);
    print_counter("vga clears", snapshot.vga_clears);
    print_counter("vga scrolls", snapshot.vga_scrolls);
    print_counter("cursor toggles", snapshot.cursor_toggles);
    print_counter("serial bytes", snapshot.serial_bytes);
    print_counter("keyboard scancodes", snapshot.keyboard_scancodes);
    print_counter("keyboard events", snapshot.keyboard_events);
    print_counter("keyboard dropped", snapshot.keyboard_dropped_events);
    print_counter("shell commands", snapshot.shell_commands);
    print_counter("shell empty", snapshot.shell_empty_commands);
    print_counter("shell errors", snapshot.shell_errors);
    print_counter("history recalls", snapshot.shell_history_recalls);
    print_counter("bench runs", snapshot.bench_runs);
}

fn command_irq(_arguments: &[u8]) {
    let snapshot = stats::snapshot();

    println("IRQ counters:");
    print_counter("timer irq", snapshot.timer_irqs);
    print_counter("keyboard irq", snapshot.keyboard_irqs);
    print_counter("default irq", snapshot.default_irqs);
    print_counter("exceptions", snapshot.exceptions);
    print_counter("current ticks", interrupts::ticks());
}

fn command_boot(_arguments: &[u8]) {
    let snapshot = stats::snapshot();
    let boot_info = multiboot::summary();
    let frames = physmem::snapshot();

    println("Boot status:");
    println("  name          : Tobacco");
    println("  version       : v0.0.5");
    println("  mode          : x86_64 long mode");
    println("  boot          : GRUB Multiboot2 ISO");
    println("  page map      : 1 GiB identity map");
    println("  serial log    : structured tags active");
    print("  mb2 magic     : ");
    print_on_off(boot_info.valid_magic);
    newline();
    print("  mb2 parsed    : ");
    print_on_off(boot_info.parsed);
    newline();
    print("  mb2 addr      : ");
    print_hex_u64(boot_info.address);
    newline();
    print("  mb2 tags      : ");
    print_u64(boot_info.tag_count as u64);
    newline();
    if !boot_info.bootloader_name.as_str().is_empty() {
        print("  bootloader    : ");
        println(boot_info.bootloader_name.as_str());
    }
    if !boot_info.command_line.as_str().is_empty() {
        print("  command line  : ");
        println(boot_info.command_line.as_str());
    }
    print("  frame alloc   : ");
    print_on_off(frames.initialized);
    newline();
    print_counter("free frames", frames.free_frames);
    print_counter("shell ready tick", snapshot.shell_ready_tick);
    print_counter("current ticks", interrupts::ticks());
}

fn command_bench(_arguments: &[u8]) {
    stats::inc_bench_run();

    let start = interrupts::ticks();
    let mut accumulator = 0x544f_4241_4343_4f00u64;

    for value in 0..100_000u64 {
        accumulator = accumulator
            .rotate_left(7)
            .wrapping_add(value ^ 0x9e37_79b9_7f4a_7c15);
        accumulator = core::hint::black_box(accumulator);
    }

    let end = interrupts::ticks();
    serial::log_u64("bench", "accumulator", accumulator);

    println("Bench:");
    print_counter("iterations", 100_000);
    print_counter("start tick", start);
    print_counter("end tick", end);
    print_counter("delta ticks", end.saturating_sub(start));
    print("  accumulator   : ");
    print_hex_u64(accumulator);
    newline();
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

fn print_counter(label: &str, value: u64) {
    print("  ");
    print(label);
    print_spaces(17usize.saturating_sub(label.len()));
    print(": ");
    print_u64(value);
    newline();
}

fn print_bytes(bytes: u64) {
    if bytes >= 1024 * 1024 {
        print_u64(bytes / 1024 / 1024);
        print(" MiB");
    } else if bytes >= 1024 {
        print_u64(bytes / 1024);
        print(" KiB");
    } else {
        print_u64(bytes);
        print(" B");
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
