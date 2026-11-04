use fah::domain::models::raw::*;

#[test]
fn data_boundaries_and_packet_counts_are_independent() {
    let trbs = normal_td(0x1ff00, 512, 512, true).unwrap();
    assert_eq!(trbs.len(), 2);
    assert_eq!((trbs[0][0], trbs[0][2] & 0x1ffff), (0x1ff00, 256));
    assert_eq!((trbs[1][0], trbs[1][2] & 0x1ffff), (0x20000, 256));
    assert_eq!((trbs[0][2] >> 17) & 31, 1);
    assert_eq!((trbs[1][2] >> 17) & 31, 0);
    assert_ne!(trbs[0][3] & 16, 0);
    assert_eq!(trbs[1][3] & 16, 0);
    let trbs = normal_td(0x10000, 131073, 64, false).unwrap();
    assert_eq!(trbs.len(), 3);
    assert_eq!((trbs[0][2] >> 17) & 31, 31);
    assert_eq!(trbs[2][2], 1);
    assert_eq!(normal_td(0, 0, 512, false).unwrap().len(), 1);
    assert!(normal_td(u64::MAX, 2, 512, false).is_err());
}

#[test]
fn protocol_parser_preserves_psi_and_unknown_capabilities() {
    let mut bytes = vec![0; 96];
    let mut put =
        |offset: usize, value: u32| bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    put(16, 8 << 16);
    put(32, 0xff | (4 << 8));
    put(48, 2 | (0x300 << 16));
    put(52, u32::from_le_bytes(*b"USB "));
    put(56, 2 | (4 << 8) | (1 << 28));
    put(60, 1);
    put(64, 0x12345678);
    let parsed = protocols(&bytes).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].speeds, vec![0x12345678]);
    assert_eq!(parsed[0].first_port, 2);
    assert!(protocols(&bytes[..63]).is_err());
}

#[test]
fn completion_never_promotes_transfer_only_code_to_command_success() {
    assert_eq!(completion_class(33, 13), CompletionClass::Unknown);
    assert_eq!(completion_class(32, 13), CompletionClass::ShortPacket);
    assert_eq!(completion_class(32, 27), CompletionClass::Stopped);
    assert_eq!(completion_class(33, 255), CompletionClass::Unknown);
}

#[test]
fn isochronous_requests_have_an_owned_buffer_and_explicit_packet_api() {
    let spec:fah::domain::models::requests::RequestSpec=serde_json::from_value(serde_json::json!({
        "device":{"session":1,"generation":1,"slot":1},"endpoint":129,"transfer_type":"isochronous","direction":"in","length":1024,"setup":null
    })).unwrap();
    let request = spec.prepare().unwrap();
    assert_eq!(request.buffer.len(), 1024);
    assert_eq!(
        request.metadata.transfer_type,
        fah::domain::models::requests::TransferType::Isochronous
    );
    let command: fah::presentation::cli::commands::ShellCommand=serde_json::from_str(r#"{"op":"raw","request":{"action":"iso_schedule","slot":1,"endpoint":129,"asap":false,"frame_id":2047}}"#).unwrap();
    assert!(matches!(
        command,
        fah::presentation::cli::commands::ShellCommand::Raw { .. }
    ));
}

#[test]
fn stream_capabilities_and_device_limits_are_not_invented() {
    assert!(validate_streams(0, 4, 4).is_err());
    assert!(validate_streams(4, 0, 4).is_err());
    assert!(validate_streams(1, 4, 8).is_err());
    assert!(validate_streams(4, 1, 8).is_err());
    assert!(validate_streams(4, 4, 7).is_err());
    assert!(validate_streams(4, 4, 16).is_ok());
    let ring: Ring =
        serde_json::from_str(r#"{"kind":"stream","slot":2,"endpoint":129,"stream":3}"#).unwrap();
    assert!(matches!(ring, Ring::Stream { stream: 3, .. }));
}

#[test]
fn scatter_td_counts_packets_across_noncontiguous_segments() {
    let trbs = scatter_td(&[(0x1ff00, 300), (0x50000, 800), (0x90000, 436)], 512, true).unwrap();
    assert_eq!(trbs.len(), 4); // first buffer splits at64KiB.
    assert_eq!(
        trbs.iter()
            .map(|t| (t[2] & 0x1ffff) as usize)
            .sum::<usize>(),
        1536
    );
    assert_eq!((trbs[0][2] >> 17) & 31, 3);
    assert_eq!((trbs[1][2] >> 17) & 31, 3);
    assert_eq!((trbs[2][2] >> 17) & 31, 1);
    assert_eq!((trbs[3][2] >> 17) & 31, 0);
    assert!(trbs[..3].iter().all(|t| t[3] & 16 != 0 && t[3] & 32 == 0));
    assert_ne!(trbs[3][3] & 32, 0);
}
