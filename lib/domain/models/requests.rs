//! Owned transfer inputs and evidence-based completion reports.
use crate::domain::{
    models::{ErrorCategory, OperationError, RequestId, RequestState},
    services::usb::DeviceId,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferType {
    Control,
    Bulk,
    Interrupt,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestSpec {
    pub device: DeviceId,
    pub endpoint: u8,
    pub transfer_type: TransferType,
    pub direction: Direction,
    pub length: usize,
    pub setup: Option<SetupPacket>,
    #[serde(default)]
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMetadata {
    pub device: DeviceId,
    pub endpoint: u8,
    pub transfer_type: TransferType,
    pub direction: Direction,
    pub requested_length: usize,
    pub setup: Option<SetupPacket>,
}
pub struct OwnedRequest {
    pub metadata: RequestMetadata,
    pub buffer: Vec<u8>,
}
impl RequestSpec {
    pub fn prepare(self) -> Result<OwnedRequest, OperationError> {
        if self.length > 65536 {
            return Err(unsupported("single-TRB profile is bounded to64KiB"));
        }
        if self.endpoint & 0x70 != 0 {
            return Err(OperationError::validation("reserved endpoint address bits"));
        }
        match self.transfer_type {
            TransferType::Control => {
                if self.endpoint != 0 || self.length > u16::MAX as usize {
                    return Err(OperationError::validation(
                        "invalid control endpoint or length",
                    ));
                }
                let setup = self
                    .setup
                    .ok_or_else(|| OperationError::validation("control SETUP required"))?;
                if setup.length as usize != self.length {
                    return Err(unsupported(
                        "ordinary control TD requires matching setup/data lengths",
                    ));
                }
                if (setup.request_type & 128 != 0) != (self.direction == Direction::In) {
                    return Err(OperationError::validation("SETUP direction mismatch"));
                }
                if setup.request_type & 31 > 3 || setup.request_type & 96 == 96 {
                    return Err(unsupported("reserved USB recipient/type"));
                }
                if setup.request_type & 96 == 0 && matches!(setup.request, 5 | 9 | 11) {
                    return Err(unsupported(
                        "use explicit address/configuration/interface operations for lifecycle requests",
                    ));
                }
            }
            _ => {
                if self.endpoint & 15 == 0 || self.setup.is_some() {
                    return Err(OperationError::validation(
                        "non-control endpoint required without SETUP",
                    ));
                }
                if (self.endpoint & 128 != 0) != (self.direction == Direction::In) {
                    return Err(OperationError::validation("endpoint direction mismatch"));
                }
            }
        }
        if (self.direction == Direction::In && !self.data.is_empty())
            || (self.direction == Direction::Out && self.data.len() != self.length)
        {
            return Err(OperationError::validation(
                "OUT data must match length; IN data is allocated by the owner",
            ));
        }
        let buffer = if self.direction == Direction::In {
            vec![0; self.length]
        } else {
            self.data
        };
        Ok(OwnedRequest {
            metadata: RequestMetadata {
                device: self.device,
                endpoint: self.endpoint,
                transfer_type: self.transfer_type,
                direction: self.direction,
                requested_length: self.length,
                setup: self.setup,
            },
            buffer,
        })
    }
}
pub fn unsupported(message: &str) -> OperationError {
    OperationError {
        category: ErrorCategory::Unsupported,
        message: message.into(),
        os_code: None,
        completion_code: None,
    }
}
#[derive(Debug, Clone)]
pub struct BackendCompletion {
    pub actual_length: Option<usize>,
    pub completion_code: Option<u32>,
    pub data: Option<Vec<u8>>,
    pub error: Option<OperationError>,
    pub quiescent: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestReport {
    pub id: RequestId,
    pub metadata: RequestMetadata,
    pub state: RequestState,
    pub logical_cancel: bool,
    pub timed_out: bool,
    pub data: Option<Vec<u8>>,
}
