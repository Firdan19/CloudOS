use crate::{
    elf, gdt, heap, initramfs, interrupts, keyboard, klog, multiboot, paging, physmem, process,
    scheduler, serial, shell, stats, syscall, user, user_program, vga,
};
use x86_64::instructions::hlt;

const CI_BOOT_FLAG: &[u8] = b"tobacco.ci=smoke";
const CI_SHELL_FLAG: &[u8] = b"tobacco.ci=shell";
const CI_SYSCALL_FLAG: &[u8] = b"tobacco.ci=syscall";
const CI_ELF_FLAG: &[u8] = b"tobacco.ci=elf";
const CI_USER_FAULT_FLAG: &[u8] = b"tobacco.ci=userfault";
const CI_HEAP_FLAG: &[u8] = b"tobacco.ci=heap";
const CI_KEYBOARD_FLAG: &[u8] = b"tobacco.ci=keyboard";
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
        if contains_bytes(command_line, CI_SHELL_FLAG) {
            run_shell_command_smoke();
            return;
        }

        if contains_bytes(command_line, CI_SYSCALL_FLAG) {
            run_syscall_probe_matrix();
            return;
        }

        if contains_bytes(command_line, CI_ELF_FLAG) {
            run_elf_loader_matrix();
            return;
        }

        if contains_bytes(command_line, CI_USER_FAULT_FLAG) {
            run_user_fault_matrix();
            return;
        }

        if contains_bytes(command_line, CI_HEAP_FLAG) {
            run_heap_stress_matrix();
            return;
        }

        if contains_bytes(command_line, CI_KEYBOARD_FLAG) {
            run_keyboard_model_matrix();
            return;
        }

        return;
    }

    run_full_smoke();
}

fn run_full_smoke() {
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

fn run_shell_command_smoke() {
    serial::log("ci", "shell command smoke requested");
    run_command_table_checks();

    if run_console_stress_checks() {
        serial::log("ci", "console status: PASS");
        serial::log("ci", "shell command smoke status: PASS");
    } else {
        serial::log("ci", "console status: FAIL");
        serial::log("ci", "shell command smoke status: FAIL");
    }

    serial::log("ci", "shell command smoke complete");
}

fn run_syscall_probe_matrix() {
    serial::log("ci", "syscall probe requested");
    if run_syscall_probe_checks() {
        serial::log("ci", "syscall probe status: PASS");
    } else {
        serial::log("ci", "syscall probe status: FAIL");
    }

    serial::log("ci", "syscall probe complete");
}

fn run_elf_loader_matrix() {
    serial::log("ci", "ELF loader test requested");
    if run_elf_loader_checks() {
        serial::log("ci", "ELF loader status: PASS");
    } else {
        serial::log("ci", "ELF loader status: FAIL");
    }

    serial::log("ci", "ELF loader test complete");
}

fn run_user_fault_matrix() {
    serial::log("ci", "user page fault requested");
    if run_user_fault_isolation_checks() {
        serial::log("ci", "user page fault status: PASS");
    } else {
        serial::log("ci", "user page fault status: FAIL");
    }

    serial::log("ci", "user page fault complete");
}

fn run_heap_stress_matrix() {
    serial::log("ci", "heap stress requested");
    if run_heap_stress_checks() {
        serial::log("ci", "heap stress status: PASS");
    } else {
        serial::log("ci", "heap stress status: FAIL");
    }

    serial::log("ci", "heap stress complete");
}

fn run_keyboard_model_matrix() {
    serial::log("ci", "keyboard model requested");
    if run_keyboard_model_checks() {
        serial::log("ci", "keyboard model status: PASS");
    } else {
        serial::log("ci", "keyboard model status: FAIL");
    }

    serial::log("ci", "keyboard model complete");
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
    check("command faults", shell::command_exists(b"faults"));
    check("command buildinfo", shell::command_exists(b"buildinfo"));
    check("command uptime", shell::command_exists(b"uptime"));
    check("command selftest", shell::command_exists(b"selftest"));
    check("command stress", shell::command_exists(b"stress"));
    check("command paging", shell::command_exists(b"paging"));
    check("command heap", shell::command_exists(b"heap"));
    check("command heaptest", shell::command_exists(b"heaptest"));
    check("command heapcheck", shell::command_exists(b"heapcheck"));
    check("command vmtest", shell::command_exists(b"vmtest"));
    check("command user", shell::command_exists(b"user"));
    check("command elf", shell::command_exists(b"elf"));
    check("command elftest", shell::command_exists(b"elftest"));
    check("command spawn", shell::command_exists(b"spawn"));
    check("command initramfs", shell::command_exists(b"initramfs"));
    check("command process", shell::command_exists(b"process"));
    check("command lifecycle", shell::command_exists(b"lifecycle"));
    check(
        "command lifecycletest",
        shell::command_exists(b"lifecycletest"),
    );
    check("command tasks", shell::command_exists(b"tasks"));
    check("command sched", shell::command_exists(b"sched"));
    check("command usertest", shell::command_exists(b"usertest"));
    check("command tasktest", shell::command_exists(b"tasktest"));
    check("command faulttest", shell::command_exists(b"faulttest"));
    check("command syscall", shell::command_exists(b"syscall"));
    check("command syscalls", shell::command_exists(b"syscalls"));
    check("command idt", shell::command_exists(b"idt"));
    check("command gdt", shell::command_exists(b"gdt"));
    check("command mmap", shell::command_exists(b"mmap"));
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
    let address_spaces = paging::address_space_stats();
    let scheduler_state = scheduler::snapshot();
    let syscall_state = syscall::snapshot();
    let user_state = user::snapshot();
    let elf_state = elf::snapshot();
    let initramfs_state = initramfs::snapshot();
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
        frames.kernel_end > frames.kernel_start
            && frames.protected_until >= frames.kernel_end
            && frames.protected_until >= boot_info.highest_module_end,
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
        "selftest read-only initramfs",
        boot_info.module_count >= 1
            && initramfs_state.initialized
            && initramfs_state.module_found
            && initramfs_state.valid
            && initramfs_state.init_found
            && initramfs::selftest(),
    );
    ok &= check(
        "selftest ELF64 loader",
        elf_state.initialized
            && elf_state.loaded
            && elf_state.load_segments >= 1
            && elf_state.executable_pages >= 1
            && elf_state.writable_pages >= 1
            && elf::selftest(),
    );
    ok &= check(
        "selftest process table ready",
        process_state.initialized && process_state.task_capacity == process::MAX_TASKS as u64,
    );
    ok &= check("selftest process model", process::selftest());
    ok &= check(
        "selftest process resource lifecycle",
        process_state.active_resources == 0
            && process_state.cleanup_failures == 0
            && address_spaces.active == 0
            && address_spaces.cleanup_failures == 0,
    );
    ok &= check(
        "selftest scheduler ready",
        scheduler_state.initialized
            && scheduler_state.queue_capacity == scheduler::QUEUE_CAPACITY as u64
            && scheduler_state.queued_tasks <= scheduler_state.queue_capacity,
    );
    ok &= check("selftest scheduler model", scheduler::selftest());
    ok &= check(
        "selftest syscall table ready",
        syscall_state.initialized && syscall_state.entries == syscall::table_len() as u64,
    );
    ok &= check("selftest syscall table model", syscall::selftest());
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
    ok &= check("selftest command table sane", shell::command_count() >= 53);

    ok
}

fn run_user_mode_checks() -> bool {
    let mut ok = true;
    ok &= run_syscall_probe_checks();
    ok &= run_elf_loader_checks();
    ok &= run_user_fault_isolation_checks();
    ok
}

fn run_elf_loader_checks() -> bool {
    let loader = elf::snapshot();
    let archive = initramfs::snapshot();
    let process_before = process::snapshot();
    let frames_before = physmem::snapshot();
    let heap_before = heap::snapshot();
    let spaces_before = paging::address_space_stats().active;
    let result = process::run_elf_init_task();
    let process_after = process::snapshot();
    let frames_after = physmem::snapshot();
    let heap_after = heap::snapshot();
    let spaces_after = paging::address_space_stats().active;
    let text = paging::translate(loader.entry_point);
    let data = paging::translate(paging::USER_ELF_BASE + paging::PAGE_SIZE_4K);
    let stack = paging::translate(paging::USER_ELF_STACK_PAGE);
    let mut ok = true;

    ok &= check(
        "initramfs module discovered",
        archive.module_found && archive.archive_size > 0,
    );
    ok &= check(
        "initramfs archive valid",
        archive.valid && archive.last_error.is_none() && initramfs::selftest(),
    );
    ok &= check(
        "initramfs /bin/init found",
        archive.init_found && archive.init_size > 0 && initramfs::find("/bin/init").is_some(),
    );

    ok &= check(
        "ELF loader initialized",
        loader.initialized && loader.loaded && loader.last_error.is_none(),
    );
    ok &= check(
        "ELF header validated",
        loader.entry_point >= loader.image_start
            && loader.entry_point < loader.image_end
            && loader.load_segments == 2,
    );
    ok &= check(
        "ELF PT_LOAD mapped",
        loader.mapped_pages == 3 && loader.executable_pages == 1 && loader.writable_pages == 2,
    );
    ok &= check(
        "ELF W^X permissions",
        text.mapped
            && text.user_accessible
            && text.executable
            && !text.writable
            && data.mapped
            && data.user_accessible
            && data.writable
            && !data.executable
            && stack.mapped
            && stack.user_accessible
            && stack.writable
            && !stack.executable
            && !paging::translate(paging::USER_ELF_STACK_GUARD).mapped,
    );
    ok &= check("ELF loader selftest", elf::selftest());
    ok &= check("ELF process spawned", result.task_id > 0);
    ok &= check(
        "ELF process entered Ring 3",
        result.ran && result.entry_point == loader.entry_point,
    );
    ok &= check(
        "ELF process exited",
        result.state == process::TaskState::Exited
            && result.exit_code == user_program::INIT_EXPECTED_EXIT_CODE,
    );
    ok &= check(
        "ELF process syscalls",
        result.syscalls_after
            >= result
                .syscalls_before
                .saturating_add(user_program::INIT_MINIMUM_SYSCALLS),
    );
    ok &= check("ELF process status", result.passed);
    ok &= check(
        "ELF process private address space",
        result.address_space_root != 0
            && result.first_user_frame != 0
            && result.owned_user_pages >= 2
            && result.owned_table_frames >= 3,
    );
    ok &= check(
        "ELF process resource cleanup",
        result.resources_cleaned
            && result.heap_released
            && result.cleanup_user_frames == result.owned_user_pages
            && result.cleanup_table_frames == result.owned_table_frames,
    );
    ok &= check(
        "ELF process resource baseline",
        frames_after.allocated_frames == frames_before.allocated_frames
            && frames_after.free_frames == frames_before.free_frames
            && heap_after.active_allocations == heap_before.active_allocations
            && heap_after.allocated_bytes == heap_before.allocated_bytes
            && heap_after.free_bytes == heap_before.free_bytes
            && heap_after.metadata_ok
            && heap_after.sentinel_ok
            && heap_after.allocation_canaries_ok
            && spaces_after == spaces_before
            && process_after.active_resources == process_before.active_resources,
    );
    ok &= check(
        "ELF process accounting",
        process_after.spawned_tasks > process_before.spawned_tasks
            && process_after.exited_total > process_before.exited_total
            && process_after.last_task_id == result.task_id
            && process_after.last_exit_code == result.exit_code,
    );

    let isolation = process::run_isolation_test();
    ok &= check("lifecycle two processes spawned", isolation.spawned);
    ok &= check("lifecycle distinct CR3", isolation.distinct_roots);
    ok &= check(
        "lifecycle distinct user frames",
        isolation.distinct_user_frames,
    );
    ok &= check(
        "lifecycle first process cleaned",
        isolation.first.resources_cleaned,
    );
    ok &= check(
        "lifecycle second process cleaned",
        isolation.second.resources_cleaned,
    );
    ok &= check("lifecycle frame baseline", isolation.frames_restored);
    ok &= check("lifecycle heap baseline", isolation.heap_restored);
    ok &= check("lifecycle resource baseline", isolation.resources_restored);
    ok &= check("lifecycle status", isolation.passed);

    ok
}

fn run_syscall_probe_checks() -> bool {
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
        result.syscalls_after >= result.syscalls_before + 4,
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
    let scheduler_after = scheduler::snapshot();
    let syscall_after = syscall::snapshot();
    ok &= check(
        "user scheduler accounting",
        scheduler_after.context_switches > 0
            && scheduler_after.cooperative_yields > 0
            && scheduler_after.last_task == result.task_id,
    );
    ok &= check(
        "user syscall table accounting",
        syscall_after.dispatches >= result.syscalls_after
            && syscall_after.last_number == syscall::SYSCALL_EXIT
            && syscall_after.last_return == syscall::RET_OK,
    );

    ok
}

fn run_user_fault_isolation_checks() -> bool {
    let mut ok = true;
    let faults_before = user::snapshot().fault_count;
    let fault_result = process::run_user_fault_task();
    let fault_after = user::snapshot();
    let process_fault_after = process::snapshot();
    ok &= check("user fault process spawned", fault_result.task_id > 0);
    ok &= check("user fault probe ran", fault_result.ran);
    ok &= check(
        "user fault task exited",
        fault_result.state == process::TaskState::Exited,
    );
    ok &= check(
        "user fault exit code",
        fault_result.exit_code == user::fault_exit_code(14),
    );
    ok &= check(
        "user fault isolated",
        fault_result.passed && process_fault_after.running_tasks == 0,
    );
    ok &= check(
        "user fault accounting",
        fault_after.fault_count > faults_before
            && fault_after.last_fault_vector == 14
            && fault_after.last_fault_address == user::fault_address()
            && fault_after.last_fault_exit_code == user::fault_exit_code(14),
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

fn run_heap_stress_checks() -> bool {
    let before = heap::snapshot();
    let mut ok = true;

    ok &= check(
        "heap stress initialized",
        before.initialized
            && before.mapped_pages == heap::HEAP_PAGES
            && before.metadata_ok
            && before.sentinel_ok
            && before.allocation_canaries_ok,
    );
    ok &= check("heap stress free coalesce", heap::selftest());
    ok &= check("heap stress corruption guard", heap::corruption_check());
    ok &= check("heap stress allocator reuse", heap::stress());

    let after = heap::snapshot();
    ok &= check(
        "heap stress accounting stable",
        after.initialized
            && after.used <= after.size
            && after.remaining <= after.size
            && after.allocated_bytes.saturating_add(after.free_bytes) <= after.size
            && after.corruption_failures == before.corruption_failures,
    );

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

fn run_keyboard_model_checks() -> bool {
    let report = shell::run_console_model_checks();
    let counters = stats::snapshot();
    let mut ok = true;

    ok &= check(
        "keyboard model command exists",
        shell::command_exists(b"keyboard"),
    );
    ok &= check(
        "keyboard model queue sane",
        keyboard::pending_events() < 256,
    );
    ok &= check(
        "keyboard model scancode counter sane",
        counters.keyboard_scancodes <= 1_000_000,
    );
    ok &= check(
        "keyboard model dropped bounded",
        counters.keyboard_dropped_events <= counters.keyboard_events.saturating_add(256),
    );
    ok &= check("keyboard model long input", report.long_input);
    ok &= check("keyboard model long backspace", report.backspace_long);
    ok &= check("keyboard model line editing", report.line_editing);
    ok &= check(
        "keyboard model history navigation",
        report.history_navigation,
    );
    ok &= check("keyboard model command lookup", report.command_lookup);

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
