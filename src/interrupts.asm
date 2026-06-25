bits 64

section .text.interrupts

global timer_interrupt_stub
global keyboard_interrupt_stub
global syscall_interrupt_stub
global default_irq_stub
global default_interrupt_stub
global default_exception_with_error_stub
global exception_00_divide_error_stub
global exception_01_debug_stub
global exception_02_nmi_stub
global exception_03_breakpoint_stub
global exception_04_overflow_stub
global exception_05_bound_range_stub
global exception_06_invalid_opcode_stub
global exception_07_device_not_available_stub
global exception_08_double_fault_stub
global exception_10_invalid_tss_stub
global exception_11_segment_not_present_stub
global exception_12_stack_segment_fault_stub
global exception_13_general_protection_fault_stub
global exception_14_page_fault_stub
global exception_16_x87_floating_point_stub
global exception_17_alignment_check_stub
global exception_18_machine_check_stub
global exception_19_simd_floating_point_stub
global exception_20_virtualization_stub
global exception_21_control_protection_stub
global exception_28_hypervisor_injection_stub
global exception_29_vmm_communication_stub
global exception_30_security_stub
global ci_trigger_double_fault_stub
global user_enter
global user_return_to_kernel

extern timer_interrupt_handler
extern keyboard_interrupt_handler
extern syscall_dispatch_handler
extern default_irq_handler
extern exception_dispatch_handler

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

%macro exception_no_error 2
%1:
    push qword 0
    push qword %2
    jmp exception_common_stub
%endmacro

%macro exception_with_error 2
%1:
    push qword %2
    jmp exception_common_stub
%endmacro

timer_interrupt_stub:
    call_rust_handler timer_interrupt_handler

keyboard_interrupt_stub:
    call_rust_handler keyboard_interrupt_handler

syscall_interrupt_stub:
    push_regs
    mov r10w, 0x10
    mov ds, r10w
    mov es, r10w
    mov ss, r10w
    mov fs, r10w
    mov gs, r10w
    mov rdx, rsp
    mov rdi, [rsp + 112]
    mov rsi, [rsp + 72]
    mov rax, rsp
    and rsp, -16
    sub rsp, 16
    mov [rsp], rax
    cld
    call syscall_dispatch_handler
    mov rsp, [rsp]
    mov r10w, 0x2b
    mov ds, r10w
    mov es, r10w
    mov fs, r10w
    mov gs, r10w
    pop_regs
    iretq

default_irq_stub:
    call_rust_handler default_irq_handler

default_interrupt_stub:
    push qword 0
    push qword 255
    jmp exception_common_stub

default_exception_with_error_stub:
    push qword 254
    jmp exception_common_stub

exception_no_error exception_00_divide_error_stub, 0
exception_no_error exception_01_debug_stub, 1
exception_no_error exception_02_nmi_stub, 2
exception_no_error exception_03_breakpoint_stub, 3
exception_no_error exception_04_overflow_stub, 4
exception_no_error exception_05_bound_range_stub, 5
exception_no_error exception_06_invalid_opcode_stub, 6
exception_no_error exception_07_device_not_available_stub, 7
exception_with_error exception_08_double_fault_stub, 8
exception_with_error exception_10_invalid_tss_stub, 10
exception_with_error exception_11_segment_not_present_stub, 11
exception_with_error exception_12_stack_segment_fault_stub, 12
exception_with_error exception_13_general_protection_fault_stub, 13
exception_with_error exception_14_page_fault_stub, 14
exception_no_error exception_16_x87_floating_point_stub, 16
exception_with_error exception_17_alignment_check_stub, 17
exception_no_error exception_18_machine_check_stub, 18
exception_no_error exception_19_simd_floating_point_stub, 19
exception_no_error exception_20_virtualization_stub, 20
exception_with_error exception_21_control_protection_stub, 21
exception_no_error exception_28_hypervisor_injection_stub, 28
exception_with_error exception_29_vmm_communication_stub, 29
exception_with_error exception_30_security_stub, 30

exception_common_stub:
    push_regs
    lea rdi, [rsp + 120]
    mov rax, rsp
    and rsp, -16
    sub rsp, 16
    mov [rsp], rax
    cld
    call exception_dispatch_handler
    mov rsp, [rsp]
    pop_regs
    add rsp, 16
    iretq

ci_trigger_double_fault_stub:
    cli
    xor rsp, rsp
    int3
.halt:
    hlt
    jmp .halt

user_enter:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rel user_return_rsp], rsp
    lea rax, [rel .return_from_user]
    mov [rel user_return_rip], rax

    mov ax, dx
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    push rdx
    push rsi
    pushfq
    pop rax
    and rax, 0xfffffffffffffdff
    push rax
    push rcx
    push rdi
    iretq

.return_from_user:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov rax, [rel user_exit_code]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

user_return_to_kernel:
    mov [rel user_exit_code], rdi
    mov rsp, [rel user_return_rsp]
    jmp [rel user_return_rip]

section .bss.user
align 8
user_return_rsp:
    resq 1
user_return_rip:
    resq 1
user_exit_code:
    resq 1
