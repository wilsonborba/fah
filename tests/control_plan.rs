use fah::domain::{
    models::{
        capabilities::{Availability, Boundary, registry},
        control_plan::*,
        requests::*,
    },
    services::usb::DeviceId,
};
fn plan() -> ControlPlan {
    ControlPlan {
        device: DeviceId {
            session: 1,
            generation: 1,
            slot: 1,
        },
        profile: ControlProfile::Compliant,
        setup: SetupPacket {
            request_type: 128,
            request: 6,
            value: 0x100,
            index: 0,
            length: 8,
        },
        data_stage: Some(DataStage {
            direction: Direction::In,
            length: 8,
        }),
        status_direction: Some(Direction::Out),
        data: vec![],
    }
}
#[test]
fn compliant_fields_and_compatibility_wire_lengths_remain_distinct() {
    let validated = plan().validate().unwrap();
    assert_eq!(validated.wire_setup, [128, 6, 0, 1, 0, 0, 8, 0]);
    let mut mismatched = plan();
    mismatched.setup.length = 17;
    assert!(mismatched.clone().validate().is_err());
    mismatched.profile = ControlProfile::DeviceCompatibility;
    let validated = mismatched.validate().unwrap();
    assert_eq!(validated.metadata.requested_length, 8);
    assert_eq!(
        u16::from_le_bytes([validated.wire_setup[6], validated.wire_setup[7]]),
        17
    );
}
#[test]
fn compatibility_no_data_parameter_does_not_invent_a_data_stage() {
    let mut declared = plan();
    declared.profile = ControlProfile::DeviceCompatibility;
    declared.setup.request_type = 64;
    declared.setup.request = 85;
    declared.setup.length = 42;
    declared.data_stage = None;
    declared.status_direction = Some(Direction::In);
    let validated = declared.validate().unwrap();
    assert!(!validated.has_data);
    assert!(validated.buffer.is_empty());
    assert_eq!(validated.wire_setup[6], 42);
}
#[test]
fn missing_status_intercepted_address_and_bad_bounds_reject() {
    let mut p = plan();
    p.status_direction = None;
    assert!(p.validate().is_err());
    let mut p = plan();
    p.setup.request_type = 0;
    p.setup.request = 5;
    p.profile = ControlProfile::DeviceCompatibility;
    assert!(p.validate().is_err());
    let mut p = plan();
    p.data_stage.as_mut().unwrap().length = 65537;
    p.profile = ControlProfile::DeviceCompatibility;
    assert!(p.validate().is_err());
}
#[test]
fn registry_separates_library_limits_from_hardware_and_physical_evidence() {
    let rows = registry();
    let line = rows
        .iter()
        .find(|row| row.operation == "direct_dpdm")
        .unwrap();
    assert_eq!(line.availability, Availability::Unsupported);
    assert_eq!(line.boundary, Boundary::XhciInterface);
    assert!(
        rows.iter()
            .all(|row| !row.evidence.contains(&"physical_tested"))
    );
}
