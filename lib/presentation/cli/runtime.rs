//! Input worker never owns a controller; one actor owns all operations.
use super::{
    commands::{MAX_LINE, parse_line},
    formatters::{self, OutputMode},
    handlers::Handler,
};
use crate::domain::{models::OperationError, services::usb::DeviceBackend};
use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
};
pub fn read_command(
    reader: &mut impl BufRead,
) -> io::Result<Option<Result<String, OperationError>>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    let mut seen = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        seen = true;
        let n = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |n| n + 1);
        let end = available[n - 1] == b'\n';
        if bytes.len() + n > MAX_LINE {
            oversized = true;
        } else if !oversized {
            bytes.extend_from_slice(&available[..n]);
        }
        reader.consume(n);
        if end {
            break;
        }
    }
    if !seen {
        return Ok(None);
    }
    if oversized {
        return Ok(Some(Err(OperationError::validation("input exceeds1MiB"))));
    }
    Ok(Some(String::from_utf8(bytes).map_err(|_| {
        OperationError::validation("input is not UTF-8")
    })))
}
pub fn run<B: DeviceBackend>(
    mut handler: Handler<B>,
    input: mpsc::Receiver<Result<String, OperationError>>,
    out: &mut impl Write,
    err: &mut impl Write,
    mode: OutputMode,
) -> i32 {
    let mut code = 0;
    while let Ok(line) = input.recv() {
        let parsed = line.and_then(|line| parse_line(&line));
        let frame = match parsed {
            Ok(command) => handler.execute(command),
            Err(error) => handler.respond(Err(error)),
        };
        if matches!(
            frame.result.outcome,
            crate::domain::models::Outcome::Failed(_)
        ) {
            code = 1;
        }
        if formatters::write(out, &frame, mode).is_err() {
            let _ = writeln!(err, "output failed; attempting coordinated close");
            let _ = handler.usb.close();
            return 1;
        }
        if handler.quitting {
            return code;
        }
    }
    let result = handler
        .usb
        .close()
        .map(|()| serde_json::json!({"closed":true,"reason":"eof"}));
    if result.is_err() {
        code = 1;
    }
    if formatters::write(out, &handler.respond(result), mode).is_err() {
        code = 1;
    }
    code
}
#[cfg(feature = "hosted-direct")]
pub fn stdin_shell(run_id: String, json: bool, out: &mut impl Write, err: &mut impl Write) -> i32 {
    let (tx, rx) = mpsc::sync_channel(128);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            match read_command(&mut input) {
                Ok(Some(line)) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(OperationError::validation(format!(
                        "input error: {error}"
                    ))));
                    break;
                }
            }
        }
    });
    run(
        Handler::new(crate::dal::local::controller::LocalController::new(run_id)),
        rx,
        out,
        err,
        if json {
            OutputMode::Json
        } else {
            OutputMode::Human
        },
    )
}
