use fah::dal::local::teardown::*;
const STAGES: [CloseStage; 10] = [
    CloseStage::StopSubmissions,
    CloseStage::HaltController,
    CloseStage::DisableBusMaster,
    CloseStage::DisableInterrupts,
    CloseStage::JoinEventPump,
    CloseStage::DropDriver,
    CloseStage::ReclaimDma,
    CloseStage::ReleaseAdapter,
    CloseStage::ReleaseBar,
    CloseStage::ReleaseVfio,
];
struct Resources {
    log: Vec<CloseStage>,
    fail: Option<CloseStage>,
    quiescent: bool,
    master: bool,
    irq: bool,
    worker: bool,
    driver: bool,
    mappings: bool,
    adapter: bool,
    bar: bool,
    vfio: bool,
    buffers: usize,
}
impl Resources {
    fn new(driver: bool, mappings: bool, irq: bool, worker: bool, buffers: usize) -> Self {
        Self {
            log: vec![],
            fail: None,
            quiescent: false,
            master: true,
            irq,
            worker,
            driver,
            mappings,
            adapter: mappings,
            bar: true,
            vfio: true,
            buffers,
        }
    }
}
impl ShutdownBackend for Resources {
    fn close_step(&mut self, stage: CloseStage) -> Result<(), String> {
        self.log.push(stage);
        if self.fail == Some(stage) {
            return Err("injected stage failure".into());
        }
        use CloseStage::*;
        match stage {
            StopSubmissions => {}
            HaltController => {
                self.quiescent = true;
            }
            DisableBusMaster => {
                assert!(self.quiescent);
                self.master = false;
            }
            DisableInterrupts => {
                assert!(self.quiescent && !self.master);
                self.irq = false;
            }
            JoinEventPump => {
                assert!(!self.irq && self.quiescent);
                self.worker = false;
            }
            DropDriver => {
                assert!(self.quiescent && !self.worker);
                self.driver = false;
            }
            ReclaimDma => {
                assert!(self.quiescent && !self.master && !self.worker && !self.driver);
                self.mappings = false;
                self.buffers = 0;
            }
            ReleaseAdapter => {
                assert!(!self.mappings && !self.driver && !self.worker);
                self.adapter = false;
            }
            ReleaseBar => {
                assert!(!self.mappings && !self.adapter && !self.driver && !self.worker);
                self.bar = false;
            }
            ReleaseVfio => {
                assert!(!self.bar && !self.mappings && !self.adapter);
                self.vfio = false;
            }
            Done => unreachable!(),
        }
        Ok(())
    }
}
#[test]
fn every_failure_retains_required_resources_and_retry_resumes_exact_stage() {
    for stage in STAGES {
        for buffers in [0, 3] {
            let mut resources = Resources::new(true, true, true, true, buffers);
            resources.fail = Some(stage);
            let mut progress = CloseProgress::default();
            assert!(progress.drive(&mut resources).is_err());
            assert_eq!(progress.stage(), stage);
            assert!(resources.vfio);
            if resources.log.iter().all(|s| *s != CloseStage::ReclaimDma) {
                assert!(resources.mappings);
                assert_eq!(resources.buffers, buffers);
            }
            let count = resources.log.len();
            resources.fail = None;
            progress.drive(&mut resources).unwrap();
            assert_eq!(resources.log[count], stage);
            assert_eq!(progress.stage(), CloseStage::Done);
            assert!(!resources.vfio && !resources.bar && !resources.mappings);
            let count = resources.log.len();
            progress.drive(&mut resources).unwrap();
            assert_eq!(resources.log.len(), count);
        }
    }
}
#[test]
fn partial_initialization_and_missing_interrupts_still_close_in_order() {
    for (driver, mappings, irq, worker) in [
        (false, false, false, false),
        (false, true, false, false),
        (true, true, false, false),
        (true, true, true, false),
        (true, true, true, true),
    ] {
        let mut resources =
            Resources::new(driver, mappings, irq, worker, if driver { 2 } else { 0 });
        CloseProgress::default().drive(&mut resources).unwrap();
        assert_eq!(resources.log, STAGES);
        assert!(!resources.vfio);
    }
}
#[test]
fn no_interrupt_or_failed_halt_never_releases_active_dma() {
    let mut resources = Resources::new(true, true, false, false, 2);
    resources.fail = Some(CloseStage::HaltController);
    let mut progress = CloseProgress::default();
    assert!(progress.drive(&mut resources).is_err());
    assert_eq!(
        resources.log,
        vec![CloseStage::StopSubmissions, CloseStage::HaltController]
    );
    assert!(resources.mappings && resources.driver && resources.vfio);
    assert_eq!(resources.buffers, 2);
}
