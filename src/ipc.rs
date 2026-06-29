use crate::{process, serial, user};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_ENDPOINTS: usize = 8;
pub const QUEUE_DEPTH: usize = 8;
pub const MAX_MESSAGE_BYTES: usize = 64;
pub const MAX_CAPABILITIES_PER_ENDPOINT: usize = 8;
const CAPABILITY_SLOT_BITS: u64 = 8;
const CAPABILITY_SLOT_MASK: u64 = (1 << CAPABILITY_SLOT_BITS) - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRights(u8);

impl CapabilityRights {
    pub const SEND: Self = Self(1 << 0);
    pub const RECEIVE: Self = Self(1 << 1);
    pub const CANCEL: Self = Self(1 << 2);
    pub const SEND_RECEIVE: Self = Self(Self::SEND.0 | Self::RECEIVE.0);
    pub const ALL: Self = Self(Self::SEND_RECEIVE.0 | Self::CANCEL.0);

    pub const fn bits(self) -> u8 {
        self.0
    }

    const fn valid(self) -> bool {
        self.0 != 0 && self.0 & !Self::ALL.0 == 0
    }

    const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    CapabilityCapacity,
    InvalidCapability,
    StaleCapability,
    PermissionDenied,
    InvalidRights,
    Timeout,
    Cancelled,
    NotWaiting,
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
pub enum SyscallBlockOutcome {
    MessageReady,
    Switched(u64),
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
    pub capability_slots_per_endpoint: u64,
    pub active_capabilities: u64,
    pub capabilities_granted: u64,
    pub capabilities_revoked: u64,
    pub capabilities_revoked_on_cleanup: u64,
    pub capability_denials: u64,
    pub stale_capability_denials: u64,
    pub last_capability_generation: u64,
    pub cancellation_requests: u64,
    pub cancellation_successes: u64,
}

#[derive(Clone, Copy)]
pub struct CapabilityInfo {
    pub slot: u64,
    pub handle: u64,
    pub target: u64,
    pub rights: u8,
    pub active: bool,
}

#[derive(Clone, Copy)]
struct Capability {
    target: u64,
    generation: u64,
    rights: CapabilityRights,
    active: bool,
}

impl Capability {
    const fn empty() -> Self {
        Self {
            target: 0,
            generation: 0,
            rights: CapabilityRights(0),
            active: false,
        }
    }
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
    capabilities: [Capability; MAX_CAPABILITIES_PER_ENDPOINT],
}

impl Endpoint {
    const fn empty() -> Self {
        Self {
            owner: 0,
            queue: [Message::empty(); QUEUE_DEPTH],
            head: 0,
            len: 0,
            waiting: false,
            capabilities: [Capability::empty(); MAX_CAPABILITIES_PER_ENDPOINT],
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
    next_capability_generation: u64,
    capabilities_granted: u64,
    capabilities_revoked: u64,
    capabilities_revoked_on_cleanup: u64,
    capability_denials: u64,
    stale_capability_denials: u64,
    last_capability_generation: u64,
    cancellation_requests: u64,
    cancellation_successes: u64,
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
            next_capability_generation: 1,
            capabilities_granted: 0,
            capabilities_revoked: 0,
            capabilities_revoked_on_cleanup: 0,
            capability_denials: 0,
            stale_capability_denials: 0,
            last_capability_generation: 0,
            cancellation_requests: 0,
            cancellation_successes: 0,
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
        let generation = self.allocate_generation();
        self.endpoints[index].capabilities[0] = Capability {
            target: task_id,
            generation,
            rights: CapabilityRights::ALL,
            active: true,
        };
        self.endpoints_created = self.endpoints_created.saturating_add(1);
        self.capabilities_granted = self.capabilities_granted.saturating_add(1);
        Ok(())
    }

    fn unregister(&mut self, task_id: u64) -> Result<u64, IpcError> {
        let Some(index) = self.find(task_id) else {
            return Err(IpcError::EndpointMissing);
        };
        let dropped = self.endpoints[index].len as u64;
        let mut revoked = self.endpoints[index]
            .capabilities
            .iter()
            .filter(|capability| capability.active)
            .count() as u64;
        self.endpoints[index] = Endpoint::empty();
        for endpoint in self.endpoints.iter_mut() {
            if endpoint.owner == 0 {
                continue;
            }
            for capability in endpoint.capabilities.iter_mut() {
                if capability.active && capability.target == task_id {
                    *capability = Capability::empty();
                    revoked = revoked.saturating_add(1);
                }
            }
        }
        self.endpoints_destroyed = self.endpoints_destroyed.saturating_add(1);
        self.dropped_on_cleanup = self.dropped_on_cleanup.saturating_add(dropped);
        self.capabilities_revoked = self.capabilities_revoked.saturating_add(revoked);
        self.capabilities_revoked_on_cleanup =
            self.capabilities_revoked_on_cleanup.saturating_add(revoked);
        Ok(dropped)
    }

    fn self_capability(&self, task_id: u64) -> Result<u64, IpcError> {
        let Some(index) = self.find(task_id) else {
            return Err(IpcError::EndpointMissing);
        };
        let capability = self.endpoints[index].capabilities[0];
        if !capability.active
            || capability.target != task_id
            || !capability.rights.contains(CapabilityRights::SEND_RECEIVE)
        {
            return Err(IpcError::PermissionDenied);
        }
        Ok(encode_handle(0, capability.generation))
    }

    fn grant(
        &mut self,
        owner: u64,
        target: u64,
        rights: CapabilityRights,
    ) -> Result<u64, IpcError> {
        if !rights.valid() || (rights.contains(CapabilityRights::RECEIVE) && owner != target) {
            return Err(IpcError::InvalidRights);
        }
        if self.find(target).is_none() {
            return Err(IpcError::EndpointMissing);
        }
        let Some(owner_index) = self.find(owner) else {
            return Err(IpcError::EndpointMissing);
        };
        let Some(slot) = self.endpoints[owner_index]
            .capabilities
            .iter()
            .position(|capability| !capability.active)
        else {
            return Err(IpcError::CapabilityCapacity);
        };

        let generation = self.allocate_generation();
        self.endpoints[owner_index].capabilities[slot] = Capability {
            target,
            generation,
            rights,
            active: true,
        };
        self.capabilities_granted = self.capabilities_granted.saturating_add(1);
        Ok(encode_handle(slot, generation))
    }

    fn revoke(&mut self, owner: u64, handle: u64) -> Result<(), IpcError> {
        let Some(owner_index) = self.find(owner) else {
            return Err(IpcError::EndpointMissing);
        };
        let (slot, generation) = decode_handle(handle).ok_or(IpcError::InvalidCapability)?;
        let capability = self.endpoints[owner_index].capabilities[slot];
        if !capability.active || capability.generation != generation {
            self.record_denial(true);
            return Err(IpcError::StaleCapability);
        }
        if slot == 0 {
            self.record_denial(false);
            return Err(IpcError::PermissionDenied);
        }

        self.endpoints[owner_index].capabilities[slot] = Capability::empty();
        self.capabilities_revoked = self.capabilities_revoked.saturating_add(1);
        Ok(())
    }

    fn resolve(
        &mut self,
        owner: u64,
        handle: u64,
        required: CapabilityRights,
    ) -> Result<u64, IpcError> {
        let Some(owner_index) = self.find(owner) else {
            return Err(IpcError::EndpointMissing);
        };
        let Some((slot, generation)) = decode_handle(handle) else {
            self.record_denial(false);
            return Err(IpcError::InvalidCapability);
        };
        let capability = self.endpoints[owner_index].capabilities[slot];
        if !capability.active || capability.generation != generation {
            self.record_denial(true);
            return Err(IpcError::StaleCapability);
        }
        if !capability.rights.contains(required) {
            self.record_denial(false);
            return Err(IpcError::PermissionDenied);
        }
        if self.find(capability.target).is_none() {
            self.endpoints[owner_index].capabilities[slot] = Capability::empty();
            self.record_denial(true);
            return Err(IpcError::StaleCapability);
        }
        Ok(capability.target)
    }

    fn send_capability(
        &mut self,
        sender: u64,
        handle: u64,
        bytes: &[u8],
    ) -> Result<(u64, bool, u64), IpcError> {
        let receiver = self.resolve(sender, handle, CapabilityRights::SEND)?;
        let (sequence, waiting) = self.send(sender, receiver, bytes)?;
        Ok((sequence, waiting, receiver))
    }

    fn can_receive(&mut self, receiver: u64) -> Result<(), IpcError> {
        let handle = self.self_capability(receiver)?;
        let target = self.resolve(receiver, handle, CapabilityRights::RECEIVE)?;
        if target != receiver {
            self.record_denial(false);
            return Err(IpcError::PermissionDenied);
        }
        Ok(())
    }

    fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_capability_generation.max(1);
        let max_generation = u64::MAX >> CAPABILITY_SLOT_BITS;
        self.next_capability_generation = if generation >= max_generation {
            1
        } else {
            generation + 1
        };
        self.last_capability_generation = generation;
        generation
    }

    fn record_denial(&mut self, stale: bool) {
        self.capability_denials = self.capability_denials.saturating_add(1);
        if stale {
            self.stale_capability_denials = self.stale_capability_denials.saturating_add(1);
        }
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
        self.can_receive(receiver)?;
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

    fn has_message(&self, task_id: u64) -> Result<bool, IpcError> {
        let Some(index) = self.find(task_id) else {
            return Err(IpcError::EndpointMissing);
        };
        Ok(self.endpoints[index].len != 0)
    }

    fn capability_info(&self, owner: u64, slot: usize) -> Option<CapabilityInfo> {
        if slot >= MAX_CAPABILITIES_PER_ENDPOINT {
            return None;
        }
        let endpoint = self.endpoints.get(self.find(owner)?)?;
        let capability = endpoint.capabilities[slot];
        Some(CapabilityInfo {
            slot: slot as u64,
            handle: if capability.active {
                encode_handle(slot, capability.generation)
            } else {
                0
            },
            target: capability.target,
            rights: capability.rights.bits(),
            active: capability.active,
        })
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
        let mut active_capabilities = 0u64;
        for endpoint in self.endpoints.iter() {
            if endpoint.owner == 0 {
                continue;
            }
            active_endpoints = active_endpoints.saturating_add(1);
            queued_messages = queued_messages.saturating_add(endpoint.len as u64);
            if endpoint.waiting {
                waiting_receivers = waiting_receivers.saturating_add(1);
            }
            active_capabilities = active_capabilities.saturating_add(
                endpoint
                    .capabilities
                    .iter()
                    .filter(|capability| capability.active)
                    .count() as u64,
            );
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
            capability_slots_per_endpoint: MAX_CAPABILITIES_PER_ENDPOINT as u64,
            active_capabilities,
            capabilities_granted: self.capabilities_granted,
            capabilities_revoked: self.capabilities_revoked,
            capabilities_revoked_on_cleanup: self.capabilities_revoked_on_cleanup,
            capability_denials: self.capability_denials,
            stale_capability_denials: self.stale_capability_denials,
            last_capability_generation: self.last_capability_generation,
            cancellation_requests: self.cancellation_requests,
            cancellation_successes: self.cancellation_successes,
        }
    }

    fn invariants(&self) -> bool {
        self.initialized
            && self
                .endpoints
                .iter()
                .all(|endpoint| endpoint.len <= QUEUE_DEPTH && endpoint.head < QUEUE_DEPTH)
            && endpoint_owners_unique(&self.endpoints)
            && capability_tables_valid(&self.endpoints)
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
    serial::log_u64(
        "ipc",
        "capability slots per endpoint",
        MAX_CAPABILITIES_PER_ENDPOINT as u64,
    );
    snapshot()
}

pub fn register_endpoint(task_id: u64) -> Result<(), IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().register(task_id))
}

pub fn unregister_endpoint(task_id: u64) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().unregister(task_id))
}

pub fn self_capability(task_id: u64) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| state().self_capability(task_id))
}

pub fn grant_capability(
    owner: u64,
    target: u64,
    rights: CapabilityRights,
) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().grant(owner, target, rights))
}

pub fn revoke_capability(owner: u64, handle: u64) -> Result<(), IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().revoke(owner, handle))
}

pub fn cancel_wait(requester: u64, handle: u64) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| {
        let state = state_mut();
        state.cancellation_requests = state.cancellation_requests.saturating_add(1);
        let target = state_mut().resolve(requester, handle, CapabilityRights::CANCEL)?;
        if !process::cancel_ipc_wait(target) {
            return Err(IpcError::NotWaiting);
        }
        state_mut().set_waiting(target, false)?;
        let state = state_mut();
        state.cancellation_successes = state.cancellation_successes.saturating_add(1);
        Ok(target)
    })
}

pub fn clear_waiting(task_id: u64) -> Result<(), IpcError> {
    cpu_interrupts::without_interrupts(|| state_mut().set_waiting(task_id, false))
}

pub fn send(sender: u64, receiver: u64, bytes: &[u8]) -> Result<u64, IpcError> {
    let (sequence, wake_receiver) =
        cpu_interrupts::without_interrupts(|| state_mut().send(sender, receiver, bytes))?;
    finish_send(sequence, wake_receiver, receiver)
}

pub fn send_capability(sender: u64, handle: u64, bytes: &[u8]) -> Result<u64, IpcError> {
    let (sequence, wake_receiver, receiver) =
        cpu_interrupts::without_interrupts(|| state_mut().send_capability(sender, handle, bytes))?;
    finish_send(sequence, wake_receiver, receiver)
}

fn finish_send(sequence: u64, wake_receiver: bool, receiver: u64) -> Result<u64, IpcError> {
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

pub fn block_syscall(
    receiver: u64,
    frame: &mut user::SyscallFrame,
    deadline_tick: u64,
) -> Result<SyscallBlockOutcome, IpcError> {
    cpu_interrupts::without_interrupts(|| {
        state_mut().can_receive(receiver)?;
        if state().has_message(receiver)? {
            return Ok(SyscallBlockOutcome::MessageReady);
        }

        state_mut().set_waiting(receiver, true)?;
        let Some(address_space_root) =
            process::block_for_ipc_syscall(receiver, frame, deadline_tick)
        else {
            let _ = state_mut().set_waiting(receiver, false);
            return Err(IpcError::BlockFailed);
        };

        let state = state_mut();
        state.blocked_receives = state.blocked_receives.saturating_add(1);
        Ok(SyscallBlockOutcome::Switched(address_space_root))
    })
}

pub fn snapshot() -> Snapshot {
    cpu_interrupts::without_interrupts(|| state().snapshot())
}

pub fn capability_info(owner: u64, slot: usize) -> Option<CapabilityInfo> {
    cpu_interrupts::without_interrupts(|| state().capability_info(owner, slot))
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
        IpcError::CapabilityCapacity => 11,
        IpcError::InvalidCapability => 12,
        IpcError::StaleCapability => 13,
        IpcError::PermissionDenied => 14,
        IpcError::InvalidRights => 15,
        IpcError::Timeout => 16,
        IpcError::Cancelled => 17,
        IpcError::NotWaiting => 18,
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
        IpcError::CapabilityCapacity => "capability capacity",
        IpcError::InvalidCapability => "invalid capability",
        IpcError::StaleCapability => "stale capability",
        IpcError::PermissionDenied => "permission denied",
        IpcError::InvalidRights => "invalid capability rights",
        IpcError::Timeout => "receive timeout",
        IpcError::Cancelled => "receive cancelled",
        IpcError::NotWaiting => "receiver is not waiting",
    }
}

fn model_selftest() -> bool {
    let mut state = IpcState::new();
    state.init();
    let registered = state.register(1).is_ok() && state.register(2).is_ok();
    let self_capability = state.self_capability(1);
    let send_capability = state.grant(1, 2, CapabilityRights::SEND);
    let receive_only = state.grant(1, 1, CapabilityRights::RECEIVE);
    let cancel_capability = state.grant(1, 2, CapabilityRights::CANCEL);
    let cancel_resolved =
        cancel_capability.and_then(|handle| state.resolve(1, handle, CapabilityRights::CANCEL));
    let sent = send_capability.and_then(|handle| state.send_capability(1, handle, b"ping"));
    let mut output = [0u8; MAX_MESSAGE_BYTES];
    let received = state.receive(2, &mut output);
    let permission_denied = receive_only
        .and_then(|handle| state.send_capability(1, handle, b"denied"))
        == Err(IpcError::PermissionDenied);
    let revoked = send_capability.and_then(|handle| state.revoke(1, handle).map(|_| handle));
    let stale_denied = revoked.and_then(|handle| state.send_capability(1, handle, b"stale"))
        == Err(IpcError::StaleCapability);
    registered
        && self_capability.is_ok()
        && matches!(sent, Ok((1, false, 2)))
        && matches!(
            received,
            Ok(Delivery {
                sender: 1,
                length: 4,
                sequence: 1
            })
        )
        && &output[..4] == b"ping"
        && permission_denied
        && stale_denied
        && cancel_resolved == Ok(2)
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

fn capability_tables_valid(endpoints: &[Endpoint; MAX_ENDPOINTS]) -> bool {
    for endpoint in endpoints.iter() {
        if endpoint.owner == 0 {
            if endpoint
                .capabilities
                .iter()
                .any(|capability| capability.active)
            {
                return false;
            }
            continue;
        }

        let self_capability = endpoint.capabilities[0];
        if !self_capability.active
            || self_capability.target != endpoint.owner
            || self_capability.generation == 0
            || !self_capability
                .rights
                .contains(CapabilityRights::SEND_RECEIVE)
        {
            return false;
        }

        for capability in endpoint.capabilities.iter() {
            if !capability.active {
                continue;
            }
            if capability.generation == 0
                || !capability.rights.valid()
                || !endpoints
                    .iter()
                    .any(|target| target.owner == capability.target)
            {
                return false;
            }
        }
    }
    true
}

fn encode_handle(slot: usize, generation: u64) -> u64 {
    (generation << CAPABILITY_SLOT_BITS) | (slot as u64 + 1)
}

fn decode_handle(handle: u64) -> Option<(usize, u64)> {
    let encoded_slot = handle & CAPABILITY_SLOT_MASK;
    let generation = handle >> CAPABILITY_SLOT_BITS;
    if encoded_slot == 0 || generation == 0 {
        return None;
    }
    let slot = encoded_slot.saturating_sub(1) as usize;
    if slot >= MAX_CAPABILITIES_PER_ENDPOINT {
        return None;
    }
    Some((slot, generation))
}

fn state() -> &'static IpcState {
    unsafe { &*IPC.value.get() }
}

fn state_mut() -> &'static mut IpcState {
    unsafe { &mut *IPC.value.get() }
}
