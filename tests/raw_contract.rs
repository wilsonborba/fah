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
    assert_eq!(parsed[0].ports, vec![2, 3, 4, 5]);
    let speed = ProtocolSpeed::decode((5 << 16) | (1 << 8) | (3 << 4) | 4);
    assert_eq!(speed.bits_per_second, 5_000_000_000);
    assert_eq!(speed.id, 4);
    assert!(speed.full_duplex);
    let reserved = ProtocolSpeed::decode((3 << 14) | (3 << 6));
    assert_eq!(reserved.link_protocol, 3);
    assert_eq!(reserved.kind, 3);
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

#[test]
fn reconcile_requires_scoped_generation_and_independent_proof() {
    let baseline = serde_json::json!({"words":[1,2],"endpoints":[[3,4]]});
    let mut state = Coherence::default();
    state
        .mutate(
            Effect::Slot {
                slot: 1,
                device: false,
            },
            Some(baseline.clone()),
            false,
        )
        .unwrap();
    assert!(!state.slots.contains_key(&2));
    assert!(!state.reconcile(1, &serde_json::json!({"words":[5]}), false));
    assert!(state.reconcile(1, &baseline, false));
    state
        .mutate(
            Effect::Slot {
                slot: 1,
                device: true,
            },
            Some(baseline.clone()),
            false,
        )
        .unwrap();
    assert!(!state.reconcile(1, &baseline, false)); // controller match does not prove device state
    state
        .mutate(
            Effect::Slot {
                slot: 2,
                device: false,
            },
            Some(baseline.clone()),
            true,
        )
        .unwrap();
    assert!(!state.reconcile(2, &baseline, false)); // pending-at-mutation needs terminal proof
    state
        .mutate(
            Effect::Slot {
                slot: 3,
                device: false,
            },
            Some(baseline.clone()),
            false,
        )
        .unwrap();
    state.mutate(Effect::Global, None, false).unwrap();
    assert!(!state.reconcile(3, &baseline, false)); // global uncertainty stays sticky
    assert_eq!(state.generation, 5);
}
#[test]
fn literal_effect_classification_does_not_narrow_unknown_or_vendor_words() {
    let command = |words| RawOperation::Command {
        words,
        timeout_ms: 1,
    };
    assert_eq!(effect(&command([0, 0, 0, 23 << 10])), Effect::Observe);
    assert_eq!(
        effect(&command([0, 0, 0, (13 << 10) | (2 << 24)])),
        Effect::Slot {
            slot: 2,
            device: false
        }
    );
    assert_eq!(
        effect(&command([0, 0, 0, (60 << 10) | (2 << 24)])),
        Effect::Global
    );
    assert_eq!(
        effect(&command([0, 0, 0, (13 << 10) | (2 << 24) | (1 << 8)])),
        Effect::Global
    );
    let mut setup = [9 << 8, 0, 8, (2 << 10) | (1 << 6)];
    let transfer = |t| RawOperation::Transfer {
        slot: 2,
        endpoint: 0,
        trbs: vec![t],
    };
    assert_eq!(
        effect(&transfer(setup)),
        Effect::Slot {
            slot: 2,
            device: true
        }
    );
    setup[0] |= 0x40;
    assert_eq!(effect(&transfer(setup)), Effect::Global);
}
