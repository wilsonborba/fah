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
        Ok(Command::Capabilities) => {
            if serde_json::to_writer(&mut *out, &crate::domain::models::capabilities::report())
                .is_err()
            {
                return 1;
            }
            writeln!(out)
        }
        Ok(Command::Help) => out.write_all(settings::HELP.as_bytes()),
        Ok(Command::Shell { json, run_id }) => {
            #[cfg(feature = "hosted-direct")]
            {
                return crate::presentation::cli::runtime::stdin_shell(run_id, json, out, err);
            }
            #[cfg(not(feature = "hosted-direct"))]
            {
                let _ = (json, run_id);
                let _ = writeln!(
                    err,
                    "shell requires the hosted-direct build; help never opens a controller"
                );
                return 2;
            }
        }
        Ok(Command::Version) => writeln!(out, "fah {}", settings::VERSION),
        Err(message) => {
            let _ = writeln!(err, "{message}");
            return 2;
        }
    };
    if result.is_ok() { 0 } else { 1 }
}
