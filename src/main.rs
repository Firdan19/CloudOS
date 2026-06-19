#![no_std]
#![no_main]

use core::panic::PanicInfo;
use x86_64::instructions::hlt;
use x86_64::instructions::interrupts as cpu_interrupts;

mod interrupts;
mod keyboard;
mod serial;
mod shell;
mod vga;

core::arch::global_asm!(
    r#"
    .att_syntax prefix
    .section .text.boot, "ax"
    .code32
    .global _start
_start:
    cli
    movl $stack_top, %esp
    xorl %ebp, %ebp

    movl $p2_table, %edi
    xorl %eax, %eax
    movl $512, %ecx
1:
    movl %eax, %ebx
    shll $21, %ebx
    orl $0x83, %ebx
    movl %ebx, (%edi)
    movl $0, 4(%edi)
    addl $8, %edi
    incl %eax
    loop 1b

    movl $p2_table, %eax
    orl $0x03, %eax
    movl %eax, p3_table
    movl $0, p3_table+4

    movl $p3_table, %eax
    orl $0x03, %eax
    movl %eax, p4_table
    movl $0, p4_table+4

    movl $p4_table, %eax
    movl %eax, %cr3

    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4

    movl $0xC0000080, %ecx
    rdmsr
    orl $0x100, %eax
    wrmsr

    lgdt gdt_descriptor

    movl %cr0, %eax
    orl $0x80000001, %eax
    movl %eax, %cr0

    ljmp $0x08, $long_mode_start

    .code64
long_mode_start:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw %ax, %fs
    movw %ax, %gs

    movq %cr0, %rax
    andq $-5, %rax
    orq $0x2, %rax
    movq %rax, %cr0

    movq %cr4, %rax
    orq $0x600, %rax
    movq %rax, %cr4

    leaq stack_top(%rip), %rsp
    xorq %rbp, %rbp
    call kernel_main

.halt:
    hlt
    jmp .halt

    .section .rodata
    .align 8
gdt:
    .quad 0x0000000000000000
    .quad 0x00af9a000000ffff
    .quad 0x00cf92000000ffff
gdt_end:
gdt_descriptor:
    .word gdt_end - gdt - 1
    .long gdt

    .section .bss
    .align 4096
p4_table:
    .skip 4096
p3_table:
    .skip 4096
p2_table:
    .skip 4096
    .align 16
stack_bottom:
    .skip 16384
stack_top:
"#
);

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    serial::init();
    serial::log("boot", "CloudOS v0.0.5 booting...");

    vga::init();
    vga::show_splash();
    serial::log("boot", "vga text console ready");

    keyboard::init();
    serial::log("keyboard", "ps/2 controller drained");
    interrupts::init();

    shell::run();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    cpu_interrupts::disable();
    serial::log("panic", "kernel panic; system halted");
    vga::show_panic_screen("Rust panic handler", "kernel panic handler invoked");
    halt_loop();
}

fn halt_loop() -> ! {
    loop {
        hlt();
    }
}
