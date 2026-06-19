# Tobacco

Sistem operasi modern, ringan, modular. Untuk mereka yang tidak punya infrastruktur perangkat keras.

Tobacco adalah kernel eksperimental berbasis Rust `no_std` untuk target `x86_64-unknown-none`. Kernel boot melalui GRUB Multiboot2 ISO, menampilkan terminal VGA, menerima input keyboard PS/2, dan menyediakan shell awal untuk Phase 1.

## Status

- Boot via GRUB Multiboot2 ISO
- VGA text console
- Multiboot2 boot info parser
- Real memory map reporting
- Physical frame allocator
- Boot page table inspector
- Runtime GDT, TSS, and double-fault IST
- PS/2 keyboard input
- Shell line editor
- Command history
- Command table
- System info commands
- Structured serial log
- Panic screen
- Vector-specific CPU exception diagnostics
- Performance counters
- CI QEMU smoke test

## Build

Build utama berjalan melalui GitHub Actions, menjalankan QEMU smoke test headless, dan menghasilkan artifact `tobacco-iso`. Serial log smoke test disimpan sebagai artifact `tobacco-serial-log`.

## Run

Gunakan QEMU hanya dengan ISO:

```sh
qemu-system-x86_64 -boot d -cdrom tobacco.iso -no-reboot -no-shutdown
```

Jangan gunakan disk fisik, `/dev/disk`, atau opsi `-drive file=/dev/...`.
