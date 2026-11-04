//! State and result contracts. These types perform no device I/O.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerState {
    Closed,
    AcquiredUnknown,
    Initialized,
    Closing,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Observed,
    PortReset,
    SlotEnabled,
    DefaultAddress,
    Addressed,
    Configured,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Validation,
    Unsupported,
    OperatingSystem,
    Controller,
    Timeout,
    Disconnect,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationError {
    pub category: ErrorCategory,
    pub message: String,
    pub os_code: Option<i32>,
    pub completion_code: Option<u32>,
}
impl OperationError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Validation,
            message: message.into(),
            os_code: None,
            completion_code: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestId {
    pub session: u64,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RequestState {
    Pending {
        logical_cancel: bool,
        timed_out: bool,
    },
    Completed {
        actual_length: usize,
        completion_code: u32,
        logical_cancel: bool,
    },
    Failed {
        error: OperationError,
        completion_code: Option<u32>,
        quiescent: bool,
    },
    Aborted,
}
impl RequestState {
    pub fn device_may_access(&self) -> bool {
        matches!(
            self,
            Self::Pending { .. }
                | Self::Failed {
                    quiescent: false,
                    ..
                }
        )
    }
}

/// Metadata only. The backend must keep the associated DMA resources alive.
#[derive(Debug)]
pub struct RequestLedger {
    session: u64,
    generation: u64,
    next: u64,
    entries: BTreeMap<RequestId, RequestState>,
}
impl RequestLedger {
    pub fn new(session: u64) -> Self {
        Self {
            session,
            generation: 0,
            next: 0,
            entries: BTreeMap::new(),
        }
    }
    pub(crate) fn session_id(&self) -> u64 {
        self.session
    }
    pub fn submit(&mut self) -> Result<RequestId, OperationError> {
        let sequence = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("request ID exhausted"))?;
        let id = RequestId {
            session: self.session,
            generation: self.generation,
            sequence,
        };
        self.entries.insert(
            id,
            RequestState::Pending {
                logical_cancel: false,
                timed_out: false,
            },
        );
        Ok(id)
    }
    pub fn state(&self, id: RequestId) -> Result<&RequestState, OperationError> {
        self.entries
            .get(&id)
            .ok_or_else(|| OperationError::validation("unknown or stale request"))
    }
    fn pending(&mut self, id: RequestId) -> Result<&mut RequestState, OperationError> {
        let state = self
            .entries
            .get_mut(&id)
            .ok_or_else(|| OperationError::validation("unknown or stale request"))?;
        if !matches!(state, RequestState::Pending { .. }) {
            return Err(OperationError::validation("request is no longer pending"));
        }
        Ok(state)
    }
    pub fn logical_cancel(&mut self, id: RequestId) -> Result<(), OperationError> {
        if let RequestState::Pending { logical_cancel, .. } = self.pending(id)? {
            *logical_cancel = true;
        }
        Ok(())
    }
    pub fn timeout(&mut self, id: RequestId) -> Result<(), OperationError> {
        if let RequestState::Pending { timed_out, .. } = self.pending(id)? {
            *timed_out = true;
        }
        Ok(())
    }
    /// Call only after the backend validates a real completion event.
    pub fn complete(
        &mut self,
        id: RequestId,
        actual_length: usize,
        completion_code: u32,
    ) -> Result<(), OperationError> {
        let state = self.pending(id)?;
        let cancelled = matches!(
            state,
            RequestState::Pending {
                logical_cancel: true,
                ..
            }
        );
        *state = RequestState::Completed {
            actual_length,
            completion_code,
            logical_cancel: cancelled,
        };
        Ok(())
    }
    /// The backend must establish quiescence before calling this metadata operation.
    pub fn confirm_abort(&mut self, id: RequestId) -> Result<(), OperationError> {
        let state = self
            .entries
            .get_mut(&id)
            .ok_or_else(|| OperationError::validation("unknown request"))?;
        if !state.device_may_access() {
            return Err(OperationError::validation("request already terminal"));
        }
        *state = RequestState::Aborted;
        Ok(())
    }
    pub fn fail(
        &mut self,
        id: RequestId,
        error: OperationError,
        completion_code: Option<u32>,
        quiescent: bool,
    ) -> Result<(), OperationError> {
        *self.pending(id)? = RequestState::Failed {
            error,
            completion_code,
            quiescent,
        };
        Ok(())
    }
    pub fn reclaim(&mut self, id: RequestId) -> Result<RequestState, OperationError> {
        if self.state(id)?.device_may_access() {
            return Err(OperationError::validation("device still owns request"));
        }
        Ok(self.entries.remove(&id).expect("entry checked above"))
    }
    pub fn advance_generation(&mut self) -> Result<(), OperationError> {
        if self.entries.values().any(RequestState::device_may_access) {
            return Err(OperationError::validation("pending DMA ownership"));
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("generation exhausted"))?;
        self.entries.clear();
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "payload", rename_all = "snake_case")]
pub enum Outcome<T> {
    Pending,
    Completed(T),
    Failed(OperationError),
    Partial { payload: T, error: OperationError },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationResult<T> {
    pub schema_version: u32,
    pub operation_id: u64,
    pub session_state: ControllerState,
    pub outcome: Outcome<T>,
}
impl<T> OperationResult<T> {
    pub fn new(operation_id: u64, session_state: ControllerState, outcome: Outcome<T>) -> Self {
        Self {
            schema_version: 1,
            operation_id,
            session_state,
            outcome,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerEvent {
    Acquired,
    InitializationConfirmed,
    CloseRequested,
    QuiescenceConfirmed,
    RecoveryNeeded,
}
impl ControllerState {
    /// Events represent validated backend results, never automatic hardware actions.
    pub fn transition(self, event: ControllerEvent) -> Result<Self, OperationError> {
        use ControllerEvent as E;
        use ControllerState as S;
        match (self, event) {
            (S::Closed, E::Acquired) => Ok(S::AcquiredUnknown),
            (S::AcquiredUnknown, E::InitializationConfirmed) => Ok(S::Initialized),
            (S::AcquiredUnknown | S::Initialized | S::RecoveryRequired, E::CloseRequested) => {
                Ok(S::Closing)
            }
            (S::Closing, E::QuiescenceConfirmed) => Ok(S::Closed),
            (S::AcquiredUnknown | S::Initialized | S::Closing, E::RecoveryNeeded) => {
                Ok(S::RecoveryRequired)
            }
            _ => Err(OperationError::validation("invalid controller transition")),
        }
    }
}

pub mod ports;

pub mod descriptors;

pub mod requests;

pub mod capabilities;
pub mod control_plan;
pub mod raw;
