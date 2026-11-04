//! Typed explicit operations share the ordinary request ledger and sole backend.
use super::{recovery::RecoveryBackend, requests::RequestRecord, usb::UsbSession};
use crate::domain::models::{
    OperationError,
    control_plan::{ControlPlan, ValidatedControlPlan},
    requests::RequestReport,
};
pub trait ExplicitBackend: RecoveryBackend {
    fn raw_operation(
        &mut self,
        _session: u64,
        _request: crate::domain::models::raw::RawOperation,
    ) -> Result<serde_json::Value, OperationError> {
        Err(crate::domain::models::requests::unsupported(
            "raw operations require the hosted owner",
        ))
    }
    fn submit_control_plan(&mut self, plan: ValidatedControlPlan) -> Result<u64, OperationError>;
    fn noop(&mut self) -> Result<[u32; 4], OperationError>;
}
impl<B: ExplicitBackend> UsbSession<B> {
    pub fn raw_operation(
        &mut self,
        request: crate::domain::models::raw::RawOperation,
    ) -> Result<serde_json::Value, OperationError> {
        let identity = self.session.identity();
        use crate::domain::models::raw::RawOperation as Op;
        if matches!(
            &request,
            Op::RegisterWrite { .. }
                | Op::SlotEnable
                | Op::StreamConfigure { .. }
                | Op::AddressRoute { .. }
                | Op::HubConfigure { .. }
                | Op::DeviceConfiguration { .. }
                | Op::EndpointConfigure { .. }
                | Op::Place { .. }
                | Op::Publish { .. }
                | Op::Doorbell { .. }
                | Op::Command { .. }
                | Op::Transfer { .. }
        ) {
            // A partial backend failure must not restore a stale managed view.
            self.session.recovery_required();
        }
        let result = self
            .session
            .recovery_backend()?
            .raw_operation(identity, request);
        if let Ok(value) = &result
            && value
                .get("managed_state")
                .and_then(serde_json::Value::as_str)
                == Some("unknown")
        {
            self.session.recovery_required();
        }
        result
    }
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
