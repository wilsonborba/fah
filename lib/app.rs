use crate::{
    core::settings,
    presentation::cli::commands::{self, Command},
};
use std::{ffi::OsString, io::Write};

pub fn run(
    args: impl IntoIterator<Item = OsString>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let result = match commands::parse(args) {
        Ok(Command::Help) => out.write_all(settings::HELP.as_bytes()),
        Ok(Command::Version) => writeln!(out, "fah {}", settings::VERSION),
        Err(message) => {
            let _ = writeln!(err, "{message}");
            return 2;
        }
    };
    if result.is_ok() { 0 } else { 1 }
}
