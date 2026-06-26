#![no_std]
#![no_main]

use core::panic::PanicInfo;
use x86_64::instructions::hlt;
use x86_64::instructions::interrupts as cpu_interrupts;

mod ci;
mod gdt;
mod heap;
mod interrupts;
mod keyboard;
mod klog;
mod multiboot;
mod paging;
mod paniclog;
mod physmem;
mod process;
mod scheduler;
mod serial;
mod shell;
mod stats;
mod syscall;
mod user;
mod vga;

core::arch::global_asm!(
    r#"
    .att_syntax prefix
    .section .text.boot, "ax"
    .code32
    .global _start
_start:
    cli
    movl %eax, multiboot_magic
    movl %ebx, multiboot_info_addr
    movl $stack_top, %esp
    xorl %ebp, %ebp

    movl $boot_p2_table, %edi
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

    movl $boot_p2_table, %eax
    orl $0x03, %eax
    movl %eax, boot_p3_table
    movl $0, boot_p3_table+4

    movl $boot_p3_table, %eax
    orl $0x03, %eax
    movl %eax, boot_p4_table
    movl $0, boot_p4_table+4

    movl $boot_p4_table, %eax
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
    movl multiboot_magic(%rip), %edi
    movl multiboot_info_addr(%rip), %esi
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
    .global boot_p4_table
boot_p4_table:
    .skip 4096
    .global boot_p3_table
boot_p3_table:
    .skip 4096
    .global boot_p2_table
boot_p2_table:
    .skip 4096
    .align 4
multiboot_magic:
    .skip 4
multiboot_info_addr:
    .skip 4
    .align 16
stack_bottom:
    .skip 65536
stack_top:
"#
);

#[no_mangle]
pub extern "C" fn kernel_main(multiboot_magic: u32, multiboot_info_addr: u32) -> ! {
    serial::init();
    klog::init();
    serial::log("klog", "ring buffer ready");
    serial::log("boot", "Tobacco v0.0.5 booting...");
    let gdt_state = gdt::init();
    serial::log("gdt", "gdt, tss, ist ready");
    serial::log_hex_u64("gdt", "gdt base", gdt_state.gdt_base);
    serial::log_hex_u64("gdt", "tss base", gdt_state.tss_base);
    serial::log_hex_u64(
        "gdt",
        "double fault ist top",
        gdt_state.double_fault_stack_top,
    );

    let boot_info = multiboot::init(multiboot_magic, multiboot_info_addr as u64);
    serial::log_bool("boot", "multiboot magic", boot_info.valid_magic);
    serial::log_u64("boot", "multiboot info addr", boot_info.address);
    serial::log_u64("boot", "multiboot tags", boot_info.tag_count as u64);
    serial::log_u64("mem", "usable bytes", boot_info.memory.usable_bytes);
    serial::log_u64(
        "mem",
        "memory regions",
        boot_info.memory.region_count as u64,
    );
    let frame_allocator = physmem::init();
    serial::log_u64("mem", "free frames", frame_allocator.free_frames);
    let paging_state = paging::init();
    serial::log_u64(
        "paging",
        "identity mapped bytes",
        paging_state.identity_mapped_bytes,
    );
    let heap_state = heap::init();
    serial::log_u64("heap", "heap bytes", heap_state.size);
    user::init();
    process::init();
    scheduler::init();
    syscall::init();

    vga::init();
    vga::show_splash();
    serial::log("boot", "vga text console ready");

    keyboard::init();
    serial::log("keyboard", "ps/2 controller drained");
    interrupts::init();
    stats::mark_shell_ready(interrupts::ticks());
    ci::run_if_requested();

    shell::run();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    cpu_interrupts::disable();
    paniclog::record_rust_panic("kernel panic handler invoked");
    serial::log("panic", "kernel panic; system halted");
    vga::show_panic_screen("Rust panic handler", "kernel panic handler invoked");
    halt_loop();
}

fn halt_loop() -> ! {
    loop {
        hlt();
    }
}
