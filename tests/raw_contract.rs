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
