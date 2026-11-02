use fah::dal::local::resources::*;
fn region() -> RegionMetadata {
    RegionMetadata {
        size: 16384,
        file_offset: 4096,
        readable: true,
        writable: true,
        mappable: true,
        sparse: None,
    }
}
#[test]
fn mapping_rounds_to_pages_and_checks_bounds() {
    let r = region();
    assert_eq!(r.mapping(32..100, 4096), Ok(0..4096));
    assert_eq!(r.mapping(0..20000, 4096), Err(RegionError::InvalidBounds));
    assert_eq!(r.mapping(5..5, 4096), Err(RegionError::InvalidBounds));
    assert_eq!(r.mapping(0..1, 3), Err(RegionError::InvalidAlignment));
}
#[test]
fn sparse_holes_are_not_mapped() {
    let mut r = region();
    r.sparse = Some(vec![0..4096, 8192..16384]);
    assert_eq!(r.mapping(0..100, 4096), Ok(0..4096));
    assert_eq!(r.mapping(0..8192, 4096), Err(RegionError::UnmappedHole));
    assert_eq!(r.mapping(8192..9000, 4096), Ok(8192..12288));
    r.sparse = Some(vec![]);
    assert_eq!(r.mapping(0..1, 4096), Err(RegionError::UnmappedHole));
}
#[test]
fn permissions_and_overflow_fail_closed() {
    let mut r = region();
    r.writable = false;
    assert_eq!(
        r.mapping(0..1, 4096),
        Err(RegionError::UnsupportedPermissions)
    );
    r.writable = true;
    r.mappable = false;
    assert_eq!(r.mapping(0..1, 4096), Err(RegionError::NotMappable));
    r.mappable = true;
    r.file_offset = u64::MAX - 4095;
    assert_eq!(r.mapping(0..4096, 4096), Err(RegionError::InvalidBounds));
}

#[cfg(feature = "hosted-direct")]
#[test]
fn owned_file_mapping_keeps_fd_alive_and_reads_expected_offset() {
    use fah::dal::local::resources::mapping::MappedRegion;
    use std::{
        ffi::CString,
        fs::File,
        os::{fd::FromRawFd, unix::fs::FileExt},
        sync::Arc,
    };
    let name = CString::new("fah-fake-bar").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    let file = Arc::new(unsafe { File::from_raw_fd(fd) });
    file.set_len(8192).unwrap();
    file.write_all_at(&[17, 23], 4096).unwrap();
    let meta = RegionMetadata {
        size: 4096,
        file_offset: 4096,
        readable: true,
        writable: true,
        mappable: true,
        sparse: None,
    };
    let mapped = unsafe { MappedRegion::new(file.clone(), &meta, 0..128, 4096) }.unwrap();
    assert_eq!(Arc::strong_count(&file), 2);
    assert_eq!(mapped.length(), 4096);
    assert_eq!(unsafe { mapped.pointer().as_ptr().read_volatile() }, 17);
    assert_eq!(
        unsafe { mapped.pointer().as_ptr().add(1).read_volatile() },
        23
    );
    drop(mapped);
    assert_eq!(Arc::strong_count(&file), 1);
}

#[test]
fn guest_target_policy_rejects_host_paths_and_unknown_models() {
    assert!(validate_guest_target("run-1", "run-1", 0x1b36, 0x000d, "0000:00:02.0").is_ok());
    assert!(validate_guest_target("run-1", "run-2", 0x1b36, 0x000d, "0000:00:02.0").is_err());
    assert!(validate_guest_target("run-1", "run-1", 0x8086, 0x7a60, "0000:00:14.0").is_err());
    assert!(validate_guest_target("run-1", "run-1", 0x1b36, 0x000d, "../../etc/x").is_err());
}

#[test]
fn xhci_register_blocks_are_checked_before_accessor_construction() {
    let mut header = [0u8; 32];
    header[0] = 0x40;
    header[2..4].copy_from_slice(&0x100u16.to_le_bytes());
    header[4..8].copy_from_slice(&((4u32 << 24) | 64).to_le_bytes());
    header[20..24].copy_from_slice(&0x2000u32.to_le_bytes());
    header[24..28].copy_from_slice(&0x1000u32.to_le_bytes());
    assert!(fah::dal::local::resources::register_layout(&header, 0x3000).is_ok());
    assert!(fah::dal::local::resources::register_layout(&header, 0x2000).is_err());
    header[24..28].copy_from_slice(&0u32.to_le_bytes());
    assert!(fah::dal::local::resources::register_layout(&header, 0x3000).is_err());
}
