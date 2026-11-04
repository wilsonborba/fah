//! Literal xHCI contracts. No memory access or controller ownership here.
use super::OperationError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handle {
    pub session: u64,
    pub sequence: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmaSlice {
    pub handle: Handle,
    pub offset: usize,
    pub length: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ring {
    Command,
    Endpoint { slot: u8, endpoint: u8 },
    Stream { slot: u8, endpoint: u8, stream: u16 },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawOperation {
    DevicePacketSize {
        slot: u8,
        size: u16,
    },
    DeviceInterface {
        slot: u8,
        interface: u8,
        alternate: u8,
    },
    Barrier,
    Timing,
    Moderation {
        interval: u16,
        counter: u16,
    },
    ScatterTransfer {
        slot: u8,
        endpoint: u8,
        segments: Vec<DmaSlice>,
        max_packet: u16,
        input: bool,
    },
    NormalBuild {
        iova: u64,
        length: usize,
        max_packet: u16,
        input: bool,
    },
    StreamConfigure {
        slot: u8,
        endpoint: u8,
        entries: u16,
        device_max_streams: u8,
    },
    SlotEnable,
    AddressRoute {
        slot: u8,
        root_port: u8,
        speed: u8,
        route: u32,
        #[serde(default)]
        parent_slot: u8,
        #[serde(default)]
        parent_port: u8,
        #[serde(default)]
        multi_tt: bool,
        #[serde(default)]
        tt_think: u8,
        #[serde(default)]
        bsr: bool,
    },
    SlotContext {
        slot: u8,
    },
    HubConfigure {
        slot: u8,
        root_port: u8,
        ports: u8,
        #[serde(default)]
        multi_tt: bool,
        #[serde(default)]
        tt_think_ns: u16,
    },
    DeviceDescriptor {
        slot: u8,
        kind: u8,
        #[serde(default)]
        index: u8,
        #[serde(default)]
        language: u16,
        length: u16,
    },
    DeviceConfiguration {
        slot: u8,
        value: u8,
    },
    EndpointConfigure {
        slot: u8,
        interface: u8,
        alternate: u8,
        configuration: Vec<u8>,
    },
    IsoSchedule {
        slot: u8,
        endpoint: u8,
        asap: bool,
        frame_id: u16,
    },
    Inspect,
    RegisterRead {
        offset: usize,
        count: usize,
    },
    RegisterWrite {
        offset: usize,
        value: u32,
    },
    DmaAlloc {
        bytes: usize,
        #[serde(default = "alignment")]
        align: usize,
    },
    DmaWrite {
        handle: Handle,
        offset: usize,
        bytes: Vec<u8>,
    },
    DmaRead {
        handle: Handle,
        offset: usize,
        length: usize,
    },
    DmaInspect {
        handle: Handle,
    },
    DmaUnmap {
        handle: Handle,
    },
    Compose {
        words: [u32; 4],
    },
    Place {
        staged: Handle,
        ring: Ring,
    },
    Publish {
        token: Handle,
    },
    Doorbell {
        slot: u8,
        #[serde(default)]
        endpoint: u8,
        #[serde(default)]
        stream: u16,
    },
    Events {
        #[serde(default)]
        since: u64,
        #[serde(default = "event_limit")]
        limit: usize,
    },
    Drain {
        #[serde(default = "drain_timeout")]
        timeout_ms: u64,
    },
    Command {
        words: [u32; 4],
        #[serde(default = "drain_timeout")]
        timeout_ms: u64,
    },
    Transfer {
        slot: u8,
        endpoint: u8,
        trbs: Vec<[u32; 4]>,
    },
    Reconcile {
        #[serde(default)]
        slot: Option<u8>,
    },
    CommandBuild {
        command: CommandCode,
        #[serde(default)]
        parameter: u64,
        #[serde(default)]
        status: u32,
        #[serde(default)]
        control: u32,
    },
}
fn alignment() -> usize {
    4096
}
fn event_limit() -> usize {
    256
}
fn drain_timeout() -> u64 {
    1000
}
pub fn validate_streams(
    controller_max: u8,
    device_max: u8,
    entries: u16,
) -> Result<(), OperationError> {
    if controller_max == 0
        || device_max == 0
        || controller_max > 15
        || device_max > 16
        || entries < 4
        || !entries.is_power_of_two()
        || entries > 4096
        || (entries as u32) > (1u32 << (controller_max + 1))
        || (entries as u32 - 1) > (1u32 << device_max)
    {
        return Err(OperationError::validation(
            "unsupported controller/device stream array size",
        ));
    }
    Ok(())
}

/// Normal TRB buffers may not cross a 64KiB address boundary (xHCI6.4.1).
/// TD Size is packets remaining after complete packets through this TRB (4.11.2.4).
pub fn normal_td(
    iova: u64,
    length: usize,
    max_packet: u16,
    input: bool,
) -> Result<Vec<[u32; 4]>, OperationError> {
    if max_packet == 0 {
        return Err(OperationError::validation("zero maximum packet size"));
    }
    iova.checked_add(length as u64)
        .ok_or_else(|| OperationError::validation("DMA address overflow"))?;
    let packets = length.div_ceil(max_packet as usize);
    let mut offset = 0;
    let mut trbs = Vec::new();
    loop {
        let address = iova + offset as u64;
        let count = (length - offset).min(65536 - (address as usize & 65535));
        offset += count;
        let last = offset == length;
        let remaining = if last {
            0
        } else {
            packets.saturating_sub(offset / max_packet as usize).min(31)
        };
        trbs.push([
            address as u32,
            (address >> 32) as u32,
            count as u32 | ((remaining as u32) << 17),
            (1 << 10) | if last { 1 << 5 } else { 1 << 4 } | if input { 1 << 2 } else { 0 },
        ]);
        if last {
            break;
        }
    }
    Ok(trbs)
}

pub fn scatter_td(
    segments: &[(u64, usize)],
    max_packet: u16,
    input: bool,
) -> Result<Vec<[u32; 4]>, OperationError> {
    if segments.is_empty() || max_packet == 0 {
        return Err(OperationError::validation(
            "segments and packet size required",
        ));
    }
    let total = segments
        .iter()
        .try_fold(0usize, |sum, (_, length)| sum.checked_add(*length))
        .ok_or_else(|| OperationError::validation("scatter length overflow"))?;
    let packets = total.div_ceil(max_packet as usize);
    let mut trbs = Vec::new();
    for (iova, length) in segments {
        trbs.extend(normal_td(*iova, *length, max_packet, input)?);
    }
    let mut done = 0usize;
    let count = trbs.len();
    for (index, trb) in trbs.iter_mut().enumerate() {
        done += (trb[2] & 0x1ffff) as usize;
        let last = index + 1 == count;
        let size = if last {
            0
        } else {
            packets.saturating_sub(done / max_packet as usize).min(31)
        };
        trb[2] = (trb[2] & 0x1ffff) | ((size as u32) << 17);
        trb[3] = (trb[3] & !((1 << 4) | (1 << 5))) | if last { 1 << 5 } else { 1 << 4 };
    }
    Ok(trbs)
}

#[derive(Debug, Serialize)]
pub struct Protocol {
    pub offset: usize,
    pub revision: u16,
    pub name: [u8; 4],
    pub first_port: u8,
    pub port_count: u8,
    pub slot_type: u8,
    pub speeds: Vec<u32>,
    pub raw: Vec<u32>,
}
pub fn protocols(bytes: &[u8]) -> Result<Vec<Protocol>, OperationError> {
    let read = |offset: usize| -> Result<u32, OperationError> {
        let end = offset
            .checked_add(4)
            .ok_or_else(|| OperationError::validation("capability overflow"))?;
        let raw = bytes
            .get(offset..end)
            .ok_or_else(|| OperationError::validation("truncated extended capability"))?;
        Ok(u32::from_le_bytes(raw.try_into().unwrap()))
    };
    let mut offset = ((read(16)? >> 16) as usize) * 4;
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    while offset != 0 {
        if !seen.insert(offset) {
            return Err(OperationError::validation("cyclic extended capability"));
        }
        let header = read(offset)?;
        if header & 255 == 2 {
            let ports = read(offset + 8)?;
            let count = (ports >> 28) as usize;
            let mut raw = Vec::new();
            for i in 0..4 + count {
                raw.push(read(offset + i * 4)?);
            }
            let first_port = ports as u8;
            let port_count = (ports >> 8) as u8;
            if first_port == 0 || port_count == 0 || first_port as u16 + port_count as u16 > 256 {
                return Err(OperationError::validation("invalid protocol port range"));
            }
            result.push(Protocol {
                offset,
                revision: (header >> 16) as u16,
                name: raw[1].to_le_bytes(),
                first_port,
                port_count,
                slot_type: (raw[3] & 31) as u8,
                speeds: raw[4..].to_vec(),
                raw,
            });
        }
        let next = ((header >> 8) & 255) as usize;
        if next == 0 {
            break;
        }
        offset = offset
            .checked_add(next * 4)
            .ok_or_else(|| OperationError::validation("capability offset overflow"))?;
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionClass {
    Success,
    ShortPacket,
    Stopped,
    Error,
    Unknown,
}
/// Codes retain their raw value. Unknown or inapplicable values never mean success.
pub fn completion_class(event_type: u8, code: u8) -> CompletionClass {
    match (event_type, code) {
        (32 | 33, 1) => CompletionClass::Success,
        (32, 13) => CompletionClass::ShortPacket,
        (32, 26..=28) => CompletionClass::Stopped,
        (33, 4 | 5 | 7 | 8 | 9 | 11 | 17 | 19 | 22 | 24 | 25 | 29 | 33 | 35) => {
            CompletionClass::Error
        }
        (32, 2 | 3 | 4 | 5 | 6 | 7 | 10 | 12 | 14 | 15 | 18 | 20 | 23 | 31 | 32 | 33 | 34 | 36) => {
            CompletionClass::Error
        }
        _ => CompletionClass::Unknown,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    EnableSlot,
    DisableSlot,
    AddressDevice,
    ConfigureEndpoint,
    EvaluateContext,
    ResetEndpoint,
    StopEndpoint,
    SetTrDequeue,
    ResetDevice,
    ForceEvent,
    NegotiateBandwidth,
    SetLatencyTolerance,
    GetPortBandwidth,
    ForceHeader,
    Noop,
    GetExtendedProperty,
    SetExtendedProperty,
}
impl CommandName {
    pub fn code(self) -> u8 {
        match self {
            Self::EnableSlot => 9,
            Self::DisableSlot => 10,
            Self::AddressDevice => 11,
            Self::ConfigureEndpoint => 12,
            Self::EvaluateContext => 13,
            Self::ResetEndpoint => 14,
            Self::StopEndpoint => 15,
            Self::SetTrDequeue => 16,
            Self::ResetDevice => 17,
            Self::ForceEvent => 18,
            Self::NegotiateBandwidth => 19,
            Self::SetLatencyTolerance => 20,
            Self::GetPortBandwidth => 21,
            Self::ForceHeader => 22,
            Self::Noop => 23,
            Self::GetExtendedProperty => 24,
            Self::SetExtendedProperty => 25,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandCode {
    Literal(u8),
    Named(CommandName),
}
impl CommandCode {
    pub fn code(self) -> u8 {
        match self {
            Self::Literal(code) => code,
            Self::Named(name) => name.code(),
        }
    }
}

pub fn completion_name(code: u8) -> &'static str {
    match code {
        0 => "invalid",
        1 => "success",
        2 => "data_buffer_error",
        3 => "babble_detected",
        4 => "usb_transaction_error",
        5 => "trb_error",
        6 => "stall",
        7 => "resource_error",
        8 => "bandwidth_error",
        9 => "no_slots",
        10 => "invalid_stream_type",
        11 => "slot_not_enabled",
        12 => "endpoint_not_enabled",
        13 => "short_packet",
        14 => "ring_underrun",
        15 => "ring_overrun",
        16 => "vf_event_ring_full",
        17 => "parameter_error",
        18 => "bandwidth_overrun",
        19 => "context_state_error",
        20 => "no_ping_response",
        21 => "event_ring_full",
        22 => "incompatible_device",
        23 => "missed_service",
        24 => "command_ring_stopped",
        25 => "command_aborted",
        26 => "stopped",
        27 => "stopped_length_invalid",
        28 => "stopped_short_packet",
        29 => "max_exit_latency_too_large",
        31 => "isoch_buffer_overrun",
        32 => "event_lost",
        33 => "undefined_error",
        34 => "invalid_stream_id",
        35 => "secondary_bandwidth_error",
        36 => "split_transaction_error",
        _ => "unknown",
    }
}

/// A known opcode never narrows an unrecognized command's effects implicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Observe,
    Slot { slot: u8, device: bool },
    Global,
}
pub fn effect(op: &RawOperation) -> Effect {
    use RawOperation as O;
    match op {
        O::Command { words, .. } => {
            let ty = (words[3] >> 10) & 63;
            let slot = (words[3] >> 24) as u8;
            // Reserved bits or unsupported layouts stay globally unknown. Cycle is owner-controlled.
            let allowed = (255 << 24)
                | (63 << 10)
                | 1
                | if ty == 12 {
                    1 << 9
                } else if ty == 15 {
                    1 << 23
                } else {
                    0
                };
            if words[3] & !allowed != 0 || words[2] != 0 {
                return Effect::Global;
            }
            if ty == 23 && words[0] == 0 && words[1] == 0 && words[3] & !1 == 23 << 10 {
                Effect::Observe
            } else if slot != 0 && matches!(ty, 12..=16) {
                Effect::Slot {
                    slot,
                    device: false,
                }
            } else {
                Effect::Global
            }
        }
        O::Transfer {
            slot,
            endpoint,
            trbs,
        } if endpoint & 15 == 0 => {
            // Read immediate SETUP for classification only; never alter it.
            if let Some(t) = trbs
                .first()
                .filter(|t| (t[3] >> 10) & 63 == 2 && t[3] & (1 << 6) != 0 && t[2] & 0x1ffff == 8)
            {
                let setup = (t[0] as u64) | ((t[1] as u64) << 32);
                let bm = setup as u8;
                let request = (setup >> 8) as u8;
                if bm & 0x60 == 0 && matches!((bm, request), (0, 5 | 9) | (1, 11) | (2, 1)) {
                    return Effect::Slot {
                        slot: *slot,
                        device: true,
                    };
                }
            }
            Effect::Global
        }
        O::DevicePacketSize { slot, .. }
        | O::DeviceInterface { slot, .. }
        | O::DeviceConfiguration { slot, .. }
        | O::EndpointConfigure { slot, .. }
        | O::HubConfigure { slot, .. }
        | O::StreamConfigure { slot, .. }
        | O::AddressRoute { slot, .. } => Effect::Slot {
            slot: *slot,
            device: true,
        },
        O::RegisterWrite { .. }
        | O::SlotEnable
        | O::Place { .. }
        | O::Publish { .. }
        | O::Doorbell { .. }
        | O::Transfer { .. }
        | O::ScatterTransfer { .. } => Effect::Global,
        _ => Effect::Observe,
    }
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct SlotEffect {
    pub generation: u64,
    pub device_uncertain: bool,
    pub pending_at_mutation: bool,
    pub baseline: Option<serde_json::Value>,
}
#[derive(Debug, Default, Serialize)]
pub struct Coherence {
    pub generation: u64,
    pub global_unknown: bool,
    pub slots: std::collections::BTreeMap<u8, SlotEffect>,
}
impl Coherence {
    pub fn mutate(
        &mut self,
        effect: Effect,
        baseline: Option<serde_json::Value>,
        pending: bool,
    ) -> Result<(), OperationError> {
        if effect == Effect::Observe {
            return Ok(());
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("coherence generation exhausted"))?;
        match effect {
            Effect::Global => self.global_unknown = true,
            Effect::Slot { slot, device } => {
                let entry = self.slots.entry(slot).or_insert_with(|| SlotEffect {
                    baseline,
                    ..Default::default()
                });
                entry.generation = self.generation;
                entry.device_uncertain |= device;
                entry.pending_at_mutation |= pending;
            }
            Effect::Observe => {}
        }
        Ok(())
    }
    /// Equality is proof only for known controller-only effects against the original view.
    /// Device uncertainty, unknown global effects and pending DMA are independent blockers.
    pub fn reconcile(&mut self, slot: u8, current: &serde_json::Value, pending: bool) -> bool {
        let matches = !self.global_unknown
            && !pending
            && self.slots.get(&slot).is_some_and(|s| {
                !s.device_uncertain
                    && !s.pending_at_mutation
                    && s.baseline.as_ref() == Some(current)
            });
        if matches {
            self.slots.remove(&slot);
        }
        matches
    }
}
