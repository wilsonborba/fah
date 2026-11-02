use fah::dal::local::iova::*;

#[test]
fn reuse_requires_quiescence_and_unmap() {
    let mut a = IovaAllocator::new(&[(4096, 16383)], u32::MAX as u64, 4096).unwrap();
    let x = a.reserve(1, 4096).unwrap();
    a.mapped(x).unwrap();
    assert_eq!(a.release(x, true), Err(AllocationError::DeviceOwned));
    a.quiesced(x).unwrap();
    assert_eq!(a.release(x, false), Err(AllocationError::DeviceOwned));
    a.release(x, true).unwrap();
    let y = a.reserve(4096, 4096).unwrap();
    assert_eq!(x.iova, y.iova);
    assert_ne!(x.id, y.id);
    assert_eq!(a.release(x, true), Err(AllocationError::UnknownLease));
}

#[test]
fn bounds_alignment_exhaustion_and_mask() {
    let mut a = IovaAllocator::new(&[(0, u64::MAX)], 0xffff, 4096).unwrap();
    assert_eq!(a.reserve(0, 4096), Err(AllocationError::InvalidRange));
    assert_eq!(a.reserve(1, 3), Err(AllocationError::InvalidAlignment));
    let x = a.reserve(1, 16384).unwrap();
    assert_eq!(x.iova % 16384, 0);
    let y = a.reserve(60000, 4096).unwrap();
    assert!(y.iova + y.length - 1 <= 0xffff);
    assert_eq!(a.reserve(4096, 4096), Err(AllocationError::Exhausted));
    let mut high = IovaAllocator::new(&[(u64::MAX - 8191, u64::MAX)], u64::MAX, 4096).unwrap();
    let x = high.reserve(8192, 4096).unwrap();
    assert_eq!(x.iova, u64::MAX - 8191);
}

#[test]
fn randomized_reservations_never_overlap() {
    let mut a = IovaAllocator::new(&[(4096, 1048575)], u32::MAX as u64, 4096).unwrap();
    let mut live = Vec::<Lease>::new();
    let mut seed = 17u64;
    for _ in 0..5000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        if seed & 1 == 0 && !live.is_empty() {
            let x = live.swap_remove((seed as usize >> 8) % live.len());
            a.release(x, false).unwrap();
        } else if let Ok(x) = a.reserve((seed % 8 + 1) * 4096, 4096) {
            for y in &live {
                assert!(x.iova + x.length <= y.iova || y.iova + y.length <= x.iova);
            }
            live.push(x);
        }
    }
    assert_eq!(a.live_count(), live.len());
}
