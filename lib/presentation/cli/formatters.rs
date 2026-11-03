use super::outputs::Frame;
use std::io::{self, Write};
#[derive(Clone, Copy)]
pub enum OutputMode {
    Json,
    Human,
}
pub fn write(out: &mut impl Write, frame: &Frame, mode: OutputMode) -> io::Result<()> {
    match mode {
        OutputMode::Json => serde_json::to_writer(&mut *out, frame)?,
        OutputMode::Human => {
            let value = serde_json::to_value(frame)?;
            write!(
                out,
                "{} #{} [{}] {}",
                frame.kind, frame.result.operation_id, value["session_state"], value["outcome"]
            )?;
        }
    }
    writeln!(out)?;
    out.flush()
}
