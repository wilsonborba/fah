//! Serialized owner. Backends retain uncertain resources until confirmed close.
use crate::domain::models::{ControllerState, OperationError, RequestLedger};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSnapshot {
    pub target: Option<String>,
    pub interrupt_mode: Option<String>,
    pub retained_allocations: usize,
    pub close_stage: Option<String>,
}
/// An acquire error states whether any resources with uncertain ownership remain.
pub struct AcquisitionFailure {
    pub error: OperationError,
    pub retained: bool,
}
pub trait ControllerBackend {
    fn acquire(&mut self, target: &str) -> Result<(), AcquisitionFailure>;
    fn initialize(&mut self) -> Result<(), OperationError>;
    /// Pure inspection: must not enumerate, reset, address or configure.
    fn inspect(&self) -> ControllerSnapshot;
    /// Success means owned FAH DMA is quiescent, workers stopped and mappings
    /// released. With no FAH allocation/publication, ownership can be released
    /// without asserting the prior foreign engine was initialized by FAH.
    fn close(&mut self) -> Result<(), OperationError>;
}
/// Not Clone; all effects require exclusive access to this owner. Resource Drop
/// is deliberately not used to guess quiescence after an incomplete shutdown.
pub struct Session<B: ControllerBackend> {
    backend: Option<B>,
    state: ControllerState,
    recovery_revision: Option<u64>,
    pub requests: RequestLedger,
}
impl<B: ControllerBackend> Session<B> {
    pub fn new(backend: B, session_id: u64) -> Self {
        Self {
            backend: Some(backend),
            state: ControllerState::Closed,
            recovery_revision: Some(0),
            requests: RequestLedger::new(session_id),
        }
    }
    pub fn identity(&self) -> u64 {
        self.requests.session_id()
    }
    pub fn state(&self) -> ControllerState {
        self.state
    }
    pub fn inspect(&self) -> ControllerSnapshot {
        self.backend.as_ref().unwrap().inspect()
    }
    pub fn acquire(&mut self, target: &str) -> Result<(), OperationError> {
        if self.state != ControllerState::Closed {
            return Err(OperationError::validation(
                "session already owns or retains a controller",
            ));
        }
        match self.backend.as_mut().unwrap().acquire(target) {
            Ok(()) => {
                self.state = ControllerState::AcquiredUnknown;
                Ok(())
            }
            Err(failure) => {
                if failure.retained {
                    self.state = ControllerState::RecoveryRequired;
                    self.bump_recovery_revision();
                }
                Err(failure.error)
            }
        }
    }
    pub fn initialize(&mut self) -> Result<(), OperationError> {
        if self.state != ControllerState::AcquiredUnknown {
            return Err(OperationError::validation(
                "initialize requires acquired unknown state",
            ));
        }
        match self.backend.as_mut().unwrap().initialize() {
            Ok(()) => {
                self.state = ControllerState::Initialized;
                Ok(())
            }
            Err(error) => {
                self.recovery_required();
                Err(error)
            }
        }
    }
    /// All later device/request operations pass through the same authority.
    pub fn initialized_backend(&mut self) -> Result<&mut B, OperationError> {
        if self.state != ControllerState::Initialized {
            return Err(OperationError::validation("controller is not initialized"));
        }
        Ok(self.backend.as_mut().unwrap())
    }
    pub(crate) fn recovery_backend(&mut self) -> Result<&mut B, OperationError> {
        if !matches!(
            self.state,
            ControllerState::Initialized | ControllerState::RecoveryRequired
        ) {
            return Err(OperationError::validation(
                "controller unavailable for recovery",
            ));
        }
        Ok(self.backend.as_mut().unwrap())
    }
    pub fn recovery_required(&mut self) {
        if self.state != ControllerState::Closed {
            self.state = ControllerState::RecoveryRequired;
            self.bump_recovery_revision();
        }
    }
    fn bump_recovery_revision(&mut self) {
        self.recovery_revision = self
            .recovery_revision
            .and_then(|value| value.checked_add(1));
    }
    pub(crate) fn recovery_revision(&self) -> Option<u64> {
        self.recovery_revision
    }
    pub(crate) fn restore_scoped_recovery(&mut self, revision: u64) -> bool {
        if self.state == ControllerState::RecoveryRequired
            && self.recovery_revision == Some(revision)
        {
            self.state = ControllerState::Initialized;
            true
        } else {
            false
        }
    }
    pub fn close(&mut self) -> Result<(), OperationError> {
        if self.state == ControllerState::Closed {
            return Ok(());
        }
        let next_identity = next_session_id()?;
        self.state = ControllerState::Closing;
        if let Err(error) = self.backend.as_mut().unwrap().close() {
            self.recovery_required();
            return Err(error);
        }
        // The backend's close contract includes quiescence of every request.
        // Outstanding metadata is invalidated only after this confirmed result.
        self.requests = RequestLedger::new(next_identity);
        self.state = ControllerState::Closed;
        Ok(())
    }
}
impl<B: ControllerBackend> Drop for Session<B> {
    fn drop(&mut self) {
        if self.state != ControllerState::Closed {
            // Keep FD/MMIO/allocator/consumer lifetimes when stop is uncertain.
            if let Some(backend) = self.backend.take() {
                std::mem::forget(backend);
            }
        }
    }
}
fn next_session_id() -> Result<u64, OperationError> {
    crate::core::identity::fresh()
        .ok_or_else(|| OperationError::validation("session identity exhausted"))
}
