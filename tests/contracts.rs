use fah::domain::models::*;

#[test]
fn cancel_and_timeout_do_not_return_dma_ownership() {
    let mut ledger = RequestLedger::new(7);
    let id = ledger.submit().unwrap();
    ledger.logical_cancel(id).unwrap();
    ledger.timeout(id).unwrap();
    assert!(ledger.state(id).unwrap().device_may_access());
    assert!(ledger.reclaim(id).is_err());
    assert!(ledger.advance_generation().is_err());
    ledger.complete(id, 3, 1).unwrap();
    assert!(matches!(
        ledger.reclaim(id).unwrap(),
        RequestState::Completed {
            actual_length: 3,
            logical_cancel: true,
            ..
        }
    ));
    assert!(ledger.reclaim(id).is_err());
}

#[test]
fn requests_and_generations_are_independent() {
    let mut ledger = RequestLedger::new(8);
    let first = ledger.submit().unwrap();
    let second = ledger.submit().unwrap();
    ledger.confirm_abort(second).unwrap();
    assert!(ledger.state(first).unwrap().device_may_access());
    assert!(!ledger.state(second).unwrap().device_may_access());
    ledger.complete(first, 0, 1).unwrap();
    ledger.advance_generation().unwrap();
    let next = ledger.submit().unwrap();
    assert_ne!(first.generation, next.generation);
    assert!(ledger.complete(first, 0, 1).is_err());
    assert!(
        ledger
            .complete(RequestId { session: 9, ..next }, 0, 1)
            .is_err()
    );
}

#[test]
fn machine_output_distinguishes_pending_and_failure() {
    let pending = OperationResult::<()>::new(1, ControllerState::AcquiredUnknown, Outcome::Pending);
    let value = serde_json::to_value(pending).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["session_state"], "acquired_unknown");
    assert_eq!(value["outcome"]["status"], "pending");
    let failed = OperationResult::<()>::new(
        2,
        ControllerState::RecoveryRequired,
        Outcome::Failed(OperationError::validation("invalid request")),
    );
    let encoded = serde_json::to_string(&failed).unwrap();
    let decoded: OperationResult<()> = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded.outcome, Outcome::Failed(_)));
}

#[test]
fn controller_transitions_require_confirmed_results() {
    use ControllerEvent as E;
    use ControllerState as S;
    assert!(S::Closed.transition(E::InitializationConfirmed).is_err());
    let acquired = S::Closed.transition(E::Acquired).unwrap();
    assert_eq!(acquired, S::AcquiredUnknown);
    assert!(acquired.transition(E::QuiescenceConfirmed).is_err());
    let running = acquired.transition(E::InitializationConfirmed).unwrap();
    let closing = running.transition(E::CloseRequested).unwrap();
    assert_eq!(
        closing.transition(E::QuiescenceConfirmed).unwrap(),
        S::Closed
    );
    assert_eq!(
        closing.transition(E::RecoveryNeeded).unwrap(),
        S::RecoveryRequired
    );
}
