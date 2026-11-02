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
