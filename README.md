# Tobacco

Token AI berlebihan? Sepertinya seru untuk membangun OS

Fokus Tobacco saat ini hanya satu: Phase 1, membangun fondasi kernel yang kokoh

Tobacco ingin belajar dari tools yang sudah ada dan jumlahnya sangat banyak

Kalau kamu profesional, pelajar, engineer, researcher, dokumentator, atau sekadar penasaran, kontribusi kecil tetap berarti

Proyek ini masih receh, tapi seru untuk dibangun bersama silahkan habiskan token AI mu jika bersedi salam hangat

## Status Saat Ini

Tobacco berada di Phase 1 - Kernel Foundation Hardening

Versi saat ini v0.0.5

Tobacco saat ini masih fokus pada fondasi kernel

Belum masuk ke GUI, filesystem besar, user space kompleks, plugin, atau AI service

## Yang Sudah Selesai

- Boot via GRUB Multiboot2 ISO
- Kernel Rust `no_std` dan `no_main`
- Target `x86_64-unknown-none`
- Long mode 64-bit
- Stack awal aktif
- Paging awal dan identity mapping
- VGA text console
- Keyboard PS/2 via interrupt
- Shell mini interaktif
- Command parser berbasis command table
- Line editor dasar
- Command history
- Serial logging via COM1
- Structured serial log
- Kernel ring log
- IDT
- PIC 8259
- PIT timer
- GDT
- TSS
- IST untuk fault handling
- Ring 3 user mode probe
- ELF64 user program loader
- Validasi header dan segment `PT_LOAD`
- Permission user page W^X dan NX
- Initramfs read-only format CPIO `newc`
- `/bin/init` dimuat sebagai Multiboot2 module
- `/bin/init` dijalankan otomatis di Ring 3
- User page mapping terbatas
- Syscall gate `int 0x80`
- Minimal syscall ABI
- Panic screen
- Exception screen
- Page fault handler
- Double fault handler
- Controlled page fault test
- Controlled double fault test
- Memory map parsing dari Multiboot2
- Physical frame allocator awal
- Paging inspector
- Virtual page map dan unmap awal
- Kernel heap awal
- Guard page untuk heap
- Health command
- Status command
- Diagnostic command
- Last panic snapshot
- Build info command
- Selftest command
- Stress test command
- GitHub Actions build ISO
- CI smoke test
- CI stress test
- CI fault test
- CI kernel test matrix
- CI khusus ELF loader dan initramfs

## Command yang Tersedia

- `help`
- `clear`
- `version`
- `about`
- `echo`
- `uptime`
- `health`
- `status`
- `diag`
- `lastpanic`
- `buildinfo`
- `sysinfo`
- `mem`
- `memmap`
- `frames`
- `frame`
- `ticks`
- `keyboard`
- `consoletest`
- `perf`
- `irq`
- `boot`
- `user`
- `elf`
- `elftest`
- `initramfs`
- `usertest`
- `syscall`
- `gdt`
- `paging`
- `heap`
- `virt`
- `vmtest`
- `log`
- `dmesg`
- `selftest`
- `stress`
- `bench`

## Build

Build utama dilakukan lewat GitHub Actions

Artifact yang dihasilkan

```text
tobacco-iso
```

Isi artifact

```text
tobacco.iso
```

## Menjalankan di QEMU

Setelah artifact diunduh dan diekstrak

```sh
qemu-system-x86_64 -boot d -cdrom tobacco.iso
```

Mode aman yang dipakai untuk pengembangan saat ini tidak memberi akses disk fisik ke QEMU

Aturan aman

- Jangan arahkan QEMU ke `/dev/disk`
- Jangan gunakan `-drive file=/dev/...`
- Jangan format disk
- Jangan install bootloader ke disk laptop
- Jangan beri akses USB atau disk fisik ke QEMU

## Roadmap Phase 1

Tobacco tetap berada di Phase 1 sampai fondasi kernel benar-benar kokoh

Target berikutnya

- Syscall ABI lanjutan
- Process Control Block
- Scheduler awal
- Memory isolation lebih matang
- Process address space terpisah
- Lifecycle `/bin/init` jangka panjang
- IPC dasar
- Fault isolation untuk user process
- CI test untuk user mode dan syscall

## Arah Jangka Panjang

| Lapisan | Bahasa | Catatan |
|---|---|---|
| Kernel inti | Rust + C | Rust untuk keamanan memori, C untuk kontrol rendah jika diperlukan |
| Driver | C + Rust | Akses hardware rendah dan logika driver yang lebih aman |
| User space tools | Go | Untuk shell lanjutan, daemon, dan agent |
| Plugin aplikasi | WASM | Sandbox aman dan cross-platform |
| CloudFS | Zig | Eksperimen filesystem ringan dan portable |
| GUI | Rust | Display server dan compositor |
| AI service | Rust + Go | Service user space, bukan langsung di ring 0 |

## Kontribusi

Kontribusi terbuka untuk siapa pun yang ingin ikut membangun, menguji, membaca, mengkritik, atau sekadar memberi ide

Bisa mulai dari hal kecil

- Review kode
- Dokumentasi
- Test di QEMU
- Ide arsitektur
- Perbaikan bug
- CI improvement
- Eksperimen driver
- Diskusi kernel, memory, scheduler, syscall, atau security
