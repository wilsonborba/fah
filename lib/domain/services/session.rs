//! Serialized owner. Backends retain uncertain resources until confirmed close.
use crate::domain::models::{ControllerState, OperationError, RequestLedger};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSnapshot {
    pub target: Option<String>,
    pub interrupt_mode: Option<String>,
    pub retained_allocations: usize,
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
    /// Success means DMA is quiescent, interrupts stopped, mappings released.
    fn close(&mut self) -> Result<(), OperationError>;
}
/// Not Clone; all effects require exclusive access to this owner. Resource Drop
/// is deliberately not used to guess quiescence after an incomplete shutdown.
pub struct Session<B: ControllerBackend> {
    backend: Option<B>,
    state: ControllerState,
    pub requests: RequestLedger,
}
impl<B: ControllerBackend> Session<B> {
    pub fn new(backend: B, session_id: u64) -> Self {
        Self {
            backend: Some(backend),
            state: ControllerState::Closed,
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
                self.state = ControllerState::RecoveryRequired;
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
    pub fn recovery_required(&mut self) {
        if self.state != ControllerState::Closed {
            self.state = ControllerState::RecoveryRequired;
        }
    }
    pub fn close(&mut self) -> Result<(), OperationError> {
        if self.state == ControllerState::Closed {
            return Ok(());
        }
        let next_identity = next_session_id()?;
        self.state = ControllerState::Closing;
        if let Err(error) = self.backend.as_mut().unwrap().close() {
            self.state = ControllerState::RecoveryRequired;
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
    use std::sync::atomic::{AtomicU64, Ordering};
    static SESSION: AtomicU64 = AtomicU64::new(1 << 32);
    SESSION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| OperationError::validation("session identity exhausted"))
}
