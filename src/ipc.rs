use crate::{process, serial};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_ENDPOINTS: usize = 8;
pub const QUEUE_DEPTH: usize = 8;
pub const MAX_MESSAGE_BYTES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    NotInitialized,
    InvalidTask,
    EndpointCapacity,
    EndpointMissing,
    MessageTooLarge,
    QueueFull,
    QueueEmpty,
    BufferTooSmall,
    BlockFailed,
    WakeFailed,
}

#[derive(Clone, Copy)]
pub struct Delivery {
    pub sender: u64,
    pub length: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy)]
pub enum ReceiveOutcome {
    Message(Delivery),
    Blocked,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub initialized: bool,
    pub endpoint_capacity: u64,
    pub active_endpoints: u64,
    pub queue_depth: u64,
    pub max_message_bytes: u64,
    pub queued_messages: u64,
    pub waiting_receivers: u64,
    pub endpoints_created: u64,
    pub endpoints_destroyed: u64,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub blocked_receives: u64,
    pub receiver_wakeups: u64,
    pub queue_full_events: u64,
    pub dropped_on_cleanup: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Copy)]
struct Message {
    sender: u64,
    length: usize,
    sequence: u64,
    bytes: [u8; MAX_MESSAGE_BYTES],
}

impl Message {
    const fn empty() -> Self {
        Self {
            sender: 0,
            length: 0,
            sequence: 0,
            bytes: [0; MAX_MESSAGE_BYTES],
        }
    }
}

#[derive(Clone, Copy)]
struct Endpoint {
    owner: u64,
    queue: [Message; QUEUE_DEPTH],
    head: usize,
    len: usize,
    waiting: bool,
}

impl Endpoint {
    const fn empty() -> Self {
        Self {
            owner: 0,
            queue: [Message::empty(); QUEUE_DEPTH],
            head: 0,
            len: 0,
            waiting: false,
        }
    }

    fn push(&mut self, message: Message) -> bool {
        if self.len >= QUEUE_DEPTH {
            return false;
        }
        let tail = (self.head + self.len) % QUEUE_DEPTH;
        self.queue[tail] = message;
        self.len += 1;
        true
    }

    fn front(&self) -> Option<Message> {
        if self.len == 0 {
            None
        } else {
            Some(self.queue[self.head])
        }
    }

    fn pop(&mut self) -> Option<Message> {
        let message = self.front()?;
        self.queue[self.head] = Message::empty();
        self.head = (self.head + 1) % QUEUE_DEPTH;
        self.len -= 1;
        if self.len == 0 {
            self.head = 0;
        }
        Some(message)
    }
}

struct IpcState {
    initialized: bool,
    endpoints: [Endpoint; MAX_ENDPOINTS],
    endpoints_created: u64,
    endpoints_destroyed: u64,
    messages_sent: u64,
    messages_received: u64,
    blocked_receives: u64,
    receiver_wakeups: u64,
    queue_full_events: u64,
    dropped_on_cleanup: u64,
    next_sequence: u64,
    last_sequence: u64,
}

impl IpcState {
    const fn new() -> Self {
        Self {
            initialized: false,
            endpoints: [Endpoint::empty(); MAX_ENDPOINTS],
            endpoints_created: 0,
            endpoints_destroyed: 0,
            messages_sent: 0,
            messages_received: 0,
            blocked_receives: 0,
            receiver_wakeups: 0,
            queue_full_events: 0,
            dropped_on_cleanup: 0,
            next_sequence: 1,
            last_sequence: 0,
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }
        *self = Self::new();
        self.initialized = true;
    }

    fn register(&mut self, task_id: u64) -> Result<(), IpcError> {
        if !self.initialized {
            return Err(IpcError::NotInitialized);
        }
        if task_id == 0 {
            return Err(IpcError::InvalidTask);
        }
        if self.find(task_id).is_some() {
            return Ok(());
        }
        let Some(index) = self
            .endpoints
            .iter()
            .position(|endpoint| endpoint.owner == 0)
        else {
            return Err(IpcError::EndpointCapacity);
        };
        self.endpoints[index] = Endpoint::empty();
        self.endpoints[index].owner = task_id;
        self.endpoints_created = self.endpoints_created.saturating_add(1);
        Ok(())
    }

    fn unregister(&mut self, task_id: u64) -> Result<u64, IpcError> {
        let Some(index) = self.find(task_id) else {
            return Err(IpcError::EndpointMissing);
        };
        let dropped = self.endpoints[index].len as u64;
        self.endpoints[index] = Endpoint::empty();
        self.endpoints_destroyed = self.endpoints_destroyed.saturating_add(1);
        self.dropped_on_cleanup = self.dropped_on_cleanup.saturating_add(dropped);
        Ok(dropped)
    }

    fn send(&mut self, sender: u64, receiver: u64, bytes: &[u8]) -> Result<(u64, bool), IpcError> {
        if !self.initialized {
            return Err(IpcError::NotInitialized);
        }
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(IpcError::MessageTooLarge);
        }
        if self.find(sender).is_none() {
            return Err(IpcError::EndpointMissing);
        }
        let Some(receiver_index) = self.find(receiver) else {
            return Err(IpcError::EndpointMissing);
        };
        if self.endpoints[receiver_index].len >= QUEUE_DEPTH {
            self.queue_full_events = self.queue_full_events.saturating_add(1);
            return Err(IpcError::QueueFull);
        }

        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut message = Message::empty();
        message.sender = sender;
        message.length = bytes.len();
        message.sequence = sequence;
        message.bytes[..bytes.len()].copy_from_slice(bytes);
        let waiting = self.endpoints[receiver_index].waiting;
        self.endpoints[receiver_index].waiting = false;
        if !self.endpoints[receiver_index].push(message) {
            return Err(IpcError::QueueFull);
        }
        self.messages_sent = self.messages_sent.saturating_add(1);
        self.last_sequence = sequence;
        Ok((sequence, waiting))
    }

    fn receive(&mut self, receiver: u64, output: &mut [u8]) -> Result<Delivery, IpcError> {
        let Some(index) = self.find(receiver) else {
            return Err(IpcError::EndpointMissing);
        };
        let Some(message) = self.endpoints[index].front() else {
            return Err(IpcError::QueueEmpty);
        };
        if output.len() < message.length {
            return Err(IpcError::BufferTooSmall);
        }
        let message = self.endpoints[index].pop().ok_or(IpcError::QueueEmpty)?;
        output[..message.length].copy_from_slice(&message.bytes[..message.length]);
        self.messages_received = self.messages_received.saturating_add(1);
        Ok(Delivery {
            sender: message.sender,
            length: message.length as u64,
            sequence: message.sequence,
        })
    }

    fn set_waiting(&mut self, task_id: u64, waiting: bool) -> Result<(), IpcError> {
        let Some(index) = self.find(task_id) else {
            return Err(IpcError::EndpointMissing);
        };
        self.endpoints[index].waiting = waiting;
        Ok(())
    }

    fn find(&self, task_id: u64) -> Option<usize> {
        self.endpoints
            .iter()
            .position(|endpoint| endpoint.owner == task_id)
    }

    fn snapshot(&self) -> Snapshot {
        let mut active_endpoints = 0u64;
        let mut queued_messages = 0u64;
        let mut waiting_receivers = 0u64;
        for endpoint in self.endpoints.iter() {
            if endpoint.owner == 0 {
                continue;
            }
            active_endpoints = active_endpoints.saturating_add(1);
            queued_messages = queued_messages.saturating_add(endpoint.len as u64);
            if endpoint.waiting {
                waiting_receivers = waiting_receivers.saturating_add(1);
            }
        }

        Snapshot {
            initialized: self.initialized,
            endpoint_capacity: MAX_ENDPOINTS as u64,
            active_endpoints,
            queue_depth: QUEUE_DEPTH as u64,
            max_message_bytes: MAX_MESSAGE_BYTES as u64,
            queued_messages,
            waiting_receivers,
            endpoints_created: self.endpoints_created,
            endpoints_destroyed: self.endpoints_destroyed,
            messages_sent: self.messages_sent,
            messages_received: self.messages_received,
            blocked_receives: self.blocked_receives,
            receiver_wakeups: self.receiver_wakeups,
            queue_full_events: self.queue_full_events,
            dropped_on_cleanup: self.dropped_on_cleanup,
            last_sequence: self.last_sequence,
        }
    }

    fn invariants(&self) -> bool {
        self.initialized
            && self
                .endpoints
                .iter()
                .all(|endpoint| endpoint.len <= QUEUE_DEPTH && endpoint.head < QUEUE_DEPTH)
            && endpoint_owners_unique(&self.endpoints)
    }
}

struct IpcStore {
    value: UnsafeCell<IpcState>,
}

unsafe impl Sync for IpcStore {}

static IPC: IpcStore = IpcStore {
    value: UnsafeCell::new(IpcState::new()),
};

pub fn init() -> Snapshot {
    cpu_interrupts::without_interrupts(|| state_mut().init());
    serial::log("ipc", "bounded mailbox ready");
    serial::log_u64("ipc", "endpoint capacity", MAX_ENDPOINTS as u64);
    serial::log_u64("ipc", "queue depth", QUEUE_DEPTH as u64);
    snapshot()
}

pub fn register_endpoint(task_id: u64) -> Result<(), IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().register(task_id))
}

pub fn unregister_endpoint(task_id: u64) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().unregister(task_id))
}

pub fn send(sender: u64, receiver: u64, bytes: &[u8]) -> Result<u64, IpcError> {
    let (sequence, wake_receiver) =
        cpu_interrupts::without_interrupts(|| state_mut().send(sender, receiver, bytes))?;
    if wake_receiver {
        if process::wake_from_ipc(receiver) {
            cpu_interrupts::without_interrupts(|| {
                let state = state_mut();
                state.receiver_wakeups = state.receiver_wakeups.saturating_add(1);
            });
        } else {
            cpu_interrupts::without_interrupts(|| {
                let _ = state_mut().set_waiting(receiver, true);
            });
            return Err(IpcError::WakeFailed);
        }
    }
    Ok(sequence)
}

pub fn receive(
    receiver: u64,
    output: &mut [u8],
    block_when_empty: bool,
) -> Result<ReceiveOutcome, IpcError> {
    cpu_interrupts::without_interrupts(|| match state_mut().receive(receiver, output) {
        Ok(delivery) => Ok(ReceiveOutcome::Message(delivery)),
        Err(IpcError::QueueEmpty) if block_when_empty => {
            state_mut().set_waiting(receiver, true)?;
            if !process::block_for_ipc(receiver) {
                let _ = state_mut().set_waiting(receiver, false);
                return Err(IpcError::BlockFailed);
            }
            let state = state_mut();
            state.blocked_receives = state.blocked_receives.saturating_add(1);
            Ok(ReceiveOutcome::Blocked)
        }
        Err(error) => Err(error),
    })
}

pub fn snapshot() -> Snapshot {
    cpu_interrupts::without_interrupts(|| state().snapshot())
}

pub fn selftest() -> bool {
    cpu_interrupts::without_interrupts(|| state().invariants()) && model_selftest()
}

pub fn error_code(error: IpcError) -> u64 {
    match error {
        IpcError::NotInitialized => 1,
        IpcError::InvalidTask => 2,
        IpcError::EndpointCapacity => 3,
        IpcError::EndpointMissing => 4,
        IpcError::MessageTooLarge => 5,
        IpcError::QueueFull => 6,
        IpcError::QueueEmpty => 7,
        IpcError::BufferTooSmall => 8,
        IpcError::BlockFailed => 9,
        IpcError::WakeFailed => 10,
    }
}

pub fn error_name(error: IpcError) -> &'static str {
    match error {
        IpcError::NotInitialized => "not initialized",
        IpcError::InvalidTask => "invalid task",
        IpcError::EndpointCapacity => "endpoint capacity",
        IpcError::EndpointMissing => "endpoint missing",
        IpcError::MessageTooLarge => "message too large",
        IpcError::QueueFull => "queue full",
        IpcError::QueueEmpty => "queue empty",
        IpcError::BufferTooSmall => "buffer too small",
        IpcError::BlockFailed => "block failed",
        IpcError::WakeFailed => "wake failed",
    }
}

fn model_selftest() -> bool {
    let mut state = IpcState::new();
    state.init();
    let registered = state.register(1).is_ok() && state.register(2).is_ok();
    let sent = state.send(1, 2, b"ping");
    let mut output = [0u8; MAX_MESSAGE_BYTES];
    let received = state.receive(2, &mut output);
    registered
        && matches!(sent, Ok((1, false)))
        && matches!(
            received,
            Ok(Delivery {
                sender: 1,
                length: 4,
                sequence: 1
            })
        )
        && &output[..4] == b"ping"
        && state.unregister(1).is_ok()
        && state.unregister(2).is_ok()
        && state.snapshot().active_endpoints == 0
        && state.invariants()
}

fn endpoint_owners_unique(endpoints: &[Endpoint; MAX_ENDPOINTS]) -> bool {
    for left in 0..MAX_ENDPOINTS {
        let owner = endpoints[left].owner;
        if owner == 0 {
            continue;
        }
        for right in (left + 1)..MAX_ENDPOINTS {
            if endpoints[right].owner == owner {
                return false;
            }
        }
    }
    true
}

fn state() -> &'static IpcState {
    unsafe { &*IPC.value.get() }
}

fn state_mut() -> &'static mut IpcState {
    unsafe { &mut *IPC.value.get() }
}
