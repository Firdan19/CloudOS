use crate::{
    gdt, heap, interrupts, keyboard, klog, multiboot, paging, physmem, process, serial, shell,
    stats, user, vga,
};
use x86_64::instructions::hlt;

const CI_BOOT_FLAG: &[u8] = b"tobacco.ci=smoke";
const CI_PAGE_FAULT_FLAG: &[u8] = b"tobacco.ci=pagefault";
const CI_DOUBLE_FAULT_FLAG: &[u8] = b"tobacco.ci=doublefault";
const VGA_BUFFER_ADDRESS: u64 = 0x000b_8000;

unsafe extern "C" {
    fn ci_trigger_double_fault_stub() -> !;
}

pub fn run_if_requested() {
    let boot_info = multiboot::summary();
    let command_line = boot_info.command_line.as_str().as_bytes();

    if contains_bytes(command_line, CI_PAGE_FAULT_FLAG) {
        trigger_page_fault_for_ci();
    }

    if contains_bytes(command_line, CI_DOUBLE_FAULT_FLAG) {
        trigger_double_fault_for_ci();
    }

    if !contains_bytes(command_line, CI_BOOT_FLAG) {
        return;
    }

    serial::log("ci", "command smoke requested");
    run_command_table_checks();

    if run_selftest_checks() {
        serial::log("ci", "selftest status: PASS");
    } else {
        serial::log("ci", "selftest status: FAIL");
    }

    if run_stability_stress() {
        serial::log("ci", "stress status: PASS");
    } else {
        serial::log("ci", "stress status: FAIL");
    }

    if run_console_stress_checks() {
        serial::log("ci", "console status: PASS");
    } else {
        serial::log("ci", "console status: FAIL");
    }

    if run_user_mode_checks() {
        serial::log("ci", "user mode status: PASS");
    } else {
        serial::log("ci", "user mode status: FAIL");
    }

    serial::log("ci", "command smoke complete");
}

fn trigger_page_fault_for_ci() -> ! {
    let target = paging::KERNEL_HEAP_GUARD_LOW;
    serial::log("ci-fault", "page fault trigger requested");
    serial::log_hex_u64("ci-fault", "target address", target);

    unsafe {
        let _ = core::ptr::read_volatile(target as *const u8);
    }

    serial::log("ci-fault", "page fault trigger unexpectedly returned");
    halt_forever()
}

fn trigger_double_fault_for_ci() -> ! {
    serial::log("ci-fault", "double fault trigger requested");
    serial::log("ci-fault", "forcing interrupt delivery on invalid stack");

    unsafe {
        ci_trigger_double_fault_stub();
    }
}

fn halt_forever() -> ! {
    loop {
        hlt();
    }
}

fn run_command_table_checks() {
    serial::log_u64("ci", "command table count", shell::command_count() as u64);
    check("command help", shell::command_exists(b"help"));
    check("command health", shell::command_exists(b"health"));
    check("command status", shell::command_exists(b"status"));
    check("command diag", shell::command_exists(b"diag"));
    check("command lastpanic", shell::command_exists(b"lastpanic"));
    check("command buildinfo", shell::command_exists(b"buildinfo"));
    check("command uptime", shell::command_exists(b"uptime"));
    check("command selftest", shell::command_exists(b"selftest"));
    check("command stress", shell::command_exists(b"stress"));
    check("command paging", shell::command_exists(b"paging"));
    check("command heap", shell::command_exists(b"heap"));
    check("command heaptest", shell::command_exists(b"heaptest"));
    check("command vmtest", shell::command_exists(b"vmtest"));
    check("command user", shell::command_exists(b"user"));
    check("command process", shell::command_exists(b"process"));
    check("command usertest", shell::command_exists(b"usertest"));
    check("command tasktest", shell::command_exists(b"tasktest"));
    check("command syscall", shell::command_exists(b"syscall"));
    check("command consoletest", shell::command_exists(b"consoletest"));
    check("command mem", shell::command_exists(b"mem"));
    check("command log", shell::command_exists(b"log"));
}

fn run_selftest_checks() -> bool {
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
    let user_state = user::snapshot();
    let interrupt_abi = interrupts::abi_snapshot();

    let mut ok = true;

    ok &= check("selftest multiboot magic", boot_info.valid_magic);
    ok &= check(
        "selftest multiboot parsed",
        boot_info.parsed && boot_info.tag_count > 0,
    );
    ok &= check(
        "selftest memory map usable",
        memory.has_memory_map && multiboot::stored_region_count() > 0 && memory.usable_bytes > 0,
    );
    ok &= check(
        "selftest frame allocator ready",
        frames.initialized && frames.allocatable_frames > 0 && frames.free_frames > 0,
    );
    ok &= check(
        "selftest kernel memory protected",
        frames.kernel_end > frames.kernel_start && frames.protected_until >= frames.kernel_end,
    );
    ok &= check(
        "selftest paging initialized",
        paging_state.initialized && paging_state.mapper_initialized && paging_state.cr3 != 0,
    );
    ok &= check(
        "selftest boot identity map",
        paging_state.p4_present_entries >= 1
            && paging_state.p3_present_entries >= 1
            && paging_state.p2_present_entries >= 512
            && paging_state.huge_pages >= 512
            && paging_state.identity_mapped_bytes >= paging::BOOT_IDENTITY_MAP_BYTES,
    );
    ok &= check(
        "selftest vga translation",
        vga_translation.mapped
            && vga_translation.phys == VGA_BUFFER_ADDRESS
            && vga_translation.huge_page,
    );
    ok &= check("selftest high address unmapped", !high_translation.mapped);
    ok &= check(
        "selftest virtual map unmap",
        paging::probe_map_unmap(paging::KERNEL_VM_TEST_PAGE),
    );
    ok &= check(
        "selftest page ownership tracking",
        paging_state.tracked_mappings > 0
            && paging_state.tracked_mappings <= paging_state.tracking_capacity
            && paging_state.tracking_overflows == 0
            && paging_state.heap_owned_pages == heap::HEAP_PAGES
            && paging_state.user_owned_pages >= 2,
    );
    ok &= check(
        "selftest user kernel permission audit",
        paging_audit.violations == 0
            && paging_audit.guard_pages_intact
            && paging_audit.tracking_consistent
            && paging_audit.user_pages >= 2
            && paging_audit.heap_pages == heap::HEAP_PAGES,
    );
    ok &= check("selftest guard page policy", paging::guard_page_test());
    ok &= check(
        "selftest heap ready",
        heap_snapshot.initialized
            && heap_snapshot.mapped_pages == heap::HEAP_PAGES
            && heap_snapshot.remaining <= heap_snapshot.size
            && heap_snapshot.metadata_ok
            && heap_snapshot.sentinel_ok
            && heap_snapshot.allocation_canaries_ok
            && !paging::translate(heap_snapshot.guard_low).mapped
            && !paging::translate(heap_snapshot.guard_high).mapped,
    );
    ok &= check("selftest heap probe", heap::probe());
    ok &= check("selftest heap allocator free coalesce", heap::selftest());
    ok &= check(
        "selftest allocator corruption guard",
        heap::corruption_check(),
    );
    ok &= check(
        "selftest gdt tss ist ready",
        gdt.loaded
            && gdt.code_selector != 0
            && gdt.data_selector != 0
            && gdt.tss_selector != 0
            && gdt.user_code_selector != 0
            && gdt.user_data_selector != 0
            && gdt.privilege_stack_bytes >= 16 * 1024
            && gdt.double_fault_stack_bytes >= 16 * 1024,
    );
    ok &= check(
        "selftest interrupt abi hardened",
        interrupt_abi.idt_entry_bytes == 16
            && interrupt_abi.exception_context_bytes == 40
            && interrupt_abi.timer_gate_present
            && interrupt_abi.keyboard_gate_present
            && interrupt_abi.syscall_gate_present
            && interrupt_abi.syscall_gate_dpl3
            && interrupt_abi.double_fault_ist
            && interrupt_abi.pic_timer_vector == 32
            && interrupt_abi.pic_keyboard_vector == 33
            && interrupt_abi.syscall_vector == 0x80,
    );
    ok &= check(
        "selftest user mode foundation",
        user_state.initialized
            && user_state.code_mapped
            && user_state.stack_mapped
            && user_state.syscall_gate_ready,
    );
    ok &= check(
        "selftest process table ready",
        process_state.initialized && process_state.task_capacity == process::MAX_TASKS as u64,
    );
    ok &= check("selftest process model", process::selftest());
    ok &= check(
        "selftest kernel log ready",
        log.initialized && log.capacity == klog::ENTRY_COUNT as u64 && log.count > 0,
    );
    ok &= check("selftest serial active", counters.serial_bytes > 0);
    ok &= check(
        "selftest vga active",
        counters.vga_clears > 0 && counters.vga_cell_writes > 0,
    );
    ok &= check("selftest timer sane", ticks >= counters.shell_ready_tick);
    ok &= check(
        "selftest keyboard queue sane",
        keyboard::pending_events() < 256,
    );
    ok &= check("selftest command table sane", shell::command_count() >= 38);

    ok
}

fn run_user_mode_checks() -> bool {
    let before = user::snapshot();
    let process_before = process::snapshot();
    let result = process::run_user_probe_task();
    let after = user::snapshot();
    let process_after = process::snapshot();
    let mut ok = true;

    ok &= check("user mode initialized", before.initialized);
    ok &= check("user syscall gate ready", before.syscall_gate_ready);
    ok &= check("user process spawned", result.task_id > 0);
    ok &= check(
        "user process exited",
        result.state == process::TaskState::Exited,
    );
    ok &= check("user probe ran", result.ran);
    ok &= check(
        "user probe exit code",
        result.exit_code == user::probe_expected_exit_code(),
    );
    ok &= check(
        "user probe syscalls",
        result.syscalls_after >= result.syscalls_before + 3,
    );
    ok &= check("user probe passed", result.passed);
    ok &= check("user pass counter", after.pass_count > before.pass_count);
    ok &= check(
        "user process accounting",
        process_after.spawned_tasks > process_before.spawned_tasks
            && process_after.exited_total > process_before.exited_total
            && process_after.last_task_id == result.task_id
            && process_after.last_exit_code == result.exit_code,
    );

    ok
}

fn run_stability_stress() -> bool {
    let ticks_before = interrupts::ticks();
    let log_before = klog::snapshot();
    let serial_before = stats::snapshot().serial_bytes;
    let paging_before = paging::snapshot();
    let mut checksum = 0x544f_4241_4343_4f43u64;
    let mut paging_ok = true;

    for cycle in 0..4u64 {
        checksum = stress_cpu_round(checksum, cycle);

        if !stress_paging_round(cycle, &mut checksum) {
            paging_ok = false;
        }

        klog::append_u64("ci", "stress cycle", cycle);
    }

    for index in 0..(klog::ENTRY_COUNT + 8) {
        klog::append_u64("ci", "ring", index as u64);
    }

    let ticks_after = interrupts::ticks();
    let log_after = klog::snapshot();
    let serial_after = stats::snapshot().serial_bytes;
    let paging_after = paging::snapshot();
    let paging_audit = paging::permission_audit();

    let mut ok = true;
    ok &= check("stress paging translation", paging_ok);
    ok &= check(
        "stress memory tracking stable",
        paging_after.tracking_overflows == paging_before.tracking_overflows
            && paging_after.tracking_misses == paging_before.tracking_misses
            && paging_audit.violations == 0,
    );
    ok &= check(
        "stress allocator corruption guard",
        heap::corruption_check(),
    );
    ok &= check("stress heap allocator reuse", heap::stress());
    ok &= check(
        "stress log bounded",
        log_after.initialized
            && log_after.capacity == klog::ENTRY_COUNT as u64
            && log_after.count <= log_after.capacity,
    );
    ok &= check(
        "stress log wrap monotonic",
        log_after.next_sequence > log_before.next_sequence
            && log_after.dropped >= log_before.dropped,
    );
    ok &= check("stress timer monotonic", ticks_after >= ticks_before);
    ok &= check("stress serial monotonic", serial_after >= serial_before);
    ok &= check("stress checksum stable", checksum != 0);
    serial::log_hex_u64("ci", "stress checksum", checksum);

    ok
}

fn run_console_stress_checks() -> bool {
    let report = shell::run_console_model_checks();
    let mut ok = true;

    ok &= check("console long input", report.long_input);
    ok &= check("console long backspace", report.backspace_long);
    ok &= check("console line editing", report.line_editing);
    ok &= check("console history navigation", report.history_navigation);
    ok &= check("console command lookup", report.command_lookup);

    let stats_before_invalid = stats::snapshot();
    for index in 0..24u64 {
        serial::log_u64("ci", "invalid burst index", index);
        shell::execute_for_ci(b"definitely-not-a-command");
    }
    let stats_after_invalid = stats::snapshot();
    ok &= check(
        "console invalid command burst",
        stats_after_invalid.shell_errors >= stats_before_invalid.shell_errors.saturating_add(24),
    );

    let log_before = klog::snapshot();
    for index in 0..(klog::ENTRY_COUNT * 3) {
        klog::append_u64("ci-console", "log flood", index as u64);
    }
    let log_after = klog::snapshot();
    ok &= check(
        "console log flood bounded",
        log_after.count <= log_after.capacity
            && log_after.next_sequence > log_before.next_sequence
            && log_after.dropped >= log_before.dropped,
    );

    let stats_before_scroll = stats::snapshot();
    for _ in 0..32 {
        vga::write_line("console scroll stress line");
    }
    let stats_after_scroll = stats::snapshot();
    ok &= check(
        "console scroll region",
        stats_after_scroll.vga_scrolls > stats_before_scroll.vga_scrolls,
    );

    let mut long_input = [b'z'; 180];
    for index in 0..long_input.len() {
        long_input[index] = b'a' + (index % 26) as u8;
    }
    let stats_before_render = stats::snapshot();
    vga::render_input_with_cursor(&long_input, 91);
    let stats_after_render = stats::snapshot();
    ok &= check(
        "console render wrapped input",
        stats_after_render.vga_cell_writes > stats_before_render.vga_cell_writes,
    );

    ok
}

fn stress_cpu_round(mut checksum: u64, cycle: u64) -> u64 {
    for value in 0..1024u64 {
        checksum = checksum
            .rotate_left(7)
            .wrapping_add(value ^ cycle.rotate_left(5))
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

fn check(label: &str, ok: bool) -> bool {
    if ok {
        serial::log("ci", label);
    } else {
        serial::log("ci", "FAIL");
        serial::log("ci", label);
    }

    ok
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }

    if needle.len() > haystack.len() {
        return false;
    }

    for start in 0..=(haystack.len() - needle.len()) {
        if &haystack[start..(start + needle.len())] == needle {
            return true;
        }
    }

    false
}
