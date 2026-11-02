use fah::domain::models::descriptors::*;
fn configuration(value: u8) -> Vec<u8> {
    vec![
        9, 2, 25, 0, 1, value, 0, 128, 50, 9, 4, 0, 0, 1, 3, 0, 0, 0, 7, 5, 129, 3, 8, 0, 10,
    ]
}
fn request(length: u16) -> DescriptorRequest {
    DescriptorRequest {
        kind: 2,
        index: 0,
        language: 0,
        length,
    }
}
#[test]
fn headers_full_bodies_and_explicit_configuration_choices() {
    for value in [1, 2, 3] {
        let raw = configuration(value);
        let header = DescriptorRead::from_bytes(request(9), raw[..9].to_vec());
        assert!(header.parse_error.is_none());
        assert_eq!(header.complete_configuration_value(), None);
        let complete = DescriptorRead::from_bytes(request(25), raw);
        assert_eq!(complete.complete_configuration_value(), Some(value));
    }
}
#[test]
fn malformed_response_keeps_exact_raw_evidence() {
    let raw = configuration(2)[..18].to_vec();
    let result = DescriptorRead::from_bytes(request(25), raw.clone());
    assert_eq!(result.bytes, raw);
    assert_eq!(result.actual_length, 18);
    assert!(result.parse_error.is_some());
    let mut raw = configuration(2);
    raw[18] = 0;
    assert!(
        DescriptorRead::from_bytes(request(25), raw)
            .parse_error
            .is_some()
    );
}
#[test]
fn endpoint_and_interface_counts_are_bounded() {
    let mut raw = configuration(1);
    raw[13] = 2;
    assert!(validate_configuration(&raw).is_err());
    let mut raw = configuration(1);
    raw[20] = 0;
    assert!(validate_configuration(&raw).is_err());
    let mut raw = configuration(1);
    raw[22] = 0;
    assert!(validate_configuration(&raw).is_err());
    let mut raw = configuration(1);
    raw[4] = 2;
    assert!(validate_configuration(&raw).is_err());
}
#[test]
fn device_prefix_is_distinguished_from_truncated_full_response() {
    let bytes = vec![18, 1, 0, 2, 0, 0, 0, 8];
    let prefix = DescriptorRead::from_bytes(
        DescriptorRequest {
            kind: 1,
            index: 0,
            language: 0,
            length: 8,
        },
        bytes.clone(),
    );
    assert!(matches!(
        prefix.parsed,
        Some(DescriptorSummary::Prefix { .. })
    ));
    let truncated = DescriptorRead::from_bytes(
        DescriptorRequest {
            kind: 1,
            index: 0,
            language: 0,
            length: 18,
        },
        bytes,
    );
    assert!(truncated.parse_error.is_some());
}
