use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let first = args.next();
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
