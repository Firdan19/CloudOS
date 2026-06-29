use crate::{interrupts, serial};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const QUEUE_CAPACITY: usize = 8;
pub const DEFAULT_QUANTUM_TICKS: u64 = 2;
pub const STARVATION_LIMIT_TICKS: u64 = 18;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub queue_capacity: u64,
    pub queued_tasks: u64,
    pub current_task: u64,
    pub last_task: u64,
    pub context_switches: u64,
    pub cooperative_yields: u64,
    pub timer_ticks: u64,
    pub accounted_ticks: u64,
    pub failed_enqueues: u64,
    pub last_switch_tick: u64,
    pub quantum_ticks: u64,
    pub slice_ticks: u64,
    pub timer_preemptions: u64,
    pub round_robin_rotations: u64,
    pub starvation_preventions: u64,
    pub max_wait_ticks: u64,
    pub blocked_tasks: u64,
    pub block_events: u64,
    pub blocking_switches: u64,
    pub wake_events: u64,
    pub failed_wakeups: u64,
}

#[derive(Clone, Copy)]
pub struct PreemptionDecision {
    pub switched: bool,
    pub previous_task: u64,
    pub next_task: u64,
    pub waited_ticks: u64,
    pub starvation_guard: bool,
}

impl PreemptionDecision {
    const fn none(current_task: u64) -> Self {
        Self {
            switched: false,
            previous_task: current_task,
            next_task: current_task,
            waited_ticks: 0,
            starvation_guard: false,
        }
    }
}

#[derive(Clone, Copy)]
struct QueueEntry {
    task_id: u64,
    ready_since: u64,
}

impl QueueEntry {
    const fn empty() -> Self {
        Self {
            task_id: 0,
            ready_since: 0,
        }
    }
}

struct Scheduler {
    initialized: bool,
    queue: [QueueEntry; QUEUE_CAPACITY],
    blocked: [u64; QUEUE_CAPACITY],
    head: usize,
    len: usize,
    blocked_len: usize,
    current_task: u64,
    last_task: u64,
    context_switches: u64,
    cooperative_yields: u64,
    timer_ticks: u64,
    accounted_ticks: u64,
    failed_enqueues: u64,
    last_switch_tick: u64,
    quantum_ticks: u64,
    slice_ticks: u64,
    timer_preemptions: u64,
    round_robin_rotations: u64,
    starvation_preventions: u64,
    max_wait_ticks: u64,
    block_events: u64,
    blocking_switches: u64,
    wake_events: u64,
    failed_wakeups: u64,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            initialized: false,
            queue: [QueueEntry::empty(); QUEUE_CAPACITY],
            blocked: [0; QUEUE_CAPACITY],
            head: 0,
            len: 0,
            blocked_len: 0,
            current_task: 0,
            last_task: 0,
            context_switches: 0,
            cooperative_yields: 0,
            timer_ticks: 0,
            accounted_ticks: 0,
            failed_enqueues: 0,
            last_switch_tick: 0,
            quantum_ticks: DEFAULT_QUANTUM_TICKS,
            slice_ticks: 0,
            timer_preemptions: 0,
            round_robin_rotations: 0,
            starvation_preventions: 0,
            max_wait_ticks: 0,
            block_events: 0,
            blocking_switches: 0,
            wake_events: 0,
            failed_wakeups: 0,
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }

        *self = Self::new();
        self.initialized = true;
        self.last_switch_tick = interrupts::ticks();

        serial::log("sched", "scheduler ready");
        serial::log("sched", "preemptive round-robin ready");
        serial::log_u64("sched", "queue capacity", QUEUE_CAPACITY as u64);
        serial::log_u64("sched", "quantum ticks", self.quantum_ticks);
        serial::log_u64("sched", "starvation limit", STARVATION_LIMIT_TICKS);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            initialized: self.initialized,
            queue_capacity: QUEUE_CAPACITY as u64,
            queued_tasks: self.len as u64,
            current_task: self.current_task,
            last_task: self.last_task,
            context_switches: self.context_switches,
            cooperative_yields: self.cooperative_yields,
            timer_ticks: self.timer_ticks,
            accounted_ticks: self.accounted_ticks,
            failed_enqueues: self.failed_enqueues,
            last_switch_tick: self.last_switch_tick,
            quantum_ticks: self.quantum_ticks,
            slice_ticks: self.slice_ticks,
            timer_preemptions: self.timer_preemptions,
            round_robin_rotations: self.round_robin_rotations,
            starvation_preventions: self.starvation_preventions,
            max_wait_ticks: self.max_wait_ticks,
            blocked_tasks: self.blocked_len as u64,
            block_events: self.block_events,
            blocking_switches: self.blocking_switches,
            wake_events: self.wake_events,
            failed_wakeups: self.failed_wakeups,
        }
    }

    fn enqueue(&mut self, task_id: u64) -> bool {
        let queued = self.enqueue_at(task_id, self.timer_ticks);
        if queued {
            serial::log_u64("sched", "task queued", task_id);
        }
        queued
    }

    fn enqueue_at(&mut self, task_id: u64, ready_since: u64) -> bool {
        if task_id == 0 {
            self.failed_enqueues = self.failed_enqueues.saturating_add(1);
            return false;
        }

        if self.contains(task_id) || self.current_task == task_id {
            return true;
        }

        if self.len >= QUEUE_CAPACITY {
            self.failed_enqueues = self.failed_enqueues.saturating_add(1);
            return false;
        }

        let tail = (self.head + self.len) % QUEUE_CAPACITY;
        self.queue[tail] = QueueEntry {
            task_id,
            ready_since,
        };
        self.len += 1;
        true
    }

    fn begin_task(&mut self, task_id: u64) {
        self.remove(task_id);
        self.remove_blocked(task_id);
        self.current_task = task_id;
        self.last_task = task_id;
        self.context_switches = self.context_switches.saturating_add(1);
        self.last_switch_tick = self.timer_ticks;
        self.slice_ticks = 0;
        serial::log_u64("sched", "context switch", task_id);
    }

    fn finish_task(&mut self, task_id: u64) {
        if self.current_task == task_id {
            self.current_task = 0;
            self.slice_ticks = 0;
            self.last_switch_tick = self.timer_ticks;
        }

        self.remove(task_id);
        self.remove_blocked(task_id);
        self.last_task = task_id;
        serial::log_u64("sched", "task complete", task_id);
    }

    fn yield_current(&mut self) -> u64 {
        self.cooperative_yields = self.cooperative_yields.saturating_add(1);

        if self.current_task == 0 {
            serial::log("sched", "yield without current task");
            return 0;
        }

        self.last_switch_tick = self.timer_ticks;
        serial::log_u64("sched", "cooperative yield", self.current_task);
        self.current_task
    }

    fn block_task(&mut self, task_id: u64) -> bool {
        if task_id == 0 {
            return false;
        }
        if self.contains_blocked(task_id) {
            return true;
        }
        if self.blocked_len >= QUEUE_CAPACITY {
            return false;
        }

        let known = if self.current_task == task_id {
            self.current_task = 0;
            self.slice_ticks = 0;
            true
        } else {
            self.remove(task_id)
        };
        if !known {
            return false;
        }

        self.blocked[self.blocked_len] = task_id;
        self.blocked_len += 1;
        self.block_events = self.block_events.saturating_add(1);
        true
    }

    fn block_current_and_dispatch(&mut self, task_id: u64) -> PreemptionDecision {
        if !self.initialized
            || task_id == 0
            || self.current_task != task_id
            || self.len == 0
            || self.blocked_len >= QUEUE_CAPACITY
        {
            return PreemptionDecision::none(self.current_task);
        }

        let (offset, starvation_guard) = self.select_next_offset();
        let next = self.remove_at(offset);
        if next.task_id == 0 {
            return PreemptionDecision::none(self.current_task);
        }

        let waited_ticks = self.timer_ticks.saturating_sub(next.ready_since);
        self.max_wait_ticks = self.max_wait_ticks.max(waited_ticks);
        self.blocked[self.blocked_len] = task_id;
        self.blocked_len += 1;
        self.current_task = next.task_id;
        self.last_task = next.task_id;
        self.context_switches = self.context_switches.saturating_add(1);
        self.block_events = self.block_events.saturating_add(1);
        self.blocking_switches = self.blocking_switches.saturating_add(1);
        if starvation_guard {
            self.starvation_preventions = self.starvation_preventions.saturating_add(1);
        }
        self.last_switch_tick = self.timer_ticks;
        self.slice_ticks = 0;

        PreemptionDecision {
            switched: true,
            previous_task: task_id,
            next_task: next.task_id,
            waited_ticks,
            starvation_guard,
        }
    }

    fn wake_task(&mut self, task_id: u64) -> bool {
        if task_id == 0 {
            self.failed_wakeups = self.failed_wakeups.saturating_add(1);
            return false;
        }
        if self.current_task == task_id || self.contains(task_id) {
            return true;
        }
        if !self.remove_blocked(task_id) {
            self.failed_wakeups = self.failed_wakeups.saturating_add(1);
            return false;
        }

        if !self.enqueue_at(task_id, self.timer_ticks) {
            self.blocked[self.blocked_len] = task_id;
            self.blocked_len += 1;
            self.failed_wakeups = self.failed_wakeups.saturating_add(1);
            return false;
        }

        self.wake_events = self.wake_events.saturating_add(1);
        true
    }

    fn on_timer_tick(&mut self) {
        self.timer_ticks = self.timer_ticks.saturating_add(1);

        if self.current_task != 0 {
            self.accounted_ticks = self.accounted_ticks.saturating_add(1);
            self.slice_ticks = self.slice_ticks.saturating_add(1);
        }
    }

    fn preempt_current(&mut self) -> PreemptionDecision {
        let previous_task = self.current_task;
        if !self.initialized
            || previous_task == 0
            || self.len == 0
            || self.slice_ticks < self.quantum_ticks
        {
            return PreemptionDecision::none(previous_task);
        }

        let (offset, starvation_guard) = self.select_next_offset();
        let next = self.remove_at(offset);
        if next.task_id == 0 {
            return PreemptionDecision::none(previous_task);
        }

        let waited_ticks = self.timer_ticks.saturating_sub(next.ready_since);
        self.max_wait_ticks = self.max_wait_ticks.max(waited_ticks);
        self.current_task = 0;
        if !self.enqueue_at(previous_task, self.timer_ticks) {
            self.current_task = previous_task;
            let _ = self.enqueue_at(next.task_id, next.ready_since);
            return PreemptionDecision::none(previous_task);
        }

        self.current_task = next.task_id;
        self.last_task = next.task_id;
        self.context_switches = self.context_switches.saturating_add(1);
        self.timer_preemptions = self.timer_preemptions.saturating_add(1);
        self.round_robin_rotations = self.round_robin_rotations.saturating_add(1);
        if starvation_guard {
            self.starvation_preventions = self.starvation_preventions.saturating_add(1);
        }
        self.last_switch_tick = self.timer_ticks;
        self.slice_ticks = 0;

        PreemptionDecision {
            switched: true,
            previous_task,
            next_task: next.task_id,
            waited_ticks,
            starvation_guard,
        }
    }

    fn select_next_offset(&self) -> (usize, bool) {
        let mut selected = 0usize;
        let mut longest_wait = 0u64;

        for offset in 0..self.len {
            let index = (self.head + offset) % QUEUE_CAPACITY;
            let waited = self
                .timer_ticks
                .saturating_sub(self.queue[index].ready_since);
            if waited > longest_wait {
                selected = offset;
                longest_wait = waited;
            }
        }

        if longest_wait >= STARVATION_LIMIT_TICKS {
            (selected, selected != 0)
        } else {
            (0, false)
        }
    }

    fn selftest(&self) -> bool {
        self.initialized
            && self.len <= QUEUE_CAPACITY
            && self.queue_capacity_consistent()
            && !self.queue_has_duplicates()
            && !self.blocked_has_duplicates()
            && !self.queue_blocked_overlap()
            && self.quantum_ticks > 0
            && model_selftest()
    }

    fn contains(&self, task_id: u64) -> bool {
        for offset in 0..self.len {
            let index = (self.head + offset) % QUEUE_CAPACITY;
            if self.queue[index].task_id == task_id {
                return true;
            }
        }
        false
    }

    fn remove(&mut self, task_id: u64) -> bool {
        for offset in 0..self.len {
            let index = (self.head + offset) % QUEUE_CAPACITY;
            if self.queue[index].task_id == task_id {
                self.remove_at(offset);
                return true;
            }
        }
        false
    }

    fn remove_at(&mut self, offset: usize) -> QueueEntry {
        if offset >= self.len {
            return QueueEntry::empty();
        }

        let removed_index = (self.head + offset) % QUEUE_CAPACITY;
        let removed = self.queue[removed_index];
        for shift in offset..(self.len - 1) {
            let from = (self.head + shift + 1) % QUEUE_CAPACITY;
            let to = (self.head + shift) % QUEUE_CAPACITY;
            self.queue[to] = self.queue[from];
        }

        let tail = (self.head + self.len - 1) % QUEUE_CAPACITY;
        self.queue[tail] = QueueEntry::empty();
        self.len -= 1;
        if self.len == 0 {
            self.head = 0;
        }
        removed
    }

    fn contains_blocked(&self, task_id: u64) -> bool {
        self.blocked[..self.blocked_len].contains(&task_id)
    }

    fn remove_blocked(&mut self, task_id: u64) -> bool {
        let Some(index) = self.blocked[..self.blocked_len]
            .iter()
            .position(|blocked| *blocked == task_id)
        else {
            return false;
        };

        for shift in index..self.blocked_len.saturating_sub(1) {
            self.blocked[shift] = self.blocked[shift + 1];
        }
        self.blocked_len -= 1;
        self.blocked[self.blocked_len] = 0;
        true
    }

    fn queue_has_duplicates(&self) -> bool {
        for left in 0..self.len {
            let left_index = (self.head + left) % QUEUE_CAPACITY;
            let left_id = self.queue[left_index].task_id;
            if left_id == 0 {
                return true;
            }

            for right in (left + 1)..self.len {
                let right_index = (self.head + right) % QUEUE_CAPACITY;
                if self.queue[right_index].task_id == left_id {
                    return true;
                }
            }
        }
        false
    }

    fn queue_capacity_consistent(&self) -> bool {
        self.len <= QUEUE_CAPACITY
            && self.blocked_len <= QUEUE_CAPACITY
            && self.head < QUEUE_CAPACITY
    }

    fn blocked_has_duplicates(&self) -> bool {
        for left in 0..self.blocked_len {
            if self.blocked[left] == 0 {
                return true;
            }
            for right in (left + 1)..self.blocked_len {
                if self.blocked[left] == self.blocked[right] {
                    return true;
                }
            }
        }
        false
    }

    fn queue_blocked_overlap(&self) -> bool {
        for offset in 0..self.len {
            let index = (self.head + offset) % QUEUE_CAPACITY;
            if self.contains_blocked(self.queue[index].task_id) {
                return true;
            }
        }
        false
    }
}

fn model_selftest() -> bool {
    let mut round_robin = Scheduler::new();
    round_robin.initialized = true;
    round_robin.quantum_ticks = 1;
    round_robin.current_task = 1;
    let queued = round_robin.enqueue_at(2, 0) && round_robin.enqueue_at(3, 0);
    round_robin.on_timer_tick();
    let first = round_robin.preempt_current();
    round_robin.on_timer_tick();
    let second = round_robin.preempt_current();

    let mut starvation = Scheduler::new();
    starvation.initialized = true;
    starvation.quantum_ticks = 1;
    starvation.timer_ticks = STARVATION_LIMIT_TICKS + 4;
    starvation.current_task = 7;
    let recent = starvation.timer_ticks.saturating_sub(1);
    let starved = starvation
        .timer_ticks
        .saturating_sub(STARVATION_LIMIT_TICKS + 1);
    let starvation_queued = starvation.enqueue_at(8, recent) && starvation.enqueue_at(9, starved);
    starvation.on_timer_tick();
    let guarded = starvation.preempt_current();

    let mut blocking = Scheduler::new();
    blocking.initialized = true;
    let blocking_queued = blocking.enqueue_at(11, 0);
    let blocked = blocking.block_task(11);
    let blocked_snapshot = blocking.snapshot();
    let woke = blocking.wake_task(11);
    let wake_snapshot = blocking.snapshot();

    let mut handoff = Scheduler::new();
    handoff.initialized = true;
    handoff.current_task = 20;
    let handoff_queued = handoff.enqueue_at(21, 0) && handoff.enqueue_at(22, 0);
    let handoff_decision = handoff.block_current_and_dispatch(20);
    let handoff_snapshot = handoff.snapshot();
    let handoff_woke = handoff.wake_task(20);

    queued
        && first.switched
        && first.previous_task == 1
        && first.next_task == 2
        && !first.starvation_guard
        && second.switched
        && second.previous_task == 2
        && second.next_task == 3
        && starvation_queued
        && guarded.switched
        && guarded.next_task == 9
        && guarded.starvation_guard
        && starvation.starvation_preventions == 1
        && blocking_queued
        && blocked
        && blocked_snapshot.blocked_tasks == 1
        && blocked_snapshot.queued_tasks == 0
        && woke
        && wake_snapshot.blocked_tasks == 0
        && wake_snapshot.queued_tasks == 1
        && wake_snapshot.block_events == 1
        && wake_snapshot.wake_events == 1
        && handoff_queued
        && handoff_decision.switched
        && handoff_decision.previous_task == 20
        && handoff_decision.next_task == 21
        && handoff_snapshot.current_task == 21
        && handoff_snapshot.blocked_tasks == 1
        && handoff_snapshot.blocking_switches == 1
        && handoff_woke
}

struct SchedulerStore {
    value: UnsafeCell<Scheduler>,
}

unsafe impl Sync for SchedulerStore {}

static SCHEDULER: SchedulerStore = SchedulerStore {
    value: UnsafeCell::new(Scheduler::new()),
};

pub fn init() -> Snapshot {
    cpu_interrupts::without_interrupts(|| scheduler_mut().init());
    snapshot()
}

pub fn snapshot() -> Snapshot {
    cpu_interrupts::without_interrupts(|| scheduler().snapshot())
}

pub fn enqueue_task(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| scheduler_mut().enqueue(task_id))
}

pub fn begin_task(task_id: u64) {
    cpu_interrupts::without_interrupts(|| scheduler_mut().begin_task(task_id));
}

pub fn finish_task(task_id: u64) {
    cpu_interrupts::without_interrupts(|| scheduler_mut().finish_task(task_id));
}

pub fn yield_current() -> u64 {
    cpu_interrupts::without_interrupts(|| scheduler_mut().yield_current())
}

pub fn block_task(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| scheduler_mut().block_task(task_id))
}

pub fn block_current_and_dispatch(task_id: u64) -> PreemptionDecision {
    cpu_interrupts::without_interrupts(|| scheduler_mut().block_current_and_dispatch(task_id))
}

pub fn wake_task(task_id: u64) -> bool {
    cpu_interrupts::without_interrupts(|| scheduler_mut().wake_task(task_id))
}

pub fn on_timer_tick() {
    let scheduler = scheduler_mut();
    if scheduler.initialized {
        scheduler.on_timer_tick();
    }
}

pub fn preempt_current_from_irq() -> PreemptionDecision {
    scheduler_mut().preempt_current()
}

pub fn selftest() -> bool {
    cpu_interrupts::without_interrupts(|| scheduler().selftest())
}

fn scheduler() -> &'static Scheduler {
    unsafe { &*SCHEDULER.value.get() }
}

fn scheduler_mut() -> &'static mut Scheduler {
    unsafe { &mut *SCHEDULER.value.get() }
}
