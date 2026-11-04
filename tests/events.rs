#![cfg(feature = "hosted-direct")]
use fah::dal::local::events::{EventDrain, EventPump, PumpNotice};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use vmm_sys_util::eventfd::{EFD_NONBLOCK, EventFd};
struct Drain {
    pending: Arc<AtomicUsize>,
    seen: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    rearmed: Arc<AtomicUsize>,
}
impl EventDrain for Drain {
    fn drain(&mut self) -> io::Result<bool> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(io::Error::other("disconnect"));
        }
        let found = self
            .pending
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if found {
            self.seen.fetch_add(1, Ordering::SeqCst);
        }
        Ok(found)
    }
    fn rearm(&mut self) -> io::Result<()> {
        self.rearmed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn fixture() -> (Drain, Arc<EventFd>) {
    (
        Drain {
            pending: Arc::new(AtomicUsize::new(3)),
            seen: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
            rearmed: Arc::new(AtomicUsize::new(0)),
        },
        Arc::new(EventFd::new(EFD_NONBLOCK).unwrap()),
    )
}
#[test]
fn registration_bursts_coalescing_and_spurious_wakeups() {
    let (drain, event) = fixture();
    let pending = drain.pending.clone();
    let seen = drain.seen.clone();
    // Pre-registration events, then multiple writes coalesced before start.
    event.write(4).unwrap();
    event.write(7).unwrap();
    let pump = EventPump::start(drain, event.clone()).unwrap();
    assert_eq!(
        pump.notices.recv_timeout(Duration::from_secs(2)).unwrap(),
        PumpNotice::Drained
    );
    assert_eq!(seen.load(Ordering::SeqCst), 3);
    for _ in 0..20 {
        pending.fetch_add(50, Ordering::SeqCst);
        event.write(1).unwrap();
        assert_eq!(
            pump.notices.recv_timeout(Duration::from_secs(2)).unwrap(),
            PumpNotice::Drained
        );
    }
    event.write(1).unwrap();
    assert_eq!(
        pump.notices.recv_timeout(Duration::from_secs(2)).unwrap(),
        PumpNotice::Drained
    );
    let drain = pump.stop().unwrap();
    assert_eq!(drain.seen.load(Ordering::SeqCst), 1003);
    assert!(drain.rearmed.load(Ordering::SeqCst) >= 21);
}
#[test]
fn source_error_notifies_owner_and_shutdown_returns_consumer() {
    let (drain, event) = fixture();
    drain.fail.store(true, Ordering::SeqCst);
    let pump = EventPump::start(drain, event).unwrap();
    assert!(matches!(
        pump.notices.recv_timeout(Duration::from_secs(2)).unwrap(),
        PumpNotice::Failed(_)
    ));
    assert_eq!(
        pump.notices.recv_timeout(Duration::from_secs(2)).unwrap(),
        PumpNotice::Stopped
    );
    assert_eq!(pump.stop().unwrap().pending.load(Ordering::SeqCst), 3);
}
#[test]
fn idle_shutdown_is_woken_without_busy_polling() {
    let (drain, event) = fixture();
    let pump = EventPump::start(drain, event).unwrap();
    assert_eq!(pump.stop().unwrap().seen.load(Ordering::SeqCst), 3);
}

#[test]
fn explicit_drain_observes_bei_without_an_irq_and_acknowledges_empty() {
    let (drain, event) = fixture();
    let seen = drain.seen.clone();
    let pending = drain.pending.clone();
    let pump = EventPump::start(drain, event).unwrap();
    pump.drain_now(Duration::from_secs(2)).unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 3);
    pending.fetch_add(7, Ordering::SeqCst); // No IRQ eventfd write.
    pump.drain_now(Duration::from_secs(2)).unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 10);
    pump.drain_now(Duration::from_secs(2)).unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 10);
    pump.stop().unwrap();
}
