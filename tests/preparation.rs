use fah::domain::{
    models::{ControllerState, ErrorCategory, OperationError},
    services::{session::*, usb::*},
};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct State {
    trace: Vec<String>,
    status: u32,
    fail: Option<&'static str>,
    next_token: u64,
    pending: std::collections::BTreeMap<u64, fah::domain::models::requests::OwnedRequest>,
    completions: std::collections::BTreeMap<u64, fah::domain::models::requests::BackendCompletion>,
}
struct Fake(Arc<Mutex<State>>);
impl Fake {
    fn effect(&self, op: &str) -> Result<(), OperationError> {
        let mut s = self.0.lock().unwrap();
        s.trace.push(op.into());
        if s.fail == Some(op) {
            Err(OperationError {
                category: ErrorCategory::Controller,
                message: "injected failure".into(),
                os_code: None,
                completion_code: None,
            })
        } else {
            Ok(())
        }
    }
}
impl ControllerBackend for Fake {
    fn acquire(&mut self, _: &str) -> Result<(), AcquisitionFailure> {
        self.effect("acquire").map_err(|error| AcquisitionFailure {
            error,
            retained: false,
        })
    }
    fn initialize(&mut self) -> Result<(), OperationError> {
        self.effect("init-controller")
    }
    fn inspect(&self) -> ControllerSnapshot {
        ControllerSnapshot {
            target: None,
            interrupt_mode: None,
            retained_allocations: 0,
        }
    }
    fn close(&mut self) -> Result<(), OperationError> {
        self.effect("close")?;
        let mut state = self.0.lock().unwrap();
        state.pending.clear();
        state.completions.clear();
        Ok(())
    }
}
impl DeviceBackend for Fake {
    fn port_count(&self) -> Result<u8, OperationError> {
        Ok(1)
    }
    fn port_status(&self, _: u8) -> Result<u32, OperationError> {
        self.effect("read")?;
        Ok(self.0.lock().unwrap().status)
    }
    fn acknowledge_port(&mut self, _: u8, _: u32) -> Result<(), OperationError> {
        self.effect("ack")
    }
    fn reset_port(&mut self, _: u8, _: bool) -> Result<(), OperationError> {
        self.effect("reset")
    }
    fn enable_slot(&mut self) -> Result<u8, OperationError> {
        self.effect("enable-slot")?;
        Ok(1)
    }
    fn address_device(&mut self, _: u8, _: u8, bsr: bool) -> Result<(), OperationError> {
        self.effect(if bsr { "address:bsr" } else { "address:normal" })
    }
}
fn fixture() -> (UsbSession<Fake>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        status: 3 | (1 << 9) | (1 << 10),
        ..Default::default()
    }));
    let mut usb = UsbSession::new(Fake(state.clone()), 55);
    usb.session.acquire("guest").unwrap();
    usb.initialize().unwrap();
    (usb, state)
}
#[test]
fn explicit_preparation_has_no_descriptor_or_configuration_side_effects() {
    let (mut usb, state) = fixture();
    let port = usb.inspect_ports().unwrap()[0].id;
    assert!(usb.enable_slot(port).is_err());
    let port = usb.reset_port(port, false).unwrap().id;
    let slot = usb.enable_slot(port).unwrap();
    let slot = usb.address_device(slot.id, true).unwrap();
    assert!(usb.address_device(slot.id, true).is_err());
    let slot = usb.address_device(slot.id, false).unwrap();
    assert!(usb.address_device(slot.id, false).is_err());
    let trace: Vec<_> = state
        .lock()
        .unwrap()
        .trace
        .iter()
        .filter(|s| s.as_str() != "read")
        .cloned()
        .collect();
    assert_eq!(
        trace,
        vec![
            "acquire",
            "init-controller",
            "reset",
            "enable-slot",
            "address:bsr",
            "address:normal"
        ]
    );
    usb.close().unwrap();
    assert!(usb.device(slot.id).is_err());
}
#[test]
fn uncertain_step_failure_requires_recovery_and_never_enumerates() {
    let (mut usb, state) = fixture();
    let port = usb.inspect_ports().unwrap()[0].id;
    let port = usb.reset_port(port, false).unwrap().id;
    state.lock().unwrap().fail = Some("enable-slot");
    assert!(usb.enable_slot(port).is_err());
    assert_eq!(usb.session.state(), ControllerState::RecoveryRequired);
    assert!(usb.enable_slot(port).is_err());
    assert!(
        !state
            .lock()
            .unwrap()
            .trace
            .iter()
            .any(|op| op.contains("descriptor"))
    );
}
#[test]
fn disconnect_and_controller_reopen_reject_stale_ids() {
    let (mut usb, state) = fixture();
    let port = usb.inspect_ports().unwrap()[0].id;
    let port = usb.reset_port(port, false).unwrap().id;
    let slot = usb.enable_slot(port).unwrap();
    state.lock().unwrap().status = 0;
    usb.inspect_ports().unwrap();
    assert!(usb.address_device(slot.id, false).is_err());
    usb.close().unwrap();
    usb.session.acquire("guest").unwrap();
    usb.initialize().unwrap();
    assert!(usb.reset_port(port, false).is_err());
}

use fah::domain::models::descriptors::DescriptorRequest;
impl ConfigurationBackend for Fake {
    fn descriptor(&mut self, _: u8, request: DescriptorRequest) -> Result<Vec<u8>, OperationError> {
        self.effect(&format!("descriptor:{}", request.length))?;
        let bytes = vec![
            9, 2, 25, 0, 1, 2, 0, 128, 50, 9, 4, 0, 0, 1, 3, 0, 0, 0, 7, 5, 129, 3, 8, 0, 10,
        ];
        Ok(bytes[..bytes.len().min(request.length as usize)].to_vec())
    }
    fn ep0_packet_size(&mut self, _: u8, size: u16) -> Result<(), OperationError> {
        self.effect(&format!("ep0:{size}"))
    }
    fn set_configuration(&mut self, _: u8, value: u8) -> Result<(), OperationError> {
        self.effect(&format!("configuration:{value}"))
    }
    fn set_interface(&mut self, _: u8, interface: u8, alternate: u8) -> Result<(), OperationError> {
        self.effect(&format!("interface:{interface}:{alternate}"))
    }
    fn setup_endpoints(
        &mut self,
        _: u8,
        interface: u8,
        alternate: u8,
        _: &[u8],
    ) -> Result<Vec<EndpointSnapshot>, OperationError> {
        self.effect(&format!("endpoints:{interface}:{alternate}"))?;
        Ok(vec![EndpointSnapshot {
            address: 129,
            transfer_type: "interrupt".into(),
            direction: "in".into(),
            max_packet_size: 8,
            interval: 10,
        }])
    }
}
#[test]
fn configuration_and_host_setup_are_independent_explicit_effects() {
    let (mut usb, state) = fixture();
    let port = usb.inspect_ports().unwrap()[0].id;
    let port = usb.reset_port(port, false).unwrap().id;
    let device = usb.enable_slot(port).unwrap();
    usb.address_device(device.id, false).unwrap();
    usb.ep0_packet_size(device.id, 8).unwrap();
    usb.set_configuration(device.id, 2).unwrap();
    assert!(usb.setup_endpoints(device.id, 0, 0).is_err());
    let request = DescriptorRequest {
        kind: 2,
        index: 1,
        language: 0,
        length: 9,
    };
    assert!(
        usb.descriptor(device.id, request)
            .unwrap()
            .parse_error
            .is_none()
    );
    assert!(usb.setup_endpoints(device.id, 0, 0).is_err());
    usb.descriptor(
        device.id,
        DescriptorRequest {
            length: 25,
            ..request
        },
    )
    .unwrap();
    usb.set_interface(device.id, 0, 0).unwrap();
    let endpoints = usb.setup_endpoints(device.id, 0, 0).unwrap();
    assert_eq!(endpoints[0].address, 129);
    assert!(usb.set_configuration(device.id, 1).is_err());
    assert!(usb.set_interface(device.id, 0, 1).is_err());
    let trace: Vec<_> = state
        .lock()
        .unwrap()
        .trace
        .iter()
        .filter(|op| op.as_str() != "read")
        .cloned()
        .collect();
    assert_eq!(
        trace,
        vec![
            "acquire",
            "init-controller",
            "reset",
            "enable-slot",
            "address:normal",
            "ep0:8",
            "configuration:2",
            "descriptor:9",
            "descriptor:25",
            "interface:0:0",
            "endpoints:0:0"
        ]
    );
}

use fah::domain::{
    models::{RequestState, requests::*},
    services::requests::RequestBackend,
};
impl RequestBackend for Fake {
    fn submit_request(&mut self, request: OwnedRequest) -> Result<u64, OperationError> {
        self.effect("submit")?;
        let mut state = self.0.lock().unwrap();
        state.next_token += 1;
        let token = state.next_token;
        state.pending.insert(token, request);
        Ok(token)
    }
    fn poll_request(&mut self, token: u64) -> Result<Option<BackendCompletion>, OperationError> {
        self.effect("poll")?;
        let mut state = self.0.lock().unwrap();
        let result = state.completions.remove(&token);
        if result.as_ref().is_some_and(|r| r.quiescent) {
            state.pending.remove(&token);
        }
        Ok(result)
    }
}
fn addressed() -> (UsbSession<Fake>, Arc<Mutex<State>>, DeviceId) {
    let (mut usb, state) = fixture();
    let port = usb.inspect_ports().unwrap()[0].id;
    let port = usb.reset_port(port, false).unwrap().id;
    let device = usb.enable_slot(port).unwrap();
    usb.address_device(device.id, false).unwrap();
    (usb, state, device.id)
}
fn control(device: DeviceId, length: usize) -> RequestSpec {
    RequestSpec {
        device,
        endpoint: 0,
        transfer_type: TransferType::Control,
        direction: Direction::In,
        length,
        setup: Some(SetupPacket {
            request_type: 128,
            request: 0,
            value: 0,
            index: 0,
            length: length as u16,
        }),
        data: vec![],
    }
}
fn complete(actual: usize, code: u32, bytes: Vec<u8>) -> BackendCompletion {
    BackendCompletion {
        actual_length: Some(actual),
        completion_code: Some(code),
        data: Some(bytes),
        error: None,
        quiescent: true,
    }
}
#[test]
fn timeout_and_logical_cancel_keep_owned_buffers_until_real_completion() {
    let (mut usb, state, device) = addressed();
    let mut spec = control(device, 4);
    spec.direction = Direction::Out;
    spec.setup.as_mut().unwrap().request_type = 64;
    spec.setup.as_mut().unwrap().request = 85;
    spec.data = vec![1, 2, 3, 4];
    let id = usb.submit_request(spec).unwrap().id;
    usb.logical_cancel_request(id).unwrap();
    assert!(
        usb.wait_request(id, std::time::Duration::ZERO)
            .unwrap()
            .timed_out
    );
    assert_eq!(state.lock().unwrap().pending[&1].buffer, vec![1, 2, 3, 4]);
    assert!(usb.reclaim_request(id).is_err());
    state
        .lock()
        .unwrap()
        .completions
        .insert(1, complete(4, 1, vec![1, 2, 3, 4]));
    let report = usb.poll_request(id).unwrap();
    assert!(matches!(report.state, RequestState::Completed { .. }));
    assert!(report.logical_cancel && report.timed_out);
    assert_eq!(report.data, Some(vec![1, 2, 3, 4]));
    let polls = state
        .lock()
        .unwrap()
        .trace
        .iter()
        .filter(|op| op.as_str() == "poll")
        .count();
    usb.poll_request(id).unwrap();
    assert_eq!(
        polls,
        state
            .lock()
            .unwrap()
            .trace
            .iter()
            .filter(|op| op.as_str() == "poll")
            .count()
    );
    assert!(state.lock().unwrap().pending.is_empty());
    usb.reclaim_request(id).unwrap();
    assert!(usb.reclaim_request(id).is_err());
}
#[test]
fn out_of_order_short_and_duplicate_completions_never_change_another_request() {
    let (mut usb, state, device) = addressed();
    let first = usb.submit_request(control(device, 2)).unwrap().id;
    let second = usb.submit_request(control(device, 2)).unwrap().id;
    state
        .lock()
        .unwrap()
        .completions
        .insert(2, complete(1, 13, vec![34]));
    assert_eq!(usb.poll_request(second).unwrap().data, Some(vec![34]));
    assert!(matches!(
        usb.poll_request(first).unwrap().state,
        RequestState::Pending { .. }
    ));
    state
        .lock()
        .unwrap()
        .completions
        .insert(2, complete(2, 1, vec![99, 99]));
    assert_eq!(usb.poll_request(second).unwrap().data, Some(vec![34]));
    state
        .lock()
        .unwrap()
        .completions
        .insert(1, complete(2, 1, vec![11, 12]));
    assert_eq!(usb.poll_request(first).unwrap().data, Some(vec![11, 12]));
    usb.reclaim_request(first).unwrap();
    usb.reclaim_request(second).unwrap();
    assert!(usb.request_status(first).is_err());
}
#[test]
fn unconfirmed_failure_retains_buffers_and_requires_quiescence() {
    let (mut usb, state, device) = addressed();
    let id = usb.submit_request(control(device, 2)).unwrap().id;
    let error = OperationError {
        category: ErrorCategory::RecoveryRequired,
        message: "invalid residual".into(),
        os_code: None,
        completion_code: Some(1),
    };
    state.lock().unwrap().completions.insert(
        1,
        BackendCompletion {
            actual_length: None,
            completion_code: Some(1),
            data: None,
            error: Some(error),
            quiescent: false,
        },
    );
    assert!(matches!(
        usb.poll_request(id).unwrap().state,
        RequestState::Failed {
            quiescent: false,
            ..
        }
    ));
    assert_eq!(usb.session.state(), ControllerState::RecoveryRequired);
    assert!(usb.reclaim_request(id).is_err());
    assert!(usb.session.requests.advance_generation().is_err());
    assert_eq!(state.lock().unwrap().pending.len(), 1);
    usb.close().unwrap();
    assert!(state.lock().unwrap().pending.is_empty());
    assert!(usb.request_status(id).is_err());
}
#[test]
fn malformed_inputs_and_low_speed_bulk_fail_before_publication() {
    let (mut usb, state, device) = addressed();
    let mut spec = control(device, 2);
    spec.setup.as_mut().unwrap().length = 3;
    assert!(usb.submit_request(spec).is_err());
    let mut spec = control(device, 2);
    spec.direction = Direction::Out;
    assert!(usb.submit_request(spec).is_err());
    usb.set_configuration(device, 2).unwrap();
    state.lock().unwrap().status = 3 | (1 << 9) | (2 << 10);
    let spec = RequestSpec {
        device,
        endpoint: 129,
        transfer_type: TransferType::Bulk,
        direction: Direction::In,
        length: 8,
        setup: None,
        data: vec![],
    };
    assert_eq!(
        usb.submit_request(spec).unwrap_err().category,
        ErrorCategory::Unsupported
    );
    assert!(!state.lock().unwrap().trace.iter().any(|op| op == "submit"));
}

#[test]
fn bounded_pending_capacity_rejects_without_publishing_or_losing_buffers() {
    let (mut usb, state, device) = addressed();
    for _ in 0..128 {
        usb.submit_request(control(device, 2)).unwrap();
    }
    assert!(usb.submit_request(control(device, 2)).is_err());
    assert_eq!(state.lock().unwrap().pending.len(), 128);
    usb.close().unwrap();
    assert!(state.lock().unwrap().pending.is_empty());
}
