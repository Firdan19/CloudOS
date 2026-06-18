#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::NonNull;
use volatile::VolatilePtr;
use x86_64::instructions::hlt;

mod memory;

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
    let vga_ptr = unsafe { NonNull::new_unchecked(0xb8000 as *mut memory::ScreenChar) };
    let vga = unsafe { VolatilePtr::new(vga_ptr) };

    memory::clear_screen(vga);
    memory::write_centered(vga, 11, "CloudOS");
    memory::write_centered(vga, 13, "Kernel v0.0.1 - Booted");

    halt_loop();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    halt_loop();
}

fn halt_loop() -> ! {
    loop {
        hlt();
    }
}
