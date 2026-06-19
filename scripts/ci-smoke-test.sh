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
assert_log "[klog] ring buffer ready"
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
assert_log "[paging] boot page tables ready"
assert_log "[paging] cr3:"
assert_log "[paging] huge pages:"
assert_log "[paging] identity mapped bytes:"
assert_log "[boot] vga text console ready"
assert_log "[keyboard] ps/2 controller drained"
assert_log "[irq] idt, pic, pit ready"
assert_log "[ci] command smoke requested"
assert_log "[ci] command help"
assert_log "[ci] command health"
assert_log "[ci] command diag"
assert_log "[ci] command lastpanic"
assert_log "[ci] command buildinfo"
assert_log "[ci] command uptime"
assert_log "[ci] command selftest"
assert_log "[ci] command stress"
assert_log "[ci] command paging"
assert_log "[ci] command heap"
assert_log "[ci] command vmtest"
assert_log "[ci] command consoletest"
assert_log "[ci] command mem"
assert_log "[ci] command log"
assert_log "[ci] selftest virtual map unmap"
assert_log "[ci] selftest heap ready"
assert_log "[ci] selftest heap probe"
assert_log "[ci] selftest status: PASS"
assert_log "[ci] stress status: PASS"
assert_log "[ci] console long input"
assert_log "[ci] console long backspace"
assert_log "[ci] console line editing"
assert_log "[ci] console history navigation"
assert_log "[ci] console command lookup"
assert_log "[ci] console invalid command burst"
assert_log "[ci] console log flood bounded"
assert_log "[ci] console scroll region"
assert_log "[ci] console render wrapped input"
assert_log "[ci] console status: PASS"
assert_log "[ci] command smoke complete"

echo "Tobacco CI smoke test passed."
