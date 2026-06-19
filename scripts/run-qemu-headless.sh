#!/usr/bin/env sh
set -eu

cd "$(dirname "$0")/.."

if [ ! -f tobacco.iso ]; then
    echo "tobacco.iso not found in the project folder."
    echo "Download the tobacco-iso artifact from GitHub Actions first."
    exit 1
fi

echo "Starting Tobacco headless QEMU."
echo "Serial log: qemu-serial.log"
echo "Stop with Ctrl-C."

exec qemu-system-x86_64 \
    -m 128M \
    -boot d \
    -cdrom tobacco.iso \
    -display none \
    -serial file:qemu-serial.log \
    -monitor none \
    -net none \
    -no-reboot \
    -no-shutdown
