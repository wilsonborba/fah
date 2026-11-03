#![cfg(feature = "hosted-direct")]
use fah::dal::local::dma::{DmaAdapter, MappingProvider};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    future::Future,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};
struct Provider;
impl MappingProvider for Provider {
    unsafe fn map(&self, _: u64, _: usize, _: *mut u8) -> Result<(), String> {
        Ok(())
    }
    fn unmap(&self, _: u64, _: usize) -> Result<(), String> {
        Ok(())
    }
}
#[test]
fn dma_permission_hook_runs_after_reset_and_before_run_stop() {
    let layout = Layout::from_size_align(0x4000, 4096).unwrap();
    let base = NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap();
    unsafe {
        base.as_ptr().cast::<u32>().write(0x01000040);
        base.as_ptr()
            .add(4)
            .cast::<u32>()
            .write((2 << 24) | (1 << 8) | 8);
        base.as_ptr().add(16).cast::<u32>().write(1);
        base.as_ptr().add(20).cast::<u32>().write(0x2000);
        base.as_ptr().add(24).cast::<u32>().write(0x1000);
        base.as_ptr().add(0x44).cast::<u32>().write(1);
        base.as_ptr().add(0x48).cast::<u32>().write(1);
    }
    let adapter = Box::leak(Box::new(
        DmaAdapter::new(Provider, &[(0x1000, 0xffffff)], u64::MAX, 4096).unwrap(),
    ));
    let mut host = unsafe { crab_usb::ExplicitHost::new(base, adapter) }.unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    let marker = invoked.clone();
    let address = base.as_ptr() as usize;
    {
        let future = host.initialize_with_dma_hook(move || {
            let command = unsafe { ((address + 0x40) as *const u32).read_volatile() };
            assert_eq!(command & 3, 0);
            marker.store(true, Ordering::SeqCst);
            Ok(())
        });
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert!(!invoked.load(Ordering::SeqCst));
        assert_ne!(
            unsafe { base.as_ptr().add(0x40).cast::<u32>().read_volatile() } & 2,
            0
        );
        // Cooperative device model: register changes occur between future polls.
        unsafe {
            base.as_ptr().add(0x40).cast::<u32>().write_volatile(0);
        }
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert!(invoked.load(Ordering::SeqCst));
        assert_ne!(
            unsafe { base.as_ptr().add(0x40).cast::<u32>().read_volatile() } & 1,
            0
        );
        unsafe {
            base.as_ptr().add(0x44).cast::<u32>().write_volatile(0);
        }
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
    }
    drop(host);
    unsafe {
        adapter.reclaim_retired_after_quiescence().unwrap();
        dealloc(base.as_ptr(), layout);
    }
}
