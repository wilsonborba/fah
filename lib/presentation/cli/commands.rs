use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
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
        _ => Err("unknown command; use --help"),
    }
}

use crate::domain::models::OperationError;
use serde::{Deserialize, Serialize};
pub const MAX_LINE: usize = 1024 * 1024;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellCommand {
    Help,
    Version,
    Capabilities,
    State,
    Open { target: String },
    Init,
    Close,
    Quit,
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
        "init" => Ok(ShellCommand::Init),
        "close" => Ok(ShellCommand::Close),
        "quit" => Ok(ShellCommand::Quit),
        json => serde_json::from_str(json)
            .map_err(|error| OperationError::validation(format!("invalid command: {error}"))),
    }
}
