# Tobacco

Sistem operasi modern, ringan, modular. Untuk mereka yang tidak punya infrastruktur perangkat keras.

Tobacco adalah kernel eksperimental berbasis Rust `no_std` untuk target `x86_64-unknown-none`. Kernel boot melalui GRUB Multiboot2 ISO, menampilkan terminal VGA, menerima input keyboard PS/2, dan menyediakan shell awal untuk Phase 1.

## Status

- Boot via GRUB Multiboot2 ISO
- VGA text console
- Multiboot2 boot info parser
- Real memory map reporting
- Physical frame allocator with recycled free-list
- Boot page table inspector
- High-half virtual memory mapper
- Kernel heap with guard pages
- Runtime GDT, TSS, and double-fault IST
- PS/2 keyboard input
- Shell line editor
- Command history
- Command table
- Console stress checks
- System info commands
- Structured serial log
- Kernel log ring buffer
- Selftest command
- Stability stress command
- CI command smoke test
- Panic screen
- Vector-specific CPU exception diagnostics
- Controlled CI page-fault and double-fault tests
- Performance counters
- CI QEMU smoke test

## Build

Build utama berjalan melalui GitHub Actions, menjalankan QEMU smoke test headless, menjalankan fault test terkontrol untuk page fault dan double fault, lalu menghasilkan artifact `tobacco-iso`. Serial log smoke test dan fault test disimpan sebagai artifact `tobacco-serial-log`.

## Run

Gunakan QEMU hanya dengan ISO:

```sh
qemu-system-x86_64 -boot d -cdrom tobacco.iso -no-reboot -no-shutdown
```

Jangan gunakan disk fisik, `/dev/disk`, atau opsi `-drive file=/dev/...`.
