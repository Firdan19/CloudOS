use crate::{process, serial, user};
use core::cell::UnsafeCell;
use x86_64::instructions::interrupts as cpu_interrupts;

pub const MAX_ENDPOINTS: usize = 8;
pub const QUEUE_DEPTH: usize = 8;
pub const MAX_MESSAGE_BYTES: usize = 64;
pub const MAX_CAPABILITIES_PER_ENDPOINT: usize = 8;
pub const MAX_CAPABILITY_LINEAGE_DEPTH: u8 = 16;
const CAPABILITY_SLOT_BITS: u64 = 8;
const CAPABILITY_SLOT_MASK: u64 = (1 << CAPABILITY_SLOT_BITS) - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRights(u8);

impl CapabilityRights {
    pub const SEND: Self = Self(1 << 0);
    pub const RECEIVE: Self = Self(1 << 1);
    pub const CANCEL: Self = Self(1 << 2);
    pub const DELEGATE: Self = Self(1 << 3);
    pub const SEND_RECEIVE: Self = Self(Self::SEND.0 | Self::RECEIVE.0);
    pub const SEND_CANCEL: Self = Self(Self::SEND.0 | Self::CANCEL.0);
    pub const SEND_DELEGATE: Self = Self(Self::SEND.0 | Self::DELEGATE.0);
    pub const CANCEL_DELEGATE: Self = Self(Self::CANCEL.0 | Self::DELEGATE.0);
    pub const SEND_CANCEL_DELEGATE: Self = Self(Self::SEND.0 | Self::CANCEL.0 | Self::DELEGATE.0);
    pub const ALL: Self = Self(Self::SEND_RECEIVE.0 | Self::CANCEL.0 | Self::DELEGATE.0);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn from_bits(bits: u8) -> Option<Self> {
        let rights = Self(bits);
        if rights.valid() {
            Some(rights)
        } else {
            None
        }
    }

    pub const fn valid(self) -> bool {
        self.0 != 0 && self.0 & !Self::ALL.0 == 0
    }

    pub const fn contains(self, required: Self) -> bool {
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
    RightsEscalation,
    LineageDepthExceeded,
}

#[derive(Clone, Copy)]
pub struct Delivery {
    pub sender: u64,
    pub length: u64,
    pub sequence: u64,
    pub capability_handle: u64,
    pub capability_rights: u8,
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
    pub capability_transfers: u64,
    pub capability_transfer_failures: u64,
    pub rights_attenuations: u64,
    pub last_transferred_rights: u64,
    pub revocation_trees: u64,
    pub cascade_revocations: u64,
    pub descendant_revocations: u64,
    pub queued_capabilities_stripped: u64,
    pub last_revocation_size: u64,
}

#[derive(Clone, Copy)]
pub struct CapabilityInfo {
    pub slot: u64,
    pub handle: u64,
    pub target: u64,
    pub rights: u8,
    pub generation: u64,
    pub parent_generation: u64,
    pub lineage_depth: u8,
    pub active: bool,
}

#[derive(Clone, Copy)]
pub struct RevocationReport {
    pub revoked: u64,
    pub descendants: u64,
    pub queued_stripped: u64,
}

impl RevocationReport {
    const fn empty() -> Self {
        Self {
            revoked: 0,
            descendants: 0,
            queued_stripped: 0,
        }
    }

    fn merge(&mut self, other: Self) {
        self.revoked = self.revoked.saturating_add(other.revoked);
        self.descendants = self.descendants.saturating_add(other.descendants);
        self.queued_stripped = self.queued_stripped.saturating_add(other.queued_stripped);
    }
}

#[derive(Clone, Copy)]
struct Capability {
    target: u64,
    generation: u64,
    parent_generation: u64,
    lineage_depth: u8,
    rights: CapabilityRights,
    active: bool,
}

impl Capability {
    const fn empty() -> Self {
        Self {
            target: 0,
            generation: 0,
            parent_generation: 0,
            lineage_depth: 0,
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
    capability_handle: u64,
    capability_rights: u8,
    bytes: [u8; MAX_MESSAGE_BYTES],
}

impl Message {
    const fn empty() -> Self {
        Self {
            sender: 0,
            length: 0,
            sequence: 0,
            capability_handle: 0,
            capability_rights: 0,
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
    capability_transfers: u64,
    capability_transfer_failures: u64,
    rights_attenuations: u64,
    last_transferred_rights: u64,
    revocation_trees: u64,
    cascade_revocations: u64,
    descendant_revocations: u64,
    queued_capabilities_stripped: u64,
    last_revocation_size: u64,
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
            capability_transfers: 0,
            capability_transfer_failures: 0,
            rights_attenuations: 0,
            last_transferred_rights: 0,
            revocation_trees: 0,
            cascade_revocations: 0,
            descendant_revocations: 0,
            queued_capabilities_stripped: 0,
            last_revocation_size: 0,
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
            parent_generation: 0,
            lineage_depth: 0,
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
        let mut report = RevocationReport::empty();

        while let Some(generation) = self.endpoints[index]
            .capabilities
            .iter()
            .find(|capability| capability.active)
            .map(|capability| capability.generation)
        {
            report.merge(self.revoke_generation(generation));
        }
        while let Some(generation) = self.find_target_generation(task_id) {
            report.merge(self.revoke_generation(generation));
        }

        self.endpoints[index] = Endpoint::empty();
        self.endpoints_destroyed = self.endpoints_destroyed.saturating_add(1);
        self.dropped_on_cleanup = self.dropped_on_cleanup.saturating_add(dropped);
        self.record_revocation(report, true);
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
            parent_generation: 0,
            lineage_depth: 0,
            rights,
            active: true,
        };
        self.capabilities_granted = self.capabilities_granted.saturating_add(1);
        Ok(encode_handle(slot, generation))
    }

    fn revoke(&mut self, owner: u64, handle: u64) -> Result<RevocationReport, IpcError> {
        let Some(owner_index) = self.find(owner) else {
            return Err(IpcError::EndpointMissing);
        };
        let (slot, generation) = decode_handle(handle).ok_or(IpcError::InvalidCapability)?;
        let capability = self.endpoints[owner_index].capabilities[slot];
        if !capability.active || capability.generation != generation {
            self.record_denial(true);
            return Err(IpcError::StaleCapability);
        }
        let report = if slot == 0 {
            self.revoke_children(generation)
        } else {
            self.revoke_generation(generation)
        };
        self.record_revocation(report, false);
        Ok(report)
    }

    fn revoke_children(&mut self, generation: u64) -> RevocationReport {
        let mut report = RevocationReport::empty();
        while let Some(child_generation) = self.find_child_generation(generation) {
            self.revoke_branch(child_generation, true, &mut report);
        }
        report
    }

    fn revoke_generation(&mut self, generation: u64) -> RevocationReport {
        let mut report = RevocationReport::empty();
        self.revoke_branch(generation, false, &mut report);
        report
    }

    fn revoke_branch(&mut self, generation: u64, descendant: bool, report: &mut RevocationReport) {
        let mut removed = false;
        'remove: for endpoint in self.endpoints.iter_mut() {
            for capability in endpoint.capabilities.iter_mut() {
                if capability.active && capability.generation == generation {
                    *capability = Capability::empty();
                    removed = true;
                    break 'remove;
                }
            }
        }
        if !removed {
            return;
        }

        report.revoked = report.revoked.saturating_add(1);
        if descendant {
            report.descendants = report.descendants.saturating_add(1);
        }
        report.queued_stripped = report
            .queued_stripped
            .saturating_add(self.strip_queued_generation(generation));

        while let Some(child_generation) = self.find_child_generation(generation) {
            self.revoke_branch(child_generation, true, report);
        }
    }

    fn find_child_generation(&self, parent_generation: u64) -> Option<u64> {
        self.endpoints
            .iter()
            .flat_map(|endpoint| endpoint.capabilities.iter())
            .find(|capability| {
                capability.active && capability.parent_generation == parent_generation
            })
            .map(|capability| capability.generation)
    }

    fn find_target_generation(&self, target: u64) -> Option<u64> {
        self.endpoints
            .iter()
            .flat_map(|endpoint| endpoint.capabilities.iter())
            .find(|capability| capability.active && capability.target == target)
            .map(|capability| capability.generation)
    }

    fn strip_queued_generation(&mut self, generation: u64) -> u64 {
        let mut stripped = 0u64;
        for endpoint in self.endpoints.iter_mut() {
            for offset in 0..endpoint.len {
                let index = (endpoint.head + offset) % QUEUE_DEPTH;
                let message = &mut endpoint.queue[index];
                let Some((_, queued_generation)) = decode_handle(message.capability_handle) else {
                    continue;
                };
                if queued_generation == generation {
                    message.capability_handle = 0;
                    message.capability_rights = 0;
                    stripped = stripped.saturating_add(1);
                }
            }
        }
        stripped
    }

    fn record_revocation(&mut self, report: RevocationReport, cleanup: bool) {
        if report.revoked == 0 {
            return;
        }
        self.revocation_trees = self.revocation_trees.saturating_add(1);
        if report.descendants != 0 {
            self.cascade_revocations = self.cascade_revocations.saturating_add(1);
        }
        self.capabilities_revoked = self.capabilities_revoked.saturating_add(report.revoked);
        if cleanup {
            self.capabilities_revoked_on_cleanup = self
                .capabilities_revoked_on_cleanup
                .saturating_add(report.revoked);
        }
        self.descendant_revocations = self
            .descendant_revocations
            .saturating_add(report.descendants);
        self.queued_capabilities_stripped = self
            .queued_capabilities_stripped
            .saturating_add(report.queued_stripped);
        self.last_revocation_size = report.revoked;
    }

    fn resolve(
        &mut self,
        owner: u64,
        handle: u64,
        required: CapabilityRights,
    ) -> Result<u64, IpcError> {
        Ok(self.resolve_capability(owner, handle, required)?.target)
    }

    fn resolve_capability(
        &mut self,
        owner: u64,
        handle: u64,
        required: CapabilityRights,
    ) -> Result<Capability, IpcError> {
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
            let report = self.revoke_generation(capability.generation);
            self.record_revocation(report, true);
            self.record_denial(true);
            return Err(IpcError::StaleCapability);
        }
        Ok(capability)
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

    fn send_with_capability(
        &mut self,
        sender: u64,
        destination_handle: u64,
        transfer_handle: u64,
        requested_rights: CapabilityRights,
        bytes: &[u8],
    ) -> Result<(u64, bool, u64, u64), IpcError> {
        let result = self.try_send_with_capability(
            sender,
            destination_handle,
            transfer_handle,
            requested_rights,
            bytes,
        );
        if result.is_err() {
            self.capability_transfer_failures = self.capability_transfer_failures.saturating_add(1);
        }
        result
    }

    fn try_send_with_capability(
        &mut self,
        sender: u64,
        destination_handle: u64,
        transfer_handle: u64,
        requested_rights: CapabilityRights,
        bytes: &[u8],
    ) -> Result<(u64, bool, u64, u64), IpcError> {
        if !requested_rights.valid() {
            self.record_denial(false);
            return Err(IpcError::InvalidRights);
        }
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(IpcError::MessageTooLarge);
        }

        let receiver = self.resolve(sender, destination_handle, CapabilityRights::SEND)?;
        let source =
            self.resolve_capability(sender, transfer_handle, CapabilityRights::DELEGATE)?;
        if !source.rights.contains(requested_rights) {
            self.record_denial(false);
            return Err(IpcError::RightsEscalation);
        }
        if requested_rights.contains(CapabilityRights::RECEIVE) && receiver != source.target {
            self.record_denial(false);
            return Err(IpcError::InvalidRights);
        }
        let Some(lineage_depth) = source.lineage_depth.checked_add(1) else {
            self.record_denial(false);
            return Err(IpcError::LineageDepthExceeded);
        };
        if lineage_depth > MAX_CAPABILITY_LINEAGE_DEPTH {
            self.record_denial(false);
            return Err(IpcError::LineageDepthExceeded);
        }

        let Some(receiver_index) = self.find(receiver) else {
            return Err(IpcError::EndpointMissing);
        };
        if self.endpoints[receiver_index].len >= QUEUE_DEPTH {
            self.queue_full_events = self.queue_full_events.saturating_add(1);
            return Err(IpcError::QueueFull);
        }
        let Some(recipient_slot) = self.endpoints[receiver_index]
            .capabilities
            .iter()
            .position(|capability| !capability.active)
        else {
            return Err(IpcError::CapabilityCapacity);
        };

        let sequence = self.next_sequence;
        let generation = self.allocate_generation();
        let recipient_handle = encode_handle(recipient_slot, generation);
        let mut message = Message::empty();
        message.sender = sender;
        message.length = bytes.len();
        message.sequence = sequence;
        message.capability_handle = recipient_handle;
        message.capability_rights = requested_rights.bits();
        message.bytes[..bytes.len()].copy_from_slice(bytes);

        let waiting = self.endpoints[receiver_index].waiting;
        self.endpoints[receiver_index].capabilities[recipient_slot] = Capability {
            target: source.target,
            generation,
            parent_generation: source.generation,
            lineage_depth,
            rights: requested_rights,
            active: true,
        };
        if !self.endpoints[receiver_index].push(message) {
            self.endpoints[receiver_index].capabilities[recipient_slot] = Capability::empty();
            return Err(IpcError::QueueFull);
        }
        self.endpoints[receiver_index].waiting = false;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.messages_sent = self.messages_sent.saturating_add(1);
        self.last_sequence = sequence;
        self.capabilities_granted = self.capabilities_granted.saturating_add(1);
        self.capability_transfers = self.capability_transfers.saturating_add(1);
        if requested_rights.bits() != source.rights.bits() {
            self.rights_attenuations = self.rights_attenuations.saturating_add(1);
        }
        self.last_transferred_rights = requested_rights.bits() as u64;
        Ok((sequence, waiting, receiver, recipient_handle))
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
        let max_generation = u64::MAX >> CAPABILITY_SLOT_BITS;
        loop {
            let generation = self.next_capability_generation.max(1);
            self.next_capability_generation = if generation >= max_generation {
                1
            } else {
                generation + 1
            };
            if find_capability_generation(&self.endpoints, generation).is_none() {
                self.last_capability_generation = generation;
                return generation;
            }
        }
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
            capability_handle: message.capability_handle,
            capability_rights: message.capability_rights,
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
            generation: capability.generation,
            parent_generation: capability.parent_generation,
            lineage_depth: capability.lineage_depth,
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
            capability_transfers: self.capability_transfers,
            capability_transfer_failures: self.capability_transfer_failures,
            rights_attenuations: self.rights_attenuations,
            last_transferred_rights: self.last_transferred_rights,
            revocation_trees: self.revocation_trees,
            cascade_revocations: self.cascade_revocations,
            descendant_revocations: self.descendant_revocations,
            queued_capabilities_stripped: self.queued_capabilities_stripped,
            last_revocation_size: self.last_revocation_size,
        }
    }

    fn invariants(&self) -> bool {
        self.initialized
            && self
                .endpoints
                .iter()
                .all(|endpoint| endpoint.len <= QUEUE_DEPTH && endpoint.head < QUEUE_DEPTH)
            && endpoint_owners_unique(&self.endpoints)
            && capability_generations_unique(&self.endpoints)
            && capability_tables_valid(&self.endpoints)
            && queued_capabilities_valid(&self.endpoints)
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
    revoke_capability_tree(owner, handle).map(|_| ())
}

pub fn revoke_capability_tree(owner: u64, handle: u64) -> Result<RevocationReport, IpcError> {
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
    cpu_interrupts::without_interrupts(|| {
        let (sequence, wake_receiver) = state_mut().send(sender, receiver, bytes)?;
        finish_send_locked(sequence, wake_receiver, receiver)
    })
}

pub fn send_capability(sender: u64, handle: u64, bytes: &[u8]) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| {
        let (sequence, wake_receiver, receiver) =
            state_mut().send_capability(sender, handle, bytes)?;
        finish_send_locked(sequence, wake_receiver, receiver)
    })
}

pub fn send_with_capability(
    sender: u64,
    destination_handle: u64,
    transfer_handle: u64,
    requested_rights: CapabilityRights,
    bytes: &[u8],
) -> Result<u64, IpcError> {
    cpu_interrupts::without_interrupts(|| {
        let (sequence, wake_receiver, receiver, recipient_handle) = state_mut()
            .send_with_capability(
                sender,
                destination_handle,
                transfer_handle,
                requested_rights,
                bytes,
            )?;
        finish_send_locked(sequence, wake_receiver, receiver)?;
        serial::log_hex_u64("ipc-cap", "transferred handle", recipient_handle);
        serial::log_hex_u64(
            "ipc-cap",
            "transferred rights",
            requested_rights.bits() as u64,
        );
        Ok(sequence)
    })
}

fn finish_send_locked(sequence: u64, wake_receiver: bool, receiver: u64) -> Result<u64, IpcError> {
    if wake_receiver {
        if process::wake_from_ipc(receiver) {
            let state = state_mut();
            state.receiver_wakeups = state.receiver_wakeups.saturating_add(1);
        } else {
            let _ = state_mut().set_waiting(receiver, true);
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
        IpcError::RightsEscalation => 19,
        IpcError::LineageDepthExceeded => 20,
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
        IpcError::RightsEscalation => "capability rights escalation",
        IpcError::LineageDepthExceeded => "capability lineage depth exceeded",
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
                sequence: 1,
                ..
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
        && transfer_model_selftest()
        && revocation_tree_model_selftest()
}

fn transfer_model_selftest() -> bool {
    let mut state = IpcState::new();
    state.init();
    if state.register(1).is_err() || state.register(2).is_err() || state.register(3).is_err() {
        return false;
    }

    let Ok(destination) = state.grant(1, 2, CapabilityRights::SEND) else {
        return false;
    };
    let Ok(source) = state.grant(1, 3, CapabilityRights::SEND_DELEGATE) else {
        return false;
    };
    let Ok((sequence, waiting, receiver, _)) =
        state.send_with_capability(1, destination, source, CapabilityRights::SEND, b"grant")
    else {
        return false;
    };
    let mut output = [0u8; MAX_MESSAGE_BYTES];
    let Ok(delivery) = state.receive(2, &mut output) else {
        return false;
    };
    let attenuated = sequence == 1
        && !waiting
        && receiver == 2
        && delivery.capability_handle != 0
        && delivery.capability_rights == CapabilityRights::SEND.bits()
        && state.resolve(2, delivery.capability_handle, CapabilityRights::SEND) == Ok(3)
        && state.resolve(2, delivery.capability_handle, CapabilityRights::CANCEL)
            == Err(IpcError::PermissionDenied)
        && &output[..5] == b"grant";
    let delegation_denied = state.send_with_capability(
        1,
        destination,
        destination,
        CapabilityRights::SEND,
        b"delegate",
    ) == Err(IpcError::PermissionDenied);
    let escalation_denied = state.send_with_capability(
        1,
        destination,
        source,
        CapabilityRights::SEND_CANCEL,
        b"escalate",
    ) == Err(IpcError::RightsEscalation);

    for index in 0..QUEUE_DEPTH {
        if state
            .send_capability(1, destination, &[index as u8])
            .is_err()
        {
            return false;
        }
    }
    let caps_before_full_queue = state.snapshot().active_capabilities;
    let queue_full_atomic =
        state.send_with_capability(1, destination, source, CapabilityRights::SEND, b"full")
            == Err(IpcError::QueueFull)
            && state.snapshot().active_capabilities == caps_before_full_queue;
    for _ in 0..QUEUE_DEPTH {
        if state.receive(2, &mut output).is_err() {
            return false;
        }
    }

    while state.endpoints[state.find(2).unwrap_or(0)]
        .capabilities
        .iter()
        .any(|capability| !capability.active)
    {
        if state.grant(2, 3, CapabilityRights::SEND).is_err() {
            return false;
        }
    }
    let queued_before_full_table = state.snapshot().queued_messages;
    let table_full_atomic =
        state.send_with_capability(1, destination, source, CapabilityRights::SEND, b"no-slot")
            == Err(IpcError::CapabilityCapacity)
            && state.snapshot().queued_messages == queued_before_full_table;

    attenuated
        && delegation_denied
        && escalation_denied
        && queue_full_atomic
        && table_full_atomic
        && state.snapshot().capability_transfers == 1
        && state.snapshot().rights_attenuations == 1
        && state.snapshot().capability_transfer_failures == 4
        && state.invariants()
}

fn revocation_tree_model_selftest() -> bool {
    let mut state = IpcState::new();
    state.init();
    if state.register(1).is_err() || state.register(2).is_err() || state.register(3).is_err() {
        return false;
    }

    let Ok(destination) = state.grant(1, 2, CapabilityRights::SEND) else {
        return false;
    };
    let Ok(root) = state.grant(1, 3, CapabilityRights::SEND_CANCEL_DELEGATE) else {
        return false;
    };
    if state
        .send_with_capability(
            1,
            destination,
            root,
            CapabilityRights::SEND_DELEGATE,
            b"child",
        )
        .is_err()
    {
        return false;
    }
    let mut output = [0u8; MAX_MESSAGE_BYTES];
    let Ok(child_delivery) = state.receive(2, &mut output) else {
        return false;
    };
    let child = child_delivery.capability_handle;
    let Ok((_, _, _, grandchild)) =
        state.send_with_capability(2, child, child, CapabilityRights::SEND, b"leaf")
    else {
        return false;
    };
    let Ok(cascade) = state.revoke(1, root) else {
        return false;
    };
    let Ok(stripped_delivery) = state.receive(3, &mut output) else {
        return false;
    };
    let queued_revoke_safe = cascade.revoked == 3
        && cascade.descendants == 2
        && cascade.queued_stripped == 1
        && stripped_delivery.capability_handle == 0
        && stripped_delivery.capability_rights == 0
        && &output[..4] == b"leaf";
    let descendants_stale = state.send_capability(2, child, b"stale")
        == Err(IpcError::StaleCapability)
        && state.send_capability(3, grandchild, b"stale") == Err(IpcError::StaleCapability);

    let Ok(revoke_first) = state.grant(1, 3, CapabilityRights::SEND_DELEGATE) else {
        return false;
    };
    let revoke_before_send = state.revoke(1, revoke_first).is_ok()
        && state.send_with_capability(
            1,
            destination,
            revoke_first,
            CapabilityRights::SEND,
            b"late",
        ) == Err(IpcError::StaleCapability);

    let Ok(receive_first_root) = state.grant(1, 3, CapabilityRights::SEND_DELEGATE) else {
        return false;
    };
    if state
        .send_with_capability(
            1,
            destination,
            receive_first_root,
            CapabilityRights::SEND,
            b"early",
        )
        .is_err()
    {
        return false;
    }
    let Ok(receive_first_delivery) = state.receive(2, &mut output) else {
        return false;
    };
    let receive_first_child = receive_first_delivery.capability_handle;
    let receive_before_revoke = state.revoke(1, receive_first_root).is_ok()
        && state.send_capability(2, receive_first_child, b"stale")
            == Err(IpcError::StaleCapability);

    let Ok(cancel_root) = state.grant(1, 3, CapabilityRights::CANCEL_DELEGATE) else {
        return false;
    };
    if state
        .send_with_capability(
            1,
            destination,
            cancel_root,
            CapabilityRights::CANCEL,
            b"cancel",
        )
        .is_err()
    {
        return false;
    }
    let Ok(cancel_delivery) = state.receive(2, &mut output) else {
        return false;
    };
    let cancel_child = cancel_delivery.capability_handle;
    let revoke_before_cancel = state.revoke(1, cancel_root).is_ok()
        && state.resolve(2, cancel_child, CapabilityRights::CANCEL)
            == Err(IpcError::StaleCapability);

    let Ok(self_root) = state.self_capability(1) else {
        return false;
    };
    if state
        .send_with_capability(1, destination, self_root, CapabilityRights::SEND, b"self")
        .is_err()
    {
        return false;
    }
    let Ok(self_child_delivery) = state.receive(2, &mut output) else {
        return false;
    };
    let self_child = self_child_delivery.capability_handle;
    let self_report = state.revoke(1, self_root);
    let self_root_preserved = matches!(
        self_report,
        Ok(report) if report.revoked == 1 && report.descendants == 1
    ) && state.self_capability(1) == Ok(self_root)
        && state.send_capability(2, self_child, b"stale") == Err(IpcError::StaleCapability);

    queued_revoke_safe
        && descendants_stale
        && revoke_before_send
        && receive_before_revoke
        && revoke_before_cancel
        && self_root_preserved
        && state.snapshot().cascade_revocations >= 4
        && state.snapshot().descendant_revocations >= 5
        && state.snapshot().queued_capabilities_stripped >= 1
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
            || self_capability.parent_generation != 0
            || self_capability.lineage_depth != 0
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
                || capability.lineage_depth > MAX_CAPABILITY_LINEAGE_DEPTH
                || !endpoints
                    .iter()
                    .any(|target| target.owner == capability.target)
            {
                return false;
            }
            if capability.parent_generation == 0 {
                if capability.lineage_depth != 0 {
                    return false;
                }
                continue;
            }
            let Some(parent) = find_capability_generation(endpoints, capability.parent_generation)
            else {
                return false;
            };
            if parent.generation == capability.generation
                || parent.target != capability.target
                || !parent.rights.contains(CapabilityRights::DELEGATE)
                || !parent.rights.contains(capability.rights)
                || parent.lineage_depth.checked_add(1) != Some(capability.lineage_depth)
            {
                return false;
            }
        }
    }
    true
}

fn capability_generations_unique(endpoints: &[Endpoint; MAX_ENDPOINTS]) -> bool {
    for left_endpoint in 0..MAX_ENDPOINTS {
        for left_slot in 0..MAX_CAPABILITIES_PER_ENDPOINT {
            let left = endpoints[left_endpoint].capabilities[left_slot];
            if !left.active {
                continue;
            }
            for right_endpoint in left_endpoint..MAX_ENDPOINTS {
                let first_slot = if right_endpoint == left_endpoint {
                    left_slot + 1
                } else {
                    0
                };
                for right_slot in first_slot..MAX_CAPABILITIES_PER_ENDPOINT {
                    let right = endpoints[right_endpoint].capabilities[right_slot];
                    if right.active && right.generation == left.generation {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn find_capability_generation(
    endpoints: &[Endpoint; MAX_ENDPOINTS],
    generation: u64,
) -> Option<Capability> {
    endpoints
        .iter()
        .flat_map(|endpoint| endpoint.capabilities.iter())
        .find(|capability| capability.active && capability.generation == generation)
        .copied()
}

fn queued_capabilities_valid(endpoints: &[Endpoint; MAX_ENDPOINTS]) -> bool {
    for endpoint in endpoints.iter() {
        if endpoint.owner == 0 {
            continue;
        }
        for offset in 0..endpoint.len {
            let message = endpoint.queue[(endpoint.head + offset) % QUEUE_DEPTH];
            if message.capability_handle == 0 {
                if message.capability_rights != 0 {
                    return false;
                }
                continue;
            }
            let Some((slot, generation)) = decode_handle(message.capability_handle) else {
                return false;
            };
            let capability = endpoint.capabilities[slot];
            if !capability.active
                || capability.generation != generation
                || capability.rights.bits() != message.capability_rights
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
