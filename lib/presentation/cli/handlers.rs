//! Stateful flow only; hardware behavior stays in domain services.
use super::{commands::ShellCommand, outputs::Frame};
use crate::domain::{
    models::{OperationError, capabilities},
    services::{explicit::ExplicitBackend, usb::UsbSession},
};
use serde_json::{Value, json};
pub struct Handler<B: ExplicitBackend> {
    pub usb: UsbSession<B>,
    pub(super) sequence: u64,
    pub quitting: bool,
    waits: std::collections::BTreeMap<u64, (crate::domain::models::RequestId, std::time::Instant)>,
}
impl<B: ExplicitBackend> Handler<B> {
    pub fn new(backend: B) -> Self {
        Self {
            usb: UsbSession::new(
                backend,
                crate::core::identity::fresh()
                    .expect("session identity budget exhausted before acquisition"),
            ),
            sequence: 0,
            quitting: false,
            waits: Default::default(),
        }
    }
    pub fn respond(&mut self, result: Result<Value, OperationError>) -> Frame {
        self.sequence += 1;
        Frame::response(self.sequence, self.usb.session.state(), result)
    }
    pub fn execute(&mut self, command: ShellCommand) -> Frame {
        let result = self.dispatch(command);
        let mut frame = self.respond(result);
        if let crate::domain::models::Outcome::Completed(value) = &frame.result.outcome {
            if let Some(timeout_ms) = value.get("wait_timeout_ms").and_then(Value::as_u64) {
                let id =
                    serde_json::from_value(value["request_id"].clone()).expect("typed request ID");
                self.waits.insert(
                    frame.result.operation_id,
                    (
                        id,
                        std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms),
                    ),
                );
                frame.result.outcome = crate::domain::models::Outcome::Pending;
            } else if let Some(error) = value.get("error").filter(|error| !error.is_null())
                && let Ok(error) = serde_json::from_value(error.clone())
            {
                frame.result.outcome = crate::domain::models::Outcome::Partial {
                    payload: value.clone(),
                    error,
                };
            }
        }
        frame
    }
    pub fn tick(&mut self) -> Vec<Frame> {
        let now = std::time::Instant::now();
        let mut frames = Vec::new();
        let waits: Vec<_> = self
            .waits
            .iter()
            .map(|(operation, (request, deadline))| (*operation, *request, *deadline))
            .collect();
        for (operation, id, deadline) in waits {
            let mut result = self.usb.poll_request(id);
            if result.as_ref().is_ok_and(|report| {
                matches!(
                    report.state,
                    crate::domain::models::RequestState::Pending { .. }
                )
            }) {
                if now < deadline {
                    continue;
                }
                result = self.usb.timeout_request(id);
            }
            self.waits.remove(&operation);
            let timed_out = result.as_ref().is_ok_and(|report| {
                report.timed_out
                    && matches!(
                        report.state,
                        crate::domain::models::RequestState::Pending { .. }
                    )
            });
            let mut frame = Frame::response(
                operation,
                self.usb.session.state(),
                result.map(|report| json!(report)),
            );
            if timed_out
                && let crate::domain::models::Outcome::Completed(payload) = frame.result.outcome
            {
                frame.result.outcome = crate::domain::models::Outcome::Partial {
                    payload,
                    error: OperationError {
                        category: crate::domain::models::ErrorCategory::Timeout,
                        message: "foreground wait deadline elapsed; request and DMA remain owned"
                            .into(),
                        os_code: None,
                        completion_code: None,
                    },
                };
            }
            frame.kind = "event";
            frames.push(frame);
        }
        frames
    }
    pub fn has_waits(&self) -> bool {
        !self.waits.is_empty()
    }
    pub fn cancel_waits(&mut self, reason: &str) -> Vec<Frame> {
        let waits = std::mem::take(&mut self.waits);
        waits.into_iter().map(|(operation,(id,_))|{let mut frame=Frame::response(operation,self.usb.session.state(),Ok(json!({"request_id":id,"wait_cancelled":true,"reason":reason,"usb_aborted":false})));frame.kind="event";frame}).collect()
    }
    pub fn dispatch(&mut self, command: ShellCommand) -> Result<Value, OperationError> {
        match command {
            ShellCommand::Help => Ok(
                json!({"input":"JSON Lines or help/version/capabilities/state/init/close/quit","commands":capabilities::command_names(),"effects":{"observe":["state","ports","status","capabilities"],"hardware_steps":["init","port_reset","slot_enable","address","descriptor","configuration","interface","endpoints","submit","control_plan","abort","reset_endpoint"],"logical_only":["logical_cancel"],"polling":["poll","wait"],"release":["close","quit"]},"evidence":"compiled_and_fake_tested; runtime-specific evidence tracked in release report","no_implicit_initialization":true}),
            ),
            ShellCommand::Version => Ok(json!({"version":env!("CARGO_PKG_VERSION")})),
            ShellCommand::Capabilities => Ok(json!(capabilities::report())),
            ShellCommand::State => Ok(
                json!({"state":self.usb.session.state(),"session_id":self.usb.session.identity(),"resources":self.usb.session.inspect()}),
            ),
            ShellCommand::Open { target } => {
                self.usb.session.acquire(&target)?;
                Ok(json!({"target":target,"initialized":false}))
            }
            ShellCommand::Init => {
                self.usb.initialize()?;
                Ok(json!({"initialized":true,"discovery_performed":false}))
            }
            ShellCommand::Close => {
                self.usb.close()?;
                Ok(json!({"closed":true}))
            }

            ShellCommand::Batch {
                operations,
                preview,
            } => self.batch(operations, preview),
            ShellCommand::Enumerate {
                port,
                configuration_index,
                configure,
                interface,
                alternate,
            } => self.enumerate(port, configuration_index, configure, interface, alternate),
            ShellCommand::Devices => {
                Ok(json!({"cached":true,"devices":self.usb.device_snapshots()}))
            }
            ShellCommand::Requests => Ok(json!(self.usb.request_reports()?)),
            ShellCommand::Ports => Ok(json!(self.usb.inspect_ports()?)),
            ShellCommand::PortAck { port, changes } => {
                Ok(json!(self.usb.acknowledge_port(port, changes)?))
            }
            ShellCommand::PortReset { port, warm } => Ok(json!(self.usb.reset_port(port, warm)?)),
            ShellCommand::SlotEnable { port } => Ok(json!(self.usb.enable_slot(port)?)),
            ShellCommand::Address { device, bsr } => {
                Ok(json!(self.usb.address_device(device, bsr)?))
            }
            ShellCommand::Descriptor { device, request } => {
                Ok(json!(self.usb.descriptor(device, request)?))
            }
            ShellCommand::PacketSize { device, size } => {
                self.usb.ep0_packet_size(device, size)?;
                Ok(json!({"packet_size":size}))
            }
            ShellCommand::Configuration { device, value } => {
                Ok(json!(self.usb.set_configuration(device, value)?))
            }
            ShellCommand::Interface {
                device,
                interface,
                alternate,
            } => {
                self.usb.set_interface(device, interface, alternate)?;
                Ok(json!({"interface":interface,"alternate":alternate}))
            }
            ShellCommand::Endpoints {
                device,
                interface,
                alternate,
            } => Ok(json!(
                self.usb.setup_endpoints(device, interface, alternate)?
            )),
            ShellCommand::Submit { request } => Ok(json!(self.usb.submit_request(request)?)),
            ShellCommand::ControlPlan { plan } => Ok(json!(self.usb.submit_control_plan(plan)?)),
            ShellCommand::Status { id } => Ok(json!(self.usb.request_status(id)?)),
            ShellCommand::Poll { id } => Ok(json!(self.usb.poll_request(id)?)),
            ShellCommand::Wait { id, timeout_ms } => {
                if timeout_ms > 3600000 || self.waits.len() >= 128 {
                    return Err(OperationError::validation(
                        "wait budget exceeded (1h/128 waits)",
                    ));
                }
                self.usb.request_status(id)?;
                Ok(json!({"request_id":id,"wait_timeout_ms":timeout_ms}))
            }
            ShellCommand::Reclaim { id } => Ok(json!(self.usb.reclaim_request(id)?)),
            ShellCommand::LogicalCancel { id } => Ok(json!(self.usb.logical_cancel_request(id)?)),
            ShellCommand::Abort { id } => Ok(json!(self.usb.abort_request(id)?)),
            ShellCommand::ResetEndpoint { id, preserve } => {
                Ok(json!(self.usb.reset_endpoint_request(id, preserve)?))
            }
            ShellCommand::Noop => {
                let words = self.usb.noop()?;
                Ok(json!({"completion_words":words,"completion_code":words[2]>>24}))
            }
            ShellCommand::Unsupported { capability } => {
                Err(crate::domain::models::requests::unsupported(&format!(
                    "capability {capability:?} unavailable; see capabilities"
                )))
            }
            ShellCommand::Quit => {
                self.usb.close()?;
                self.quitting = true;
                Ok(json!({"closed":true,"quit":true}))
            }
        }
    }
}
