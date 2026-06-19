#!/usr/bin/env sh
set -eu

cd "$(dirname "$0")/.."

export LANG=en_US.UTF-8
export LC_ALL=en_US.UTF-8
export TERM="${TERM:-xterm-256color}"

if [ ! -f tobacco.iso ]; then
    echo "tobacco.iso not found in the project folder."
    echo "Download the tobacco-iso artifact from GitHub Actions first."
    exit 1
fi

exec qemu-system-x86_64 \
    -m 128M \
    -boot d \
    -cdrom tobacco.iso \
    -display curses \
    -monitor none \
    -net none \
    -no-reboot \
    -no-shutdown
