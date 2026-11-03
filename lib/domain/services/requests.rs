//! Request ownership lives in the backend until terminal completion/quiescence.
use super::usb::{ConfigurationBackend, UsbSession};
use crate::domain::models::{
    DeviceState, ErrorCategory, OperationError, RequestId, RequestState, requests::*,
};
use std::time::{Duration, Instant};
pub trait RequestBackend: ConfigurationBackend {
    /// Err guarantees no request was published. Uncertain publication must retain
    /// ownership and return an accepted token whose later result requires recovery.
    fn submit_request(&mut self, request: OwnedRequest) -> Result<u64, OperationError>;
    fn poll_request(&mut self, token: u64) -> Result<Option<BackendCompletion>, OperationError>;
}
pub(crate) struct RequestRecord {
    pub metadata: RequestMetadata,
    pub token: u64,
    pub logical_cancel: bool,
    pub timed_out: bool,
    pub data: Option<Vec<u8>>,
}
impl<B: RequestBackend> UsbSession<B> {
    pub fn submit_request(&mut self, spec: RequestSpec) -> Result<RequestReport, OperationError> {
        self.check_request_capacity()?;
        let request = spec.prepare()?;
        let metadata = request.metadata.clone();
        let device = self.prepared_device(metadata.device)?;
        if metadata.transfer_type != TransferType::Control {
            if device.state != DeviceState::Configured {
                return Err(OperationError::validation(
                    "endpoint requests require configuration",
                ));
            }
            let speed = self.fresh_port(device.port)?.speed_code;
            if speed == 2 && metadata.transfer_type == TransferType::Bulk {
                return Err(unsupported("Low Speed has no bulk transfers"));
            }
            let expected = match metadata.transfer_type {
                TransferType::Bulk => "bulk",
                TransferType::Interrupt => "interrupt",
                _ => unreachable!(),
            };
            if !self
                .endpoints
                .get(&metadata.device.slot)
                .is_some_and(|eps| {
                    eps.iter()
                        .any(|ep| ep.address == metadata.endpoint && ep.transfer_type == expected)
                })
            {
                return Err(OperationError::validation(
                    "endpoint/type not configured in host",
                ));
            }
        }
        let id = self.session.requests.submit()?;
        let result = self.session.initialized_backend()?.submit_request(request);
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
    pub(crate) fn check_request_capacity(&self) -> Result<(), OperationError> {
        if self.records.len() >= 1024
            || self
                .records
                .keys()
                .filter(|id| {
                    self.session
                        .requests
                        .state(**id)
                        .is_ok_and(|state| state.device_may_access())
                })
                .count()
                >= 128
        {
            return Err(OperationError::validation(
                "request limit reached; reclaim terminal requests",
            ));
        }
        Ok(())
    }
    pub fn request_reports(&self) -> Result<Vec<RequestReport>, OperationError> {
        self.records
            .keys()
            .map(|id| self.request_status(*id))
            .collect()
    }
    pub fn request_status(&self, id: RequestId) -> Result<RequestReport, OperationError> {
        let record = self
            .records
            .get(&id)
            .ok_or_else(|| OperationError::validation("unknown or reclaimed request"))?;
        Ok(RequestReport {
            id,
            metadata: record.metadata.clone(),
            state: self.session.requests.state(id)?.clone(),
            logical_cancel: record.logical_cancel,
            timed_out: record.timed_out,
            data: record.data.clone(),
        })
    }
    pub fn poll_request(&mut self, id: RequestId) -> Result<RequestReport, OperationError> {
        let record = self
            .records
            .get(&id)
            .ok_or_else(|| OperationError::validation("unknown request"))?;
        if matches!(
            self.session.requests.state(id)?,
            RequestState::Pending { .. }
        ) {
            let token = record.token;
            let result = self.session.initialized_backend()?.poll_request(token);
            if let Some(completion) = self.effect(result)? {
                if let Some(error) = completion.error {
                    self.session.requests.fail(
                        id,
                        error,
                        completion.completion_code,
                        completion.quiescent,
                    )?;
                    if !completion.quiescent {
                        self.session.recovery_required();
                    }
                } else {
                    let length = self.records[&id].metadata.requested_length;
                    let valid = completion.quiescent
                        && completion.actual_length.is_some_and(|actual| {
                            actual <= length
                                && completion
                                    .data
                                    .as_ref()
                                    .is_some_and(|data| data.len() == actual)
                        })
                        && completion
                            .completion_code
                            .is_some_and(|code| matches!(code, 1 | 13));
                    if !valid {
                        let error = OperationError {
                            category: ErrorCategory::RecoveryRequired,
                            message: "backend returned an unconfirmed or invalid completion".into(),
                            os_code: None,
                            completion_code: completion.completion_code,
                        };
                        self.session
                            .requests
                            .fail(id, error, completion.completion_code, false)?;
                        self.session.recovery_required();
                    } else {
                        self.session.requests.complete(
                            id,
                            completion.actual_length.unwrap(),
                            completion.completion_code.unwrap(),
                        )?;
                        self.records.get_mut(&id).unwrap().data = completion.data;
                    }
                }
            }
        }
        self.request_status(id)
    }
    pub fn logical_cancel_request(
        &mut self,
        id: RequestId,
    ) -> Result<RequestReport, OperationError> {
        self.session.requests.logical_cancel(id)?;
        self.records
            .get_mut(&id)
            .ok_or_else(|| OperationError::validation("unknown request"))?
            .logical_cancel = true;
        self.request_status(id)
    }
    pub fn timeout_request(&mut self, id: RequestId) -> Result<RequestReport, OperationError> {
        self.session.requests.timeout(id)?;
        self.records
            .get_mut(&id)
            .ok_or_else(|| OperationError::validation("unknown request"))?
            .timed_out = true;
        self.request_status(id)
    }
    /// Synchronous library helper. The interactive actor uses poll/deadline records
    /// so waiting on one request does not block input or other commands.
    pub fn wait_request(
        &mut self,
        id: RequestId,
        timeout: Duration,
    ) -> Result<RequestReport, OperationError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| OperationError::validation("deadline overflow"))?;
        loop {
            let report = self.poll_request(id)?;
            if !matches!(report.state, RequestState::Pending { .. }) {
                return Ok(report);
            }
            if Instant::now() >= deadline {
                return self.timeout_request(id);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    /// Reclaim only an already observed terminal result, without a hidden poll.
    pub fn reclaim_request(&mut self, id: RequestId) -> Result<RequestReport, OperationError> {
        let report = self.request_status(id)?;
        self.session.requests.reclaim(id)?;
        self.records.remove(&id);
        Ok(report)
    }
}
