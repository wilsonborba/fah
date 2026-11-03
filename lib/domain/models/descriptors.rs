//! Bounded descriptor parsing preserves the transport response on parse errors.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescriptorRequest {
    pub kind: u8,
    pub index: u8,
    pub language: u16,
    pub length: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DescriptorSummary {
    Prefix {
        descriptor_type: u8,
        declared_length: u8,
    },
    Device {
        vendor: u16,
        product: u16,
        ep0_packet_encoding: u8,
        configurations: u8,
    },
    Configuration {
        value: u8,
        total_length: u16,
        interfaces: u8,
        complete: bool,
    },
    Opaque {
        descriptor_type: u8,
        declared_length: u8,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescriptorRead {
    pub request: DescriptorRequest,
    pub actual_length: usize,
    pub bytes: Vec<u8>,
    pub parsed: Option<DescriptorSummary>,
    pub parse_error: Option<String>,
}
impl DescriptorRead {
    pub fn from_bytes(request: DescriptorRequest, bytes: Vec<u8>) -> Self {
        let parsed = parse(request, &bytes);
        let (actual_length, parsed, parse_error) =
            (bytes.len(), parsed.as_ref().ok().cloned(), parsed.err());
        Self {
            request,
            actual_length,
            bytes,
            parsed,
            parse_error,
        }
    }
    pub fn complete_configuration_value(&self) -> Option<u8> {
        match self.parsed {
            Some(DescriptorSummary::Configuration {
                value,
                complete: true,
                ..
            }) if self.parse_error.is_none() => Some(value),
            _ => None,
        }
    }
}
fn parse(request: DescriptorRequest, bytes: &[u8]) -> Result<DescriptorSummary, String> {
    if bytes.len() > request.length as usize {
        return Err("response exceeds requested length".into());
    }
    if bytes.len() < 2 {
        return Err("descriptor header incomplete".into());
    }
    let length = bytes[0] as usize;
    if length < 2 || bytes[1] != request.kind {
        return Err("descriptor length or type mismatch".into());
    }
    if (request.kind == 1 && length != 18) || (request.kind == 2 && length != 9) {
        return Err("invalid standard descriptor length".into());
    }
    if length > bytes.len() {
        if bytes.len() == request.length as usize {
            return Ok(DescriptorSummary::Prefix {
                descriptor_type: bytes[1],
                declared_length: bytes[0],
            });
        }
        return Err("short descriptor response".into());
    }
    match request.kind {
        1 => {
            if length != 18 || bytes.len() < 18 {
                return Err("invalid device descriptor length".into());
            }
            Ok(DescriptorSummary::Device {
                vendor: u16::from_le_bytes([bytes[8], bytes[9]]),
                product: u16::from_le_bytes([bytes[10], bytes[11]]),
                ep0_packet_encoding: bytes[7],
                configurations: bytes[17],
            })
        }
        2 => {
            if length != 9 {
                return Err("invalid configuration header length".into());
            }
            let total = u16::from_le_bytes([bytes[2], bytes[3]]);
            if total < 9 {
                return Err("invalid configuration total length".into());
            }
            let complete = bytes.len() >= total as usize;
            if !complete && request.length >= total {
                return Err("short configuration body".into());
            }
            if complete {
                validate_configuration(&bytes[..total as usize])?;
            }
            Ok(DescriptorSummary::Configuration {
                value: bytes[5],
                total_length: total,
                interfaces: bytes[4],
                complete,
            })
        }
        _ => Ok(DescriptorSummary::Opaque {
            descriptor_type: bytes[1],
            declared_length: bytes[0],
        }),
    }
}
pub fn validate_configuration(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 9 || bytes[0] != 9 || bytes[1] != 2 {
        return Err("invalid configuration descriptor".into());
    }
    let total = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    if total != bytes.len() || bytes[5] == 0 {
        return Err("configuration length or value invalid".into());
    }
    let mut offset = 9;
    let mut interface_endpoints: Option<(u8, u8)> = None;
    let mut interfaces = std::collections::BTreeSet::new();
    let mut addresses = std::collections::BTreeSet::new();
    let mut owners = std::collections::BTreeMap::new();
    let mut current_interface = None;
    while offset < total {
        if total - offset < 2 {
            return Err("truncated subordinate descriptor".into());
        }
        let length = bytes[offset] as usize;
        let kind = bytes[offset + 1];
        let minimum = match kind {
            4 => 9,
            5 => 7,
            _ => 2,
        };
        if length < minimum || length > total - offset {
            return Err("invalid subordinate descriptor length".into());
        }
        if kind == 4 {
            if interface_endpoints.is_some_and(|(declared, seen)| declared != seen) {
                return Err("interface endpoint count mismatch".into());
            }
            let number = bytes[offset + 2];
            let alternate = bytes[offset + 3];
            current_interface = Some(number);
            if !interfaces.insert((number, alternate)) {
                return Err("duplicate interface alternative".into());
            }
            addresses.clear();
            interface_endpoints = Some((bytes[offset + 4], 0));
        }
        if kind == 5 {
            let Some((declared, seen)) = interface_endpoints.as_mut() else {
                return Err("endpoint without interface".into());
            };
            let address = bytes[offset + 2];
            let attributes = bytes[offset + 3] & 3;
            let packet = u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]) & 0x7ff;
            if address & 0x70 != 0
                || address & 15 == 0
                || attributes == 0
                || packet == 0
                || !addresses.insert(address)
            {
                return Err("invalid or duplicate endpoint".into());
            }
            let owner = current_interface.ok_or("endpoint without interface")?;
            if owners
                .insert(address, owner)
                .is_some_and(|old| old != owner)
            {
                return Err("endpoint address reused by different interfaces".into());
            }
            *seen = seen.checked_add(1).ok_or("endpoint count overflow")?;
            if *seen > *declared {
                return Err("too many endpoints".into());
            }
        }
        offset += length;
    }
    if interface_endpoints.is_some_and(|(declared, seen)| declared != seen) {
        return Err("interface endpoint count mismatch".into());
    }
    let count = interfaces
        .iter()
        .map(|(number, _)| *number)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if count != bytes[4] as usize {
        return Err("configuration interface count mismatch".into());
    }
    Ok(())
}

/// Decode bMaxPacketSize0 using the observed root-port speed (xHCI PSI code).
pub fn decode_ep0_packet_size(speed: u8, encoded: u8) -> Result<u16, String> {
    match (speed, encoded) {
        (1, 8 | 16 | 32 | 64) | (2, 8) | (3, 64) => Ok(encoded as u16),
        (4 | 5, 9) => Ok(512),
        _ => Err("invalid bMaxPacketSize0 for observed speed".into()),
    }
}
