//! Stateful flow only; hardware behavior stays in domain services.
use super::{commands::ShellCommand, outputs::Frame};
use crate::domain::{
    models::{OperationError, capabilities},
    services::usb::{DeviceBackend, UsbSession},
};
use serde_json::{Value, json};
pub struct Handler<B: DeviceBackend> {
    pub usb: UsbSession<B>,
    sequence: u64,
    pub quitting: bool,
}
impl<B: DeviceBackend> Handler<B> {
    pub fn new(backend: B) -> Self {
        Self {
            usb: UsbSession::new(backend, 1),
            sequence: 0,
            quitting: false,
        }
    }
    pub fn respond(&mut self, result: Result<Value, OperationError>) -> Frame {
        self.sequence += 1;
        Frame::response(self.sequence, self.usb.session.state(), result)
    }
    pub fn execute(&mut self, command: ShellCommand) -> Frame {
        let result = self.dispatch(command);
        self.respond(result)
    }
    pub fn dispatch(&mut self, command: ShellCommand) -> Result<Value, OperationError> {
        match command {
            ShellCommand::Help => Ok(
                json!({"input":"JSON Lines or help/version/capabilities/state/init/close/quit","commands":["help","version","capabilities","state","open","init","close","quit"],"no_implicit_initialization":true}),
            ),
            ShellCommand::Version => Ok(json!({"version":env!("CARGO_PKG_VERSION")})),
            ShellCommand::Capabilities => Ok(json!(capabilities::registry())),
            ShellCommand::State => {
                Ok(json!({"state":self.usb.session.state(),"resources":self.usb.session.inspect()}))
            }
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
            ShellCommand::Quit => {
                self.usb.close()?;
                self.quitting = true;
                Ok(json!({"closed":true,"quit":true}))
            }
        }
    }
}
