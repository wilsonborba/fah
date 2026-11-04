#![cfg(feature = "hosted-direct")]
use crab_usb::{DmaDirection, DmaOp};
use fah::dal::local::dma::{DmaAdapter, MappingProvider};
use std::{
    alloc::Layout,
    collections::BTreeMap,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Default)]
struct Fake {
    maps: Arc<Mutex<BTreeMap<u64, (usize, usize)>>>,
    fail_map: Arc<AtomicBool>,
    fail_unmap: Arc<AtomicBool>,
}
impl MappingProvider for Fake {
    unsafe fn map(&self, iova: u64, size: usize, cpu: *mut u8) -> Result<(), String> {
        if self.fail_map.load(Ordering::SeqCst) {
            return Err("map failed".into());
        }
        assert_eq!(cpu as usize % 4096, 0);
        assert_eq!(size % 4096, 0);
        assert!(
            self.maps
                .lock()
                .unwrap()
                .insert(iova, (cpu as usize, size))
                .is_none()
        );
        Ok(())
    }
    fn unmap(&self, iova: u64, size: usize) -> Result<(), String> {
        if self.fail_unmap.load(Ordering::SeqCst) {
            return Err("unmap failed".into());
        }
        let (_, mapped) = self
            .maps
            .lock()
            .unwrap()
            .remove(&iova)
            .ok_or("unknown mapping")?;
        assert_eq!(mapped, size);
        Ok(())
    }
}
fn adapter(f: Fake) -> DmaAdapter<Fake> {
    DmaAdapter::new(f, &[(4096, 0xffffff)], u32::MAX as u64, 4096).unwrap()
}
#[test]
fn raw_dma_above_64k_is_bounded_and_retained_until_proven_release() {
    let f = Fake::default();
    let a = adapter(f.clone());
    let lease = a.raw_allocate(131072, 4096).unwrap();
    a.raw_write(lease, 65530, &[1, 2, 3, 4, 5, 6, 7, 8])
        .unwrap();
    assert_eq!(
        a.raw_read(lease, 65530, 8).unwrap(),
        vec![1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert!(a.raw_write(lease, 131072, &[1]).is_err());
    assert!(a.raw_read(lease, usize::MAX, 8).is_err());
    a.raw_retire(lease).unwrap();
    assert_eq!(a.retained_allocations(), 1);
    // This fake has never published any controller references.
    unsafe { a.reclaim_lease_after_quiescence(lease) }.unwrap();
    assert!(a.raw_read(lease, 0, 1).is_err());
    assert!(f.maps.lock().unwrap().is_empty());
}
#[test]
fn retired_storage_stays_mapped_on_unmap_failure() {
    let f = Fake::default();
    let a = adapter(f.clone());
    let h = unsafe { a.alloc_coherent(u32::MAX as u64, Layout::from_size_align(65, 64).unwrap()) }
        .unwrap();
    assert_eq!(h.as_ptr().as_ptr() as usize % 4096, 0);
    unsafe { a.dealloc_coherent(h) };
    assert_eq!(a.retained_allocations(), 1);
    f.fail_unmap.store(true, Ordering::SeqCst);
    // No fake device can access the allocation in these synchronous tests.
    assert!(unsafe { a.reclaim_retired_after_quiescence() }.is_err());
    assert_eq!(a.retained_allocations(), 1);
    f.fail_unmap.store(false, Ordering::SeqCst);
    unsafe { a.reclaim_retired_after_quiescence() }.unwrap();
    assert_eq!(a.retained_allocations(), 0);
    assert!(f.maps.lock().unwrap().is_empty());
}
#[test]
fn bounce_maps_only_owned_pages_and_copies_by_direction() {
    let f = Fake::default();
    let a = adapter(f.clone());
    let mut original = [1u8, 2, 3, 4, 5];
    let ptr = NonNull::new(original.as_mut_ptr()).unwrap();
    let h = unsafe {
        a.map_single(
            u32::MAX as u64,
            ptr,
            NonZeroUsize::new(5).unwrap(),
            64,
            DmaDirection::Bidirectional,
        )
    }
    .unwrap();
    let bounce = h.alloc_virt().unwrap().as_ptr();
    assert_ne!(ptr.as_ptr(), bounce);
    a.confirm_write(&h, 0, 5, DmaDirection::ToDevice);
    for (i, expected) in original.iter().enumerate() {
        assert_eq!(unsafe { bounce.add(i).read_volatile() }, *expected);
    }
    unsafe { bounce.add(2).write_volatile(99) };
    a.prepare_read(&h, 0, 5, DmaDirection::FromDevice);
    assert_eq!(original[2], 99);
    unsafe { a.unmap_single(h) };
    assert_eq!(a.retained_allocations(), 1);
    unsafe { a.reclaim_retired_after_quiescence() }.unwrap();
}
#[test]
fn failed_map_rolls_back_and_does_not_retain_storage() {
    let f = Fake::default();
    let a = adapter(f.clone());
    let layout = Layout::from_size_align(4, 4).unwrap();
    f.fail_map.store(true, Ordering::SeqCst);
    assert!(unsafe { a.alloc_coherent(u32::MAX as u64, layout) }.is_none());
    assert_eq!(a.retained_allocations(), 0);
    assert!(a.last_error().is_some());
    f.fail_map.store(false, Ordering::SeqCst);
    let h = unsafe { a.alloc_coherent(u32::MAX as u64, layout) }.unwrap();
    assert_eq!(h.dma_addr().as_u64(), 65536);
    unsafe {
        a.dealloc_coherent(h);
        a.reclaim_retired_after_quiescence().unwrap();
    }
}

#[test]
fn real_coherent_array_sync_does_not_require_bounce_storage() {
    let provider = Fake::default();
    let adapter = Box::leak(Box::new(
        DmaAdapter::new(provider.clone(), &[(0x1000, 0xfffff)], u64::MAX, 4096).unwrap(),
    ));
    let dma = dma_api::DeviceDma::new(u64::MAX, adapter);
    let mut array = dma
        .array_zero_with_align::<u32>(16, 64, DmaDirection::Bidirectional)
        .unwrap();
    array.set(3, 0x12345678);
    assert_eq!(array.read(3), Some(0x12345678));
    drop(array);
    assert_eq!(provider.maps.lock().unwrap().len(), 1);
    unsafe {
        adapter.reclaim_retired_after_quiescence().unwrap();
    }
    assert!(provider.maps.lock().unwrap().is_empty());
}

#[test]
fn allocations_respect_segment_boundaries_and_reclamation_tokens() {
    let provider = Fake::default();
    let adapter = adapter(provider.clone());
    for size in [1, 4096, 65535, 65536] {
        let handle = unsafe {
            adapter.alloc_coherent(u32::MAX as u64, Layout::from_size_align(size, 64).unwrap())
        }
        .unwrap();
        let iova = handle.dma_addr().as_u64();
        assert_eq!(iova % 65536, 0);
        assert!((iova & 65535) + size as u64 <= 65536);
        let lease = adapter.mapping_lease(iova).unwrap();
        assert!(unsafe { adapter.reclaim_lease_after_quiescence(lease) }.is_err());
        unsafe {
            adapter.dealloc_coherent(handle);
            adapter.reclaim_lease_after_quiescence(lease).unwrap();
        }
        assert!(unsafe { adapter.reclaim_lease_after_quiescence(lease) }.is_err());
    }
    assert!(
        unsafe {
            adapter.alloc_coherent(u32::MAX as u64, Layout::from_size_align(65537, 64).unwrap())
        }
        .is_none()
    );
    assert!(provider.maps.lock().unwrap().is_empty());
}
#[test]
fn streaming_epoch_does_not_reclaim_older_uncertain_or_coherent_storage() {
    let provider = Fake::default();
    let adapter = adapter(provider.clone());
    let mut data = [0u8; 8];
    let pointer = NonNull::new(data.as_mut_ptr()).unwrap();
    let map = || {
        unsafe {
            adapter.map_single(
                u32::MAX as u64,
                pointer,
                NonZeroUsize::new(8).unwrap(),
                64,
                DmaDirection::FromDevice,
            )
        }
        .unwrap()
    };
    let old = map();
    let old_lease = adapter.mapping_lease(old.dma_addr().as_u64()).unwrap();
    unsafe {
        adapter.unmap_single(old);
    }
    let epoch = adapter.allocation_epoch();
    let coherent =
        unsafe { adapter.alloc_coherent(u32::MAX as u64, Layout::from_size_align(8, 64).unwrap()) }
            .unwrap();
    let current = map();
    let current_lease = adapter.mapping_lease(current.dma_addr().as_u64()).unwrap();
    unsafe {
        adapter.dealloc_coherent(coherent);
        adapter.unmap_single(current);
    }
    assert_eq!(adapter.retired_streaming_since(epoch), vec![current_lease]);
    unsafe {
        adapter
            .reclaim_lease_after_quiescence(current_lease)
            .unwrap();
    }
    assert_eq!(adapter.mapping_lease(old_lease.iova), Some(old_lease));
    assert_eq!(adapter.retained_allocations(), 2);
    unsafe {
        adapter.reclaim_retired_after_quiescence().unwrap();
    }
}
