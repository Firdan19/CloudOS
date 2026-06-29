<p align="center">
  <img src="assets/brand/tobacco-logo.png" width="150" alt="Tobacco leaf logo">
</p>

<h1 align="center">Tobacco</h1>

<p align="center">
  <strong>Kernel foundation yang kecil, nyata, dan terus diperkeras</strong><br>
  Rust · x86_64 · Multiboot2 · no_std
</p>

<p align="center">
  <a href="https://github.com/Firdan19/Tobacco/actions/workflows/build.yml"><img src="https://github.com/Firdan19/Tobacco/actions/workflows/build.yml/badge.svg" alt="Build Tobacco Kernel"></a>
</p>

<table align="center">
  <tr>
    <td align="center"><strong>Version</strong><br><code>v0.0.5</code></td>
    <td align="center"><strong>Phase</strong><br>1 · Foundation</td>
    <td align="center"><strong>Architecture</strong><br><code>x86_64</code></td>
    <td align="center"><strong>CI Matrix</strong><br>12 jobs</td>
  </tr>
</table>

Token AI berlebihan? Sepertinya seru untuk membangun OS

Fokus Tobacco saat ini hanya satu: Phase 1, membangun fondasi kernel yang kokoh

Tobacco ingin belajar dari tools yang sudah ada dan jumlahnya sangat banyak

Kalau kamu profesional, pelajar, engineer, researcher, dokumentator, atau sekadar penasaran, kontribusi kecil tetap berarti

Proyek ini masih receh, tapi seru untuk dibangun bersama silahkan habiskan token AI mu jika bersedi salam hangat

## Fondasi yang Sudah Hidup

<table>
  <tr>
    <td width="25%" valign="top">
      <strong>Boot & CPU</strong><br><br>
      GRUB Multiboot2<br>
      Long mode 64-bit<br>
      Stack dan paging awal<br>
      GDT · TSS · IST<br>
      IDT · PIC · PIT
    </td>
    <td width="25%" valign="top">
      <strong>Memory</strong><br><br>
      Multiboot memory map<br>
      Physical frame allocator<br>
      Virtual map dan unmap<br>
      Kernel heap<br>
      Guard page dan audit
    </td>
    <td width="25%" valign="top">
      <strong>Process</strong><br><br>
      Ring 3 user mode<br>
      ELF64 loader<br>
      Initramfs dan <code>/bin/init</code><br>
      Private CR3 per process<br>
      Spawn · yield · exit<br>
      Preemptive round-robin<br>
      Parent · child · wait · reaping<br>
      Bounded IPC mailboxes
    </td>
    <td width="25%" valign="top">
      <strong>Reliability</strong><br><br>
      Panic dan exception screen<br>
      User fault isolation<br>
      Serial dan kernel ring log<br>
      Selftest dan stress test<br>
      12-job CI kernel matrix
    </td>
  </tr>
</table>

## Terminal Mini

<table>
  <tr>
    <td><strong>Console</strong><br>VGA text mode · PS/2 keyboard · cursor · wrapping · scroll region</td>
    <td><strong>Editor</strong><br>Line editing · backspace · history naik turun · parser case-insensitive</td>
    <td><strong>Observability</strong><br>Health · diagnostics · build metadata · process lifecycle · fault reports</td>
  </tr>
</table>

## Command Utama

| Sistem | Memory | Process | Debug dan Test |
|---|---|---|---|
| `help` `version` `about` `buildinfo` | `mem` `mmap` `frames` `paging` | `process` `tasks` `sched` `spawn` | `health` `diag` `log` `faults` |
| `uptime` `ticks` `sysinfo` `boot` | `heap` `heapcheck` `virt` `vmtest` | `proctree` `waittest` `ipc` `ipctest` | `selftest` `stress` `preempt` `faulttest` |

Gunakan `help` di Tobacco untuk melihat seluruh command yang tersedia

## Build

Build utama berjalan melalui GitHub Actions dan menghasilkan artifact `tobacco-iso` berisi `tobacco.iso`

```sh
qemu-system-x86_64 -boot d -cdrom tobacco.iso
```

QEMU dijalankan tanpa akses ke disk fisik

- Jangan arahkan QEMU ke `/dev/disk`
- Jangan gunakan `-drive file=/dev/...`
- Jangan format atau memasang bootloader ke disk laptop
- Jangan memberi QEMU akses USB atau disk fisik

## Fokus Phase 1 Berikutnya

<table>
  <tr>
    <td><strong>Scheduling</strong><br>Preemptive context switch dan accounting yang lebih matang</td>
    <td><strong>Scheduler</strong><br>Context switch umum dan accounting per task</td>
    <td><strong>Isolation</strong><br>Fault policy per address space dan cleanup tahan gagal</td>
    <td><strong>IPC</strong><br>Blocking syscall handoff dan capability endpoint</td>
  </tr>
</table>

Tobacco tetap berada di Phase 1 sampai fondasi kernel benar-benar kokoh

## Kontribusi

Review kode, dokumentasi, test QEMU, ide arsitektur, perbaikan bug, eksperimen driver, dan kritik teknis semuanya diterima
