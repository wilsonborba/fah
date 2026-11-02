use fah::domain::{
    models::{ControllerState, OperationError},
    services::session::*,
};
use std::sync::{Arc, Mutex};
struct Backend {
    log: Arc<Mutex<Vec<&'static str>>>,
    init_fails: bool,
    close_fails: bool,
    retained_acquire: Option<bool>,
}
impl ControllerBackend for Backend {
    fn acquire(&mut self, _: &str) -> Result<(), AcquisitionFailure> {
        self.log.lock().unwrap().push("acquire");
        if let Some(retained) = self.retained_acquire {
            return Err(AcquisitionFailure {
                retained,
                error: OperationError::validation("fake acquisition failure"),
            });
        }
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), OperationError> {
        self.log.lock().unwrap().push("init");
        if self.init_fails {
            Err(OperationError::validation("fake init failure"))
        } else {
            Ok(())
        }
    }
    fn inspect(&self) -> ControllerSnapshot {
        ControllerSnapshot {
            target: None,
            interrupt_mode: None,
            retained_allocations: 0,
        }
    }
    fn close(&mut self) -> Result<(), OperationError> {
        self.log.lock().unwrap().push("close");
        if self.close_fails {
            Err(OperationError::validation("fake stop failure"))
        } else {
            Ok(())
        }
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("drop");
    }
}
fn fixture() -> (Backend, Arc<Mutex<Vec<&'static str>>>) {
    let log = Arc::new(Mutex::new(vec![]));
    (
        Backend {
            log: log.clone(),
            init_fails: false,
            close_fails: false,
            retained_acquire: None,
        },
        log,
    )
}
#[test]
fn one_owner_validates_before_effects_and_inspection_has_no_effects() {
    let (b, log) = fixture();
    let mut session = Session::new(b, 1);
    session.inspect();
    assert!(session.initialize().is_err());
    assert!(log.lock().unwrap().is_empty());
    session.acquire("guest").unwrap();
    assert_eq!(session.state(), ControllerState::AcquiredUnknown);
    assert!(session.acquire("second").is_err());
    session.initialize().unwrap();
    assert_eq!(session.state(), ControllerState::Initialized);
    let old = session.requests.submit().unwrap();
    session.close().unwrap();
    assert!(session.requests.state(old).is_err());
    drop(session);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["acquire", "init", "close", "drop"]
    );
}
#[test]
fn failed_init_or_stop_retains_backend_for_recovery() {
    let (mut b, log) = fixture();
    b.init_fails = true;
    b.close_fails = true;
    let mut session = Session::new(b, 2);
    session.acquire("guest").unwrap();
    assert!(session.initialize().is_err());
    assert_eq!(session.state(), ControllerState::RecoveryRequired);
    assert!(session.close().is_err());
    assert_eq!(session.state(), ControllerState::RecoveryRequired);
    drop(session);
    assert_eq!(*log.lock().unwrap(), vec!["acquire", "init", "close"]);
}
#[test]
fn failed_acquisition_distinguishes_rollback_from_retained_resources() {
    for retained in [true, false] {
        let (mut b, log) = fixture();
        b.retained_acquire = Some(retained);
        let mut session = Session::new(b, 3);
        assert!(session.acquire("guest").is_err());
        assert_eq!(
            session.state(),
            if retained {
                ControllerState::RecoveryRequired
            } else {
                ControllerState::Closed
            }
        );
        drop(session);
        assert_eq!(log.lock().unwrap().contains(&"drop"), !retained);
    }
}

#[cfg(feature = "hosted-direct")]
#[test]
fn backend_future_deadline_returns_recovery_error() {
    use fah::dal::local::controller::block_until;
    use std::time::Duration;
    assert_eq!(
        block_until(async { 42 }, Duration::from_secs(1)).unwrap(),
        42
    );
    assert!(
        block_until(std::future::pending::<()>(), Duration::from_millis(2))
            .unwrap_err()
            .contains("recovery")
    );
}
