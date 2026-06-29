use crate::{elf, gdt, heap, interrupts, ipc, paging, scheduler, serial, user, user_program};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_TASKS: usize = 8;
const TASK_HEAP_BYTES: u64 = 128;
const TASK_HEAP_ALIGNMENT: u64 = 64;
const PREEMPT_TEST_SWITCHES: u64 = 8;
const PREEMPT_TEST_EXIT_CODE: u64 = 0x5052;
const SPIN_PROGRAM: [u8; 4] = [0xf3, 0x90, 0xeb, 0xfc];
const MAX_USER_BUFFER_BYTES: u64 = 1024 * 1024;
const SYSCALL_INSTRUCTION_BYTES: u64 = 2;
const IPC_TEST_EXIT_CODE: u64 = 42;
const IPC_TEST_DATA: u64 = paging::USER_ELF_BASE + paging::PAGE_SIZE_4K;
const IPC_TEST_BUFFER: u64 = IPC_TEST_DATA + 64;
const IPC_TEST_CODE_CAPACITY: usize = 512;

struct UserCodeBuilder {
    bytes: [u8; IPC_TEST_CODE_CAPACITY],
    len: usize,
    valid: bool,
}

impl UserCodeBuilder {
    const fn new() -> Self {
        Self {
            bytes: [0; IPC_TEST_CODE_CAPACITY],
            len: 0,
            valid: true,
        }
    }

    fn byte(&mut self, value: u8) {
        if self.len >= self.bytes.len() {
            self.valid = false;
            return;
        }
        self.bytes[self.len] = value;
        self.len += 1;
    }

    fn bytes(&mut self, values: &[u8]) {
        for value in values.iter().copied() {
            self.byte(value);
        }
    }

    fn mov_imm64(&mut self, opcode: u8, value: u64) {
        self.bytes(&[0x48, opcode]);
        self.bytes(&value.to_le_bytes());
    }

    fn syscall(&mut self, number: u64) {
        self.mov_imm64(0xb8, number);
        self.bytes(&[0xcd, 0x80]);
    }

    fn send(&mut self, peer: u64, data: u64) {
        self.mov_imm64(0xbf, peer);
        self.mov_imm64(0xbe, data);
        self.mov_imm64(0xba, 4);
        self.syscall(crate::syscall::SYSCALL_IPC_SEND);
    }

    fn receive(&mut self) {
        self.mov_imm64(0xbf, IPC_TEST_BUFFER);
        self.mov_imm64(0xbe, ipc::MAX_MESSAGE_BYTES as u64);
        self.syscall(crate::syscall::SYSCALL_IPC_RECEIVE);
    }

    fn receive_timeout(&mut self, timeout_ticks: u64) {
        self.mov_imm64(0xbf, IPC_TEST_BUFFER);
        self.mov_imm64(0xbe, ipc::MAX_MESSAGE_BYTES as u64);
        self.mov_imm64(0xba, timeout_ticks);
        self.syscall(crate::syscall::SYSCALL_IPC_RECEIVE_TIMEOUT);
    }

    fn cancel(&mut self, capability: u64) {
        self.mov_imm64(0xbf, capability);
        self.syscall(crate::syscall::SYSCALL_IPC_CANCEL);
    }

    fn wait_ticks(&mut self, ticks: u8) {
        self.syscall(crate::syscall::SYSCALL_UPTIME);
        self.bytes(&[0x49, 0x89, 0xc4]);
        self.bytes(&[0x49, 0x83, 0xc4, ticks]);
        self.syscall(crate::syscall::SYSCALL_UPTIME);
        self.bytes(&[0x4c, 0x39, 0xe0]);
        self.bytes(&[0x72, 0xef]);
    }

    fn expect_rax_and_exit(&mut self, expected: u64, success: u64, failure: u64) {
        self.bytes(&[0x49, 0xbc]);
        self.bytes(&expected.to_le_bytes());
        self.bytes(&[0x4c, 0x39, 0xe0]);
        self.bytes(&[0x75, 0x16]);
        self.exit(success);
        self.exit(failure);
    }

    fn exit(&mut self, code: u64) {
        self.mov_imm64(0xbf, code);
        self.syscall(crate::syscall::SYSCALL_EXIT);
    }
}

#[derive(Clone, Copy)]
pub enum TimerAction {
    Continue,
    Switch(u64),
    Stop(u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Empty,
    Ready,
    Running,
    Blocked,
    Exited,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IpcWakeReason {
    None,
    Message,
    Timeout,
    Cancelled,
}

#[derive(Clone, Copy)]
pub struct Task {
    pub id: u64,
    pub parent_id: u64,
    pub state: TaskState,
    pub entry_point: u64,
    pub stack_top: u64,
    pub exit_code: u64,
    pub syscalls_before: u64,
    pub syscalls_after: u64,
    pub runs: u64,
    pub address_space_root: u64,
    pub first_user_frame: u64,
    pub owned_user_pages: u64,
    pub owned_table_frames: u64,
    pub heap_allocation: u64,
    pub resources_active: bool,
    pub cleanup_complete: bool,
    pub cleanup_user_frames: u64,
    pub cleanup_table_frames: u64,
    pub heap_released: bool,
    pub timer_preemptions: u64,
    pub scheduled_slices: u64,
    pub last_scheduled_tick: u64,
    pub max_wait_ticks: u64,
    pub wait_target: u64,
    pub ipc_waiting: bool,
    pub ipc_restart_pending: bool,
    pub ipc_deadline_tick: u64,
    pub ipc_wake_reason: IpcWakeReason,
}

impl Task {
    const fn empty() -> Self {
        Self {
            id: 0,
            parent_id: 0,
            state: TaskState::Empty,
            entry_point: 0,
            stack_top: 0,
            exit_code: 0,
            syscalls_before: 0,
            syscalls_after: 0,
            runs: 0,
            address_space_root: 0,
            first_user_frame: 0,
            owned_user_pages: 0,
            owned_table_frames: 0,
            heap_allocation: 0,
            resources_active: false,
            cleanup_complete: false,
            cleanup_user_frames: 0,
            cleanup_table_frames: 0,
            heap_released: false,
            timer_preemptions: 0,
            scheduled_slices: 0,
            last_scheduled_tick: 0,
            max_wait_ticks: 0,
            wait_target: 0,
            ipc_waiting: false,
            ipc_restart_pending: false,
            ipc_deadline_tick: 0,
            ipc_wake_reason: IpcWakeReason::None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub task_capacity: u64,
    pub task_slots_used: u64,
    pub ready_tasks: u64,
    pub running_tasks: u64,
    pub blocked_tasks: u64,
    pub exited_tasks: u64,
    pub zombie_children: u64,
    pub active_resources: u64,
    pub next_task_id: u64,
    pub spawned_tasks: u64,
    pub exited_total: u64,
    pub failed_spawns: u64,
    pub cleanup_successes: u64,
    pub cleanup_failures: u64,
    pub reclaimed_user_frames: u64,
    pub reclaimed_table_frames: u64,
    pub heap_releases: u64,
    pub last_task_id: u64,
    pub last_exit_code: u64,
    pub preemption_active: bool,
    pub preemption_runs: u64,
    pub preemption_passes: u64,
    pub reaped_total: u64,
    pub wait_blocks: u64,
    pub parent_wakeups: u64,
    pub ipc_blocking_switches: u64,
    pub ipc_restart_completions: u64,
    pub ipc_timeouts: u64,
    pub ipc_cancellations: u64,
}

#[derive(Clone, Copy)]
pub struct TaskRunResult {
    pub ran: bool,
    pub passed: bool,
    pub task_id: u64,
    pub state: TaskState,
    pub entry_point: u64,
    pub stack_top: u64,
    pub exit_code: u64,
    pub syscalls_before: u64,
    pub syscalls_after: u64,
    pub address_space_root: u64,
    pub first_user_frame: u64,
    pub owned_user_pages: u64,
    pub owned_table_frames: u64,
    pub cleanup_user_frames: u64,
    pub cleanup_table_frames: u64,
    pub heap_released: bool,
    pub resources_cleaned: bool,
}

impl TaskRunResult {
    fn not_run(entry_point: u64, stack_top: u64) -> Self {
        let syscalls = user::snapshot().syscall_count;
        Self {
            ran: false,
            passed: false,
            task_id: 0,
            state: TaskState::Empty,
            entry_point,
            stack_top,
            exit_code: 0,
            syscalls_before: syscalls,
            syscalls_after: syscalls,
            address_space_root: 0,
            first_user_frame: 0,
            owned_user_pages: 0,
            owned_table_frames: 0,
            cleanup_user_frames: 0,
            cleanup_table_frames: 0,
            heap_released: false,
            resources_cleaned: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct IsolationReport {
    pub spawned: bool,
    pub distinct_roots: bool,
    pub distinct_user_frames: bool,
    pub first: TaskRunResult,
    pub second: TaskRunResult,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

#[derive(Clone, Copy)]
pub struct PreemptionReport {
    pub ran: bool,
    pub passed: bool,
    pub first_task: u64,
    pub second_task: u64,
    pub timer_switches: u64,
    pub first_slices: u64,
    pub second_slices: u64,
    pub max_wait_ticks: u64,
    pub round_robin_balanced: bool,
    pub starvation_bounded: bool,
    pub distinct_roots: bool,
    pub distinct_user_frames: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
}

#[derive(Clone, Copy)]
pub struct WaitResult {
    pub found: bool,
    pub blocked: bool,
    pub reaped: bool,
    pub child_id: u64,
    pub exit_code: u64,
}

impl WaitResult {
    const fn missing() -> Self {
        Self {
            found: false,
            blocked: false,
            reaped: false,
            child_id: 0,
            exit_code: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ProcessTreeReport {
    pub parent_id: u64,
    pub child_id: u64,
    pub relation_registered: bool,
    pub parent_blocked: bool,
    pub parent_woken: bool,
    pub child_exited: bool,
    pub child_reaped: bool,
    pub child_exit_code: u64,
    pub parent_completed: bool,
    pub user_buffer_validation: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

#[derive(Clone, Copy)]
pub struct UserBufferValidation {
    pub valid: bool,
    pub pages_checked: u64,
    pub writable: bool,
}

#[derive(Clone, Copy)]
pub struct IpcTestReport {
    pub sender_id: u64,
    pub receiver_id: u64,
    pub queued_delivery: bool,
    pub receiver_blocked: bool,
    pub receiver_woken: bool,
    pub wake_delivery: bool,
    pub fifo_order: bool,
    pub backpressure: bool,
    pub endpoint_cleanup: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

#[derive(Clone, Copy)]
pub struct IpcHandoffReport {
    pub ran: bool,
    pub sender_id: u64,
    pub receiver_id: u64,
    pub exit_code: u64,
    pub blocking_switches: u64,
    pub restart_completions: u64,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub endpoints_cleaned: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

#[derive(Clone, Copy)]
pub struct CapabilityTestReport {
    pub owner_id: u64,
    pub target_id: u64,
    pub self_capability: bool,
    pub authorized_delivery: bool,
    pub invalid_denied: bool,
    pub permission_denied: bool,
    pub revoked_denied: bool,
    pub generation_advanced: bool,
    pub cleanup_revoked: bool,
    pub capability_baseline: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

#[derive(Clone, Copy)]
pub struct IpcWaitControlReport {
    pub timeout_ran: bool,
    pub timeout_exit_code: u64,
    pub timeout_wakeups: u64,
    pub cancellation_ran: bool,
    pub cancellation_exit_code: u64,
    pub cancellation_wakeups: u64,
    pub restart_completions: u64,
    pub scheduler_clean: bool,
    pub endpoint_clean: bool,
    pub frames_restored: bool,
    pub heap_restored: bool,
    pub resources_restored: bool,
    pub passed: bool,
}

struct ProcessSlot {
    task: Task,
    address_space: paging::AddressSpace,
    timer_context: interrupts::TimerContext,
    context_valid: bool,
}

impl ProcessSlot {
    const fn empty() -> Self {
        Self {
            task: Task::empty(),
            address_space: paging::AddressSpace::empty(),
            timer_context: interrupts::TimerContext::empty(),
            context_valid: false,
        }
    }
}

struct ProcessTable {
    initialized: bool,
    next_task_id: u64,
    spawned_tasks: u64,
    exited_total: u64,
    failed_spawns: u64,
    cleanup_successes: u64,
    cleanup_failures: u64,
    reclaimed_user_frames: u64,
    reclaimed_table_frames: u64,
    heap_releases: u64,
    active_resources: u64,
    last_task_id: u64,
    last_exit_code: u64,
    preemption_active: bool,
    preemption_target: u64,
    preemption_switches: u64,
    preemption_runs: u64,
    preemption_passes: u64,
    reaped_total: u64,
    wait_blocks: u64,
    parent_wakeups: u64,
    ipc_blocking_switches: u64,
    ipc_restart_completions: u64,
    ipc_timeouts: u64,
    ipc_cancellations: u64,
    slots: [ProcessSlot; MAX_TASKS],
}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            initialized: false,
            next_task_id: 1,
            spawned_tasks: 0,
            exited_total: 0,
            failed_spawns: 0,
            cleanup_successes: 0,
            cleanup_failures: 0,
            reclaimed_user_frames: 0,
            reclaimed_table_frames: 0,
            heap_releases: 0,
            active_resources: 0,
            last_task_id: 0,
            last_exit_code: 0,
            preemption_active: false,
            preemption_target: 0,
            preemption_switches: 0,
            preemption_runs: 0,
            preemption_passes: 0,
            reaped_total: 0,
            wait_blocks: 0,
            parent_wakeups: 0,
            ipc_blocking_switches: 0,
            ipc_restart_completions: 0,
            ipc_timeouts: 0,
            ipc_cancellations: 0,
            slots: [const { ProcessSlot::empty() }; MAX_TASKS],
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }

        self.initialized = true;
        serial::log("process", "task model ready");
        serial::log("process", "resource ownership ready");
        serial::log_u64("process", "task capacity", MAX_TASKS as u64);
    }

    fn snapshot(&self) -> Snapshot {
        let mut task_slots_used = 0u64;
        let mut ready_tasks = 0u64;
        let mut running_tasks = 0u64;
        let mut blocked_tasks = 0u64;
        let mut exited_tasks = 0u64;
        let mut zombie_children = 0u64;

        for slot in self.slots.iter() {
            match slot.task.state {
                TaskState::Empty => {}
                TaskState::Ready => {
                    task_slots_used = task_slots_used.saturating_add(1);
                    ready_tasks = ready_tasks.saturating_add(1);
                }
                TaskState::Running => {
                    task_slots_used = task_slots_used.saturating_add(1);
                    running_tasks = running_tasks.saturating_add(1);
                }
                TaskState::Blocked => {
                    task_slots_used = task_slots_used.saturating_add(1);
                    blocked_tasks = blocked_tasks.saturating_add(1);
                }
                TaskState::Exited => {
                    task_slots_used = task_slots_used.saturating_add(1);
                    exited_tasks = exited_tasks.saturating_add(1);
                    if slot.task.parent_id != 0 {
                        zombie_children = zombie_children.saturating_add(1);
                    }
                }
            }
        }

        Snapshot {
            initialized: self.initialized,
            task_capacity: MAX_TASKS as u64,
            task_slots_used,
            ready_tasks,
            running_tasks,
            blocked_tasks,
            exited_tasks,
            zombie_children,
            active_resources: self.active_resources,
            next_task_id: self.next_task_id,
            spawned_tasks: self.spawned_tasks,
            exited_total: self.exited_total,
            failed_spawns: self.failed_spawns,
            cleanup_successes: self.cleanup_successes,
            cleanup_failures: self.cleanup_failures,
            reclaimed_user_frames: self.reclaimed_user_frames,
            reclaimed_table_frames: self.reclaimed_table_frames,
            heap_releases: self.heap_releases,
            last_task_id: self.last_task_id,
            last_exit_code: self.last_exit_code,
            preemption_active: self.preemption_active,
            preemption_runs: self.preemption_runs,
            preemption_passes: self.preemption_passes,
            reaped_total: self.reaped_total,
            wait_blocks: self.wait_blocks,
            parent_wakeups: self.parent_wakeups,
            ipc_blocking_switches: self.ipc_blocking_switches,
            ipc_restart_completions: self.ipc_restart_completions,
            ipc_timeouts: self.ipc_timeouts,
            ipc_cancellations: self.ipc_cancellations,
        }
    }

    fn spawn_legacy(&mut self, entry_point: u64, stack_top: u64) -> Option<Task> {
        let heap_allocation = match heap::alloc(TASK_HEAP_BYTES, TASK_HEAP_ALIGNMENT) {
            Some(address) => address,
            None => {
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log("process", "task heap allocation failed");
                return None;
            }
        };

        let task = self.install_task(
            entry_point,
            stack_top,
            0,
            paging::AddressSpace::empty(),
            0,
            0,
            0,
            heap_allocation,
        );
        if task.is_none() {
            let _ = heap::free(heap_allocation);
        }
        task
    }

    fn spawn_elf_init(&mut self) -> Option<Task> {
        self.spawn_elf_child(0)
    }

    fn spawn_elf_child(&mut self, parent_id: u64) -> Option<Task> {
        if parent_id != 0 {
            let valid_parent = self
                .find_task(parent_id)
                .map(|index| {
                    !matches!(
                        self.slots[index].task.state,
                        TaskState::Empty | TaskState::Exited
                    )
                })
                .unwrap_or(false);
            if !valid_parent {
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log_u64("process", "invalid parent task", parent_id);
                return None;
            }
        }

        let mut image = match elf::create_process_image() {
            Ok(image) => image,
            Err(error) => {
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log("process", elf::load_error_name(error));
                return None;
            }
        };

        let heap_allocation = match heap::alloc(TASK_HEAP_BYTES, TASK_HEAP_ALIGNMENT) {
            Some(address) => address,
            None => {
                let _ = image.address_space.destroy();
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log("process", "task heap allocation failed");
                return None;
            }
        };

        let task = self.install_task(
            image.entry_point,
            image.stack_top,
            parent_id,
            image.address_space,
            image.first_user_frame,
            image.mapped_pages,
            image.table_frames,
            heap_allocation,
        );
        if task.is_none() {
            let _ = heap::free(heap_allocation);
        }
        task
    }

    fn spawn_preempt_task(&mut self) -> Option<Task> {
        let mut image = match elf::create_process_image() {
            Ok(image) => image,
            Err(error) => {
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log("process", elf::load_error_name(error));
                return None;
            }
        };

        let translation = image.address_space.translate(image.entry_point);
        if !translation.mapped || !translation.user_accessible || !translation.executable {
            let _ = image.address_space.destroy();
            self.failed_spawns = self.failed_spawns.saturating_add(1);
            serial::log("sched", "preemption program mapping invalid");
            return None;
        }

        unsafe {
            let destination = translation.phys as *mut u8;
            for (offset, byte) in SPIN_PROGRAM.iter().copied().enumerate() {
                destination.add(offset).write_volatile(byte);
            }
        }

        let heap_allocation = match heap::alloc(TASK_HEAP_BYTES, TASK_HEAP_ALIGNMENT) {
            Some(address) => address,
            None => {
                let _ = image.address_space.destroy();
                self.failed_spawns = self.failed_spawns.saturating_add(1);
                serial::log("process", "task heap allocation failed");
                return None;
            }
        };

        let task = self.install_task(
            image.entry_point,
            image.stack_top,
            0,
            image.address_space,
            image.first_user_frame,
            image.mapped_pages,
            image.table_frames,
            heap_allocation,
        );
        if task.is_none() {
            let _ = heap::free(heap_allocation);
        }
        task
    }

    fn install_ipc_handoff_program(
        &mut self,
        task_id: u64,
        peer_capability: u64,
        receiver_first: bool,
    ) -> bool {
        let mut program = UserCodeBuilder::new();
        if receiver_first {
            program.receive();
            program.send(peer_capability, IPC_TEST_DATA);
            program.receive();
            program.send(peer_capability, IPC_TEST_DATA + 4);
            program.receive();
            program.bytes(&[0xf4, 0xeb, 0xfd]);
        } else {
            program.send(peer_capability, IPC_TEST_DATA);
            program.receive();
            program.send(peer_capability, IPC_TEST_DATA + 4);
            program.receive();
            program.exit(IPC_TEST_EXIT_CODE);
            program.bytes(&[0xf4, 0xeb, 0xfd]);
        }
        self.write_user_program(task_id, &program)
    }

    fn install_ipc_timeout_program(&mut self, task_id: u64, receiver: bool) -> bool {
        let mut program = UserCodeBuilder::new();
        if receiver {
            program.receive_timeout(2);
            program.expect_rax_and_exit(
                crate::syscall::ipc_error_return(ipc::IpcError::Timeout),
                IPC_TEST_EXIT_CODE,
                IPC_TEST_EXIT_CODE + 1,
            );
        } else {
            program.wait_ticks(3);
            program.receive_timeout(100);
            program.bytes(&[0xf4, 0xeb, 0xfd]);
        }
        self.write_user_program(task_id, &program)
    }

    fn install_ipc_cancel_program(
        &mut self,
        task_id: u64,
        cancel_capability: u64,
        receiver: bool,
    ) -> bool {
        let mut program = UserCodeBuilder::new();
        if receiver {
            program.receive_timeout(100);
            program.expect_rax_and_exit(
                crate::syscall::ipc_error_return(ipc::IpcError::Cancelled),
                IPC_TEST_EXIT_CODE,
                IPC_TEST_EXIT_CODE + 1,
            );
        } else {
            program.cancel(cancel_capability);
            program.receive_timeout(100);
            program.bytes(&[0xf4, 0xeb, 0xfd]);
        }
        self.write_user_program(task_id, &program)
    }

    fn write_user_program(&mut self, task_id: u64, program: &UserCodeBuilder) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        let entry_point = self.slots[index].task.entry_point;
        let code = self.slots[index].address_space.translate(entry_point);
        let data = self.slots[index].address_space.translate(IPC_TEST_DATA);
        if !code.mapped
            || !code.user_accessible
            || !code.executable
            || data.phys == 0
            || !data.mapped
            || !data.user_accessible
            || !data.writable
        {
            return false;
        }
        if !program.valid || program.len > paging::PAGE_SIZE_4K as usize {
            return false;
        }

        unsafe {
            let destination = code.phys as *mut u8;
            core::ptr::write_bytes(destination, 0, paging::PAGE_SIZE_4K as usize);
            for (offset, byte) in program.bytes[..program.len].iter().copied().enumerate() {
                destination.add(offset).write_volatile(byte);
            }

            let data_destination = data.phys as *mut u8;
            for (offset, byte) in b"pingpongdoneSTOP".iter().copied().enumerate() {
                data_destination.add(offset).write_volatile(byte);
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn install_task(
        &mut self,
        entry_point: u64,
        stack_top: u64,
        parent_id: u64,
        address_space: paging::AddressSpace,
        first_user_frame: u64,
        owned_user_pages: u64,
        owned_table_frames: u64,
        heap_allocation: u64,
    ) -> Option<Task> {
        if !self.initialized {
            self.init();
        }

        let Some(index) = self.free_slot() else {
            let mut address_space = address_space;
            let _ = address_space.destroy();
            self.failed_spawns = self.failed_spawns.saturating_add(1);
            serial::log("process", "task table is full");
            return None;
        };

        let id = self.next_task_id;
        self.next_task_id = self.next_task_id.saturating_add(1);
        if let Err(error) = ipc::register_endpoint(id) {
            let mut address_space = address_space;
            let _ = address_space.destroy();
            self.failed_spawns = self.failed_spawns.saturating_add(1);
            serial::log("process", ipc::error_name(error));
            return None;
        }
        let task = Task {
            id,
            parent_id,
            state: TaskState::Ready,
            entry_point,
            stack_top,
            exit_code: 0,
            syscalls_before: 0,
            syscalls_after: 0,
            runs: 0,
            address_space_root: address_space.root_frame(),
            first_user_frame,
            owned_user_pages,
            owned_table_frames,
            heap_allocation,
            resources_active: true,
            cleanup_complete: false,
            cleanup_user_frames: 0,
            cleanup_table_frames: 0,
            heap_released: false,
            timer_preemptions: 0,
            scheduled_slices: 0,
            last_scheduled_tick: 0,
            max_wait_ticks: 0,
            wait_target: 0,
            ipc_waiting: false,
            ipc_restart_pending: false,
            ipc_deadline_tick: 0,
            ipc_wake_reason: IpcWakeReason::None,
        };

        self.slots[index] = ProcessSlot {
            task,
            address_space,
            timer_context: initial_timer_context(entry_point, stack_top),
            context_valid: true,
        };
        self.spawned_tasks = self.spawned_tasks.saturating_add(1);
        self.active_resources = self.active_resources.saturating_add(1);
        self.last_task_id = id;
        serial::log_u64("process", "task spawned", id);
        serial::log_u64("process", "parent task", parent_id);
        serial::log_hex_u64("process", "address space root", task.address_space_root);
        scheduler::enqueue_task(id);
        Some(task)
    }

    fn mark_running(&mut self, id: u64, syscalls_before: u64) -> bool {
        let Some(index) = self.find_task(id) else {
            return false;
        };

        let task = &mut self.slots[index].task;
        task.state = TaskState::Running;
        task.syscalls_before = syscalls_before;
        task.runs = task.runs.saturating_add(1);
        task.scheduled_slices = task.scheduled_slices.saturating_add(1);
        task.last_scheduled_tick = interrupts::ticks();
        self.last_task_id = id;
        serial::log_u64("process", "task running", id);
        scheduler::begin_task(id);
        true
    }

    fn mark_exited(&mut self, id: u64, exit_code: u64, syscalls_after: u64) -> bool {
        let Some(index) = self.find_task(id) else {
            return false;
        };

        let parent_id = self.slots[index].task.parent_id;
        self.slots[index].task.state = TaskState::Exited;
        self.slots[index].task.exit_code = exit_code;
        self.slots[index].task.syscalls_after = syscalls_after;
        self.slots[index].task.ipc_waiting = false;
        self.slots[index].task.ipc_restart_pending = false;
        self.slots[index].task.ipc_deadline_tick = 0;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::None;
        self.exited_total = self.exited_total.saturating_add(1);
        self.last_task_id = id;
        self.last_exit_code = exit_code;
        serial::log_u64("process", "task exited", id);
        serial::log_u64("process", "exit code", exit_code);
        scheduler::finish_task(id);
        if parent_id != 0 {
            self.wake_waiting_parent(parent_id, id);
        }
        true
    }

    fn cleanup_task(&mut self, id: u64) -> bool {
        let Some(index) = self.find_task(id) else {
            return false;
        };
        if !self.slots[index].task.resources_active {
            return self.slots[index].task.cleanup_complete;
        }

        let heap_address = self.slots[index].task.heap_allocation;
        let heap_released = heap_address != 0 && heap::free(heap_address).is_ok();
        let endpoint_released = ipc::unregister_endpoint(id).is_ok();
        let cleanup = self.slots[index].address_space.destroy();
        let (user_frames, table_frames, pages_complete) = match cleanup {
            Ok(report) => (
                report.user_frames_freed,
                report.table_frames_freed,
                report.complete,
            ),
            Err(error) => {
                serial::log("process", paging::address_space_error_name(error));
                (0, 0, false)
            }
        };
        let complete = heap_released && endpoint_released && pages_complete;

        let task = &mut self.slots[index].task;
        task.cleanup_user_frames = user_frames;
        task.cleanup_table_frames = table_frames;
        task.heap_released = heap_released;
        task.cleanup_complete = complete;
        task.resources_active = false;
        if heap_released {
            task.heap_allocation = 0;
        }

        self.active_resources = self.active_resources.saturating_sub(1);
        self.reclaimed_user_frames = self.reclaimed_user_frames.saturating_add(user_frames);
        self.reclaimed_table_frames = self.reclaimed_table_frames.saturating_add(table_frames);
        if heap_released {
            self.heap_releases = self.heap_releases.saturating_add(1);
        }
        if complete {
            self.cleanup_successes = self.cleanup_successes.saturating_add(1);
            serial::log_u64("process", "task resources reclaimed", id);
        } else {
            self.cleanup_failures = self.cleanup_failures.saturating_add(1);
            serial::log_u64("process", "task cleanup failed", id);
        }
        complete
    }

    fn start_preemption_test(&mut self) {
        self.preemption_active = true;
        self.preemption_target = PREEMPT_TEST_SWITCHES;
        self.preemption_switches = 0;
        self.preemption_runs = self.preemption_runs.saturating_add(1);
    }

    fn stop_preemption_test(&mut self, passed: bool) -> u64 {
        self.preemption_active = false;
        self.preemption_target = 0;
        let switches = self.preemption_switches;
        if passed {
            self.preemption_passes = self.preemption_passes.saturating_add(1);
        }
        switches
    }

    fn on_timer_interrupt(&mut self, context: &mut interrupts::TimerContext) -> TimerAction {
        if !self.preemption_active || !context.from_user() {
            return TimerAction::Continue;
        }

        if self.preemption_switches >= self.preemption_target {
            return TimerAction::Stop(PREEMPT_TEST_EXIT_CODE);
        }

        let decision = scheduler::preempt_current_from_irq();
        if !decision.switched {
            return TimerAction::Continue;
        }

        let Some(previous_index) = self.find_task(decision.previous_task) else {
            self.preemption_active = false;
            return TimerAction::Stop(PREEMPT_TEST_EXIT_CODE + 1);
        };
        let Some(next_index) = self.find_task(decision.next_task) else {
            self.preemption_active = false;
            return TimerAction::Stop(PREEMPT_TEST_EXIT_CODE + 2);
        };

        self.slots[previous_index].timer_context = *context;
        self.slots[previous_index].context_valid = true;
        self.slots[previous_index].task.state = TaskState::Ready;
        self.slots[previous_index].task.timer_preemptions = self.slots[previous_index]
            .task
            .timer_preemptions
            .saturating_add(1);

        if !self.slots[next_index].context_valid
            || self.slots[next_index].task.address_space_root == 0
        {
            self.preemption_active = false;
            return TimerAction::Stop(PREEMPT_TEST_EXIT_CODE + 3);
        }

        let next_task = &mut self.slots[next_index].task;
        next_task.state = TaskState::Running;
        next_task.runs = next_task.runs.saturating_add(1);
        next_task.scheduled_slices = next_task.scheduled_slices.saturating_add(1);
        next_task.last_scheduled_tick = interrupts::ticks();
        next_task.max_wait_ticks = next_task.max_wait_ticks.max(decision.waited_ticks);

        self.preemption_switches = self.preemption_switches.saturating_add(1);
        self.last_task_id = decision.next_task;
        *context = self.slots[next_index].timer_context;
        TimerAction::Switch(self.slots[next_index].task.address_space_root)
    }

    fn wait_child(&mut self, parent_id: u64, child_id: u64) -> WaitResult {
        let Some(parent_index) = self.find_task(parent_id) else {
            return WaitResult::missing();
        };
        if matches!(
            self.slots[parent_index].task.state,
            TaskState::Empty | TaskState::Exited
        ) {
            return WaitResult::missing();
        }

        let Some(child_index) = self.find_child(parent_id, child_id) else {
            return WaitResult::missing();
        };
        let child = self.slots[child_index].task;
        if child.state == TaskState::Exited && !child.resources_active && child.cleanup_complete {
            let result = WaitResult {
                found: true,
                blocked: false,
                reaped: true,
                child_id: child.id,
                exit_code: child.exit_code,
            };
            self.slots[child_index] = ProcessSlot::empty();
            self.reaped_total = self.reaped_total.saturating_add(1);
            serial::log_u64("process", "child reaped", child.id);
            serial::log_u64("process", "wait exit code", child.exit_code);
            return result;
        }

        if child.state != TaskState::Exited && scheduler::block_task(parent_id) {
            self.slots[parent_index].task.state = TaskState::Blocked;
            self.slots[parent_index].task.wait_target = child.id;
            self.wait_blocks = self.wait_blocks.saturating_add(1);
            serial::log_u64("process", "parent blocked", parent_id);
            return WaitResult {
                found: true,
                blocked: true,
                reaped: false,
                child_id: child.id,
                exit_code: 0,
            };
        }

        WaitResult {
            found: true,
            blocked: false,
            reaped: false,
            child_id: child.id,
            exit_code: child.exit_code,
        }
    }

    fn wake_waiting_parent(&mut self, parent_id: u64, child_id: u64) -> bool {
        let Some(parent_index) = self.find_task(parent_id) else {
            return false;
        };
        let parent = self.slots[parent_index].task;
        if parent.state != TaskState::Blocked
            || (parent.wait_target != 0 && parent.wait_target != child_id)
        {
            return false;
        }
        if !scheduler::wake_task(parent_id) {
            return false;
        }

        self.slots[parent_index].task.state = TaskState::Ready;
        self.slots[parent_index].task.wait_target = 0;
        self.parent_wakeups = self.parent_wakeups.saturating_add(1);
        serial::log_u64("process", "parent woken", parent_id);
        true
    }

    fn block_for_ipc(&mut self, task_id: u64) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        let task = self.slots[index].task;
        if !matches!(task.state, TaskState::Ready | TaskState::Running)
            || task.wait_target != 0
            || task.ipc_waiting
        {
            return false;
        }
        if !scheduler::block_task(task_id) {
            return false;
        }

        self.slots[index].task.state = TaskState::Blocked;
        self.slots[index].task.ipc_waiting = true;
        self.slots[index].task.ipc_deadline_tick = 0;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::None;
        serial::log_u64("ipc", "receiver blocked", task_id);
        true
    }

    fn block_for_ipc_syscall(
        &mut self,
        task_id: u64,
        frame: &mut user::SyscallFrame,
        deadline_tick: u64,
    ) -> Option<u64> {
        let previous_index = self.find_task(task_id)?;
        let previous = self.slots[previous_index].task;
        if previous.state != TaskState::Running
            || previous.wait_target != 0
            || previous.ipc_waiting
            || previous.ipc_restart_pending
        {
            return None;
        }

        save_syscall_context(&mut self.slots[previous_index].timer_context, frame, true);
        self.slots[previous_index].context_valid = true;
        let decision = scheduler::block_current_and_dispatch(task_id);
        if !decision.switched || decision.previous_task != task_id {
            return None;
        }

        let Some(next_index) = self.find_task(decision.next_task) else {
            return None;
        };
        if !self.slots[next_index].context_valid
            || self.slots[next_index].task.address_space_root == 0
        {
            return None;
        }

        self.slots[previous_index].task.state = TaskState::Blocked;
        self.slots[previous_index].task.ipc_waiting = true;
        self.slots[previous_index].task.ipc_restart_pending = true;
        self.slots[previous_index].task.ipc_deadline_tick = deadline_tick;
        self.slots[previous_index].task.ipc_wake_reason = IpcWakeReason::None;

        let next_task = &mut self.slots[next_index].task;
        next_task.state = TaskState::Running;
        next_task.runs = next_task.runs.saturating_add(1);
        next_task.scheduled_slices = next_task.scheduled_slices.saturating_add(1);
        next_task.last_scheduled_tick = interrupts::ticks();
        next_task.max_wait_ticks = next_task.max_wait_ticks.max(decision.waited_ticks);

        load_syscall_context(frame, &self.slots[next_index].timer_context);
        self.ipc_blocking_switches = self.ipc_blocking_switches.saturating_add(1);
        self.last_task_id = decision.next_task;
        serial::log_u64("ipc", "blocking syscall switched from", task_id);
        serial::log_u64("ipc", "blocking syscall switched to", decision.next_task);
        Some(self.slots[next_index].task.address_space_root)
    }

    fn wake_from_ipc(&mut self, task_id: u64) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        if self.slots[index].task.state != TaskState::Blocked || !self.slots[index].task.ipc_waiting
        {
            return false;
        }
        if !scheduler::wake_task(task_id) {
            return false;
        }

        self.slots[index].task.state = TaskState::Ready;
        self.slots[index].task.ipc_waiting = false;
        self.slots[index].task.ipc_deadline_tick = 0;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::Message;
        serial::log_u64("ipc", "receiver woken", task_id);
        true
    }

    fn cancel_ipc_wait(&mut self, task_id: u64) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        if self.slots[index].task.state != TaskState::Blocked || !self.slots[index].task.ipc_waiting
        {
            return false;
        }
        if !scheduler::wake_task(task_id) {
            return false;
        }

        self.slots[index].task.state = TaskState::Ready;
        self.slots[index].task.ipc_waiting = false;
        self.slots[index].task.ipc_deadline_tick = 0;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::Cancelled;
        self.ipc_cancellations = self.ipc_cancellations.saturating_add(1);
        serial::log_u64("ipc", "receiver cancelled", task_id);
        true
    }

    fn expire_ipc_deadlines(&mut self, now: u64) {
        for index in 0..MAX_TASKS {
            let task = self.slots[index].task;
            if task.state != TaskState::Blocked
                || !task.ipc_waiting
                || task.ipc_deadline_tick == 0
                || now < task.ipc_deadline_tick
            {
                continue;
            }
            if !scheduler::wake_task(task.id) {
                continue;
            }

            let _ = ipc::clear_waiting(task.id);
            self.slots[index].task.state = TaskState::Ready;
            self.slots[index].task.ipc_waiting = false;
            self.slots[index].task.ipc_deadline_tick = 0;
            self.slots[index].task.ipc_wake_reason = IpcWakeReason::Timeout;
            self.ipc_timeouts = self.ipc_timeouts.saturating_add(1);
            serial::log_u64("ipc", "receiver timeout", task.id);
        }
    }

    fn take_ipc_wake_reason(&mut self, task_id: u64) -> IpcWakeReason {
        let Some(index) = self.find_task(task_id) else {
            return IpcWakeReason::None;
        };
        let reason = self.slots[index].task.ipc_wake_reason;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::None;
        if matches!(reason, IpcWakeReason::Timeout | IpcWakeReason::Cancelled)
            && self.slots[index].task.ipc_restart_pending
        {
            self.slots[index].task.ipc_restart_pending = false;
            self.ipc_restart_completions = self.ipc_restart_completions.saturating_add(1);
        }
        reason
    }

    fn complete_ipc_restart(&mut self, task_id: u64) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        if !self.slots[index].task.ipc_restart_pending {
            return false;
        }

        self.slots[index].task.ipc_restart_pending = false;
        self.slots[index].task.ipc_deadline_tick = 0;
        self.slots[index].task.ipc_wake_reason = IpcWakeReason::None;
        self.ipc_restart_completions = self.ipc_restart_completions.saturating_add(1);
        serial::log_u64("ipc", "blocking syscall resumed", task_id);
        true
    }

    fn find_child(&self, parent_id: u64, child_id: u64) -> Option<usize> {
        let mut first_match = None;
        for index in 0..MAX_TASKS {
            let task = self.slots[index].task;
            if task.state == TaskState::Empty
                || task.parent_id != parent_id
                || (child_id != 0 && task.id != child_id)
            {
                continue;
            }
            if task.state == TaskState::Exited && !task.resources_active {
                return Some(index);
            }
            if first_match.is_none() {
                first_match = Some(index);
            }
        }
        first_match
    }

    fn child_count(&self, parent_id: u64) -> u64 {
        self.slots
            .iter()
            .filter(|slot| slot.task.state != TaskState::Empty && slot.task.parent_id == parent_id)
            .count() as u64
    }

    fn validate_user_buffer(
        &self,
        task_id: u64,
        address: u64,
        length: u64,
        writable: bool,
    ) -> UserBufferValidation {
        let invalid = UserBufferValidation {
            valid: false,
            pages_checked: 0,
            writable,
        };
        let Some(index) = self.find_task(task_id) else {
            return invalid;
        };
        if self.slots[index].task.address_space_root == 0 || length > MAX_USER_BUFFER_BYTES {
            return invalid;
        }
        if length == 0 {
            return UserBufferValidation {
                valid: true,
                pages_checked: 0,
                writable,
            };
        }
        let Some(last_byte) = address.checked_add(length - 1) else {
            return invalid;
        };

        let page_mask = !(paging::PAGE_SIZE_4K - 1);
        let mut page = address & page_mask;
        let final_page = last_byte & page_mask;
        let mut pages_checked = 0u64;
        loop {
            let translation = self.slots[index].address_space.translate(page);
            pages_checked = pages_checked.saturating_add(1);
            if !translation.mapped
                || !translation.user_accessible
                || (writable && !translation.writable)
            {
                return UserBufferValidation {
                    valid: false,
                    pages_checked,
                    writable,
                };
            }
            if page == final_page {
                break;
            }
            let Some(next) = page.checked_add(paging::PAGE_SIZE_4K) else {
                return UserBufferValidation {
                    valid: false,
                    pages_checked,
                    writable,
                };
            };
            page = next;
        }

        UserBufferValidation {
            valid: true,
            pages_checked,
            writable,
        }
    }

    fn copy_from_user(&self, task_id: u64, address: u64, output: &mut [u8]) -> bool {
        if !self
            .validate_user_buffer(task_id, address, output.len() as u64, false)
            .valid
        {
            return false;
        }
        self.copy_user_bytes(task_id, address, output.as_mut_ptr(), output.len(), false)
    }

    fn copy_to_user(&self, task_id: u64, address: u64, input: &[u8]) -> bool {
        if !self
            .validate_user_buffer(task_id, address, input.len() as u64, true)
            .valid
        {
            return false;
        }
        self.copy_user_bytes(
            task_id,
            address,
            input.as_ptr() as *mut u8,
            input.len(),
            true,
        )
    }

    fn copy_user_bytes(
        &self,
        task_id: u64,
        address: u64,
        buffer: *mut u8,
        length: usize,
        to_user: bool,
    ) -> bool {
        let Some(index) = self.find_task(task_id) else {
            return false;
        };
        let mut copied = 0usize;
        while copied < length {
            let virtual_address = address.saturating_add(copied as u64);
            let translation = self.slots[index].address_space.translate(virtual_address);
            if !translation.mapped
                || !translation.user_accessible
                || (to_user && !translation.writable)
            {
                return false;
            }
            let page_offset = (virtual_address & (paging::PAGE_SIZE_4K - 1)) as usize;
            let chunk = (paging::PAGE_SIZE_4K as usize - page_offset).min(length - copied);
            unsafe {
                if to_user {
                    core::ptr::copy_nonoverlapping(
                        buffer.add(copied) as *const u8,
                        translation.phys as *mut u8,
                        chunk,
                    );
                } else {
                    core::ptr::copy_nonoverlapping(
                        translation.phys as *const u8,
                        buffer.add(copied),
                        chunk,
                    );
                }
            }
            copied += chunk;
        }
        true
    }

    fn free_slot(&self) -> Option<usize> {
        for index in 0..MAX_TASKS {
            let task = self.slots[index].task;
            if task.state == TaskState::Empty
                || (task.state == TaskState::Exited
                    && !task.resources_active
                    && task.parent_id == 0)
            {
                return Some(index);
            }
        }
        None
    }

    fn find_task(&self, id: u64) -> Option<usize> {
        for index in 0..MAX_TASKS {
            if self.slots[index].task.state != TaskState::Empty && self.slots[index].task.id == id {
                return Some(index);
            }
        }
        None
    }

    fn task(&self, index: usize) -> Option<Task> {
        if index >= MAX_TASKS || self.slots[index].task.state == TaskState::Empty {
            return None;
        }
        Some(self.slots[index].task)
    }

    fn task_by_id(&self, id: u64) -> Option<Task> {
        self.find_task(id).map(|index| self.slots[index].task)
    }
}

struct ProcessStore {
    value: UnsafeCell<ProcessTable>,
}

unsafe impl Sync for ProcessStore {}

static PROCESS_TABLE: ProcessStore = ProcessStore {
    value: UnsafeCell::new(ProcessTable::new()),
};

pub fn init() -> Snapshot {
    cpu_interrupts::without_interrupts(|| table_mut().init());
    snapshot()
}

pub fn snapshot() -> Snapshot {
    cpu_interrupts::without_interrupts(|| table().snapshot())
}

pub fn task(index: usize) -> Option<Task> {
    cpu_interrupts::without_interrupts(|| table().task(index))
}

pub fn task_by_id(id: u64) -> Option<Task> {
    cpu_interrupts::without_interrupts(|| table().task_by_id(id))
}

pub fn child_count(parent_id: u64) -> u64 {
    cpu_interrupts::without_interrupts(|| table().child_count(parent_id))
}

pub fn wait_child(parent_id: u64, child_id: u64) -> WaitResult {
    cpu_interrupts::without_interrupts(|| table_mut().wait_child(parent_id, child_id))
}

pub fn validate_user_buffer(
    task_id: u64,
    address: u64,
    length: u64,
    writable: bool,
) -> UserBufferValidation {
    cpu_interrupts::without_interrupts(|| {
        table().validate_user_buffer(task_id, address, length, writable)
    })
}

pub fn copy_from_user(task_id: u64, address: u64, output: &mut [u8]) -> bool {
    cpu_interrupts::without_interrupts(|| table().copy_from_user(task_id, address, output))
}

pub fn copy_to_user(task_id: u64, address: u64, input: &[u8]) -> bool {
    cpu_interrupts::without_interrupts(|| table().copy_to_user(task_id, address, input))
}

pub fn on_timer_interrupt(context: &mut interrupts::TimerContext) -> TimerAction {
    table_mut().on_timer_interrupt(context)
}

pub fn on_timer_tick(now: u64) {
    table_mut().expire_ipc_deadlines(now);
}

pub fn block_for_ipc(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().block_for_ipc(task_id))
}

pub fn block_for_ipc_syscall(
    task_id: u64,
    frame: &mut user::SyscallFrame,
    deadline_tick: u64,
) -> Option<u64> {
    cpu_interrupts::without_interrupts(|| {
        table_mut().block_for_ipc_syscall(task_id, frame, deadline_tick)
    })
}

pub fn wake_from_ipc(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().wake_from_ipc(task_id))
}

pub fn complete_ipc_restart(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().complete_ipc_restart(task_id))
}

pub fn cancel_ipc_wait(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().cancel_ipc_wait(task_id))
}

pub fn take_ipc_wake_reason(task_id: u64) -> IpcWakeReason {
    cpu_interrupts::without_interrupts(|| table_mut().take_ipc_wake_reason(task_id))
}

pub fn run_user_probe_task() -> TaskRunResult {
    let task = cpu_interrupts::without_interrupts(|| {
        table_mut().spawn_legacy(user::probe_entry_point(), user::probe_stack_top())
    });
    run_spawned_task(
        task,
        user::probe_entry_point(),
        user::probe_stack_top(),
        user::probe_expected_exit_code(),
        4,
    )
}

pub fn run_user_fault_task() -> TaskRunResult {
    let expected_exit = user::fault_exit_code(14);
    let task = cpu_interrupts::without_interrupts(|| {
        table_mut().spawn_legacy(user::fault_entry_point(), user::probe_stack_top())
    });
    let mut result = run_spawned_task(
        task,
        user::fault_entry_point(),
        user::probe_stack_top(),
        expected_exit,
        0,
    );
    result.passed = result.passed
        && result.exit_code == expected_exit
        && user::snapshot().last_fault_vector == 14;
    result
}

pub fn run_elf_init_task() -> TaskRunResult {
    let metadata = elf::loaded_image();
    let fallback_entry = metadata.map(|image| image.entry_point).unwrap_or(0);
    let fallback_stack = metadata.map(|image| image.stack_top).unwrap_or(0);
    let task = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    run_spawned_task(
        task,
        fallback_entry,
        fallback_stack,
        user_program::INIT_EXPECTED_EXIT_CODE,
        user_program::INIT_MINIMUM_SYSCALLS,
    )
}

pub fn run_isolation_test() -> IsolationReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let first_task = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let second_task = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());

    let spawned = first_task.is_some() && second_task.is_some();
    let first_root = first_task.map(|task| task.address_space_root).unwrap_or(0);
    let second_root = second_task.map(|task| task.address_space_root).unwrap_or(0);
    let first_frame = first_task.map(|task| task.first_user_frame).unwrap_or(0);
    let second_frame = second_task.map(|task| task.first_user_frame).unwrap_or(0);
    let distinct_roots = spawned && first_root != 0 && first_root != second_root;
    let distinct_user_frames = spawned && first_frame != 0 && first_frame != second_frame;

    let first = run_spawned_task(
        first_task,
        0,
        0,
        user_program::INIT_EXPECTED_EXIT_CODE,
        user_program::INIT_MINIMUM_SYSCALLS,
    );
    let second = run_spawned_task(
        second_task,
        0,
        0,
        user_program::INIT_EXPECTED_EXIT_CODE,
        user_program::INIT_MINIMUM_SYSCALLS,
    );
    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let passed = spawned
        && distinct_roots
        && distinct_user_frames
        && first.passed
        && second.passed
        && frames_restored
        && heap_restored
        && resources_restored;

    IsolationReport {
        spawned,
        distinct_roots,
        distinct_user_frames,
        first,
        second,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_process_tree_test() -> ProcessTreeReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let process_before = snapshot();
    let scheduler_before = scheduler::snapshot();

    let parent = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let child = match parent {
        Some(parent) => {
            cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_child(parent.id))
        }
        None => None,
    };
    let parent_id = parent.map(|task| task.id).unwrap_or(0);
    let child_id = child.map(|task| task.id).unwrap_or(0);
    let relation_registered = parent_id != 0
        && child_id != 0
        && child
            .map(|task| task.parent_id == parent_id)
            .unwrap_or(false)
        && child_count(parent_id) == 1;
    let user_buffer_validation = parent
        .map(|task| {
            let code = validate_user_buffer(task.id, task.entry_point, 4, false);
            let stack = validate_user_buffer(task.id, task.stack_top - 16, 16, true);
            let kernel = validate_user_buffer(task.id, 0x000b_8000, 16, false);
            let overflow = validate_user_buffer(task.id, u64::MAX - 7, 16, false);
            code.valid
                && code.pages_checked == 1
                && stack.valid
                && stack.writable
                && !kernel.valid
                && !overflow.valid
        })
        .unwrap_or(false);

    let initial_wait = wait_child(parent_id, child_id);
    let scheduler_blocked = scheduler::snapshot();
    let parent_blocked = initial_wait.found
        && initial_wait.blocked
        && task_by_id(parent_id)
            .map(|task| task.state == TaskState::Blocked && task.wait_target == child_id)
            .unwrap_or(false)
        && scheduler_blocked.blocked_tasks == scheduler_before.blocked_tasks.saturating_add(1);

    let child_result = run_spawned_task(
        child,
        0,
        0,
        user_program::INIT_EXPECTED_EXIT_CODE,
        user_program::INIT_MINIMUM_SYSCALLS,
    );
    let scheduler_woken = scheduler::snapshot();
    let parent_woken = task_by_id(parent_id)
        .map(|task| task.state == TaskState::Ready && task.wait_target == 0)
        .unwrap_or(false)
        && scheduler_woken.blocked_tasks == scheduler_before.blocked_tasks
        && scheduler_woken.wake_events > scheduler_before.wake_events;

    let completed_wait = wait_child(parent_id, child_id);
    let child_reaped = completed_wait.reaped
        && completed_wait.child_id == child_id
        && completed_wait.exit_code == user_program::INIT_EXPECTED_EXIT_CODE
        && task_by_id(child_id).is_none();
    let parent_result = run_spawned_task(
        parent,
        0,
        0,
        user_program::INIT_EXPECTED_EXIT_CODE,
        user_program::INIT_MINIMUM_SYSCALLS,
    );

    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let process_after = snapshot();
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = process_after.active_resources == resources_before;
    let passed = relation_registered
        && parent_blocked
        && child_result.passed
        && parent_woken
        && child_reaped
        && parent_result.passed
        && user_buffer_validation
        && process_after.reaped_total > process_before.reaped_total
        && process_after.wait_blocks > process_before.wait_blocks
        && process_after.parent_wakeups > process_before.parent_wakeups
        && frames_restored
        && heap_restored
        && resources_restored;

    serial::log_bool("process", "parent child relation", relation_registered);
    serial::log_bool("process", "parent blocked on wait", parent_blocked);
    serial::log_bool("process", "parent wakeup", parent_woken);
    serial::log_bool("process", "child reaped", child_reaped);
    serial::log_bool("process", "user buffer validation", user_buffer_validation);
    serial::log_bool("process", "process tree test", passed);

    ProcessTreeReport {
        parent_id,
        child_id,
        relation_registered,
        parent_blocked,
        parent_woken,
        child_exited: child_result.passed,
        child_reaped,
        child_exit_code: completed_wait.exit_code,
        parent_completed: parent_result.passed,
        user_buffer_validation,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_ipc_test() -> IpcTestReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let ipc_before = ipc::snapshot();
    let scheduler_before = scheduler::snapshot();
    let sender = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let receiver = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let sender_id = sender.map(|task| task.id).unwrap_or(0);
    let receiver_id = receiver.map(|task| task.id).unwrap_or(0);

    let mut output = [0u8; ipc::MAX_MESSAGE_BYTES];
    let queued_delivery = ipc::send(sender_id, receiver_id, b"ping").is_ok()
        && matches!(
            ipc::receive(receiver_id, &mut output, false),
            Ok(ipc::ReceiveOutcome::Message(ipc::Delivery {
                sender,
                length: 4,
                ..
            })) if sender == sender_id
        )
        && &output[..4] == b"ping";

    output.fill(0);
    let blocked_outcome = ipc::receive(receiver_id, &mut output, true);
    let receiver_blocked = matches!(blocked_outcome, Ok(ipc::ReceiveOutcome::Blocked))
        && task_by_id(receiver_id)
            .map(|task| task.state == TaskState::Blocked && task.ipc_waiting)
            .unwrap_or(false)
        && scheduler::snapshot().blocked_tasks == scheduler_before.blocked_tasks.saturating_add(1);
    let wake_send = ipc::send(sender_id, receiver_id, b"wake").is_ok();
    let receiver_woken = wake_send
        && task_by_id(receiver_id)
            .map(|task| task.state == TaskState::Ready && !task.ipc_waiting)
            .unwrap_or(false)
        && scheduler::snapshot().blocked_tasks == scheduler_before.blocked_tasks;
    let wake_delivery = matches!(
        ipc::receive(receiver_id, &mut output, false),
        Ok(ipc::ReceiveOutcome::Message(ipc::Delivery {
            sender,
            length: 4,
            ..
        })) if sender == sender_id
    ) && &output[..4] == b"wake";

    let mut filled = true;
    for value in 0..ipc::QUEUE_DEPTH {
        if ipc::send(sender_id, receiver_id, &[value as u8]).is_err() {
            filled = false;
        }
    }
    let backpressure =
        filled && ipc::send(sender_id, receiver_id, b"overflow") == Err(ipc::IpcError::QueueFull);
    let mut fifo_order = true;
    for expected in 0..ipc::QUEUE_DEPTH {
        output.fill(0);
        match ipc::receive(receiver_id, &mut output, false) {
            Ok(ipc::ReceiveOutcome::Message(delivery))
                if delivery.sender == sender_id
                    && delivery.length == 1
                    && output[0] == expected as u8 => {}
            _ => fifo_order = false,
        }
    }

    let syscalls = user::snapshot().syscall_count;
    if sender_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(sender_id, 0, syscalls);
            let _ = table_mut().cleanup_task(sender_id);
        });
    }
    if receiver_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(receiver_id, 0, syscalls);
            let _ = table_mut().cleanup_task(receiver_id);
        });
    }

    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;
    let ipc_after = ipc::snapshot();
    let endpoint_cleanup = ipc_after.active_endpoints == ipc_before.active_endpoints
        && ipc_after.active_capabilities == ipc_before.active_capabilities
        && ipc_after.queued_messages == ipc_before.queued_messages
        && ipc_after.waiting_receivers == ipc_before.waiting_receivers;
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let passed = sender_id != 0
        && receiver_id != 0
        && queued_delivery
        && receiver_blocked
        && receiver_woken
        && wake_delivery
        && fifo_order
        && backpressure
        && endpoint_cleanup
        && frames_restored
        && heap_restored
        && resources_restored
        && ipc::selftest();

    serial::log_bool("ipc", "queued delivery", queued_delivery);
    serial::log_bool("ipc", "receiver blocked", receiver_blocked);
    serial::log_bool("ipc", "receiver woken", receiver_woken);
    serial::log_bool("ipc", "wake delivery", wake_delivery);
    serial::log_bool("ipc", "fifo order", fifo_order);
    serial::log_bool("ipc", "backpressure", backpressure);
    serial::log_bool("ipc", "endpoint cleanup", endpoint_cleanup);
    serial::log_bool("ipc", "ipc test", passed);

    IpcTestReport {
        sender_id,
        receiver_id,
        queued_delivery,
        receiver_blocked,
        receiver_woken,
        wake_delivery,
        fifo_order,
        backpressure,
        endpoint_cleanup,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_ipc_handoff_test() -> IpcHandoffReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let process_before = snapshot();
    let ipc_before = ipc::snapshot();
    let scheduler_before = scheduler::snapshot();
    let receiver = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let sender = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let receiver_id = receiver.map(|task| task.id).unwrap_or(0);
    let sender_id = sender.map(|task| task.id).unwrap_or(0);

    let receiver_to_sender = if receiver_id != 0 && sender_id != 0 {
        ipc::grant_capability(receiver_id, sender_id, ipc::CapabilityRights::SEND).ok()
    } else {
        None
    };
    let sender_to_receiver = if receiver_id != 0 && sender_id != 0 {
        ipc::grant_capability(sender_id, receiver_id, ipc::CapabilityRights::SEND).ok()
    } else {
        None
    };

    let configured = if let (Some(receiver_handle), Some(sender_handle)) =
        (receiver_to_sender, sender_to_receiver)
    {
        cpu_interrupts::without_interrupts(|| {
            let table = table_mut();
            table.install_ipc_handoff_program(receiver_id, receiver_handle, true)
                && table.install_ipc_handoff_program(sender_id, sender_handle, false)
        })
    } else {
        false
    };

    let mut ran = false;
    let mut exit_code = 0;
    let mut user_passed = false;
    if configured {
        let syscalls_before = user::snapshot().syscall_count;
        let running = cpu_interrupts::without_interrupts(|| {
            table_mut().mark_running(receiver_id, syscalls_before)
        });
        if running {
            let result = user::run_program_in_address_space(
                receiver.map(|task| task.entry_point).unwrap_or(0),
                receiver.map(|task| task.stack_top).unwrap_or(0),
                IPC_TEST_EXIT_CODE,
                14,
                receiver.map(|task| task.address_space_root).unwrap_or(0),
            );
            ran = result.ran;
            exit_code = result.exit_code;
            user_passed = result.passed;
        }
    }

    let process_after_run = snapshot();
    let ipc_after_run = ipc::snapshot();
    let scheduler_after_run = scheduler::snapshot();
    let blocking_switches = scheduler_after_run
        .blocking_switches
        .saturating_sub(scheduler_before.blocking_switches);
    let restart_completions = process_after_run
        .ipc_restart_completions
        .saturating_sub(process_before.ipc_restart_completions);
    let messages_sent = ipc_after_run
        .messages_sent
        .saturating_sub(ipc_before.messages_sent);
    let messages_received = ipc_after_run
        .messages_received
        .saturating_sub(ipc_before.messages_received);
    let handoff_complete = blocking_switches == 5
        && restart_completions == 4
        && messages_sent == 4
        && messages_received == 4
        && ipc_after_run.receiver_wakeups >= ipc_before.receiver_wakeups.saturating_add(4);

    let syscalls = user::snapshot().syscall_count;
    if receiver_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(receiver_id, 0, syscalls);
            let _ = table_mut().cleanup_task(receiver_id);
        });
    }
    if sender_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(sender_id, exit_code, syscalls);
            let _ = table_mut().cleanup_task(sender_id);
        });
    }

    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;
    let ipc_after = ipc::snapshot();
    let endpoints_cleaned = ipc_after.active_endpoints == ipc_before.active_endpoints
        && ipc_after.active_capabilities == ipc_before.active_capabilities
        && ipc_after.queued_messages == ipc_before.queued_messages
        && ipc_after.waiting_receivers == ipc_before.waiting_receivers;
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let passed = ran
        && user_passed
        && exit_code == IPC_TEST_EXIT_CODE
        && handoff_complete
        && endpoints_cleaned
        && frames_restored
        && heap_restored
        && resources_restored;

    serial::log_bool("ipc", "blocking handoff ran", ran);
    serial::log_u64("ipc", "blocking switches", blocking_switches);
    serial::log_u64("ipc", "restart completions", restart_completions);
    serial::log_u64("ipc", "handoff messages sent", messages_sent);
    serial::log_u64("ipc", "handoff messages received", messages_received);
    serial::log_bool("ipc", "blocking handoff test", passed);

    IpcHandoffReport {
        ran,
        sender_id,
        receiver_id,
        exit_code,
        blocking_switches,
        restart_completions,
        messages_sent,
        messages_received,
        endpoints_cleaned,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_capability_test() -> CapabilityTestReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let ipc_before = ipc::snapshot();
    let owner = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let target = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let owner_id = owner.map(|task| task.id).unwrap_or(0);
    let target_id = target.map(|task| task.id).unwrap_or(0);

    let self_handle = ipc::self_capability(owner_id).ok();
    let send_handle = ipc::grant_capability(owner_id, target_id, ipc::CapabilityRights::SEND).ok();
    let receive_only =
        ipc::grant_capability(owner_id, owner_id, ipc::CapabilityRights::RECEIVE).ok();
    let mut output = [0u8; ipc::MAX_MESSAGE_BYTES];
    let authorized_delivery = send_handle
        .map(|handle| ipc::send_capability(owner_id, handle, b"cap").is_ok())
        .unwrap_or(false)
        && matches!(
            ipc::receive(target_id, &mut output, false),
            Ok(ipc::ReceiveOutcome::Message(ipc::Delivery {
                sender,
                length: 3,
                ..
            })) if sender == owner_id
        )
        && &output[..3] == b"cap";
    let invalid_denied =
        ipc::send_capability(owner_id, 0, b"invalid") == Err(ipc::IpcError::InvalidCapability);
    let permission_denied = receive_only
        .map(|handle| {
            ipc::send_capability(owner_id, handle, b"denied")
                == Err(ipc::IpcError::PermissionDenied)
        })
        .unwrap_or(false);
    let revoked_denied = send_handle
        .map(|handle| {
            ipc::revoke_capability(owner_id, handle).is_ok()
                && ipc::send_capability(owner_id, handle, b"revoked")
                    == Err(ipc::IpcError::StaleCapability)
        })
        .unwrap_or(false);
    let replacement = ipc::grant_capability(owner_id, target_id, ipc::CapabilityRights::SEND).ok();
    let generation_advanced =
        matches!((send_handle, replacement), (Some(old), Some(new)) if old != new);

    let syscalls = user::snapshot().syscall_count;
    if target_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(target_id, 0, syscalls);
            let _ = table_mut().cleanup_task(target_id);
        });
    }
    let cleanup_revoked = replacement
        .map(|handle| {
            ipc::send_capability(owner_id, handle, b"cleanup")
                == Err(ipc::IpcError::StaleCapability)
        })
        .unwrap_or(false);
    if owner_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(owner_id, 0, syscalls);
            let _ = table_mut().cleanup_task(owner_id);
        });
    }

    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;
    let ipc_after = ipc::snapshot();
    let capability_baseline = ipc_after.active_endpoints == ipc_before.active_endpoints
        && ipc_after.active_capabilities == ipc_before.active_capabilities
        && ipc_after.queued_messages == ipc_before.queued_messages
        && ipc_after.capability_denials >= ipc_before.capability_denials.saturating_add(4)
        && ipc_after.stale_capability_denials
            >= ipc_before.stale_capability_denials.saturating_add(2);
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let passed = owner_id != 0
        && target_id != 0
        && self_handle.is_some()
        && authorized_delivery
        && invalid_denied
        && permission_denied
        && revoked_denied
        && generation_advanced
        && cleanup_revoked
        && capability_baseline
        && frames_restored
        && heap_restored
        && resources_restored
        && ipc::selftest();

    serial::log_bool("ipc-cap", "self capability", self_handle.is_some());
    serial::log_bool("ipc-cap", "authorized delivery", authorized_delivery);
    serial::log_bool("ipc-cap", "invalid denied", invalid_denied);
    serial::log_bool("ipc-cap", "permission denied", permission_denied);
    serial::log_bool("ipc-cap", "revoked denied", revoked_denied);
    serial::log_bool("ipc-cap", "generation advanced", generation_advanced);
    serial::log_bool("ipc-cap", "cleanup revoked", cleanup_revoked);
    serial::log_bool("ipc-cap", "capability test", passed);

    CapabilityTestReport {
        owner_id,
        target_id,
        self_capability: self_handle.is_some(),
        authorized_delivery,
        invalid_denied,
        permission_denied,
        revoked_denied,
        generation_advanced,
        cleanup_revoked,
        capability_baseline,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_ipc_wait_control_test() -> IpcWaitControlReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let process_before = snapshot();
    let scheduler_before = scheduler::snapshot();
    let ipc_before = ipc::snapshot();

    let timeout_receiver = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let timeout_peer = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let timeout_receiver_id = timeout_receiver.map(|task| task.id).unwrap_or(0);
    let timeout_peer_id = timeout_peer.map(|task| task.id).unwrap_or(0);
    let timeout_configured = timeout_receiver_id != 0
        && timeout_peer_id != 0
        && cpu_interrupts::without_interrupts(|| {
            let table = table_mut();
            table.install_ipc_timeout_program(timeout_receiver_id, true)
                && table.install_ipc_timeout_program(timeout_peer_id, false)
        });

    let mut timeout_ran = false;
    let mut timeout_passed = false;
    let mut timeout_exit_code = 0;
    if timeout_configured {
        let before = user::snapshot().syscall_count;
        if cpu_interrupts::without_interrupts(|| {
            table_mut().mark_running(timeout_receiver_id, before)
        }) {
            let result = user::run_program_in_address_space(
                timeout_receiver.map(|task| task.entry_point).unwrap_or(0),
                timeout_receiver.map(|task| task.stack_top).unwrap_or(0),
                IPC_TEST_EXIT_CODE,
                5,
                timeout_receiver
                    .map(|task| task.address_space_root)
                    .unwrap_or(0),
            );
            timeout_ran = result.ran;
            timeout_passed = result.passed;
            timeout_exit_code = result.exit_code;
        }
    }

    let syscalls = user::snapshot().syscall_count;
    for task_id in [timeout_receiver_id, timeout_peer_id] {
        if task_id == 0 {
            continue;
        }
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(task_id, timeout_exit_code, syscalls);
            let _ = table_mut().cleanup_task(task_id);
        });
    }

    let cancel_receiver = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let canceller = cpu_interrupts::without_interrupts(|| table_mut().spawn_elf_init());
    let cancel_receiver_id = cancel_receiver.map(|task| task.id).unwrap_or(0);
    let canceller_id = canceller.map(|task| task.id).unwrap_or(0);
    let cancel_capability = if cancel_receiver_id != 0 && canceller_id != 0 {
        ipc::grant_capability(
            canceller_id,
            cancel_receiver_id,
            ipc::CapabilityRights::CANCEL,
        )
        .ok()
    } else {
        None
    };
    let cancel_configured = if let Some(capability) = cancel_capability {
        cpu_interrupts::without_interrupts(|| {
            let table = table_mut();
            table.install_ipc_cancel_program(cancel_receiver_id, 0, true)
                && table.install_ipc_cancel_program(canceller_id, capability, false)
        })
    } else {
        false
    };

    let mut cancellation_ran = false;
    let mut cancellation_passed = false;
    let mut cancellation_exit_code = 0;
    if cancel_configured {
        let before = user::snapshot().syscall_count;
        if cpu_interrupts::without_interrupts(|| {
            table_mut().mark_running(cancel_receiver_id, before)
        }) {
            let result = user::run_program_in_address_space(
                cancel_receiver.map(|task| task.entry_point).unwrap_or(0),
                cancel_receiver.map(|task| task.stack_top).unwrap_or(0),
                IPC_TEST_EXIT_CODE,
                5,
                cancel_receiver
                    .map(|task| task.address_space_root)
                    .unwrap_or(0),
            );
            cancellation_ran = result.ran;
            cancellation_passed = result.passed;
            cancellation_exit_code = result.exit_code;
        }
    }

    let syscalls = user::snapshot().syscall_count;
    for task_id in [cancel_receiver_id, canceller_id] {
        if task_id == 0 {
            continue;
        }
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(task_id, cancellation_exit_code, syscalls);
            let _ = table_mut().cleanup_task(task_id);
        });
    }

    let process_after = snapshot();
    let scheduler_after = scheduler::snapshot();
    let ipc_after = ipc::snapshot();
    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;
    let timeout_wakeups = process_after
        .ipc_timeouts
        .saturating_sub(process_before.ipc_timeouts);
    let cancellation_wakeups = process_after
        .ipc_cancellations
        .saturating_sub(process_before.ipc_cancellations);
    let restart_completions = process_after
        .ipc_restart_completions
        .saturating_sub(process_before.ipc_restart_completions);
    let scheduler_clean = scheduler_after.current_task == scheduler_before.current_task
        && scheduler_after.queued_tasks == scheduler_before.queued_tasks
        && scheduler_after.blocked_tasks == scheduler_before.blocked_tasks;
    let endpoint_clean = ipc_after.active_endpoints == ipc_before.active_endpoints
        && ipc_after.active_capabilities == ipc_before.active_capabilities
        && ipc_after.queued_messages == ipc_before.queued_messages
        && ipc_after.waiting_receivers == ipc_before.waiting_receivers;
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let passed = timeout_ran
        && timeout_passed
        && timeout_exit_code == IPC_TEST_EXIT_CODE
        && timeout_wakeups == 1
        && cancellation_ran
        && cancellation_passed
        && cancellation_exit_code == IPC_TEST_EXIT_CODE
        && cancellation_wakeups == 1
        && restart_completions == 2
        && ipc_after.cancellation_successes == ipc_before.cancellation_successes.saturating_add(1)
        && scheduler_clean
        && endpoint_clean
        && frames_restored
        && heap_restored
        && resources_restored;

    serial::log_bool("ipc-wait", "timeout Ring 3", timeout_passed);
    serial::log_u64("ipc-wait", "timeout wakeups", timeout_wakeups);
    serial::log_bool("ipc-wait", "cancellation Ring 3", cancellation_passed);
    serial::log_u64("ipc-wait", "cancellation wakeups", cancellation_wakeups);
    serial::log_bool("ipc-wait", "scheduler clean", scheduler_clean);
    serial::log_bool("ipc-wait", "wait control test", passed);

    IpcWaitControlReport {
        timeout_ran,
        timeout_exit_code,
        timeout_wakeups,
        cancellation_ran,
        cancellation_exit_code,
        cancellation_wakeups,
        restart_completions,
        scheduler_clean,
        endpoint_clean,
        frames_restored,
        heap_restored,
        resources_restored,
        passed,
    }
}

pub fn run_preemption_test() -> PreemptionReport {
    let frames_before = crate::physmem::snapshot();
    let heap_before = heap::snapshot();
    let resources_before = snapshot().active_resources;
    let scheduler_before = scheduler::snapshot();
    let first = cpu_interrupts::without_interrupts(|| table_mut().spawn_preempt_task());
    let second = cpu_interrupts::without_interrupts(|| table_mut().spawn_preempt_task());

    let first_id = first.map(|task| task.id).unwrap_or(0);
    let second_id = second.map(|task| task.id).unwrap_or(0);
    let distinct_roots = match (first, second) {
        (Some(first), Some(second)) => {
            first.address_space_root != 0 && first.address_space_root != second.address_space_root
        }
        _ => false,
    };
    let distinct_user_frames = match (first, second) {
        (Some(first), Some(second)) => {
            first.first_user_frame != 0 && first.first_user_frame != second.first_user_frame
        }
        _ => false,
    };

    let mut ran = false;
    let mut user_passed = false;
    if let (Some(first), Some(_second)) = (first, second) {
        let syscalls_before = user::snapshot().syscall_count;
        let running = cpu_interrupts::without_interrupts(|| {
            let table = table_mut();
            let running = table.mark_running(first.id, syscalls_before);
            if running {
                table.start_preemption_test();
            }
            running
        });

        if running {
            let result = user::run_program_in_address_space(
                first.entry_point,
                first.stack_top,
                PREEMPT_TEST_EXIT_CODE,
                0,
                first.address_space_root,
            );
            ran = result.ran;
            user_passed = result.passed;
        }
    }

    let (first_task, second_task, timer_switches) = cpu_interrupts::without_interrupts(|| {
        let table = table_mut();
        let first_task = table.task_by_id(first_id).unwrap_or(Task::empty());
        let second_task = table.task_by_id(second_id).unwrap_or(Task::empty());
        let switches = table.stop_preemption_test(false);
        (first_task, second_task, switches)
    });

    let syscalls_after = user::snapshot().syscall_count;
    if first_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(first_id, PREEMPT_TEST_EXIT_CODE, syscalls_after);
        });
    }
    if second_id != 0 {
        cpu_interrupts::without_interrupts(|| {
            let _ = table_mut().mark_exited(second_id, PREEMPT_TEST_EXIT_CODE, syscalls_after);
        });
    }

    let first_cleaned =
        first_id != 0 && cpu_interrupts::without_interrupts(|| table_mut().cleanup_task(first_id));
    let second_cleaned = second_id != 0
        && cpu_interrupts::without_interrupts(|| table_mut().cleanup_task(second_id));
    let scheduler_after = scheduler::snapshot();
    let frames_after = crate::physmem::snapshot();
    let heap_after = heap::snapshot();
    let resources_after = snapshot().active_resources;

    let first_slices = first_task.scheduled_slices;
    let second_slices = second_task.scheduled_slices;
    let slice_difference = first_slices.abs_diff(second_slices);
    let max_wait_ticks = first_task.max_wait_ticks.max(second_task.max_wait_ticks);
    let round_robin_balanced = first_slices >= 2 && second_slices >= 2 && slice_difference <= 1;
    let starvation_bounded = max_wait_ticks
        <= scheduler::STARVATION_LIMIT_TICKS.saturating_add(scheduler::DEFAULT_QUANTUM_TICKS);
    let frames_restored = frames_after.allocated_frames == frames_before.allocated_frames
        && frames_after.free_frames == frames_before.free_frames;
    let heap_restored = heap_after.active_allocations == heap_before.active_allocations
        && heap_after.allocated_bytes == heap_before.allocated_bytes
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.metadata_ok
        && heap_after.sentinel_ok
        && heap_after.allocation_canaries_ok;
    let resources_restored = resources_after == resources_before;
    let scheduler_advanced = scheduler_after.timer_preemptions
        >= scheduler_before
            .timer_preemptions
            .saturating_add(PREEMPT_TEST_SWITCHES);
    let passed = ran
        && user_passed
        && timer_switches >= PREEMPT_TEST_SWITCHES
        && scheduler_advanced
        && round_robin_balanced
        && starvation_bounded
        && distinct_roots
        && distinct_user_frames
        && first_cleaned
        && second_cleaned
        && frames_restored
        && heap_restored
        && resources_restored;

    cpu_interrupts::without_interrupts(|| {
        if passed {
            let table = table_mut();
            table.preemption_passes = table.preemption_passes.saturating_add(1);
        }
    });

    serial::log_u64("sched", "timer context switches", timer_switches);
    serial::log_u64("sched", "first task slices", first_slices);
    serial::log_u64("sched", "second task slices", second_slices);
    serial::log_u64("sched", "maximum wait ticks", max_wait_ticks);
    serial::log_bool("sched", "round robin balanced", round_robin_balanced);
    serial::log_bool("sched", "starvation bounded", starvation_bounded);
    serial::log_bool("sched", "preemption test", passed);

    PreemptionReport {
        ran,
        passed,
        first_task: first_id,
        second_task: second_id,
        timer_switches,
        first_slices,
        second_slices,
        max_wait_ticks,
        round_robin_balanced,
        starvation_bounded,
        distinct_roots,
        distinct_user_frames,
        frames_restored,
        heap_restored,
        resources_restored,
    }
}

fn run_spawned_task(
    task: Option<Task>,
    fallback_entry: u64,
    fallback_stack: u64,
    expected_exit_code: u64,
    minimum_syscalls: u64,
) -> TaskRunResult {
    let Some(task) = task else {
        return TaskRunResult::not_run(fallback_entry, fallback_stack);
    };
    let syscalls_before = user::snapshot().syscall_count;
    let running =
        cpu_interrupts::without_interrupts(|| table_mut().mark_running(task.id, syscalls_before));
    if !running {
        return TaskRunResult::not_run(task.entry_point, task.stack_top);
    }

    if task.address_space_root != 0 {
        serial::log("process", "entering isolated user address space");
        serial::log("elf", "entering ELF64 user entry point");
    }
    let user_result = if task.address_space_root == 0 {
        user::run_program(
            task.entry_point,
            task.stack_top,
            expected_exit_code,
            minimum_syscalls,
        )
    } else {
        user::run_program_in_address_space(
            task.entry_point,
            task.stack_top,
            expected_exit_code,
            minimum_syscalls,
            task.address_space_root,
        )
    };
    let exited = cpu_interrupts::without_interrupts(|| {
        table_mut().mark_exited(task.id, user_result.exit_code, user_result.syscalls_after)
    });
    let resources_cleaned =
        cpu_interrupts::without_interrupts(|| table_mut().cleanup_task(task.id));
    let final_task = cpu_interrupts::without_interrupts(|| {
        table()
            .find_task(task.id)
            .and_then(|index| table().task(index))
    })
    .unwrap_or(task);
    let passed =
        user_result.passed && exited && final_task.state == TaskState::Exited && resources_cleaned;
    if task.address_space_root != 0 {
        if passed {
            serial::log("elf", "ELF64 user process passed");
        } else {
            serial::log("elf", "ELF64 user process failed");
        }
    }

    TaskRunResult {
        ran: user_result.ran,
        passed,
        task_id: task.id,
        state: final_task.state,
        entry_point: task.entry_point,
        stack_top: task.stack_top,
        exit_code: user_result.exit_code,
        syscalls_before: user_result.syscalls_before,
        syscalls_after: user_result.syscalls_after,
        address_space_root: task.address_space_root,
        first_user_frame: task.first_user_frame,
        owned_user_pages: task.owned_user_pages,
        owned_table_frames: task.owned_table_frames,
        cleanup_user_frames: final_task.cleanup_user_frames,
        cleanup_table_frames: final_task.cleanup_table_frames,
        heap_released: final_task.heap_released,
        resources_cleaned,
    }
}

pub fn selftest() -> bool {
    let snapshot = snapshot();
    let address_spaces = paging::address_space_stats();
    let scheduler_state = scheduler::snapshot();

    snapshot.initialized
        && snapshot.task_capacity == MAX_TASKS as u64
        && snapshot.running_tasks == 0
        && snapshot.blocked_tasks == 0
        && snapshot.zombie_children == 0
        && snapshot.active_resources == 0
        && !snapshot.preemption_active
        && snapshot.cleanup_failures == 0
        && snapshot.next_task_id >= 1
        && address_spaces.active == 0
        && address_spaces.cleanup_failures == 0
        && scheduler_state.blocked_tasks == 0
        && scheduler::selftest()
        && user::probe_entry_point() != 0
        && user::probe_stack_top() != 0
        && user::probe_expected_exit_code() == 42
        && elf::selftest()
        && ipc::selftest()
        && syscall_context_selftest()
}

fn syscall_context_selftest() -> bool {
    let source = user::SyscallFrame {
        r15: 15,
        r14: 14,
        r13: 13,
        r12: 12,
        r11: 11,
        r10: 10,
        r9: 9,
        r8: 8,
        rbp: 7,
        rdi: 6,
        rsi: 5,
        rdx: 4,
        rcx: 3,
        rbx: 2,
        rax: crate::syscall::SYSCALL_IPC_RECEIVE,
        rip: 0x4010_0080,
        cs: gdt::USER_CODE_SELECTOR as u64,
        rflags: 0x202,
        rsp: paging::USER_ELF_STACK_TOP,
        ss: gdt::USER_DATA_SELECTOR as u64,
    };
    let mut context = interrupts::TimerContext::empty();
    save_syscall_context(&mut context, &source, true);
    let mut restored = user::SyscallFrame {
        r15: 0,
        r14: 0,
        r13: 0,
        r12: 0,
        r11: 0,
        r10: 0,
        r9: 0,
        r8: 0,
        rbp: 0,
        rdi: 0,
        rsi: 0,
        rdx: 0,
        rcx: 0,
        rbx: 0,
        rax: 0,
        rip: 0,
        cs: 0,
        rflags: 0,
        rsp: 0,
        ss: 0,
    };
    load_syscall_context(&mut restored, &context);

    context.instruction_pointer == source.rip - SYSCALL_INSTRUCTION_BYTES
        && restored.r15 == source.r15
        && restored.r12 == source.r12
        && restored.rdi == source.rdi
        && restored.rsi == source.rsi
        && restored.rdx == source.rdx
        && restored.rcx == source.rcx
        && restored.rax == source.rax
        && restored.rip == source.rip - SYSCALL_INSTRUCTION_BYTES
        && restored.cs == source.cs
        && restored.rflags == source.rflags
        && restored.rsp == source.rsp
        && restored.ss == source.ss
}

fn save_syscall_context(
    context: &mut interrupts::TimerContext,
    frame: &user::SyscallFrame,
    restart_syscall: bool,
) {
    let user_data = gdt::USER_DATA_SELECTOR as u64;
    context.gs = user_data;
    context.fs = user_data;
    context.es = user_data;
    context.ds = user_data;
    context.r15 = frame.r15;
    context.r14 = frame.r14;
    context.r13 = frame.r13;
    context.r12 = frame.r12;
    context.r11 = frame.r11;
    context.r10 = frame.r10;
    context.r9 = frame.r9;
    context.r8 = frame.r8;
    context.rbp = frame.rbp;
    context.rdi = frame.rdi;
    context.rsi = frame.rsi;
    context.rdx = frame.rdx;
    context.rcx = frame.rcx;
    context.rbx = frame.rbx;
    context.rax = frame.rax;
    context.instruction_pointer = if restart_syscall {
        frame.rip.saturating_sub(SYSCALL_INSTRUCTION_BYTES)
    } else {
        frame.rip
    };
    context.code_segment = frame.cs;
    context.cpu_flags = frame.rflags;
    context.stack_pointer = frame.rsp;
    context.stack_segment = frame.ss;
}

fn load_syscall_context(frame: &mut user::SyscallFrame, context: &interrupts::TimerContext) {
    frame.r15 = context.r15;
    frame.r14 = context.r14;
    frame.r13 = context.r13;
    frame.r12 = context.r12;
    frame.r11 = context.r11;
    frame.r10 = context.r10;
    frame.r9 = context.r9;
    frame.r8 = context.r8;
    frame.rbp = context.rbp;
    frame.rdi = context.rdi;
    frame.rsi = context.rsi;
    frame.rdx = context.rdx;
    frame.rcx = context.rcx;
    frame.rbx = context.rbx;
    frame.rax = context.rax;
    frame.rip = context.instruction_pointer;
    frame.cs = context.code_segment;
    frame.rflags = context.cpu_flags;
    frame.rsp = context.stack_pointer;
    frame.ss = context.stack_segment;
}

fn initial_timer_context(entry_point: u64, stack_top: u64) -> interrupts::TimerContext {
    let user_data = gdt::USER_DATA_SELECTOR as u64;
    interrupts::TimerContext {
        gs: user_data,
        fs: user_data,
        es: user_data,
        ds: user_data,
        r15: 0,
        r14: 0,
        r13: 0,
        r12: 0,
        r11: 0,
        r10: 0,
        r9: 0,
        r8: 0,
        rbp: 0,
        rdi: 0,
        rsi: 0,
        rdx: 0,
        rcx: 0,
        rbx: 0,
        rax: 0,
        instruction_pointer: entry_point,
        code_segment: gdt::USER_CODE_SELECTOR as u64,
        cpu_flags: 0x202,
        stack_pointer: stack_top,
        stack_segment: user_data,
    }
}

pub fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Empty => "empty",
        TaskState::Ready => "ready",
        TaskState::Running => "running",
        TaskState::Blocked => "blocked",
        TaskState::Exited => "exited",
    }
}

pub fn ipc_wake_reason_name(reason: IpcWakeReason) -> &'static str {
    match reason {
        IpcWakeReason::None => "none",
        IpcWakeReason::Message => "message",
        IpcWakeReason::Timeout => "timeout",
        IpcWakeReason::Cancelled => "cancelled",
    }
}

fn table() -> &'static ProcessTable {
    unsafe { &*PROCESS_TABLE.value.get() }
}

fn table_mut() -> &'static mut ProcessTable {
    unsafe { &mut *PROCESS_TABLE.value.get() }
}
