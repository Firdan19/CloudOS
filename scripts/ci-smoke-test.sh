#!/usr/bin/env sh
set -eu

ISO_PATH="${1:-tobacco.iso}"
SERIAL_LOG="${2:-serial.log}"
TIMEOUT_SECONDS="${3:-12}"

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
assert_log "[gdt] tss base:"
assert_log "[gdt] double fault ist top:"
assert_log "[boot] multiboot magic: on"
assert_log "[boot] multiboot info addr:"
assert_log "[boot] multiboot tags:"
assert_log "[mem] usable bytes:"
assert_log "[mem] memory regions:"
assert_log "[mem] frame allocator regions:"
assert_log "[mem] allocatable frames:"
assert_log "[mem] free frames:"
assert_log "[boot] vga text console ready"
assert_log "[keyboard] ps/2 controller drained"
assert_log "[irq] idt, pic, pit ready"

echo "Tobacco CI smoke test passed."
