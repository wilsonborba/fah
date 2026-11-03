//! Resumable shutdown ordering. Only completed stages advance the cursor.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseStage {
    StopSubmissions,
    HaltController,
    DisableBusMaster,
    DisableInterrupts,
    JoinEventPump,
    DropDriver,
    ReclaimDma,
    ReleaseAdapter,
    ReleaseBar,
    ReleaseVfio,
    Done,
}
impl CloseStage {
    fn next(self) -> Self {
        use CloseStage::*;
        match self {
            StopSubmissions => HaltController,
            HaltController => DisableBusMaster,
            DisableBusMaster => DisableInterrupts,
            DisableInterrupts => JoinEventPump,
            JoinEventPump => DropDriver,
            DropDriver => ReclaimDma,
            ReclaimDma => ReleaseAdapter,
            ReleaseAdapter => ReleaseBar,
            ReleaseBar => ReleaseVfio,
            ReleaseVfio | Done => Done,
        }
    }
}
pub trait ShutdownBackend {
    /// A failed step must retain resources needed for retry.
    fn close_step(&mut self, stage: CloseStage) -> Result<(), String>;
}
pub struct CloseProgress {
    stage: CloseStage,
}
impl Default for CloseProgress {
    fn default() -> Self {
        Self {
            stage: CloseStage::StopSubmissions,
        }
    }
}
impl CloseProgress {
    pub fn stage(&self) -> CloseStage {
        self.stage
    }
    pub fn drive(&mut self, backend: &mut impl ShutdownBackend) -> Result<(), String> {
        while self.stage != CloseStage::Done {
            backend.close_step(self.stage)?;
            self.stage = self.stage.next();
        }
        Ok(())
    }
}
