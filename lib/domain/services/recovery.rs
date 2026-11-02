//! Hardware abort is endpoint-wide and reports collateral requests explicitly.
use super::{requests::RequestBackend, usb::UsbSession};
use crate::domain::models::{ErrorCategory, OperationError, RequestId, requests::*};
pub trait RecoveryBackend: RequestBackend {
    /// Err means no complete quiescence proof; retain every uncertain resource.
    /// After stop confirmation, partial progress is returned with an error field.
    fn abort_endpoint(&mut self, slot: u8, endpoint: u8) -> Result<BackendAbort, OperationError>;
}
impl<B: RecoveryBackend> UsbSession<B> {
    pub fn abort_request(&mut self, target: RequestId) -> Result<AbortReport, OperationError> {
        let original = self.request_status(target)?;
        if !original.state.device_may_access() {
            return Err(OperationError::validation("request already terminal"));
        }
        let device = original.metadata.device;
        let endpoint = original.metadata.endpoint;
        let affected: Vec<_> = self
            .records
            .iter()
            .filter(|(id, record)| {
                record.metadata.device == device
                    && record.metadata.endpoint == endpoint
                    && self
                        .session
                        .requests
                        .state(**id)
                        .is_ok_and(|state| state.device_may_access())
            })
            .map(|(id, record)| (*id, record.token))
            .collect();
        let outcome = self
            .session
            .recovery_backend()?
            .abort_endpoint(device.slot, endpoint);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.session.recovery_required();
                return Err(error);
            }
        };
        let expected: std::collections::BTreeSet<_> =
            affected.iter().map(|(_, token)| *token).collect();
        let returned: Vec<_> = outcome
            .completed
            .iter()
            .map(|(token, _)| *token)
            .chain(outcome.aborted.iter().copied())
            .collect();
        let unique: std::collections::BTreeSet<_> = returned.iter().copied().collect();
        if unique.len() != returned.len()
            || !unique.is_subset(&expected)
            || (outcome.error.is_none() && outcome.restart_ready && unique != expected)
        {
            self.session.recovery_required();
            return Err(OperationError::validation(
                "inconsistent abort scope/result tokens",
            ));
        }
        let mut completed = Vec::new();
        let mut aborted = Vec::new();
        for (token, result) in outcome.completed {
            let Some((id, _)) = affected.iter().find(|(_, candidate)| *candidate == token) else {
                self.session.recovery_required();
                return Err(OperationError::validation(
                    "abort returned unknown completion token",
                ));
            };
            let actual = result
                .actual_length
                .ok_or_else(|| OperationError::validation("abort completion missing length"))?;
            if !result.quiescent
                || result.error.is_some()
                || actual > self.records[id].metadata.requested_length
                || !result
                    .completion_code
                    .is_some_and(|code| matches!(code, 1 | 13))
                || result.data.as_ref().is_none_or(|data| data.len() != actual)
            {
                self.session.recovery_required();
                return Err(OperationError {
                    category: ErrorCategory::RecoveryRequired,
                    message: "invalid completion in abort result".into(),
                    os_code: None,
                    completion_code: result.completion_code,
                });
            }
            self.session
                .requests
                .complete(*id, actual, result.completion_code.unwrap())?;
            self.records.get_mut(id).unwrap().data = result.data;
            completed.push(*id);
        }
        for token in outcome.aborted {
            let Some((id, _)) = affected.iter().find(|(_, candidate)| *candidate == token) else {
                self.session.recovery_required();
                return Err(OperationError::validation(
                    "abort returned unknown collateral token",
                ));
            };
            self.session.requests.confirm_abort(*id)?;
            aborted.push(*id);
        }
        if outcome.error.is_some() || !outcome.restart_ready {
            self.session.recovery_required();
        }
        Ok(AbortReport {
            target,
            device,
            endpoint,
            scope: "endpoint_wide".into(),
            completed,
            aborted,
            restart_ready: outcome.restart_ready,
            error: outcome.error,
        })
    }
}
