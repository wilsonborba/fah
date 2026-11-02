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
    assert_eq!(h.dma_addr().as_u64(), 4096);
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
