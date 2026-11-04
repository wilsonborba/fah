//! Literal xHCI contracts. No memory access or controller ownership here.
use super::OperationError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handle {
    pub session: u64,
    pub sequence: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ring {
    Command,
    Endpoint { slot: u8, endpoint: u8 },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawOperation {
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
        command: u8,
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
        (32 | 33, 2..=12 | 14..=25 | 29..=36) => CompletionClass::Error,
        _ => CompletionClass::Unknown,
    }
}
