//! One event-ring consumer. Eventfd counters are wakeups, never completion counts.
use std::{
    io,
    os::fd::AsRawFd,
    sync::{Arc, mpsc},
    thread,
};
use vfio_ioctls::VfioDevice;
use vmm_sys_util::eventfd::{EFD_CLOEXEC, EFD_NONBLOCK, EventFd};

/// A drain owns the sole event-ring consumer. Return true if progress was made.
/// Implementations must acknowledge the controller interrupt before INTx rearm.
pub trait EventDrain: Send + 'static {
    fn drain(&mut self) -> io::Result<bool>;
    fn rearm(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl EventDrain for crab_usb::EventHandler {
    fn drain(&mut self) -> io::Result<bool> {
        match self.handle_event() {
            crab_usb::Event::Nothing => Ok(false),
            crab_usb::Event::Stopped => Err(io::Error::other("controller stopped")),
            _ => Ok(true),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptMode {
    Msix,
    Msi,
    Intx,
}
impl InterruptMode {
    fn index(self) -> u32 {
        match self {
            Self::Msix => 2,
            Self::Msi => 1,
            Self::Intx => 0,
        }
    }
}
/// The caller must configure xHCI interrupter zero for this selected mode.
pub fn select_mode(device: &VfioDevice) -> io::Result<InterruptMode> {
    [InterruptMode::Msix, InterruptMode::Msi, InterruptMode::Intx]
        .into_iter()
        .find(|mode| {
            device
                .get_irq_info(mode.index())
                .is_some_and(|irq| irq.count > 0 && irq.flags & 1 != 0)
        })
        .ok_or_else(|| io::Error::other("no eventfd-capable interrupt mode"))
}

/// Retains device and eventfd. No automatic disable: a session must first quiesce.
pub struct InterruptBinding {
    device: Arc<VfioDevice>,
    pub mode: InterruptMode,
    event: Arc<EventFd>,
}
impl InterruptBinding {
    pub fn register(device: Arc<VfioDevice>, mode: InterruptMode) -> io::Result<Self> {
        let event = Arc::new(EventFd::new(EFD_NONBLOCK | EFD_CLOEXEC)?);
        device
            .enable_irq(mode.index(), vec![&event])
            .map_err(io::Error::other)?;
        Ok(Self {
            device,
            mode,
            event,
        })
    }
    pub fn event(&self) -> Arc<EventFd> {
        self.event.clone()
    }
    pub fn rearm(&self) -> io::Result<()> {
        if self.mode == InterruptMode::Intx {
            self.device.unmask_irq(0).map_err(io::Error::other)?;
        }
        Ok(())
    }
    /// # Safety
    /// The controller must be quiescent and no pending work may need an interrupt.
    pub unsafe fn disable_after_quiescence(&self) -> io::Result<()> {
        self.device
            .disable_irq(self.mode.index())
            .map_err(io::Error::other)
    }
}
pub struct BoundDrain<D> {
    pub drain: D,
    pub binding: Arc<InterruptBinding>,
}
impl<D: EventDrain> EventDrain for BoundDrain<D> {
    fn drain(&mut self) -> io::Result<bool> {
        self.drain.drain()
    }
    fn rearm(&mut self) -> io::Result<()> {
        self.binding.rearm()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PumpNotice {
    Drained,
    Failed(String),
    Stopped,
}
/// Worker never owns the session's controller or DMA allocator. Stopping this
/// worker is not proof of hardware quiescence. Errors require session recovery.
pub struct EventPump<D> {
    stop: Arc<EventFd>,
    control: Arc<EventFd>,
    drains: mpsc::SyncSender<mpsc::SyncSender<io::Result<()>>>,
    worker: Option<thread::JoinHandle<D>>,
    pub notices: mpsc::Receiver<PumpNotice>,
}
impl<D: EventDrain> EventPump<D> {
    pub fn start(mut drain: D, event: Arc<EventFd>) -> io::Result<Self> {
        let stop = Arc::new(EventFd::new(EFD_NONBLOCK | EFD_CLOEXEC)?);
        let thread_stop = stop.clone();
        let control = Arc::new(EventFd::new(EFD_NONBLOCK | EFD_CLOEXEC)?);
        let thread_control = control.clone();
        let (drains, requests) = mpsc::sync_channel::<mpsc::SyncSender<io::Result<()>>>(128);
        let (tx, notices) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("fah-events".into())
            .spawn(move || {
                let mut run = || -> io::Result<()> {
                    // Drain once after registration: a completion may predate poll.
                    drain_available(&mut drain)?;
                    loop {
                        let mut fds = [
                            libc::pollfd {
                                fd: event.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                            libc::pollfd {
                                fd: thread_stop.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                            libc::pollfd {
                                fd: thread_control.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                        ];
                        // SAFETY: two valid pollfd entries, held fds, blocking wait.
                        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
                        if ready < 0 {
                            let err = io::Error::last_os_error();
                            if err.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            return Err(err);
                        }
                        if fds.iter().any(|fd| {
                            fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
                        }) {
                            return Err(io::Error::other("event source closed or failed"));
                        }
                        if fds[1].revents & libc::POLLIN != 0 {
                            return Ok(());
                        }
                        if fds[2].revents & libc::POLLIN != 0 {
                            let _ = thread_control.read();
                            while let Ok(ack) = requests.try_recv() {
                                match drain_available(&mut drain) {
                                    Ok(()) => {
                                        let _ = ack.send(Ok(()));
                                    }
                                    Err(error) => {
                                        let _ = ack.send(Err(io::Error::new(
                                            error.kind(),
                                            error.to_string(),
                                        )));
                                        return Err(error);
                                    }
                                }
                            }
                        }
                        if fds[0].revents & libc::POLLIN != 0 {
                            match event.read() {
                                Ok(_) => (),
                                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                                Err(e) => return Err(e),
                            }
                            drain_available(&mut drain)?;
                            // Notifications during drain stay readable; no wakeup is lost.
                            let _ = tx.send(PumpNotice::Drained);
                        }
                    }
                };
                if let Err(err) = run() {
                    let _ = tx.send(PumpNotice::Failed(err.to_string()));
                }
                let _ = tx.send(PumpNotice::Stopped);
                drain
            })?;
        Ok(Self {
            stop,
            control,
            drains,
            worker: Some(worker),
            notices,
        })
    }
    /// Wake the sole consumer even when BEI suppressed every hardware interrupt.
    /// An acknowledgement proves a completed drain, not controller quiescence.
    pub fn drain_now(&self, timeout: std::time::Duration) -> io::Result<()> {
        let (ack, response) = mpsc::sync_channel(1);
        self.drains
            .try_send(ack)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.control.write(1)?;
        response
            .recv_timeout(timeout)
            .map_err(|e| io::Error::new(io::ErrorKind::TimedOut, e.to_string()))?
    }
    /// Returns the consumer for recovery/teardown; does not assert DMA is stopped.
    pub fn stop(mut self) -> thread::Result<D> {
        let _ = self.stop.write(1);
        self.worker.take().expect("worker exists").join()
    }
}
impl<D> Drop for EventPump<D> {
    fn drop(&mut self) {
        let _ = self.stop.write(1);
        if let Some(worker) = self.worker.take() {
            // Keep the drain alive if the owner omitted explicit stop: its state
            // may still be needed for recovery. Never free DMA from this Drop.
            if let Ok(drain) = worker.join() {
                std::mem::forget(drain);
            }
        }
    }
}
fn drain_available(drain: &mut impl EventDrain) -> io::Result<()> {
    // A wedged fake/controller must not monopolize shutdown indefinitely.
    for _ in 0..65536 {
        if !drain.drain()? {
            return drain.rearm();
        }
    }
    Err(io::Error::other(
        "event drain failed to reach idle; recovery required",
    ))
}
