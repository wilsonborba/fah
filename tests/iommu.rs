use fah::dal::local::iommu::parse_info;
fn put(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_ne_bytes());
}
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 56];
    put(&mut b, 0, 56);
    put(&mut b, 4, 3);
    b[8..16].copy_from_slice(&4096u64.to_ne_bytes());
    put(&mut b, 16, 24);
    b[24..26].copy_from_slice(&1u16.to_ne_bytes());
    b[26..28].copy_from_slice(&1u16.to_ne_bytes());
    put(&mut b, 32, 1);
    b[40..48].copy_from_slice(&4096u64.to_ne_bytes());
    b[48..56].copy_from_slice(&u64::MAX.to_ne_bytes());
    b
}
#[test]
fn parses_full_width_ranges_and_rejects_bad_metadata() {
    let mut b = fixture();
    assert_eq!(parse_info(&b).unwrap().ranges, vec![(4096, u64::MAX)]);
    put(&mut b, 28, 24);
    assert!(parse_info(&b).is_err());
    let mut b = fixture();
    put(&mut b, 32, u32::MAX);
    assert!(parse_info(&b).is_err());
    let mut b = fixture();
    put(&mut b, 4, 1);
    assert!(parse_info(&b).is_err());
    assert!(parse_info(&fixture()[..48]).is_err());
}
