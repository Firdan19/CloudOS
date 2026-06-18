bits 64

section .text.interrupts

global timer_interrupt_stub
global keyboard_interrupt_stub
global default_irq_stub
global default_interrupt_stub
global default_exception_with_error_stub

extern timer_interrupt_handler
extern keyboard_interrupt_handler
extern default_irq_handler
extern exception_handler

%macro push_regs 0
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
%endmacro

%macro pop_regs 0
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
%endmacro

%macro call_rust_handler 1
    push_regs
    mov rax, rsp
    and rsp, -16
    sub rsp, 16
    mov [rsp], rax
    cld
    call %1
    mov rsp, [rsp]
    pop_regs
    iretq
%endmacro

timer_interrupt_stub:
    call_rust_handler timer_interrupt_handler

keyboard_interrupt_stub:
    call_rust_handler keyboard_interrupt_handler

default_irq_stub:
    call_rust_handler default_irq_handler

default_interrupt_stub:
    call_rust_handler exception_handler

default_exception_with_error_stub:
    add rsp, 8
    call_rust_handler exception_handler
