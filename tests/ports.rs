use fah::domain::models::ports::Ports;
#[test]
fn inspect_trace_contains_only_reads_and_does_not_clear_change_bits() {
    let mut ports = Ports::new(77, 3);
    let mut trace = Vec::new();
    let snapshots = ports
        .inspect(|number| {
            trace.push(format!("read:{number}"));
            Ok(match number {
                1 => 0,
                2 => 1 | (3 << 10) | (1 << 17),
                _ => 3 | (1 << 4) | (1 << 21),
            })
        })
        .unwrap();
    assert_eq!(trace, vec!["read:1", "read:2", "read:3"]);
    assert!(!snapshots[0].connected);
    assert!(snapshots[1].connected);
    assert!(!snapshots[1].enabled);
    assert_eq!(snapshots[1].speed_code, 3);
    assert_eq!(snapshots[1].change_bits, 1 << 17);
    assert!(snapshots[2].resetting);
    let id = snapshots[1].id;
    assert_eq!(ports.observe(2, snapshots[1].raw).unwrap().id, id);
    assert!(ports.validate(id).is_ok());
    ports.observe(2, 0).unwrap();
    assert!(ports.validate(id).is_err());
    let next = ports.observe(2, 1).unwrap().id;
    assert_ne!(id.generation, next.generation);
    assert!(ports.observe(0, 0).is_err());
    assert!(ports.observe(4, 0).is_err());
}
#[test]
fn explicit_epoch_invalidation_rejects_stale_ids() {
    let mut ports = Ports::new(1, 1);
    let id = ports.observe(1, 1).unwrap().id;
    ports.invalidate(1).unwrap();
    assert!(ports.validate(id).is_err());
    let id = ports.observe(1, 1).unwrap().id;
    let mut foreign = id;
    foreign.session = 2;
    assert!(ports.validate(foreign).is_err());
}

#[cfg(feature = "hosted-direct")]
#[test]
fn actual_backend_port_inspection_leaves_fake_mmio_unchanged() {
    use fah::dal::local::dma::{DmaAdapter, MappingProvider};
    use std::{
        alloc::{Layout, alloc_zeroed, dealloc},
        ptr::NonNull,
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
    let layout = Layout::from_size_align(0x4000, 4096).unwrap();
    let base = NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap();
    unsafe {
        base.as_ptr().write(0x40);
        base.as_ptr().add(4).cast::<u32>().write((2 << 24) | 8);
        base.as_ptr().add(16).cast::<u32>().write(1);
        base.as_ptr().add(20).cast::<u32>().write(0x2000);
        base.as_ptr().add(24).cast::<u32>().write(0x1000);
        base.as_ptr()
            .add(0x40 + 0x400 + 16)
            .cast::<u32>()
            .write(1 | (3 << 10) | (1 << 17));
    }
    let before = unsafe { std::slice::from_raw_parts(base.as_ptr(), layout.size()) }.to_vec();
    let adapter = Box::leak(Box::new(
        DmaAdapter::new(Provider, &[(0x1000, 0xffffff)], u64::MAX, 4096).unwrap(),
    ));
    let mut host = unsafe { crab_usb::ExplicitHost::new(base, adapter) }.unwrap();
    assert_eq!(host.port_count(), 2);
    assert_eq!(host.port_status(2).unwrap(), 1 | (3 << 10) | (1 << 17));
    assert!(host.port_status(0).is_err());
    assert!(host.port_status(3).is_err());
    assert_eq!(before, unsafe {
        std::slice::from_raw_parts(base.as_ptr(), layout.size())
    });
    // Explicit mutation is tested with a cooperatively simulated controller.
    // CPU writes happen strictly between future polls, never on another thread.
    let port = unsafe { base.as_ptr().add(0x440).cast::<u32>() };
    unsafe {
        port.write_volatile(3 | (1 << 9) | (1 << 10) | (1 << 17));
    }
    {
        use std::{
            future::Future,
            task::{Context, Poll},
        };
        let future = host.reset_port(1, false);
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(unsafe { port.read_volatile() }, (1 << 9) | (1 << 4));
        // Simulate completed reset, preserving its hardware change indicator.
        unsafe {
            port.write_volatile(3 | (1 << 9) | (1 << 10) | (1 << 21));
        }
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
    }
    host.acknowledge_port(1, 1 << 21).unwrap();
    assert_eq!(unsafe { port.read_volatile() }, (1 << 9) | (1 << 21));
    assert!(host.acknowledge_port(1, 1 << 4).is_err());
    drop(host);
    unsafe {
        adapter.reclaim_retired_after_quiescence().unwrap();
        dealloc(base.as_ptr(), layout);
    }
}
