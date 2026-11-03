//! One JSON object per line; command responses and async events are distinct.
use crate::domain::models::{ControllerState, OperationError, OperationResult, Outcome};
use serde::Serialize;
use serde_json::Value;
#[derive(Debug, Serialize)]
pub struct Frame {
    pub kind: &'static str,
    #[serde(flatten)]
    pub result: OperationResult<Value>,
}
impl Frame {
    pub fn response(
        id: u64,
        state: ControllerState,
        result: Result<Value, OperationError>,
    ) -> Self {
        Self {
            kind: "response",
            result: OperationResult::new(
                id,
                state,
                match result {
                    Ok(value) => Outcome::Completed(value),
                    Err(error) => Outcome::Failed(error),
                },
            ),
        }
    }
}
