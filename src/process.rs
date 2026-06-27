use crate::{elf, heap, paging, scheduler, serial, user, user_program};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_TASKS: usize = 8;
const TASK_HEAP_BYTES: u64 = 128;
const TASK_HEAP_ALIGNMENT: u64 = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Empty,
    Ready,
    Running,
    Exited,
}

#[derive(Clone, Copy)]
pub struct Task {
    pub id: u64,
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
}

impl Task {
    const fn empty() -> Self {
        Self {
            id: 0,
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
    pub exited_tasks: u64,
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

struct ProcessSlot {
    task: Task,
    address_space: paging::AddressSpace,
}

impl ProcessSlot {
    const fn empty() -> Self {
        Self {
            task: Task::empty(),
            address_space: paging::AddressSpace::empty(),
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
            slots: [const { ProcessSlot::empty() }; MAX_TASKS],
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }

        self.initialized = true;
        serial::log("process", "resource-owning task model ready");
        serial::log_u64("process", "task capacity", MAX_TASKS as u64);
    }

    fn snapshot(&self) -> Snapshot {
        let mut task_slots_used = 0u64;
        let mut ready_tasks = 0u64;
        let mut running_tasks = 0u64;
        let mut exited_tasks = 0u64;

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
                TaskState::Exited => {
                    task_slots_used = task_slots_used.saturating_add(1);
                    exited_tasks = exited_tasks.saturating_add(1);
                }
            }
        }

        Snapshot {
            initialized: self.initialized,
            task_capacity: MAX_TASKS as u64,
            task_slots_used,
            ready_tasks,
            running_tasks,
            exited_tasks,
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
        let task = Task {
            id,
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
        };

        self.slots[index] = ProcessSlot {
            task,
            address_space,
        };
        self.spawned_tasks = self.spawned_tasks.saturating_add(1);
        self.active_resources = self.active_resources.saturating_add(1);
        self.last_task_id = id;
        serial::log_u64("process", "task spawned", id);
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
        self.last_task_id = id;
        serial::log_u64("process", "task running", id);
        scheduler::begin_task(id);
        true
    }

    fn mark_exited(&mut self, id: u64, exit_code: u64, syscalls_after: u64) -> bool {
        let Some(index) = self.find_task(id) else {
            return false;
        };

        let task = &mut self.slots[index].task;
        task.state = TaskState::Exited;
        task.exit_code = exit_code;
        task.syscalls_after = syscalls_after;
        self.exited_total = self.exited_total.saturating_add(1);
        self.last_task_id = id;
        self.last_exit_code = exit_code;
        serial::log_u64("process", "task exited", id);
        serial::log_u64("process", "exit code", exit_code);
        scheduler::finish_task(id);
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
        let complete = heap_released && pages_complete;

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

    fn free_slot(&self) -> Option<usize> {
        for index in 0..MAX_TASKS {
            let task = self.slots[index].task;
            if task.state == TaskState::Empty
                || (task.state == TaskState::Exited && !task.resources_active)
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

    snapshot.initialized
        && snapshot.task_capacity == MAX_TASKS as u64
        && snapshot.running_tasks == 0
        && snapshot.active_resources == 0
        && snapshot.cleanup_failures == 0
        && snapshot.next_task_id >= 1
        && address_spaces.active == 0
        && address_spaces.cleanup_failures == 0
        && scheduler::selftest()
        && user::probe_entry_point() != 0
        && user::probe_stack_top() != 0
        && user::probe_expected_exit_code() == 42
        && elf::selftest()
}

pub fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Empty => "empty",
        TaskState::Ready => "ready",
        TaskState::Running => "running",
        TaskState::Exited => "exited",
    }
}

fn table() -> &'static ProcessTable {
    unsafe { &*PROCESS_TABLE.value.get() }
}

fn table_mut() -> &'static mut ProcessTable {
    unsafe { &mut *PROCESS_TABLE.value.get() }
}
