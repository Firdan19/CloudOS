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
        serial::log_u64("ipc", "receiver blocked", task_id);
        true
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
        serial::log_u64("ipc", "receiver woken", task_id);
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

pub fn block_for_ipc(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().block_for_ipc(task_id))
}

pub fn wake_from_ipc(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| table_mut().wake_from_ipc(task_id))
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

fn table() -> &'static ProcessTable {
    unsafe { &*PROCESS_TABLE.value.get() }
}

fn table_mut() -> &'static mut ProcessTable {
    unsafe { &mut *PROCESS_TABLE.value.get() }
}
