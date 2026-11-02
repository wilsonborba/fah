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
        self.effect("close")
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
