//! Full stage declarations; compatibility choices never silently normalize fields.
use super::{
    OperationError,
    requests::{Direction, RequestMetadata, SetupPacket, TransferType, unsupported},
};
use crate::domain::services::usb::DeviceId;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlProfile {
    Compliant,
    DeviceCompatibility,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DataStage {
    pub direction: Direction,
    pub length: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlan {
    pub device: DeviceId,
    pub profile: ControlProfile,
    pub setup: SetupPacket,
    pub data_stage: Option<DataStage>,
    pub status_direction: Option<Direction>,
    #[serde(default)]
    pub data: Vec<u8>,
}
pub struct ValidatedControlPlan {
    pub metadata: RequestMetadata,
    pub buffer: Vec<u8>,
    pub wire_setup: [u8; 8],
    pub has_data: bool,
    pub data_direction: Direction,
    pub status_direction: Direction,
    pub profile: ControlProfile,
}
impl ControlPlan {
    pub fn validate(self) -> Result<ValidatedControlPlan, OperationError> {
        let status = self.status_direction.ok_or_else(|| {
            unsupported("partial/delayed stage execution is outside the reviewed library profile")
        })?;
        if self.setup.request_type == 0 && self.setup.request == 5 {
            return Err(unsupported(
                "xHCI intercepts SET_ADDRESS; use Address Device/BSR",
            ));
        }
        if (self.setup.request_type == 0 && self.setup.request == 9)
            || (self.setup.request_type == 1 && self.setup.request == 11)
        {
            return Err(unsupported(
                "use the explicit configuration/interface command to retain host state",
            ));
        }
        let wire_direction = if self.setup.request_type & 128 != 0 {
            Direction::In
        } else {
            Direction::Out
        };
        let length = self.data_stage.map_or(0, |stage| stage.length);
        let direction = self
            .data_stage
            .map_or(wire_direction, |stage| stage.direction);
        if length > 65536 {
            return Err(unsupported("single data-stage TRB is limited to64KiB"));
        }
        if self.profile == ControlProfile::Compliant {
            if self.setup.request_type & 31 > 3 || self.setup.request_type & 96 == 96 {
                return Err(unsupported("reserved compliant USB request fields"));
            }
            if length != self.setup.length as usize
                || self.data_stage.is_some() != (self.setup.length != 0)
                || direction != wire_direction
            {
                return Err(OperationError::validation(
                    "compliant SETUP/data stages disagree",
                ));
            }
            let expected = if length > 0 && direction == Direction::In {
                Direction::Out
            } else {
                Direction::In
            };
            if status != expected {
                return Err(OperationError::validation(
                    "compliant status direction mismatch",
                ));
            }
        }
        if (direction == Direction::In && !self.data.is_empty())
            || (direction == Direction::Out && self.data.len() != length)
        {
            return Err(OperationError::validation(
                "stage data length/direction mismatch",
            ));
        }
        if self.data_stage.is_none() && !self.data.is_empty() {
            return Err(OperationError::validation("no-data plan contains payload"));
        }
        let mut wire = [0u8; 8];
        wire[0] = self.setup.request_type;
        wire[1] = self.setup.request;
        wire[2..4].copy_from_slice(&self.setup.value.to_le_bytes());
        wire[4..6].copy_from_slice(&self.setup.index.to_le_bytes());
        wire[6..8].copy_from_slice(&self.setup.length.to_le_bytes());
        Ok(ValidatedControlPlan {
            metadata: RequestMetadata {
                device: self.device,
                endpoint: 0,
                transfer_type: TransferType::Control,
                direction,
                requested_length: length,
                setup: Some(self.setup),
            },
            buffer: if direction == Direction::In {
                vec![0; length]
            } else {
                self.data
            },
            wire_setup: wire,
            has_data: self.data_stage.is_some(),
            data_direction: direction,
            status_direction: status,
            profile: self.profile,
        })
    }
}
