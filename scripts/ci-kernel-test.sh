#!/usr/bin/env sh
set -eu

TEST_SUITE="${1:-boot-normal}"
IMAGE_DIR="${2:-.}"
TIMEOUT_SECONDS="${3:-12}"

run_qemu() {
    iso_path="$1"
    serial_log="$2"
    timeout_seconds="${3:-$TIMEOUT_SECONDS}"

    if [ ! -f "$iso_path" ]; then
        echo "ISO not found: $iso_path"
        exit 1
    fi

    : > "$serial_log"

    set +e
    timeout "${timeout_seconds}s" qemu-system-x86_64 \
        -m 128M \
        -boot d \
        -cdrom "$iso_path" \
        -display none \
        -serial "file:$serial_log" \
        -monitor none \
        -net none \
        -no-reboot \
        -no-shutdown
    status=$?
    set -e

    if [ "$status" -ne 0 ] && [ "$status" -ne 124 ]; then
        echo "QEMU exited unexpectedly with status $status"
        exit "$status"
    fi

    if [ ! -s "$serial_log" ]; then
        echo "Serial log was not created or is empty: $serial_log"
        exit 1
    fi
}

assert_log() {
    serial_log="$1"
    pattern="$2"

    if ! grep -Fq "$pattern" "$serial_log"; then
        echo "Missing serial log pattern: $pattern"
        echo "----- serial log: $serial_log -----"
        cat "$serial_log"
        echo "----------------------"
        exit 1
    fi
}

assert_not_log() {
    serial_log="$1"
    pattern="$2"

    if grep -Fq "$pattern" "$serial_log"; then
        echo "Unexpected serial log pattern: $pattern"
        echo "----- serial log: $serial_log -----"
        cat "$serial_log"
        echo "----------------------"
        exit 1
    fi
}

assert_boot_baseline() {
    serial_log="$1"

    assert_log "$serial_log" "[boot] Tobacco v0.0.5 booting..."
    assert_log "$serial_log" "[build] git commit:"
    assert_log "$serial_log" "[build] build time:"
    assert_log "$serial_log" "[build] profile: release"
    assert_log "$serial_log" "[build] target: x86_64-unknown-none.json"
    assert_log "$serial_log" "[build] feature flags: none"
    assert_log "$serial_log" "[klog] ring buffer ready"
    assert_log "$serial_log" "[gdt] gdt, tss, ist ready"
    assert_log "$serial_log" "[boot] multiboot magic: on"
    assert_log "$serial_log" "[mem] usable bytes:"
    assert_log "$serial_log" "[paging] boot page tables ready"
    assert_log "$serial_log" "[heap] free list allocator ready"
    assert_log "$serial_log" "[elf] ELF64 user image loaded"
    assert_log "$serial_log" "[elf] W^X permissions active"
    assert_log "$serial_log" "[process] task model ready"
    assert_log "$serial_log" "[sched] scheduler ready"
    assert_log "$serial_log" "[syscall] table ready"
    assert_log "$serial_log" "[boot] vga text console ready"
    assert_log "$serial_log" "[irq] idt, pic, pit ready"
}

assert_no_kernel_failure() {
    serial_log="$1"

    assert_not_log "$serial_log" "[panic]"
    assert_not_log "$serial_log" "[ci] FAIL"
}

case "$TEST_SUITE" in
    boot-normal)
        log_path="serial-boot-normal.log"
        run_qemu "$IMAGE_DIR/tobacco.iso" "$log_path" 8
        assert_boot_baseline "$log_path"
        assert_no_kernel_failure "$log_path"
        ;;
    shell-command-smoke)
        log_path="serial-shell-command-smoke.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-shell.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[ci] shell command smoke requested"
        assert_log "$log_path" "[ci] command help"
        assert_log "$log_path" "[ci] command health"
        assert_log "$log_path" "[ci] command faulttest"
        assert_log "$log_path" "[ci] command elf"
        assert_log "$log_path" "[ci] command elftest"
        assert_log "$log_path" "[ci] console long input"
        assert_log "$log_path" "[ci] console history navigation"
        assert_log "$log_path" "[ci] console invalid command burst"
        assert_log "$log_path" "[ci] console status: PASS"
        assert_log "$log_path" "[ci] shell command smoke status: PASS"
        assert_log "$log_path" "[ci] shell command smoke complete"
        assert_no_kernel_failure "$log_path"
        ;;
    syscall-probe)
        log_path="serial-syscall-probe.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-syscall.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[ci] syscall probe requested"
        assert_log "$log_path" "[user] entering ring3 probe"
        assert_log "$log_path" "[syscall] dispatch"
        assert_log "$log_path" "[syscall] user log id: 1"
        assert_log "$log_path" "[sched] cooperative yield"
        assert_log "$log_path" "[syscall] exit: 42"
        assert_log "$log_path" "[ci] user process exited"
        assert_log "$log_path" "[ci] user syscall table accounting"
        assert_log "$log_path" "[ci] syscall probe status: PASS"
        assert_log "$log_path" "[ci] syscall probe complete"
        assert_no_kernel_failure "$log_path"
        ;;
    elf-loader)
        log_path="serial-elf-loader.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-elf.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[ci] ELF loader test requested"
        assert_log "$log_path" "[ci] ELF loader initialized"
        assert_log "$log_path" "[ci] ELF header validated"
        assert_log "$log_path" "[ci] ELF PT_LOAD mapped"
        assert_log "$log_path" "[ci] ELF W^X permissions"
        assert_log "$log_path" "[elf] entering ELF64 user entry point"
        assert_log "$log_path" "[elf] ELF64 user process passed"
        assert_log "$log_path" "[ci] ELF process status"
        assert_log "$log_path" "[ci] ELF loader status: PASS"
        assert_log "$log_path" "[ci] ELF loader test complete"
        assert_no_kernel_failure "$log_path"
        ;;
    user-page-fault)
        log_path="serial-user-page-fault.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-userfault.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[ci] user page fault requested"
        assert_log "$log_path" "[fault] user exception isolated"
        assert_log "$log_path" "[fault] ring3 page fault killed user task"
        assert_log "$log_path" "[fault] page user: on"
        assert_log "$log_path" "[ci] user fault isolated"
        assert_log "$log_path" "[ci] user fault accounting"
        assert_log "$log_path" "[ci] user page fault status: PASS"
        assert_log "$log_path" "[ci] user page fault complete"
        assert_no_kernel_failure "$log_path"
        ;;
    heap-stress)
        log_path="serial-heap-stress.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-heap.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[ci] heap stress requested"
        assert_log "$log_path" "[ci] heap stress initialized"
        assert_log "$log_path" "[ci] heap stress free coalesce"
        assert_log "$log_path" "[ci] heap stress corruption guard"
        assert_log "$log_path" "[ci] heap stress allocator reuse"
        assert_log "$log_path" "[ci] heap stress accounting stable"
        assert_log "$log_path" "[ci] heap stress status: PASS"
        assert_log "$log_path" "[ci] heap stress complete"
        assert_no_kernel_failure "$log_path"
        ;;
    keyboard-model)
        log_path="serial-keyboard-model.log"
        run_qemu "$IMAGE_DIR/tobacco-ci-keyboard.iso" "$log_path" "$TIMEOUT_SECONDS"
        assert_boot_baseline "$log_path"
        assert_log "$log_path" "[keyboard] ps/2 controller drained"
        assert_log "$log_path" "[ci] keyboard model requested"
        assert_log "$log_path" "[ci] keyboard model command exists"
        assert_log "$log_path" "[ci] keyboard model queue sane"
        assert_log "$log_path" "[ci] keyboard model long backspace"
        assert_log "$log_path" "[ci] keyboard model line editing"
        assert_log "$log_path" "[ci] keyboard model history navigation"
        assert_log "$log_path" "[ci] keyboard model status: PASS"
        assert_log "$log_path" "[ci] keyboard model complete"
        assert_no_kernel_failure "$log_path"
        ;;
    panic-fault-screen)
        page_log="serial-panic-fault-screen-pagefault.log"
        double_log="serial-panic-fault-screen-doublefault.log"

        run_qemu "$IMAGE_DIR/tobacco-ci-pagefault.iso" "$page_log" 8
        assert_boot_baseline "$page_log"
        assert_log "$page_log" "[ci-fault] page fault trigger requested"
        assert_log "$page_log" "[panic] CPU exception captured"
        assert_log "$page_log" "[panic] Page Fault"
        assert_log "$page_log" "[panic] exception vector: 14"
        assert_log "$page_log" "[panic] heap guard page violation"
        assert_log "$page_log" "[panic] exception screen rendered"

        run_qemu "$IMAGE_DIR/tobacco-ci-doublefault.iso" "$double_log" 8
        assert_boot_baseline "$double_log"
        assert_log "$double_log" "[ci-fault] double fault trigger requested"
        assert_log "$double_log" "[panic] CPU exception captured"
        assert_log "$double_log" "[panic] Double Fault"
        assert_log "$double_log" "[panic] exception vector: 8"
        assert_log "$double_log" "[panic] exception screen rendered"
        ;;
    *)
        echo "Unknown CI kernel test suite: $TEST_SUITE"
        exit 1
        ;;
esac

echo "Tobacco CI kernel test passed: $TEST_SUITE"
