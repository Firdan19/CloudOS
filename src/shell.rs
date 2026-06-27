use crate::keyboard::{self, KeyEvent};
use crate::{
    buildinfo, elf, gdt, heap, interrupts, klog, multiboot, paging, paniclog, physmem, scheduler,
    serial, stats, user_program, vga,
};
use crate::{process, syscall, user};
use x86_64::instructions::interrupts as cpu_interrupts;

const INPUT_BUFFER_SIZE: usize = 512;
const HISTORY_SIZE: usize = 16;
const PIT_HZ: u64 = 18;
const STACK_BYTES: u64 = 16 * 1024;
const VGA_BUFFER_ADDRESS: u64 = 0x000b_8000;
const STRESS_DEFAULT_CYCLES: usize = 4;
const STRESS_MAX_CYCLES: usize = 32;

struct Command {
    name: &'static str,
    description: &'static str,
    handler: fn(&[u8]),
}

const COMMANDS: [Command; 49] = [
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
        description: "tampilkan waktu hidup kernel",
        handler: command_uptime,
    },
    Command {
        name: "buildinfo",
        description: "tampilkan metadata build kernel",
        handler: command_buildinfo,
    },
    Command {
        name: "health",
        description: "ringkasan kesehatan kernel",
        handler: command_health,
    },
    Command {
        name: "status",
        description: "alias cepat untuk health",
        handler: command_health,
    },
    Command {
        name: "diag",
        description: "diagnostik observability lengkap",
        handler: command_diag,
    },
    Command {
        name: "lastpanic",
        description: "tampilkan panic terakhir di boot ini",
        handler: command_lastpanic,
    },
    Command {
        name: "faults",
        description: "status fault dan exception kernel",
        handler: command_faults,
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
        name: "mmap",
        description: "alias untuk memmap",
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
        name: "consoletest",
        description: "uji editor, history, dan command lookup",
        handler: command_consoletest,
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
        name: "user",
        description: "status Ring 3 dan user mode",
        handler: command_user,
    },
    Command {
        name: "elf",
        description: "status loader ELF64 user",
        handler: command_elf,
    },
    Command {
        name: "elftest",
        description: "jalankan ELF /init di Ring 3",
        handler: command_elftest,
    },
    Command {
        name: "process",
        description: "status task dan process model",
        handler: command_process,
    },
    Command {
        name: "tasks",
        description: "daftar task kernel",
        handler: command_tasks,
    },
    Command {
        name: "sched",
        description: "status scheduler minimal",
        handler: command_scheduler,
    },
    Command {
        name: "scheduler",
        description: "alias untuk sched",
        handler: command_scheduler,
    },
    Command {
        name: "usertest",
        description: "jalankan user probe sebagai task",
        handler: command_usertest,
    },
    Command {
        name: "tasktest",
        description: "alias untuk usertest",
        handler: command_usertest,
    },
    Command {
        name: "faulttest",
        description: "uji isolasi page fault user",
        handler: command_faulttest,
    },
    Command {
        name: "syscall",
        description: "tampilkan ABI syscall minimal",
        handler: command_syscall,
    },
    Command {
        name: "syscalls",
        description: "alias untuk syscall",
        handler: command_syscall,
    },
    Command {
        name: "gdt",
        description: "status GDT, TSS, dan IST",
        handler: command_gdt,
    },
    Command {
        name: "idt",
        description: "status IDT dan interrupt gates",
        handler: command_idt,
    },
    Command {
        name: "paging",
        description: "status page table boot",
        handler: command_paging,
    },
    Command {
        name: "heap",
        description: "status kernel heap",
        handler: command_heap,
    },
    Command {
        name: "heaptest",
        description: "uji allocator heap",
        handler: command_heaptest,
    },
    Command {
        name: "heapcheck",
        description: "cek integritas heap",
        handler: command_heapcheck,
    },
    Command {
        name: "virt",
        description: "translate virtual address",
        handler: command_virt,
    },
    Command {
        name: "vmtest",
        description: "uji map/unmap virtual page",
        handler: command_vmtest,
    },
    Command {
        name: "log",
        description: "tampilkan kernel ring log",
        handler: command_log,
    },
    Command {
        name: "dmesg",
        description: "alias untuk kernel ring log",
        handler: command_log,
    },
    Command {
        name: "selftest",
        description: "uji mandiri subsistem Phase 1",
        handler: command_selftest,
    },
    Command {
        name: "stress",
        description: "uji stabilitas ringan kernel",
        handler: command_stress,
    },
    Command {
        name: "bench",
        description: "benchmark ringan kernel",
        handler: command_bench,
    },
];

pub fn command_count() -> usize {
    COMMANDS.len()
}

pub fn command_exists(name: &[u8]) -> bool {
    for command in COMMANDS.iter() {
        if eq_ignore_ascii_case(name, command.name.as_bytes()) {
            return true;
        }
    }

    false
}

pub struct ConsoleModelReport {
    pub long_input: bool,
    pub backspace_long: bool,
    pub line_editing: bool,
    pub history_navigation: bool,
    pub command_lookup: bool,
}

pub fn run_console_model_checks() -> ConsoleModelReport {
    ConsoleModelReport {
        long_input: check_long_input_capacity(),
        backspace_long: check_long_backspace(),
        line_editing: check_line_editing(),
        history_navigation: check_history_navigation(),
        command_lookup: check_command_lookup(),
    }
}

pub fn execute_for_ci(input: &[u8]) {
    execute(input);
}

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

fn check_long_input_capacity() -> bool {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;
    let mut inserted = 0usize;

    for index in 0..(INPUT_BUFFER_SIZE + 16) {
        if insert_input_byte(
            &mut input,
            &mut input_len,
            &mut cursor,
            b'a' + (index % 26) as u8,
        ) {
            inserted += 1;
        }
    }

    inserted == INPUT_BUFFER_SIZE
        && input_len == INPUT_BUFFER_SIZE
        && cursor == INPUT_BUFFER_SIZE
        && !insert_input_byte(&mut input, &mut input_len, &mut cursor, b'!')
}

fn check_long_backspace() -> bool {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;

    for _ in 0..INPUT_BUFFER_SIZE {
        if !insert_input_byte(&mut input, &mut input_len, &mut cursor, b'x') {
            return false;
        }
    }

    let mut deleted = 0usize;
    while delete_previous_input_byte(&mut input, &mut input_len, &mut cursor) {
        deleted += 1;
    }

    deleted == INPUT_BUFFER_SIZE && input_len == 0 && cursor == 0
}

fn check_line_editing() -> bool {
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;

    for byte in b"helo".iter().copied() {
        if !insert_input_byte(&mut input, &mut input_len, &mut cursor, byte) {
            return false;
        }
    }

    cursor = 3;
    if !insert_input_byte(&mut input, &mut input_len, &mut cursor, b'l') {
        return false;
    }

    cursor = 1;
    if !insert_input_byte(&mut input, &mut input_len, &mut cursor, b'E') {
        return false;
    }

    if !delete_previous_input_byte(&mut input, &mut input_len, &mut cursor) {
        return false;
    }

    input_len == 5 && cursor == 1 && bytes_eq(&input[..input_len], b"hello")
}

fn check_history_navigation() -> bool {
    let mut history = CommandHistory::new();
    let mut input = [0u8; INPUT_BUFFER_SIZE];
    let mut input_len = 0usize;
    let mut cursor = 0usize;

    history.push(b"help");
    history.push(b"version");
    history.push(b"about");
    history.push(b"about");

    if history.len != 3 {
        return false;
    }

    let Some(latest) = history.previous_index(None) else {
        return false;
    };

    if !history.load(latest, &mut input, &mut input_len, &mut cursor)
        || !bytes_eq(&input[..input_len], b"about")
        || cursor != input_len
    {
        return false;
    }

    let Some(previous) = history.previous_index(Some(latest)) else {
        return false;
    };

    if !history.load(previous, &mut input, &mut input_len, &mut cursor)
        || !bytes_eq(&input[..input_len], b"version")
    {
        return false;
    }

    let Some(next) = history.next_index(Some(previous)) else {
        return false;
    };

    history.load(next, &mut input, &mut input_len, &mut cursor)
        && bytes_eq(&input[..input_len], b"about")
        && history.next_index(Some(next)).is_none()
}

fn check_command_lookup() -> bool {
    command_exists(b"HELP")
        && command_exists(b"HeLp")
        && command_exists(b"stress")
        && !command_exists(b"definitely-not-a-command")
}

fn bytes_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    for index in 0..left.len() {
        if left[index] != right[index] {
            return false;
        }
    }

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
    let ticks = interrupts::ticks();
    let seconds = ticks / PIT_HZ;
    let remainder_ticks = ticks % PIT_HZ;
    let ready_tick = stats::snapshot().shell_ready_tick;

    serial::log_u64("shell", "uptime seconds", seconds);
    println("Uptime:");
    print("  time      : ");
    print_duration(seconds);
    newline();
    print_counter("seconds", seconds);
    print_counter("ticks", ticks);
    print_counter("tick hz", PIT_HZ);
    print_counter("partial tick", remainder_ticks);
    print_counter("shell ready", ready_tick);
}

fn command_buildinfo(_arguments: &[u8]) {
    serial::log("build", "buildinfo requested");

    println("Build info:");
    print("  name      : ");
    println(buildinfo::OS_NAME);
    print("  package   : ");
    println(buildinfo::PACKAGE_NAME);
    print("  version   : ");
    println(buildinfo::PACKAGE_VERSION);
    print("  git commit: ");
    println(buildinfo::GIT_COMMIT);
    print("  build time: ");
    println(buildinfo::BUILD_TIME);
    print("  profile   : ");
    println(buildinfo::BUILD_PROFILE);
    print("  target    : ");
    println(buildinfo::BUILD_TARGET);
    print("  features  : ");
    println(buildinfo::FEATURE_FLAGS);
    print("  toolchain : ");
    println(buildinfo::TOOLCHAIN);
    print("  edition   : Rust ");
    println(buildinfo::RUST_EDITION);
    print("  boot      : ");
    println(buildinfo::BOOT_PROTOCOL);
    print("  mode      : ");
    println(buildinfo::KERNEL_MODE);
    print("  reproducible: ");
    println(buildinfo::REPRODUCIBILITY);
}

fn command_health(_arguments: &[u8]) {
    let issues = print_health_report();

    if issues == 0 {
        serial::log("health", "status ok");
        println("status: OK");
    } else {
        serial::log_u64("health", "status issues", issues);
        print("status: WARN (");
        print_u64(issues);
        println(" issue(s))");
    }
}

fn command_diag(_arguments: &[u8]) {
    let boot_info = multiboot::summary();
    let frames = physmem::snapshot();
    let paging_state = paging::snapshot();
    let paging_audit = paging::permission_audit();
    let heap_state = heap::snapshot();
    let log = klog::snapshot();
    let counters = stats::snapshot();
    let gdt_state = gdt::snapshot();
    let panic = paniclog::snapshot();
    let process_state = process::snapshot();
    let scheduler_state = scheduler::snapshot();
    let syscall_state = syscall::snapshot();
    let user_state = user::snapshot();
    let interrupt_abi = interrupts::abi_snapshot();

    serial::log("diag", "diagnostic report requested");
    println("Tobacco diagnostics:");

    print("  uptime       : ");
    print_duration(interrupts::ticks() / PIT_HZ);
    newline();
    print("  build        : ");
    print(buildinfo::PACKAGE_VERSION);
    print(" / ");
    println(buildinfo::GIT_COMMIT);
    print("  boot parsed  : ");
    print_on_off(boot_info.parsed);
    newline();
    print_counter("boot tags", boot_info.tag_count as u64);
    print_counter("usable bytes", boot_info.memory.usable_bytes);
    print_counter("free frames", frames.free_frames);
    print_counter("mapped pages", paging_state.mapped_pages);
    print_counter("tracked pages", paging_state.tracked_mappings);
    print_counter("permission faults", paging_audit.violations);
    print_counter("heap used", heap_state.used);
    print_counter("heap remain", heap_state.remaining);
    print_counter("log entries", log.count);
    print_counter("log dropped", log.dropped);
    print_counter("timer irq", counters.timer_irqs);
    print_counter("keyboard irq", counters.keyboard_irqs);
    print_counter("exceptions", counters.exceptions);
    print_counter("shell errors", counters.shell_errors);
    print_counter("vga scrolls", counters.vga_scrolls);
    print_counter("serial bytes", counters.serial_bytes);
    print("  gdt/tss      : ");
    print_on_off(gdt_state.loaded);
    newline();
    print("  irq abi      : ");
    print_on_off(interrupt_abi_is_healthy(interrupt_abi));
    newline();
    print_counter("syscall vector", interrupt_abi.syscall_vector);
    print_counter("syscall table", syscall_state.entries);
    print_counter("syscall dispatch", syscall_state.dispatches);
    print_counter("syscall unknown", syscall_state.unknown_syscalls);
    print_counter("tasks spawned", process_state.spawned_tasks);
    print_counter("tasks exited", process_state.exited_total);
    print_counter("ctx switches", scheduler_state.context_switches);
    print_counter("task ticks", scheduler_state.accounted_ticks);
    print("  user mode    : ");
    print_on_off(user_state.initialized && user_state.syscall_gate_ready);
    newline();
    print_counter("syscalls", user_state.syscall_count);
    print_counter("user probes", user_state.run_count);
    print("  last panic   : ");
    print_on_off(panic.present);
    newline();

    let issues = health_issue_count();
    print_counter("health issues", issues);
}

fn command_lastpanic(_arguments: &[u8]) {
    let panic = paniclog::snapshot();

    println("Last panic:");
    if !panic.present {
        println("  none recorded in this boot");
        return;
    }

    print("  tick      : ");
    print_u64(panic.tick);
    newline();
    print("  kind      : ");
    print_log_bytes(panic.kind());
    newline();
    print("  detail    : ");
    print_log_bytes(panic.detail());
    newline();
    print("  vector    : ");
    print_u64(panic.vector);
    newline();
    print("  error     : ");
    print_hex_u64(panic.error_code);
    newline();
    print("  rip       : ");
    print_hex_u64(panic.instruction_pointer);
    newline();
    print("  cr2       : ");
    print_hex_u64(panic.fault_address);
    newline();
    print("  rflags    : ");
    print_hex_u64(panic.cpu_flags);
    newline();
}

fn command_faults(_arguments: &[u8]) {
    let panic = paniclog::snapshot();
    let counters = stats::snapshot();
    let paging_state = paging::snapshot();

    serial::log("faults", "fault status requested");
    println("Faults:");
    print_counter("exceptions", counters.exceptions);
    print_counter("default irq", counters.default_irqs);
    print_counter("user faults", user::snapshot().fault_count);
    print_counter("page track miss", paging_state.tracking_misses);
    print_counter("page track overflow", paging_state.tracking_overflows);
    print_counter("permission faults", paging_state.permission_violations);
    print("  last panic       : ");
    print_on_off(panic.present);
    newline();

    if !panic.present {
        println("  last detail      : none recorded");
        return;
    }

    print("  kind             : ");
    print_log_bytes(panic.kind());
    newline();
    print("  detail           : ");
    print_log_bytes(panic.detail());
    newline();
    print("  vector           : ");
    print_u64(panic.vector);
    newline();
    print("  error            : ");
    print_hex_u64(panic.error_code);
    newline();
    print("  rip              : ");
    print_hex_u64(panic.instruction_pointer);
    newline();
    print("  cr2              : ");
    print_hex_u64(panic.fault_address);
    newline();
    print("  rflags           : ");
    print_hex_u64(panic.cpu_flags);
    newline();
}

fn print_health_report() -> u64 {
    let boot_info = multiboot::summary();
    let frames = physmem::snapshot();
    let paging_state = paging::snapshot();
    let paging_audit = paging::permission_audit();
    let heap_state = heap::snapshot();
    let gdt_state = gdt::snapshot();
    let log = klog::snapshot();
    let counters = stats::snapshot();
    let ticks = interrupts::ticks();
    let panic = paniclog::snapshot();
    let process_state = process::snapshot();
    let scheduler_state = scheduler::snapshot();
    let syscall_state = syscall::snapshot();
    let user_state = user::snapshot();
    let interrupt_abi = interrupts::abi_snapshot();
    let mut issues = 0u64;

    println("Tobacco health:");
    health_line(
        "boot",
        boot_info.valid_magic && boot_info.parsed && boot_info.tag_count > 0,
        &mut issues,
    );
    health_line(
        "memory map",
        boot_info.memory.has_memory_map && boot_info.memory.usable_bytes > 0,
        &mut issues,
    );
    health_line(
        "frame allocator",
        frames.initialized && frames.allocatable_frames > 0 && frames.free_frames > 0,
        &mut issues,
    );
    health_line(
        "paging/vmm",
        paging_state.initialized
            && paging_state.mapper_initialized
            && paging_state.identity_mapped_bytes >= paging::BOOT_IDENTITY_MAP_BYTES,
        &mut issues,
    );
    health_line(
        "page ownership",
        paging_state.tracked_mappings > 0
            && paging_state.tracked_mappings <= paging_state.tracking_capacity
            && paging_state.tracking_overflows == 0,
        &mut issues,
    );
    health_line(
        "permission audit",
        paging_audit.violations == 0
            && paging_audit.guard_pages_intact
            && paging_audit.tracking_consistent,
        &mut issues,
    );
    health_line(
        "kernel heap",
        heap_state.initialized
            && heap_state.remaining <= heap_state.size
            && heap_state.metadata_ok
            && heap_state.sentinel_ok
            && heap_state.allocation_canaries_ok,
        &mut issues,
    );
    health_line(
        "guard pages",
        !paging::translate(heap_state.guard_low).mapped
            && !paging::translate(heap_state.guard_high).mapped,
        &mut issues,
    );
    health_line(
        "gdt/tss/ist",
        gdt_state.loaded
            && gdt_state.privilege_stack_bytes >= STACK_BYTES
            && gdt_state.double_fault_stack_bytes >= STACK_BYTES
            && gdt_state.user_code_selector != 0
            && gdt_state.user_data_selector != 0,
        &mut issues,
    );
    health_line(
        "interrupt abi",
        interrupt_abi_is_healthy(interrupt_abi),
        &mut issues,
    );
    health_line(
        "user mode",
        user_state.initialized
            && user_state.code_mapped
            && user_state.stack_mapped
            && user_state.syscall_gate_ready,
        &mut issues,
    );
    health_line(
        "process model",
        process_state.initialized && process_state.running_tasks == 0 && process::selftest(),
        &mut issues,
    );
    health_line(
        "scheduler",
        scheduler_state.initialized && scheduler_state.current_task == 0 && scheduler::selftest(),
        &mut issues,
    );
    health_line(
        "syscall table",
        syscall_state.initialized && syscall::selftest(),
        &mut issues,
    );
    health_line(
        "kernel log",
        log.initialized && log.count <= log.capacity,
        &mut issues,
    );
    health_line(
        "serial/vga",
        counters.serial_bytes > 0 && counters.vga_cell_writes > 0,
        &mut issues,
    );
    health_line("timer", ticks >= counters.shell_ready_tick, &mut issues);
    health_line(
        "keyboard queue",
        keyboard::pending_events() < 256,
        &mut issues,
    );
    health_line("command table", COMMANDS.len() >= 49, &mut issues);
    health_line("last panic", !panic.present, &mut issues);

    issues
}

fn health_issue_count() -> u64 {
    let boot_info = multiboot::summary();
    let frames = physmem::snapshot();
    let paging_state = paging::snapshot();
    let paging_audit = paging::permission_audit();
    let heap_state = heap::snapshot();
    let gdt_state = gdt::snapshot();
    let log = klog::snapshot();
    let counters = stats::snapshot();
    let ticks = interrupts::ticks();
    let panic = paniclog::snapshot();
    let process_state = process::snapshot();
    let scheduler_state = scheduler::snapshot();
    let syscall_state = syscall::snapshot();
    let user_state = user::snapshot();
    let interrupt_abi = interrupts::abi_snapshot();
    let mut issues = 0u64;

    count_issue(
        boot_info.valid_magic && boot_info.parsed && boot_info.tag_count > 0,
        &mut issues,
    );
    count_issue(
        boot_info.memory.has_memory_map && boot_info.memory.usable_bytes > 0,
        &mut issues,
    );
    count_issue(
        frames.initialized && frames.allocatable_frames > 0 && frames.free_frames > 0,
        &mut issues,
    );
    count_issue(
        paging_state.initialized
            && paging_state.mapper_initialized
            && paging_state.identity_mapped_bytes >= paging::BOOT_IDENTITY_MAP_BYTES,
        &mut issues,
    );
    count_issue(
        paging_state.tracked_mappings > 0
            && paging_state.tracked_mappings <= paging_state.tracking_capacity
            && paging_state.tracking_overflows == 0,
        &mut issues,
    );
    count_issue(
        paging_audit.violations == 0
            && paging_audit.guard_pages_intact
            && paging_audit.tracking_consistent,
        &mut issues,
    );
    count_issue(
        heap_state.initialized
            && heap_state.remaining <= heap_state.size
            && heap_state.metadata_ok
            && heap_state.sentinel_ok
            && heap_state.allocation_canaries_ok,
        &mut issues,
    );
    count_issue(
        !paging::translate(heap_state.guard_low).mapped
            && !paging::translate(heap_state.guard_high).mapped,
        &mut issues,
    );
    count_issue(
        gdt_state.loaded
            && gdt_state.privilege_stack_bytes >= STACK_BYTES
            && gdt_state.double_fault_stack_bytes >= STACK_BYTES
            && gdt_state.user_code_selector != 0
            && gdt_state.user_data_selector != 0,
        &mut issues,
    );
    count_issue(interrupt_abi_is_healthy(interrupt_abi), &mut issues);
    count_issue(
        user_state.initialized
            && user_state.code_mapped
            && user_state.stack_mapped
            && user_state.syscall_gate_ready,
        &mut issues,
    );
    count_issue(
        process_state.initialized && process_state.running_tasks == 0 && process::selftest(),
        &mut issues,
    );
    count_issue(
        scheduler_state.initialized && scheduler_state.current_task == 0 && scheduler::selftest(),
        &mut issues,
    );
    count_issue(
        syscall_state.initialized && syscall::selftest(),
        &mut issues,
    );
    count_issue(log.initialized && log.count <= log.capacity, &mut issues);
    count_issue(
        counters.serial_bytes > 0 && counters.vga_cell_writes > 0,
        &mut issues,
    );
    count_issue(ticks >= counters.shell_ready_tick, &mut issues);
    count_issue(keyboard::pending_events() < 256, &mut issues);
    count_issue(COMMANDS.len() >= 49, &mut issues);
    count_issue(!panic.present, &mut issues);

    issues
}

fn interrupt_abi_is_healthy(snapshot: interrupts::AbiSnapshot) -> bool {
    snapshot.idt_entry_bytes == 16
        && snapshot.exception_context_bytes == 40
        && snapshot.timer_gate_present
        && snapshot.keyboard_gate_present
        && snapshot.syscall_gate_present
        && snapshot.syscall_gate_dpl3
        && snapshot.double_fault_ist
        && snapshot.pic_timer_vector == 32
        && snapshot.pic_keyboard_vector == 33
        && snapshot.syscall_vector == 0x80
}

fn health_line(label: &str, ok: bool, issues: &mut u64) {
    print("  ");
    if ok {
        print("[OK]   ");
    } else {
        print("[WARN] ");
        *issues = (*issues).saturating_add(1);
    }
    println(label);
}

fn count_issue(ok: bool, issues: &mut u64) {
    if !ok {
        *issues = (*issues).saturating_add(1);
    }
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
    println("  paging    : identity map inspector + tracked map/unmap");
    println("  vmm       : ownership, permission audit, guard pages, heap");
    println("  gdt/tss   : double fault IST stack");
    println("  user mode : Ring 3 probe pages + int 0x80 syscall gate");
    println("  exceptions: vector-specific panic diagnostics");
    println("  shell     : line editor, history, command table");
    println("  klog      : in-memory kernel ring buffer");
    println("  observe   : health, diag, lastpanic, buildinfo");
    println("  selftest  : non-destructive kernel diagnostics");
    println("  stability : selftest + bounded stress command");
    println("  metrics   : perf, irq, boot, bench");
}

fn command_mem(_arguments: &[u8]) {
    let boot_info = multiboot::summary();
    let memory = boot_info.memory;
    let frames = physmem::snapshot();
    let heap_snapshot = heap::snapshot();

    println("Tobacco memory info:");
    print("  kernel start      : ");
    print_hex_u64(frames.kernel_start);
    newline();
    print("  kernel end        : ");
    print_hex_u64(frames.kernel_end);
    newline();
    print("  identity map      : ");
    print_u64(paging::BOOT_IDENTITY_MAP_BYTES / 1024 / 1024);
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
    print_u64(paging::PAGE_TABLE_MEMORY_BYTES / 1024);
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
    print("  kernel heap       : ");
    print_bytes(heap_snapshot.size);
    print(" / used ");
    print_bytes(heap_snapshot.used);
    newline();
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
    print_counter("recycled", frames.recycled_frames);
    print_counter("recycle cap", frames.recycled_capacity);
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
            print("allocated test frame: ");
            print_hex_u64(frame);
            newline();
            print("recycled back: ");
            print_on_off(physmem::free_frame(frame));
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

fn command_consoletest(_arguments: &[u8]) {
    let report = run_console_model_checks();
    let mut passed = 0u64;
    let mut failed = 0u64;

    serial::log("console", "manual test started");
    println("Console model test:");
    selftest_check("long input", report.long_input, &mut passed, &mut failed);
    selftest_check(
        "long backspace",
        report.backspace_long,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "line editing",
        report.line_editing,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "history navigation",
        report.history_navigation,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "command lookup",
        report.command_lookup,
        &mut passed,
        &mut failed,
    );
    print_counter("passed", passed);
    print_counter("failed", failed);

    if failed == 0 {
        serial::log("console", "manual test passed");
        println("status: PASS");
    } else {
        serial::log("console", "manual test failed");
        stats::inc_shell_error();
        println("status: FAIL");
    }
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
    print_counter("syscalls", snapshot.syscalls);
    print_counter("user probes", snapshot.user_probes);
    print_counter("user probe pass", snapshot.user_probe_passes);
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

fn command_idt(_arguments: &[u8]) {
    let abi = interrupts::abi_snapshot();
    let counters = stats::snapshot();

    serial::log("idt", "idt status requested");
    println("IDT/Interrupt ABI:");
    print_counter("idt entry bytes", abi.idt_entry_bytes);
    print_counter("exception ctx", abi.exception_context_bytes);
    print("  timer gate       : ");
    print_on_off(abi.timer_gate_present);
    newline();
    print("  keyboard gate    : ");
    print_on_off(abi.keyboard_gate_present);
    newline();
    print("  syscall gate     : ");
    print_on_off(abi.syscall_gate_present);
    newline();
    print("  syscall dpl3     : ");
    print_on_off(abi.syscall_gate_dpl3);
    newline();
    print("  double fault ist : ");
    print_on_off(abi.double_fault_ist);
    newline();
    print_counter("timer vector", abi.pic_timer_vector);
    print_counter("keyboard vector", abi.pic_keyboard_vector);
    print_counter("syscall vector", abi.syscall_vector);
    print_counter("timer irq", counters.timer_irqs);
    print_counter("keyboard irq", counters.keyboard_irqs);
    print_counter("default irq", counters.default_irqs);
    print_counter("exceptions", counters.exceptions);
}

fn command_boot(_arguments: &[u8]) {
    let snapshot = stats::snapshot();
    let boot_info = multiboot::summary();
    let frames = physmem::snapshot();
    let gdt = gdt::snapshot();
    let paging = paging::snapshot();
    let heap_snapshot = heap::snapshot();
    let log = klog::snapshot();

    println("Boot status:");
    println("  name          : Tobacco");
    println("  version       : v0.0.5");
    println("  mode          : x86_64 long mode");
    println("  boot          : GRUB Multiboot2 ISO");
    print("  page map      : ");
    print_u64(paging.identity_mapped_bytes / 1024 / 1024);
    println(" MiB identity map");
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
    print("  gdt/tss       : ");
    print_on_off(gdt.loaded);
    newline();
    print("  user mode     : ");
    print_on_off(user::snapshot().initialized);
    newline();
    print("  paging        : ");
    print_on_off(paging.initialized);
    newline();
    print("  vm mapper     : ");
    print_on_off(paging.mapper_initialized);
    newline();
    print("  kernel heap   : ");
    print_on_off(heap_snapshot.initialized);
    newline();
    print("  kernel log    : ");
    print_on_off(log.initialized);
    newline();
    print_counter("log entries", log.count);
    print_counter("free frames", frames.free_frames);
    print_counter("shell ready tick", snapshot.shell_ready_tick);
    print_counter("current ticks", interrupts::ticks());
}

fn command_user(_arguments: &[u8]) {
    let snapshot = user::snapshot();
    let process_state = process::snapshot();
    let code = paging::translate(snapshot.code_virtual);
    let stack = paging::translate(paging::USER_PROBE_STACK_PAGE);

    println("User mode:");
    print("  initialized       : ");
    print_on_off(snapshot.initialized);
    newline();
    print("  syscall gate      : ");
    print_on_off(snapshot.syscall_gate_ready);
    newline();
    print("  code selector     : ");
    print_hex_u64(snapshot.code_selector as u64);
    newline();
    print("  data selector     : ");
    print_hex_u64(snapshot.data_selector as u64);
    newline();
    print_counter("syscall vector", snapshot.syscall_vector);
    print("  code virt         : ");
    print_hex_u64(snapshot.code_virtual);
    print(" ");
    print_on_off(code.mapped && code.user_accessible);
    newline();
    print("  stack top         : ");
    print_hex_u64(snapshot.stack_top);
    newline();
    print("  stack page        : ");
    print_hex_u64(paging::USER_PROBE_STACK_PAGE);
    print(" ");
    print_on_off(stack.mapped && stack.user_accessible);
    newline();
    print_counter("runs", snapshot.run_count);
    print_counter("passes", snapshot.pass_count);
    print_counter("syscalls", snapshot.syscall_count);
    print_counter("faults", snapshot.fault_count);
    print_counter("last fault", snapshot.last_fault_vector);
    print("  fault address     : ");
    print_hex_u64(snapshot.last_fault_address);
    newline();
    print_counter("fault exit", snapshot.last_fault_exit_code);
    print_counter("last exit", snapshot.last_exit_code);
    print_counter("last uptime", snapshot.last_uptime_return);
    print_counter("tasks spawned", process_state.spawned_tasks);
    print_counter("tasks exited", process_state.exited_total);
    print_counter("last task", process_state.last_task_id);
}

fn command_elf(_arguments: &[u8]) {
    let snapshot = elf::snapshot();
    let text = paging::translate(snapshot.entry_point);
    let data = paging::translate(paging::USER_ELF_BASE + paging::PAGE_SIZE_4K);
    let stack = paging::translate(paging::USER_ELF_STACK_PAGE);

    println("ELF64 user loader:");
    print("  initialized      : ");
    print_on_off(snapshot.initialized);
    newline();
    print("  image loaded     : ");
    print_on_off(snapshot.loaded);
    newline();
    print("  entry point      : ");
    print_hex_u64(snapshot.entry_point);
    newline();
    print("  image range      : ");
    print_hex_u64(snapshot.image_start);
    print(" - ");
    print_hex_u64(snapshot.image_end);
    newline();
    print_counter("PT_LOAD segments", snapshot.load_segments);
    print_counter("mapped pages", snapshot.mapped_pages);
    print_counter("executable pages", snapshot.executable_pages);
    print_counter("writable pages", snapshot.writable_pages);
    print("  text RX          : ");
    print_on_off(text.mapped && text.user_accessible && text.executable && !text.writable);
    newline();
    print("  data RW + NX     : ");
    print_on_off(data.mapped && data.user_accessible && data.writable && !data.executable);
    newline();
    print("  stack RW + NX    : ");
    print_on_off(stack.mapped && stack.user_accessible && stack.writable && !stack.executable);
    newline();
    print("  stack guard      : ");
    print_on_off(!paging::translate(paging::USER_ELF_STACK_GUARD).mapped);
    newline();
    print("  loader selftest  : ");
    print_on_off(elf::selftest());
    newline();

    if let Some(error) = snapshot.last_error {
        print("  last error       : ");
        println(elf::load_error_name(error));
    }
}

fn command_elftest(_arguments: &[u8]) {
    let result = process::run_elf_init_task();

    println("ELF64 /init process:");
    print("  ran              : ");
    print_on_off(result.ran);
    newline();
    print_counter("task id", result.task_id);
    print("  task state       : ");
    println(process::state_name(result.state));
    print("  entry point      : ");
    print_hex_u64(result.entry_point);
    newline();
    print("  stack top        : ");
    print_hex_u64(result.stack_top);
    newline();
    print_counter("exit code", result.exit_code);
    print_counter("expected exit", user_program::INIT_EXPECTED_EXIT_CODE);
    print_counter("syscalls before", result.syscalls_before);
    print_counter("syscalls after", result.syscalls_after);
    print("  status           : ");

    if result.passed {
        println("PASS");
    } else {
        stats::inc_shell_error();
        println("FAIL");
    }
}

fn command_usertest(_arguments: &[u8]) {
    let result = process::run_user_probe_task();

    println("User process probe:");
    print("  ran              : ");
    print_on_off(result.ran);
    newline();
    print_counter("task id", result.task_id);
    print("  task state       : ");
    println(process::state_name(result.state));
    print("  entry point      : ");
    print_hex_u64(result.entry_point);
    newline();
    print("  stack top        : ");
    print_hex_u64(result.stack_top);
    newline();
    print_counter("exit code", result.exit_code);
    print_counter("syscalls before", result.syscalls_before);
    print_counter("syscalls after", result.syscalls_after);
    print("  status           : ");

    if result.passed {
        println("PASS");
    } else {
        stats::inc_shell_error();
        println("FAIL");
    }
}

fn command_faulttest(_arguments: &[u8]) {
    let result = process::run_user_fault_task();
    let user_state = user::snapshot();

    println("User fault isolation test:");
    print("  ran              : ");
    print_on_off(result.ran);
    newline();
    print_counter("task id", result.task_id);
    print("  task state       : ");
    println(process::state_name(result.state));
    print("  entry point      : ");
    print_hex_u64(result.entry_point);
    newline();
    print("  fault address    : ");
    print_hex_u64(user_state.last_fault_address);
    newline();
    print_counter("fault vector", user_state.last_fault_vector);
    print_counter("exit code", result.exit_code);
    print_counter("fault count", user_state.fault_count);
    print("  kernel survived  : ");
    print_on_off(result.passed && scheduler::snapshot().current_task == 0);
    newline();
    print("  status           : ");

    if result.passed {
        println("PASS");
    } else {
        stats::inc_shell_error();
        println("FAIL");
    }
}

fn command_process(_arguments: &[u8]) {
    let snapshot = process::snapshot();
    let scheduler_snapshot = scheduler::snapshot();

    println("Process model:");
    print("  initialized      : ");
    print_on_off(snapshot.initialized);
    newline();
    print_counter("capacity", snapshot.task_capacity);
    print_counter("slots used", snapshot.task_slots_used);
    print_counter("ready", snapshot.ready_tasks);
    print_counter("running", snapshot.running_tasks);
    print_counter("exited", snapshot.exited_tasks);
    print_counter("next task id", snapshot.next_task_id);
    print_counter("spawned total", snapshot.spawned_tasks);
    print_counter("exited total", snapshot.exited_total);
    print_counter("failed spawns", snapshot.failed_spawns);
    print_counter("last task", snapshot.last_task_id);
    print_counter("last exit", snapshot.last_exit_code);
    print_counter("ctx switches", scheduler_snapshot.context_switches);
    print_counter("yield calls", scheduler_snapshot.cooperative_yields);
    print_counter("task ticks", scheduler_snapshot.accounted_ticks);

    print_task_rows();
}

fn command_tasks(_arguments: &[u8]) {
    let snapshot = process::snapshot();
    let scheduler_snapshot = scheduler::snapshot();

    serial::log("tasks", "task listing requested");
    println("Tasks:");
    print_counter("capacity", snapshot.task_capacity);
    print_counter("used", snapshot.task_slots_used);
    print_counter("ready", snapshot.ready_tasks);
    print_counter("running", snapshot.running_tasks);
    print_counter("exited", snapshot.exited_tasks);
    print_counter("current", scheduler_snapshot.current_task);
    print_counter("last", scheduler_snapshot.last_task);
    print_task_rows();
}

fn print_task_rows() {
    println("Task table:");
    for index in 0..process::MAX_TASKS {
        if let Some(task) = process::task(index) {
            print("  #");
            print_u64(task.id);
            print(" ");
            print(process::state_name(task.state));
            print(" entry=");
            print_hex_u64(task.entry_point);
            print(" stack=");
            print_hex_u64(task.stack_top);
            print(" runs=");
            print_u64(task.runs);
            print(" exit=");
            print_u64(task.exit_code);
            print(" syscalls=");
            print_u64(task.syscalls_after.saturating_sub(task.syscalls_before));
            newline();
        }
    }
}

fn command_scheduler(_arguments: &[u8]) {
    let snapshot = scheduler::snapshot();

    println("Scheduler:");
    print("  initialized      : ");
    print_on_off(snapshot.initialized);
    newline();
    print_counter("queue capacity", snapshot.queue_capacity);
    print_counter("queued tasks", snapshot.queued_tasks);
    print_counter("current task", snapshot.current_task);
    print_counter("last task", snapshot.last_task);
    print_counter("ctx switches", snapshot.context_switches);
    print_counter("yields", snapshot.cooperative_yields);
    print_counter("timer ticks", snapshot.timer_ticks);
    print_counter("task ticks", snapshot.accounted_ticks);
    print_counter("failed enqueue", snapshot.failed_enqueues);
    print_counter("last switch", snapshot.last_switch_tick);
}

fn command_syscall(_arguments: &[u8]) {
    let snapshot = syscall::snapshot();

    println("Syscall ABI:");
    println("  gate            : int 0x80");
    println("  number          : rax");
    println("  arg0            : rdi");
    println("  return          : rax");
    print("  table           : ");
    print_on_off(snapshot.initialized && syscall::selftest());
    newline();
    print_counter("entries", snapshot.entries);
    print_counter("dispatches", snapshot.dispatches);
    print_counter("unknown", snapshot.unknown_syscalls);
    print_counter("last number", snapshot.last_number);
    print_counter("last return", snapshot.last_return);

    println("Syscall table:");
    for index in 0..syscall::table_len() {
        if let Some(entry) = syscall::table_entry(index) {
            print("  ");
            print_u64(entry.number);
            print(" ");
            print(entry.name);
            print(" args=");
            print_u64(entry.arg_count as u64);
            print(" ret=");
            print(syscall::return_code_name(entry.return_code));
            print(" log=");
            print_on_off(entry.logging);
            print(" handler=on");
            newline();
        }
    }
}

fn command_gdt(_arguments: &[u8]) {
    let snapshot = gdt::snapshot();

    println("GDT/TSS/IST:");
    print("  loaded          : ");
    print_on_off(snapshot.loaded);
    newline();
    print("  code selector   : ");
    print_hex_u64(snapshot.code_selector as u64);
    newline();
    print("  data selector   : ");
    print_hex_u64(snapshot.data_selector as u64);
    newline();
    print("  tss selector    : ");
    print_hex_u64(snapshot.tss_selector as u64);
    newline();
    print("  user code sel   : ");
    print_hex_u64(snapshot.user_code_selector as u64);
    newline();
    print("  user data sel   : ");
    print_hex_u64(snapshot.user_data_selector as u64);
    newline();
    print("  gdt base        : ");
    print_hex_u64(snapshot.gdt_base);
    newline();
    print_counter("gdt limit", snapshot.gdt_limit as u64);
    print("  tss base        : ");
    print_hex_u64(snapshot.tss_base);
    newline();
    print_counter("tss limit", snapshot.tss_limit as u64);
    print("  ring0 stack top : ");
    print_hex_u64(snapshot.privilege_stack_top);
    newline();
    print("  ring0 stack     : ");
    print_bytes(snapshot.privilege_stack_bytes);
    newline();
    print_counter("df ist index", snapshot.double_fault_ist_index as u64);
    print("  df stack top    : ");
    print_hex_u64(snapshot.double_fault_stack_top);
    newline();
    print("  df stack bytes  : ");
    print_bytes(snapshot.double_fault_stack_bytes);
    newline();
}

fn command_paging(_arguments: &[u8]) {
    let snapshot = paging::snapshot();
    let audit = paging::permission_audit();

    println("Paging:");
    print("  initialized       : ");
    print_on_off(snapshot.initialized);
    newline();
    print("  cr3               : ");
    print_hex_u64(snapshot.cr3);
    newline();
    print("  p4 table          : ");
    print_hex_u64(snapshot.p4_addr);
    newline();
    print("  p3 table          : ");
    print_hex_u64(snapshot.p3_addr);
    newline();
    print("  p2 table          : ");
    print_hex_u64(snapshot.p2_addr);
    newline();
    print_counter("p4 present", snapshot.p4_present_entries);
    print_counter("p3 present", snapshot.p3_present_entries);
    print_counter("p2 present", snapshot.p2_present_entries);
    print_counter("huge pages", snapshot.huge_pages);
    print("  identity mapped   : ");
    print_bytes(snapshot.identity_mapped_bytes);
    newline();
    print("  page table memory : ");
    print_bytes(snapshot.page_table_bytes);
    newline();
    print("  vm mapper         : ");
    print_on_off(snapshot.mapper_initialized);
    newline();
    print_counter("mapped pages", snapshot.mapped_pages);
    print_counter("unmapped pages", snapshot.unmapped_pages);
    print_counter("page table frames", snapshot.page_table_frames);
    print_counter("guard pages", snapshot.guard_pages);
    print_counter("tracked pages", snapshot.tracked_mappings);
    print_counter("tracking capacity", snapshot.tracking_capacity);
    print_counter("tracking overflows", snapshot.tracking_overflows);
    print_counter("tracking misses", snapshot.tracking_misses);
    print_counter("kernel owned", snapshot.kernel_owned_pages);
    print_counter("heap owned", snapshot.heap_owned_pages);
    print_counter("user owned", snapshot.user_owned_pages);
    print_counter("test owned", snapshot.test_owned_pages);
    print("  permission audit  : ");
    print_on_off(audit.violations == 0 && audit.tracking_consistent && audit.guard_pages_intact);
    newline();
    print_counter("audit checked", audit.checked_pages);
    print_counter("audit violations", audit.violations);
    print("  managed start     : ");
    print_hex_u64(paging::KERNEL_VIRTUAL_BASE);
    newline();
    print("  managed end       : ");
    print_hex_u64(paging::KERNEL_VIRTUAL_END);
    newline();
}

fn command_heap(_arguments: &[u8]) {
    let snapshot = heap::snapshot();

    println("Kernel heap:");
    print("  initialized       : ");
    print_on_off(snapshot.initialized);
    newline();
    print("  base              : ");
    print_hex_u64(snapshot.base);
    newline();
    print("  size              : ");
    print_bytes(snapshot.size);
    newline();
    print("  used              : ");
    print_bytes(snapshot.used);
    newline();
    print("  remaining         : ");
    print_bytes(snapshot.remaining);
    newline();
    print_counter("mapped pages", snapshot.mapped_pages);
    print_counter("allocations", snapshot.allocations);
    print_counter("frees", snapshot.frees);
    print_counter("failed alloc", snapshot.failed_allocations);
    print_counter("failed frees", snapshot.failed_frees);
    print_counter("double frees", snapshot.double_frees);
    print_counter("invalid frees", snapshot.invalid_frees);
    print_counter("coalesces", snapshot.coalesces);
    print_counter("active alloc", snapshot.active_allocations);
    print_counter("free blocks", snapshot.free_blocks);
    print_counter("metadata blocks", snapshot.metadata_blocks);
    print_counter("metadata cap", snapshot.metadata_capacity);
    print_counter("allocated bytes", snapshot.allocated_bytes);
    print_counter("free bytes", snapshot.free_bytes);
    print_counter("largest free", snapshot.largest_free_block);
    print_counter("high watermark", snapshot.high_watermark);
    print_counter("corruption checks", snapshot.corruption_checks);
    print_counter("corruption fail", snapshot.corruption_failures);
    print_counter("corruption detect", snapshot.corruption_detections);
    print("  metadata          : ");
    print_on_off(snapshot.metadata_ok);
    newline();
    print("  sentinel          : ");
    print_on_off(snapshot.sentinel_ok);
    newline();
    print("  alloc canaries    : ");
    print_on_off(snapshot.allocation_canaries_ok);
    newline();
    print("  guard low         : ");
    print_hex_u64(snapshot.guard_low);
    print(" ");
    print_on_off(!paging::translate(snapshot.guard_low).mapped);
    newline();
    print("  guard high        : ");
    print_hex_u64(snapshot.guard_high);
    print(" ");
    print_on_off(!paging::translate(snapshot.guard_high).mapped);
    newline();
}

fn command_heaptest(_arguments: &[u8]) {
    println("Kernel heap selftest:");

    if heap::selftest() {
        serial::log("heap", "heaptest passed");
        println("  status            : PASS");
    } else {
        serial::log("heap", "heaptest failed");
        stats::inc_shell_error();
        println("  status            : FAIL");
    }
}

fn command_heapcheck(_arguments: &[u8]) {
    let corruption_ok = heap::corruption_check();
    let snapshot = heap::snapshot();
    let low_guard_ok = !paging::translate(snapshot.guard_low).mapped;
    let high_guard_ok = !paging::translate(snapshot.guard_high).mapped;
    let accounting_ok = snapshot.allocated_bytes.saturating_add(snapshot.free_bytes)
        <= snapshot.size
        && snapshot.used <= snapshot.size
        && snapshot.remaining <= snapshot.size;
    let mut passed = 0u64;
    let mut failed = 0u64;

    serial::log("heap", "heapcheck requested");
    println("Heap integrity:");
    selftest_check(
        "initialized",
        snapshot.initialized,
        &mut passed,
        &mut failed,
    );
    selftest_check("metadata", snapshot.metadata_ok, &mut passed, &mut failed);
    selftest_check("sentinel", snapshot.sentinel_ok, &mut passed, &mut failed);
    selftest_check(
        "allocation canaries",
        snapshot.allocation_canaries_ok,
        &mut passed,
        &mut failed,
    );
    selftest_check("guard low", low_guard_ok, &mut passed, &mut failed);
    selftest_check("guard high", high_guard_ok, &mut passed, &mut failed);
    selftest_check("accounting", accounting_ok, &mut passed, &mut failed);
    selftest_check("corruption scan", corruption_ok, &mut passed, &mut failed);
    print_counter("allocations", snapshot.allocations);
    print_counter("frees", snapshot.frees);
    print_counter("active", snapshot.active_allocations);
    print_counter("free blocks", snapshot.free_blocks);
    print_counter("largest free", snapshot.largest_free_block);
    print_counter("corruption checks", snapshot.corruption_checks);
    print_counter("passed", passed);
    print_counter("failed", failed);

    if failed == 0 {
        serial::log("heap", "heapcheck passed");
        println("status: PASS");
    } else {
        serial::log("heap", "heapcheck failed");
        stats::inc_shell_error();
        println("status: FAIL");
    }
}

fn command_virt(arguments: &[u8]) {
    let arguments = trim_ascii(arguments);

    if arguments.is_empty() {
        println("Usage: virt <address>");
        println("Example: virt 0xb8000");
        return;
    }

    match parse_u64(arguments) {
        Some(address) => {
            let result = paging::translate(address);
            serial::log_hex_u64("paging", "translate virt", result.virt);

            if result.mapped {
                serial::log_hex_u64("paging", "translate phys", result.phys);
                print("virt ");
                print_hex_u64(result.virt);
                print(" -> phys ");
                print_hex_u64(result.phys);
                print(" (");
                print_bytes(result.page_size);
                if result.huge_page {
                    print(" huge page");
                }
                println(")");
            } else {
                print("virt ");
                print_hex_u64(result.virt);
                println(" belum terpetakan di boot identity map.");
            }
        }
        None => {
            stats::inc_shell_error();
            println("Alamat tidak valid. Gunakan decimal atau hex, contoh: virt 0xb8000");
        }
    }
}

fn command_vmtest(_arguments: &[u8]) {
    println("Virtual memory map/unmap test:");

    if paging::translate(paging::KERNEL_VM_TEST_PAGE).mapped {
        println("  test page sudah terpakai.");
        stats::inc_shell_error();
        return;
    }

    if paging::probe_map_unmap(paging::KERNEL_VM_TEST_PAGE) {
        serial::log("paging", "vmtest passed");
        print("  page              : ");
        print_hex_u64(paging::KERNEL_VM_TEST_PAGE);
        newline();
        println("  status            : PASS");
    } else {
        serial::log("paging", "vmtest failed");
        stats::inc_shell_error();
        println("  status            : FAIL");
    }
}

fn command_log(arguments: &[u8]) {
    let snapshot = klog::snapshot();
    let limit = log_limit(arguments, snapshot.count as usize);
    let start = (snapshot.count as usize).saturating_sub(limit);

    println("Kernel log ring:");
    print_counter("capacity", snapshot.capacity);
    print_counter("entries", snapshot.count);
    print_counter("dropped", snapshot.dropped);
    print_counter("next sequence", snapshot.next_sequence);

    if snapshot.count == 0 {
        println("  belum ada entry log.");
        return;
    }

    for index in start..(snapshot.count as usize) {
        if let Some(entry) = klog::entry(index) {
            print("  #");
            print_u64(entry.sequence);
            print(" t+");
            print_u64(entry.tick);
            print(" [");
            print_log_bytes(entry.tag());
            print("] ");
            print_log_bytes(entry.message());
            newline();
        }
    }
}

fn command_selftest(_arguments: &[u8]) {
    let boot_info = multiboot::summary();
    let memory = boot_info.memory;
    let frames = physmem::snapshot();
    let paging_state = paging::snapshot();
    let paging_audit = paging::permission_audit();
    let vga_translation = paging::translate(VGA_BUFFER_ADDRESS);
    let high_translation = paging::translate(0xffff_8000_0000_0000);
    let gdt = gdt::snapshot();
    let heap_snapshot = heap::snapshot();
    let log = klog::snapshot();
    let counters = stats::snapshot();
    let ticks = interrupts::ticks();
    let process_state = process::snapshot();
    let scheduler_state = scheduler::snapshot();
    let syscall_state = syscall::snapshot();
    let user_state = user::snapshot();
    let elf_state = elf::snapshot();
    let interrupt_abi = interrupts::abi_snapshot();

    let mut passed = 0u64;
    let mut failed = 0u64;

    serial::log("selftest", "started");
    println("Tobacco selftest:");

    selftest_check(
        "multiboot magic",
        boot_info.valid_magic,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "multiboot parsed",
        boot_info.parsed && boot_info.tag_count > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "memory map usable",
        memory.has_memory_map && multiboot::stored_region_count() > 0 && memory.usable_bytes > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "frame allocator ready",
        frames.initialized && frames.allocatable_frames > 0 && frames.free_frames > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "kernel memory protected",
        frames.kernel_end > frames.kernel_start && frames.protected_until >= frames.kernel_end,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "paging initialized",
        paging_state.initialized && paging_state.mapper_initialized && paging_state.cr3 != 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "boot identity map",
        paging_state.p4_present_entries >= 1
            && paging_state.p3_present_entries >= 1
            && paging_state.p2_present_entries >= 512
            && paging_state.huge_pages >= 512
            && paging_state.identity_mapped_bytes >= paging::BOOT_IDENTITY_MAP_BYTES,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "vga address translation",
        vga_translation.mapped
            && vga_translation.phys == VGA_BUFFER_ADDRESS
            && vga_translation.huge_page,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "high address unmapped",
        !high_translation.mapped,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "virtual map unmap",
        paging::probe_map_unmap(paging::KERNEL_VM_TEST_PAGE),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "page ownership tracking",
        paging_state.tracked_mappings > 0
            && paging_state.tracked_mappings <= paging_state.tracking_capacity
            && paging_state.tracking_overflows == 0
            && paging_state.heap_owned_pages == heap::HEAP_PAGES
            && paging_state.user_owned_pages >= 2,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "user kernel permission audit",
        paging_audit.violations == 0
            && paging_audit.guard_pages_intact
            && paging_audit.tracking_consistent
            && paging_audit.user_pages >= 2
            && paging_audit.heap_pages == heap::HEAP_PAGES,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "guard page policy",
        paging::guard_page_test(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "heap initialized",
        heap_snapshot.initialized
            && heap_snapshot.mapped_pages == heap::HEAP_PAGES
            && heap_snapshot.remaining <= heap_snapshot.size
            && heap_snapshot.metadata_ok
            && heap_snapshot.sentinel_ok
            && heap_snapshot.allocation_canaries_ok
            && !paging::translate(heap_snapshot.guard_low).mapped
            && !paging::translate(heap_snapshot.guard_high).mapped,
        &mut passed,
        &mut failed,
    );
    selftest_check("heap probe", heap::probe(), &mut passed, &mut failed);
    selftest_check(
        "heap allocator free coalesce",
        heap::selftest(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "allocator corruption guard",
        heap::corruption_check(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "gdt tss ist ready",
        gdt.loaded
            && gdt.code_selector != 0
            && gdt.data_selector != 0
            && gdt.tss_selector != 0
            && gdt.user_code_selector != 0
            && gdt.user_data_selector != 0
            && gdt.privilege_stack_bytes >= STACK_BYTES
            && gdt.double_fault_stack_bytes >= STACK_BYTES,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "interrupt abi hardened",
        interrupt_abi_is_healthy(interrupt_abi),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "user mode foundation",
        user_state.initialized
            && user_state.code_mapped
            && user_state.stack_mapped
            && user_state.syscall_gate_ready,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "ELF64 user loader",
        elf_state.initialized
            && elf_state.loaded
            && elf_state.load_segments >= 1
            && elf_state.executable_pages >= 1
            && elf_state.writable_pages >= 1
            && elf::selftest(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "process table ready",
        process_state.initialized && process_state.task_capacity == process::MAX_TASKS as u64,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "process model selftest",
        process::selftest(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "scheduler ready",
        scheduler_state.initialized
            && scheduler_state.queue_capacity == scheduler::QUEUE_CAPACITY as u64
            && scheduler_state.queued_tasks <= scheduler_state.queue_capacity,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "scheduler selftest",
        scheduler::selftest(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "syscall table ready",
        syscall_state.initialized && syscall_state.entries == syscall::table_len() as u64,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "syscall table selftest",
        syscall::selftest(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "kernel log ring ready",
        log.initialized && log.capacity == klog::ENTRY_COUNT as u64 && log.count > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "serial output active",
        counters.serial_bytes > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "vga output active",
        counters.vga_clears > 0 && counters.vga_cell_writes > 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "interrupt counters sane",
        ticks >= counters.shell_ready_tick,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "keyboard queue sane",
        keyboard::pending_events() < 256,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "command table sane",
        COMMANDS.len() >= 49,
        &mut passed,
        &mut failed,
    );

    println("Selftest summary:");
    print_counter("passed", passed);
    print_counter("failed", failed);

    if failed == 0 {
        serial::log("selftest", "passed");
        println("status: PASS");
    } else {
        serial::log("selftest", "failed");
        stats::inc_shell_error();
        println("status: FAIL");
    }
}

fn command_stress(arguments: &[u8]) {
    let cycles = match stress_cycles(arguments) {
        Some(cycles) => cycles,
        None => {
            stats::inc_shell_error();
            println("Usage: stress [1..32]");
            return;
        }
    };

    let ticks_before = interrupts::ticks();
    let log_before = klog::snapshot();
    let serial_before = stats::snapshot().serial_bytes;
    let paging_before = paging::snapshot();
    let mut checksum = 0x544f_4241_4343_4f53u64;
    let mut paging_failures = 0u64;

    serial::log_u64("stress", "cycles", cycles as u64);
    println("Tobacco stability stress:");
    print_counter("cycles", cycles as u64);

    for cycle in 0..cycles {
        checksum = stress_cpu_round(checksum, cycle as u64);

        if !stress_paging_round(cycle as u64, &mut checksum) {
            paging_failures = paging_failures.saturating_add(1);
        }

        klog::append_u64("stress", "cycle", cycle as u64);
    }

    stress_log_ring_wrap();

    let ticks_after = interrupts::ticks();
    let log_after = klog::snapshot();
    let serial_after = stats::snapshot().serial_bytes;
    let paging_after = paging::snapshot();
    let paging_audit = paging::permission_audit();
    let mut passed = 0u64;
    let mut failed = 0u64;

    selftest_check(
        "paging repeated translation",
        paging_failures == 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "memory tracking stable",
        paging_after.tracking_overflows == paging_before.tracking_overflows
            && paging_after.tracking_misses == paging_before.tracking_misses
            && paging_audit.violations == 0,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "allocator corruption guard",
        heap::corruption_check(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "heap allocator reuse",
        heap::stress(),
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "kernel log bounded",
        log_after.initialized
            && log_after.capacity == klog::ENTRY_COUNT as u64
            && log_after.count <= log_after.capacity,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "kernel log wrap monotonic",
        log_after.next_sequence > log_before.next_sequence
            && log_after.dropped >= log_before.dropped,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "timer monotonic",
        ticks_after >= ticks_before,
        &mut passed,
        &mut failed,
    );
    selftest_check(
        "serial counter monotonic",
        serial_after >= serial_before,
        &mut passed,
        &mut failed,
    );
    selftest_check("checksum stable", checksum != 0, &mut passed, &mut failed);

    println("Stress summary:");
    print_counter("passed", passed);
    print_counter("failed", failed);
    print("  checksum         : ");
    print_hex_u64(checksum);
    newline();
    print_counter("log entries", log_after.count);
    print_counter("log dropped", log_after.dropped);

    if failed == 0 {
        serial::log("stress", "passed");
        println("status: PASS");
    } else {
        serial::log("stress", "failed");
        stats::inc_shell_error();
        println("status: FAIL");
    }
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

fn selftest_check(label: &str, ok: bool, passed: &mut u64, failed: &mut u64) {
    print("  ");
    if ok {
        print("[PASS] ");
        *passed = (*passed).saturating_add(1);
    } else {
        print("[FAIL] ");
        *failed = (*failed).saturating_add(1);
    }
    println(label);
}

fn stress_cpu_round(mut checksum: u64, cycle: u64) -> u64 {
    for value in 0..2048u64 {
        checksum = checksum
            .rotate_left(9)
            .wrapping_add(value ^ cycle.rotate_left(3))
            .wrapping_mul(0x1000_0000_01b3);
        checksum = core::hint::black_box(checksum);
    }

    checksum
}

fn stress_paging_round(cycle: u64, checksum: &mut u64) -> bool {
    let rolling_address = (cycle % 512) * paging::HUGE_PAGE_SIZE;
    let mapped_addresses = [
        0,
        0x1000,
        VGA_BUFFER_ADDRESS,
        rolling_address,
        paging::BOOT_IDENTITY_MAP_BYTES - 1,
    ];
    let unmapped_addresses = [paging::USER_SPACE_END, 0xffff_8000_0000_0000];

    for address in mapped_addresses.iter().copied() {
        let result = paging::translate(address);

        if !result.mapped || result.phys != address {
            return false;
        }

        *checksum ^= result.phys.rotate_left(((address >> 12) as u32 & 31) + 1);
    }

    for address in unmapped_addresses.iter().copied() {
        if paging::translate(address).mapped {
            return false;
        }
    }

    true
}

fn stress_log_ring_wrap() {
    for index in 0..(klog::ENTRY_COUNT + 8) {
        klog::append_u64("stress", "ring", index as u64);
    }
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

fn print_log_bytes(bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        vga::write_byte(byte);
        serial::write_byte(byte);
    }
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

fn print_duration(seconds: u64) {
    let days = seconds / 86_400;
    let hours = (seconds / 3_600) % 24;
    let minutes = (seconds / 60) % 60;
    let secs = seconds % 60;

    print_u64(days);
    print("d ");
    print_two_digits(hours);
    print(":");
    print_two_digits(minutes);
    print(":");
    print_two_digits(secs);
}

fn print_two_digits(value: u64) {
    let value = value % 100;
    if value < 10 {
        print("0");
    }
    print_u64(value);
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

fn parse_u64(input: &[u8]) -> Option<u64> {
    let input = trim_ascii(input);

    if input.is_empty() {
        return None;
    }

    let (digits, base) = if input.len() > 2 && input[0] == b'0' && to_ascii_lower(input[1]) == b'x'
    {
        (&input[2..], 16u64)
    } else {
        (input, 10u64)
    };

    if digits.is_empty() {
        return None;
    }

    let mut value = 0u64;
    let mut parsed_digit = false;

    for byte in digits.iter().copied() {
        if byte == b'_' {
            continue;
        }

        let digit = ascii_digit_value(byte)?;
        if digit >= base {
            return None;
        }

        value = value.checked_mul(base)?.checked_add(digit)?;
        parsed_digit = true;
    }

    if parsed_digit {
        Some(value)
    } else {
        None
    }
}

fn log_limit(arguments: &[u8], total: usize) -> usize {
    let arguments = trim_ascii(arguments);

    if arguments.is_empty() {
        return total.min(16);
    }

    if eq_ignore_ascii_case(arguments, b"all") {
        return total;
    }

    match parse_u64(arguments) {
        Some(value) => (value as usize).min(total),
        None => {
            stats::inc_shell_error();
            16usize.min(total)
        }
    }
}

fn stress_cycles(arguments: &[u8]) -> Option<usize> {
    let arguments = trim_ascii(arguments);

    if arguments.is_empty() {
        return Some(STRESS_DEFAULT_CYCLES);
    }

    let value = parse_u64(arguments)?;
    if value == 0 {
        return None;
    }

    if value > STRESS_MAX_CYCLES as u64 {
        println("cycles dibatasi ke 32 agar tetap aman.");
        Some(STRESS_MAX_CYCLES)
    } else {
        Some(value as usize)
    }
}

fn ascii_digit_value(byte: u8) -> Option<u64> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u64),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u64),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u64),
        _ => None,
    }
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
