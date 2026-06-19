#!/usr/bin/env sh
set -eu

ISO_PATH="${1:-tobacco-ci-fault.iso}"
SERIAL_LOG="${2:-serial-fault.log}"
FAULT_KIND="${3:-pagefault}"
TIMEOUT_SECONDS="${4:-8}"

if [ ! -f "$ISO_PATH" ]; then
    echo "ISO not found: $ISO_PATH"
    exit 1
fi

: > "$SERIAL_LOG"

set +e
timeout "${TIMEOUT_SECONDS}s" qemu-system-x86_64 \
    -m 128M \
    -boot d \
    -cdrom "$ISO_PATH" \
    -display none \
    -serial "file:$SERIAL_LOG" \
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

if [ ! -s "$SERIAL_LOG" ]; then
    echo "Serial log was not created or is empty."
    exit 1
fi

assert_log() {
    pattern="$1"
    if ! grep -Fq "$pattern" "$SERIAL_LOG"; then
        echo "Missing serial log pattern: $pattern"
        echo "----- serial log -----"
        cat "$SERIAL_LOG"
        echo "----------------------"
        exit 1
    fi
}

assert_log "[boot] Tobacco v0.0.5 booting..."
assert_log "[gdt] gdt, tss, ist ready"
assert_log "[gdt] double fault ist top:"
assert_log "[irq] idt, pic, pit ready"
assert_log "[panic] CPU exception captured"
assert_log "[panic] exception screen rendered"

case "$FAULT_KIND" in
    pagefault)
        assert_log "[ci-fault] page fault trigger requested"
        assert_log "[ci-fault] target address:"
        assert_log "[panic] Page Fault"
        assert_log "[panic] exception vector: 14"
        assert_log "[panic] cr2:"
        assert_log "[panic] heap guard page violation"
        assert_log "[panic] page present: off"
        ;;
    doublefault)
        assert_log "[ci-fault] double fault trigger requested"
        assert_log "[ci-fault] forcing interrupt delivery on invalid stack"
        assert_log "[panic] Double Fault"
        assert_log "[panic] exception vector: 8"
        assert_log "[panic] error code: 0x0"
        ;;
    *)
        echo "Unknown fault kind: $FAULT_KIND"
        exit 1
        ;;
esac

echo "Tobacco CI fault test passed: $FAULT_KIND"
