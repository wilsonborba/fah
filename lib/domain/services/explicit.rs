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
        use crate::domain::models::raw::{Effect, RawOperation as Op, effect};
        let identity = self.session.identity();
        if self.session.state() == crate::domain::models::ControllerState::RecoveryRequired
            && !self.raw_coherence.global_unknown
        {
            self.raw_coherence.mutate(Effect::Global, None, true)?;
        }
        if let Op::Reconcile { slot } = request {
            let mut restored = false;
            let mut current = None;
            if let Some(slot) = slot {
                let snapshot = self
                    .session
                    .recovery_backend()?
                    .raw_operation(identity, Op::SlotContext { slot })?;
                let pending = self.records.iter().any(|(id, r)| {
                    r.metadata.device.slot == slot
                        && self
                            .session
                            .requests
                            .state(*id)
                            .is_ok_and(|s| s.device_may_access())
                });
                restored = self.raw_coherence.reconcile(slot, &snapshot, pending);
                current = Some(snapshot);
            }
            return Ok(
                serde_json::json!({"scope":slot,"restored":restored,"managed_state":if self.raw_coherence.global_unknown{"unknown"}else if self.raw_coherence.slots.is_empty(){"coherent"}else{"slot_reconcile_required"},"coherence":self.raw_coherence,"current":current,"proof":"only known controller-only effects may match the original owned context; device mutations require explicit reconstruction; unknown global effects require coordinated close"}),
            );
        }
        let effect = effect(&request);
        let (baseline, pending) = if let Effect::Slot { slot, .. } = effect {
            let baseline = self
                .session
                .recovery_backend()?
                .raw_operation(identity, Op::SlotContext { slot })
                .ok();
            let pending = self.records.iter().any(|(id, r)| {
                r.metadata.device.slot == slot
                    && self
                        .session
                        .requests
                        .state(*id)
                        .is_ok_and(|s| s.device_may_access())
            });
            (baseline, pending)
        } else {
            (None, false)
        };
        self.raw_coherence.mutate(effect, baseline, pending)?;
        if self.raw_coherence.global_unknown {
            self.session.recovery_required();
        }
        let mut result = self
            .session
            .recovery_backend()?
            .raw_operation(identity, request)?;
        if let Some(value) = result.as_object_mut() {
            value.insert(
                "coherence_generation".into(),
                self.raw_coherence.generation.into(),
            );
            if value.contains_key("managed_state") {
                value.insert(
                    "managed_state".into(),
                    serde_json::json!(if self.raw_coherence.global_unknown {
                        "unknown"
                    } else if self.raw_coherence.slots.is_empty() {
                        "coherent"
                    } else {
                        "slot_reconcile_required"
                    }),
                );
            }
        }
        Ok(result)
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
