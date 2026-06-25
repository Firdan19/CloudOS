use crate::{serial, user};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_TASKS: usize = 8;

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
    pub next_task_id: u64,
    pub spawned_tasks: u64,
    pub exited_total: u64,
    pub failed_spawns: u64,
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
}

struct ProcessTable {
    initialized: bool,
    next_task_id: u64,
    spawned_tasks: u64,
    exited_total: u64,
    failed_spawns: u64,
    last_task_id: u64,
    last_exit_code: u64,
    tasks: [Task; MAX_TASKS],
}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            initialized: false,
            next_task_id: 1,
            spawned_tasks: 0,
            exited_total: 0,
            failed_spawns: 0,
            last_task_id: 0,
            last_exit_code: 0,
            tasks: [Task::empty(); MAX_TASKS],
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }

        self.initialized = true;
        serial::log("process", "task model ready");
        serial::log_u64("process", "task capacity", MAX_TASKS as u64);
    }

    fn snapshot(&self) -> Snapshot {
        let mut task_slots_used = 0u64;
        let mut ready_tasks = 0u64;
        let mut running_tasks = 0u64;
        let mut exited_tasks = 0u64;

        for task in self.tasks.iter().copied() {
            match task.state {
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
            next_task_id: self.next_task_id,
            spawned_tasks: self.spawned_tasks,
            exited_total: self.exited_total,
            failed_spawns: self.failed_spawns,
            last_task_id: self.last_task_id,
            last_exit_code: self.last_exit_code,
        }
    }

    fn spawn_user_probe(&mut self) -> Option<Task> {
        if !self.initialized {
            self.init();
        }

        let Some(index) = self.free_slot() else {
            self.failed_spawns = self.failed_spawns.saturating_add(1);
            serial::log("process", "task spawn failed");
            return None;
        };

        let id = self.next_task_id;
        self.next_task_id = self.next_task_id.saturating_add(1);

        let task = Task {
            id,
            state: TaskState::Ready,
            entry_point: user::probe_entry_point(),
            stack_top: user::probe_stack_top(),
            exit_code: 0,
            syscalls_before: 0,
            syscalls_after: 0,
            runs: 0,
        };

        self.tasks[index] = task;
        self.spawned_tasks = self.spawned_tasks.saturating_add(1);
        self.last_task_id = id;
        serial::log_u64("process", "task ready", id);

        Some(task)
    }

    fn mark_running(&mut self, id: u64, syscalls_before: u64) -> bool {
        let Some(index) = self.find_task(id) else {
            return false;
        };

        self.tasks[index].state = TaskState::Running;
        self.tasks[index].syscalls_before = syscalls_before;
        self.tasks[index].runs = self.tasks[index].runs.saturating_add(1);
        self.last_task_id = id;
        serial::log_u64("process", "task running", id);

        true
    }

    fn mark_exited(&mut self, id: u64, exit_code: u64, syscalls_after: u64) -> Option<Task> {
        let index = self.find_task(id)?;

        self.tasks[index].state = TaskState::Exited;
        self.tasks[index].exit_code = exit_code;
        self.tasks[index].syscalls_after = syscalls_after;
        self.exited_total = self.exited_total.saturating_add(1);
        self.last_task_id = id;
        self.last_exit_code = exit_code;
        serial::log_u64("process", "task exited", id);
        serial::log_u64("process", "exit code", exit_code);

        Some(self.tasks[index])
    }

    fn free_slot(&self) -> Option<usize> {
        for index in 0..MAX_TASKS {
            if self.tasks[index].state == TaskState::Empty
                || self.tasks[index].state == TaskState::Exited
            {
                return Some(index);
            }
        }

        None
    }

    fn find_task(&self, id: u64) -> Option<usize> {
        for index in 0..MAX_TASKS {
            if self.tasks[index].state != TaskState::Empty && self.tasks[index].id == id {
                return Some(index);
            }
        }

        None
    }

    fn task(&self, index: usize) -> Option<Task> {
        if index >= MAX_TASKS || self.tasks[index].state == TaskState::Empty {
            return None;
        }

        Some(self.tasks[index])
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
    let mut snapshot = Snapshot {
        initialized: false,
        task_capacity: MAX_TASKS as u64,
        task_slots_used: 0,
        ready_tasks: 0,
        running_tasks: 0,
        exited_tasks: 0,
        next_task_id: 1,
        spawned_tasks: 0,
        exited_total: 0,
        failed_spawns: 0,
        last_task_id: 0,
        last_exit_code: 0,
    };

    cpu_interrupts::without_interrupts(|| {
        snapshot = table().snapshot();
    });

    snapshot
}

pub fn task(index: usize) -> Option<Task> {
    let mut result = None;

    cpu_interrupts::without_interrupts(|| {
        result = table().task(index);
    });

    result
}

pub fn run_user_probe_task() -> TaskRunResult {
    let Some(task) = cpu_interrupts::without_interrupts(|| table_mut().spawn_user_probe()) else {
        return TaskRunResult {
            ran: false,
            passed: false,
            task_id: 0,
            state: TaskState::Empty,
            entry_point: user::probe_entry_point(),
            stack_top: user::probe_stack_top(),
            exit_code: 0,
            syscalls_before: user::snapshot().syscall_count,
            syscalls_after: user::snapshot().syscall_count,
        };
    };

    let syscalls_before = user::snapshot().syscall_count;
    let running =
        cpu_interrupts::without_interrupts(|| table_mut().mark_running(task.id, syscalls_before));
    if !running {
        return TaskRunResult {
            ran: false,
            passed: false,
            task_id: task.id,
            state: TaskState::Ready,
            entry_point: task.entry_point,
            stack_top: task.stack_top,
            exit_code: 0,
            syscalls_before,
            syscalls_after: syscalls_before,
        };
    }

    let user_result = user::run_entry(task.entry_point, task.stack_top);
    let exited_task = cpu_interrupts::without_interrupts(|| {
        table_mut().mark_exited(task.id, user_result.exit_code, user_result.syscalls_after)
    });

    let state = exited_task
        .map(|finished| finished.state)
        .unwrap_or(TaskState::Exited);
    let passed = user_result.passed && state == TaskState::Exited;

    TaskRunResult {
        ran: user_result.ran,
        passed,
        task_id: task.id,
        state,
        entry_point: task.entry_point,
        stack_top: task.stack_top,
        exit_code: user_result.exit_code,
        syscalls_before: user_result.syscalls_before,
        syscalls_after: user_result.syscalls_after,
    }
}

pub fn selftest() -> bool {
    let snapshot = snapshot();

    snapshot.initialized
        && snapshot.task_capacity == MAX_TASKS as u64
        && snapshot.running_tasks == 0
        && snapshot.next_task_id >= 1
        && user::probe_entry_point() != 0
        && user::probe_stack_top() != 0
        && user::probe_expected_exit_code() == 42
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
