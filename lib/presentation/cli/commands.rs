use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Capabilities,
    Shell { json: bool, run_id: String },
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let first = args.next();
    if first.as_deref().and_then(|arg| arg.to_str()) == Some("shell") {
        let mut json = false;
        let mut run_id = String::new();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--json") => json = true,
                Some("--run-id") => {
                    run_id = args
                        .next()
                        .and_then(|id| id.into_string().ok())
                        .ok_or("--run-id requires a string")?;
                    if run_id.len() > 128 {
                        return Err("run-id too long");
                    }
                }
                _ => return Err("unknown shell option"),
            }
        }
        return Ok(Command::Shell { json, run_id });
    }
    if args.next().is_some() {
        return Err("expected one command; use --help");
    }
    match first.as_deref().and_then(|arg| arg.to_str()) {
        None if first.is_none() => Ok(Command::Help),
        Some("--help" | "-h" | "help") => Ok(Command::Help),
        Some("--version" | "-V" | "version") => Ok(Command::Version),
        Some("capabilities") => Ok(Command::Capabilities),
        _ => Err("unknown command; use --help"),
    }
}

use crate::domain::models::OperationError;
use serde::{Deserialize, Serialize};
pub const MAX_LINE: usize = 1024 * 1024;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellCommand {
    Raw {
        request: crate::domain::models::raw::RawOperation,
    },
    Help,
    Version,
    Capabilities,
    State,
    Open {
        target: String,
    },
    Init,
    Close,
    Quit,
    Batch {
        operations: Vec<ShellCommand>,
        #[serde(default)]
        preview: bool,
    },
    Enumerate {
        port: crate::domain::models::ports::PortId,
        #[serde(default)]
        configuration_index: u8,
        #[serde(default)]
        configure: bool,
        #[serde(default)]
        interface: u8,
        #[serde(default)]
        alternate: u8,
    },
    Ports,
    Devices,
    Requests,
    PortAck {
        port: crate::domain::models::ports::PortId,
        changes: u32,
    },
    PortReset {
        port: crate::domain::models::ports::PortId,
        #[serde(default)]
        warm: bool,
    },
    SlotEnable {
        port: crate::domain::models::ports::PortId,
    },
    Address {
        device: crate::domain::services::usb::DeviceId,
        #[serde(default)]
        bsr: bool,
    },
    Descriptor {
        device: crate::domain::services::usb::DeviceId,
        request: crate::domain::models::descriptors::DescriptorRequest,
    },
    PacketSize {
        device: crate::domain::services::usb::DeviceId,
        size: u16,
    },
    Configuration {
        device: crate::domain::services::usb::DeviceId,
        value: u8,
    },
    Interface {
        device: crate::domain::services::usb::DeviceId,
        interface: u8,
        alternate: u8,
    },
    Endpoints {
        device: crate::domain::services::usb::DeviceId,
        interface: u8,
        #[serde(default)]
        alternate: u8,
    },
    Submit {
        request: crate::domain::models::requests::RequestSpec,
    },
    ControlPlan {
        plan: crate::domain::models::control_plan::ControlPlan,
    },
    Status {
        id: crate::domain::models::RequestId,
    },
    Poll {
        id: crate::domain::models::RequestId,
    },
    Wait {
        id: crate::domain::models::RequestId,
        timeout_ms: u64,
    },
    Reclaim {
        id: crate::domain::models::RequestId,
    },
    LogicalCancel {
        id: crate::domain::models::RequestId,
    },
    Abort {
        id: crate::domain::models::RequestId,
    },
    ResetEndpoint {
        id: crate::domain::models::RequestId,
        preserve: bool,
    },
    Noop,
    Unsupported {
        capability: String,
    },
}
pub fn parse_line(line: &str) -> Result<ShellCommand, OperationError> {
    if line.len() > MAX_LINE {
        return Err(OperationError::validation("input exceeds1MiB"));
    }
    match line.trim() {
        "help" => Ok(ShellCommand::Help),
        "version" => Ok(ShellCommand::Version),
        "capabilities" => Ok(ShellCommand::Capabilities),
        "state" => Ok(ShellCommand::State),
        "ports" => Ok(ShellCommand::Ports),
        "noop" => Ok(ShellCommand::Noop),
        "init" => Ok(ShellCommand::Init),
        "close" => Ok(ShellCommand::Close),
        "quit" => Ok(ShellCommand::Quit),
        json => serde_json::from_str(json)
            .map_err(|error| OperationError::validation(format!("invalid command: {error}"))),
    }
}
