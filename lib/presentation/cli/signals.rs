//! Signal handlers set atomic flags only; all hardware work stays on the actor.
#[derive(Debug, Clone, Copy)]
pub enum Interrupt {
    Int,
    Term,
}
#[cfg(feature = "hosted-direct")]
mod native {
    use super::Interrupt;
    use std::sync::atomic::{AtomicBool, Ordering};
    static INT: AtomicBool = AtomicBool::new(false);
    static TERM: AtomicBool = AtomicBool::new(false);
    extern "C" fn notify(signal: libc::c_int) {
        match signal {
            libc::SIGINT => INT.store(true, Ordering::Relaxed),
            libc::SIGTERM => TERM.store(true, Ordering::Relaxed),
            _ => {}
        }
    }
    pub struct Guard {
        old_int: libc::sigaction,
        old_term: libc::sigaction,
    }
    impl Guard {
        pub fn install() -> std::io::Result<Self> {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = notify as usize;
                libc::sigemptyset(&mut action.sa_mask);
                let mut old_int = std::mem::zeroed();
                let mut old_term = std::mem::zeroed();
                if libc::sigaction(libc::SIGINT, &action, &mut old_int) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::sigaction(libc::SIGTERM, &action, &mut old_term) != 0 {
                    let error = std::io::Error::last_os_error();
                    libc::sigaction(libc::SIGINT, &old_int, std::ptr::null_mut());
                    return Err(error);
                }
                Ok(Self { old_int, old_term })
            }
        }
        pub fn take(&self) -> Option<Interrupt> {
            if TERM.swap(false, Ordering::Relaxed) {
                Some(Interrupt::Term)
            } else if INT.swap(false, Ordering::Relaxed) {
                Some(Interrupt::Int)
            } else {
                None
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                libc::sigaction(libc::SIGINT, &self.old_int, std::ptr::null_mut());
                libc::sigaction(libc::SIGTERM, &self.old_term, std::ptr::null_mut());
            }
        }
    }
}
#[cfg(feature = "hosted-direct")]
pub use native::Guard;
