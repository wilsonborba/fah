//! Optional compound flows compose the explicit domain API; no second engine.
use super::{commands::ShellCommand, handlers::Handler};
use crate::domain::{
    models::{OperationError, Outcome, descriptors::DescriptorRequest, ports::PortId},
    services::explicit::ExplicitBackend,
};
use serde_json::{Value, json};
impl<B: ExplicitBackend> Handler<B> {
    pub(super) fn batch(
        &mut self,
        operations: Vec<ShellCommand>,
        preview: bool,
    ) -> Result<Value, OperationError> {
        if operations.len() > 64 {
            return Err(OperationError::validation("batch exceeds64 operations"));
        }
        if operations
            .iter()
            .any(|op| matches!(op, ShellCommand::Batch { .. } | ShellCommand::Quit))
        {
            return Err(OperationError::validation(
                "nested batch and quit are not batch operations",
            ));
        }
        if preview {
            return Ok(
                json!({"preview":true,"operations":operations,"state_validation":"unknown_until_execution","effects_performed":false,"timing_guarantee":false}),
            );
        }
        let mut completed = Vec::new();
        let mut failed = None;
        for operation in operations {
            let frame = self.execute(operation);
            let failure = match &frame.result.outcome {
                Outcome::Failed(error) | Outcome::Partial { error, .. } => Some(error.clone()),
                _ => None,
            };
            completed.push(frame);
            if let Some(error) = failure {
                failed = Some(error);
                break;
            }
        }
        Ok(
            json!({"operations":completed,"error":failed,"failure_policy":"stop_at_first_failure","outstanding_requests":self.usb.request_reports()?,"ordering":"input_order; submit does not await; wait registers an asynchronous deadline","timing_guarantee":false}),
        )
    }
    pub(super) fn enumerate(
        &mut self,
        port: PortId,
        index: u8,
        configure: bool,
        interface: u8,
        alternate: u8,
    ) -> Result<Value, OperationError> {
        let mut steps = Vec::new();
        macro_rules! step{($name:expr,$operation:expr)=>{{match $operation{Ok(value)=>{steps.push(json!({"operation":$name,"result":value}));value},Err(error)=>return Ok(json!({"compound":true,"steps":steps,"failed_operation":$name,"error":error,"cleanup":"resources retained; inspect state and close explicitly"}))}}};}
        macro_rules! require{($condition:expr,$message:expr)=>{if !$condition{return Ok(json!({"compound":true,"steps":steps,"error":OperationError::validation($message),"cleanup":"resources retained; inspect state and close explicitly"}));}};}
        let port = step!("port_reset", self.usb.reset_port(port, false));
        let device = step!("slot_enable", self.usb.enable_slot(port.id));
        let device = step!("address", self.usb.address_device(device.id, false));
        let header = step!(
            "device_prefix",
            self.usb.descriptor(
                device.id,
                DescriptorRequest {
                    kind: 1,
                    index: 0,
                    language: 0,
                    length: 8
                }
            )
        );
        require!(
            header.parse_error.is_none() && header.bytes.len() >= 8,
            "invalid device prefix"
        );
        let packet = step!(
            "decode_packet_size",
            crate::domain::models::descriptors::decode_ep0_packet_size(
                port.speed_code,
                header.bytes[7]
            )
            .map_err(OperationError::validation)
        );
        step!("packet_size", self.usb.ep0_packet_size(device.id, packet));
        let full = step!(
            "device_descriptor",
            self.usb.descriptor(
                device.id,
                DescriptorRequest {
                    kind: 1,
                    index: 0,
                    language: 0,
                    length: 18
                }
            )
        );
        require!(
            full.parse_error.is_none() && full.bytes.len() >= 18 && index < full.bytes[17],
            "invalid device descriptor or configuration index"
        );
        let header = step!(
            "configuration_header",
            self.usb.descriptor(
                device.id,
                DescriptorRequest {
                    kind: 2,
                    index,
                    language: 0,
                    length: 9
                }
            )
        );
        require!(
            header.parse_error.is_none() && header.bytes.len() >= 9,
            "invalid configuration header"
        );
        let length = u16::from_le_bytes([header.bytes[2], header.bytes[3]]);
        let full = step!(
            "configuration_body",
            self.usb.descriptor(
                device.id,
                DescriptorRequest {
                    kind: 2,
                    index,
                    language: 0,
                    length
                }
            )
        );
        require!(
            full.parse_error.is_none() && full.complete_configuration_value().is_some(),
            "incomplete configuration"
        );
        if configure {
            step!(
                "configuration",
                self.usb
                    .set_configuration(device.id, full.complete_configuration_value().unwrap())
            );
            if alternate != 0 {
                step!(
                    "interface",
                    self.usb.set_interface(device.id, interface, alternate)
                );
            }
            step!(
                "endpoints",
                self.usb.setup_endpoints(device.id, interface, alternate)
            );
        }
        Ok(
            json!({"compound":true,"steps":steps,"device":device.id,"configured":configure,"timing_guarantee":false,"error":null}),
        )
    }
}
