//! Input worker never owns a controller; one actor owns all operations.
use super::{
    commands::{MAX_LINE, parse_line},
    formatters::{self, OutputMode},
    handlers::Handler,
};
use crate::domain::{models::OperationError, services::explicit::ExplicitBackend};
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
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
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
pub fn run<B: ExplicitBackend>(
    handler: Handler<B>,
    input: mpsc::Receiver<Result<String, OperationError>>,
    out: &mut impl Write,
    err: &mut impl Write,
    mode: OutputMode,
) -> i32 {
    run_with_signals(handler, input, out, err, mode, || None)
}
fn failed(frame: &super::outputs::Frame) -> bool {
    matches!(
        frame.result.outcome,
        crate::domain::models::Outcome::Failed(_) | crate::domain::models::Outcome::Partial { .. }
    )
}
fn emit(
    out: &mut impl Write,
    err: &mut impl Write,
    frame: &super::outputs::Frame,
    mode: OutputMode,
) -> bool {
    if formatters::write(out, frame, mode).is_err() {
        let _ = writeln!(
            err,
            "output unavailable; retaining ownership until coordinated close succeeds"
        );
        false
    } else {
        true
    }
}
/// Interrupts cancel CLI waits first; idle INT and TERM request coordinated close.
/// EOF/output failure stays alive and retries close if quiescence is uncertain.
pub fn run_with_signals<B: ExplicitBackend>(
    mut handler: Handler<B>,
    input: mpsc::Receiver<Result<String, OperationError>>,
    out: &mut impl Write,
    err: &mut impl Write,
    mode: OutputMode,
    mut signal: impl FnMut() -> Option<super::signals::Interrupt>,
) -> i32 {
    use super::signals::Interrupt;
    use std::time::{Duration, Instant};
    let mut code = 0;
    let mut reason = None;
    let mut eof = false;
    let mut output = true;
    let mut next_close = Instant::now();
    loop {
        if let Some(interrupt) = signal() {
            if matches!(interrupt, Interrupt::Int) && handler.has_waits() && reason.is_none() {
                for frame in handler.cancel_waits("sigint_wait_only") {
                    if output && !emit(out, err, &frame, mode) {
                        output = false;
                        code = 1;
                        reason = Some("output_failure");
                    }
                }
            } else {
                reason = Some(match interrupt {
                    Interrupt::Int => "sigint",
                    Interrupt::Term => "sigterm",
                });
                next_close = Instant::now();
            }
        }
        if let Some(reason) = reason {
            if Instant::now() >= next_close {
                for frame in handler.cancel_waits(reason) {
                    if output && !emit(out, err, &frame, mode) {
                        output = false;
                        code = 1;
                    }
                }
                let result = handler
                    .usb
                    .close()
                    .map(|()| serde_json::json!({"closed":true,"reason":reason}));
                let closed = result.is_ok();
                if !closed {
                    code = 1;
                    let _ = writeln!(
                        err,
                        "close incomplete; resources retained; retrying coordinated close"
                    );
                }
                let frame = handler.respond(result);
                if output && !emit(out, err, &frame, mode) {
                    output = false;
                    code = 1;
                }
                if closed {
                    return code;
                }
                next_close = Instant::now() + Duration::from_millis(250);
            }
        } else {
            for frame in handler.tick() {
                if failed(&frame) {
                    code = 1;
                }
                if output && !emit(out, err, &frame, mode) {
                    output = false;
                    code = 1;
                    reason = Some("output_failure");
                }
            }
        }
        if eof || !output {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let line = match input.recv_timeout(Duration::from_millis(5)) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eof = true;
                reason = Some("eof");
                next_close = Instant::now();
                continue;
            }
        };
        let frame = match line.and_then(|line| parse_line(&line)) {
            Ok(command) => handler.execute(command),
            Err(error) => handler.respond(Err(error)),
        };
        if failed(&frame) {
            code = 1;
        }
        if !emit(out, err, &frame, mode) {
            output = false;
            code = 1;
            reason = Some("output_failure");
            next_close = Instant::now();
        }
        if handler.quitting {
            for frame in handler.cancel_waits("quit") {
                if output && !emit(out, err, &frame, mode) {
                    code = 1;
                }
            }
            return code;
        }
    }
}
#[cfg(feature = "hosted-direct")]
pub fn stdin_shell(run_id: String, json: bool, out: &mut impl Write, err: &mut impl Write) -> i32 {
    let guard = match super::signals::Guard::install() {
        Ok(guard) => guard,
        Err(error) => {
            let _ = writeln!(err, "cannot install shutdown signals: {error}");
            return 1;
        }
    };
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
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = tx.send(Err(OperationError::validation(format!(
                        "input error: {error}"
                    ))));
                    break;
                }
            }
        }
    });
    run_with_signals(
        Handler::new(crate::dal::local::controller::LocalController::new(run_id)),
        rx,
        out,
        err,
        if json {
            OutputMode::Json
        } else {
            OutputMode::Human
        },
        || guard.take(),
    )
}
