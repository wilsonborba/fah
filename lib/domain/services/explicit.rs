//! Typed explicit operations share the ordinary request ledger and sole backend.
use super::{recovery::RecoveryBackend, requests::RequestRecord, usb::UsbSession};
use crate::domain::models::{
    OperationError,
    control_plan::{ControlPlan, ValidatedControlPlan},
    requests::RequestReport,
};
pub trait ExplicitBackend: RecoveryBackend {
    fn submit_control_plan(&mut self, plan: ValidatedControlPlan) -> Result<u64, OperationError>;
    fn noop(&mut self) -> Result<[u32; 4], OperationError>;
}
impl<B: ExplicitBackend> UsbSession<B> {
    pub fn submit_control_plan(
        &mut self,
        plan: ControlPlan,
    ) -> Result<RequestReport, OperationError> {
        self.check_request_capacity()?;
        let plan = plan.validate()?;
        self.prepared_device(plan.metadata.device)?;
        let metadata = plan.metadata.clone();
        let id = self.session.requests.submit()?;
        let result = self
            .session
            .initialized_backend()?
            .submit_control_plan(plan);
        let token = match self.effect(result) {
            Ok(token) => token,
            Err(error) => {
                self.session.requests.fail(id, error.clone(), None, true)?;
                self.session.requests.reclaim(id)?;
                return Err(error);
            }
        };
        self.records.insert(
            id,
            RequestRecord {
                metadata,
                token,
                logical_cancel: false,
                timed_out: false,
                data: None,
            },
        );
        self.request_status(id)
    }
    pub fn noop(&mut self) -> Result<[u32; 4], OperationError> {
        let result = self.session.initialized_backend()?.noop();
        self.effect(result)
    }
    pub fn reset_endpoint_request(
        &mut self,
        target: crate::domain::models::RequestId,
        preserve: bool,
    ) -> Result<crate::domain::models::requests::AbortReport, OperationError> {
        self.recover_request(target, Some(preserve))
    }
}
