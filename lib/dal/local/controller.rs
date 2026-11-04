//! Guest-only local resource owner. No acquisition is performed by construction.
use super::{
    dma::{DmaAdapter, VfioMappingProvider},
    events::{BoundDrain, EventPump, InterruptBinding, select_mode},
    resources::{self, GuestResources, mapping::MappedRegion},
};
use crate::domain::{
    models::{ErrorCategory, OperationError},
    services::session::{AcquisitionFailure, ControllerBackend, ControllerSnapshot},
};
use std::{
    fs::File,
    future::Future,
    os::fd::{AsRawFd, FromRawFd},
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
type Adapter = DmaAdapter<VfioMappingProvider>;
type Pump = EventPump<BoundDrain<crab_usb::EventHandler>>;
#[path = "controller_raw.rs"]
mod raw;
/// Owns the only CrabUSB engine, its consumer, mappings and VFIO lifetime.
pub struct LocalController {
    raw: raw::RawOwner,
    run_id: String,
    target: Option<String>,
    resources: Option<GuestResources>,
    mapping: Option<MappedRegion>,
    layout: Option<resources::RegisterLayout>,
    adapter: Option<&'static Adapter>,
    host: Option<crab_usb::ExplicitHost>,
    binding: Option<Arc<InterruptBinding>>,
    pump: Option<Pump>,
    pending: std::collections::BTreeMap<u64, PendingRequest>,
    next_request: u64,
    ring_rebuilds: u32,
    shutdown: super::teardown::CloseProgress,
    closing: bool,
}
impl LocalController {
    pub fn new(run_id: String) -> Self {
        Self {
            raw: Default::default(),
            run_id,
            target: None,
            resources: None,
            mapping: None,
            layout: None,
            adapter: None,
            host: None,
            binding: None,
            pump: None,
            pending: std::collections::BTreeMap::new(),
            next_request: 1,
            ring_rebuilds: 0,
            shutdown: Default::default(),
            closing: false,
        }
    }
    pub fn port_count(&self) -> Result<u8, OperationError> {
        self.host
            .as_ref()
            .map(|host| host.port_count())
            .ok_or_else(|| OperationError::validation("controller is not initialized"))
    }
    pub fn port_status(&self, number: u8) -> Result<u32, OperationError> {
        self.host
            .as_ref()
            .ok_or_else(|| OperationError::validation("controller is not initialized"))?
            .port_status(number)
            .map_err(usb_error)
    }
    fn initialize_inner(&mut self) -> Result<(), String> {
        if self.host.is_some() || self.adapter.is_some() {
            return Err("initialization already attempted; close/recover first".into());
        }
        let resource = self.resources.as_ref().ok_or("controller not acquired")?;
        let limits = super::iommu::query(&resource.container)?;
        let meta = resources::vfio_region(&resource.device, 0);
        let mut end = meta.size;
        if let Some(areas) = &meta.sparse {
            let mut sorted = areas.clone();
            sorted.sort_by_key(|a| a.start);
            end = 0;
            for area in sorted {
                if area.start > end {
                    break;
                }
                end = end.max(area.end);
            }
        }
        if !(4096..=1024 * 1024).contains(&end) {
            return Err("unsupported contiguous MMIO prefix".into());
        }
        // SAFETY: fd duplication retains this VFIO device; no host physical FD is opened.
        let fd = unsafe { libc::fcntl(resource.device.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let file = Arc::new(unsafe { File::from_raw_fd(fd) });
        // SAFETY: single session owns the VFIO resource; validate all accessor spans next.
        let mapping = unsafe { MappedRegion::new(file, &meta, 0..end, 4096) }?;
        let base = mapping.pointer().as_ptr();
        let length = mapping.length();
        let mut header = [0u8; 32];
        for (index, chunk) in header.chunks_exact_mut(4).enumerate() {
            let value = unsafe { base.cast::<u32>().add(index).read_volatile() };
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        let layout = resources::register_layout(&header, length)?;
        // Validate the full extended capability chain before CrabUSB traverses it.
        let mut offset =
            ((u32::from_le_bytes(header[16..20].try_into().unwrap()) >> 16) * 4) as usize;
        let mut seen = std::collections::BTreeSet::new();
        while offset != 0 {
            if offset.checked_add(16).is_none_or(|end| end > length) || !seen.insert(offset) {
                return Err("extended capability exceeds valid mapping".into());
            }
            let cap = unsafe { base.add(offset).cast::<u32>().read_volatile() };
            let capability_size = match cap & 255 {
                1 => 8, // USB Legacy Support
                2 => {
                    // Supported Protocol, including its PSI entries
                    let protocol = unsafe { base.add(offset + 12).cast::<u32>().read_volatile() };
                    16 + ((protocol >> 28) as usize) * 4
                }
                _ => return Err("unreviewed extended capability type".into()),
            };
            if offset
                .checked_add(capability_size)
                .is_none_or(|end| end > length)
            {
                return Err("extended capability payload exceeds mapping".into());
            }
            let next = ((cap >> 8) & 255) as usize * 4;
            if next == 0 {
                break;
            }
            offset = offset
                .checked_add(next)
                .ok_or("extended capability overflow")?;
        }
        self.layout = Some(layout);
        self.mapping = Some(mapping);
        let mask = if u32::from_le_bytes(header[16..20].try_into().unwrap()) & 1 != 0 {
            u64::MAX
        } else {
            u32::MAX as u64
        };
        let adapter = Box::leak(Box::new(Adapter::new(
            VfioMappingProvider {
                container: resource.container.clone(),
            },
            &limits.ranges,
            mask,
            4096,
        )?));
        self.adapter = Some(adapter);
        // SAFETY: mapping and adapter are retained until backend stop+unmap succeeds.
        self.host = Some(
            unsafe {
                crab_usb::ExplicitHost::new(self.mapping.as_ref().unwrap().pointer(), adapter)
            }
            .map_err(|e| format!("{e:?}"))?,
        );
        let device = resource.device.clone();
        let mode = select_mode(&device).map_err(|e| e.to_string())?;
        let binding_slot = &mut self.binding;
        block_until(
            self.host
                .as_mut()
                .unwrap()
                .initialize_with_dma_hook(move || {
                    let binding = Arc::new(
                        InterruptBinding::register(device.clone(), mode)
                            .map_err(|e| crab_usb::usb_if::err::USBError::from(e.to_string()))?,
                    );
                    *binding_slot = Some(binding);
                    set_pci_dma(&device, true).map_err(crab_usb::usb_if::err::USBError::from)
                }),
            Duration::from_secs(5),
        )?
        .map_err(|e| format!("{e:?}"))?;
        let binding = self.binding.as_ref().unwrap().clone();
        let handler = self.host.as_mut().unwrap().event_handler();
        self.pump = Some(
            EventPump::start(
                BoundDrain {
                    drain: handler,
                    binding: binding.clone(),
                },
                binding.event(),
            )
            .map_err(|e| e.to_string())?,
        );
        Ok(())
    }
}
impl ControllerBackend for LocalController {
    fn acquire(&mut self, target: &str) -> Result<(), AcquisitionFailure> {
        if self.resources.is_some() {
            return Err(AcquisitionFailure {
                retained: true,
                error: OperationError::validation("backend already acquired"),
            });
        }
        let resource =
            GuestResources::open(target, &self.run_id).map_err(|message| AcquisitionFailure {
                retained: false,
                error: os_error(message),
            })?;
        self.shutdown = Default::default();
        self.closing = false;
        self.ring_rebuilds = 0;
        self.resources = Some(resource);
        self.target = Some(target.into());
        // Ownership is explicit but controller state is still unknown. Prevent
        // any old engine from DMA through the new container mappings.
        set_pci_dma(&self.resources.as_ref().unwrap().device, false).map_err(|message| {
            AcquisitionFailure {
                retained: true,
                error: os_error(message),
            }
        })?;
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), OperationError> {
        self.initialize_inner().map_err(os_error)
    }
    fn inspect(&self) -> ControllerSnapshot {
        ControllerSnapshot {
            target: self.target.clone(),
            interrupt_mode: self
                .binding
                .as_ref()
                .map(|b| format!("{:?}", b.mode).to_lowercase()),
            retained_allocations: self.adapter.map_or(0, |a| a.retained_allocations()),
            close_stage: self.closing.then(|| format!("{:?}", self.shutdown.stage())),
        }
    }
    fn close(&mut self) -> Result<(), OperationError> {
        let mut progress = std::mem::take(&mut self.shutdown);
        let result = progress.drive(&mut LocalShutdown(self));
        let stage = progress.stage();
        self.shutdown = progress;
        result.map_err(|message| {
            recovery_error(
                format!("close failed at {stage:?}: {message}; resources retained; retry close"),
                None,
            )
        })
    }
}
fn os_error(message: String) -> OperationError {
    OperationError {
        category: ErrorCategory::OperatingSystem,
        message,
        os_code: None,
        completion_code: None,
    }
}
/// Deadline bounds even upstream futures that self-wake while polling MMIO.
/// This polls a future, not the event ring; completion delivery remains IRQ-driven.
pub fn block_until<F: Future>(future: F, timeout: Duration) -> Result<F::Output, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("deadline overflow")?;
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return Ok(value),
            Poll::Pending => {
                if Instant::now() >= deadline {
                    return Err("backend operation timed out; recovery required".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
impl Drop for LocalController {
    fn drop(&mut self) {
        if self.resources.is_some() {
            std::mem::forget(std::mem::take(&mut self.pending));
            // An owner dropped without confirmed close. Preserve the entire hardware
            // lifetime graph; process exit's kernel cleanup is separate evidence.
            if let Some(value) = self.pump.take() {
                std::mem::forget(value);
            }
            if let Some(value) = self.binding.take() {
                std::mem::forget(value);
            }
            if let Some(value) = self.host.take() {
                std::mem::forget(value);
            }
            if let Some(value) = self.mapping.take() {
                std::mem::forget(value);
            }
            if let Some(value) = self.resources.take() {
                std::mem::forget(value);
            }
        }
    }
}

impl crate::domain::services::usb::DeviceBackend for LocalController {
    fn port_count(&self) -> Result<u8, OperationError> {
        LocalController::port_count(self)
    }
    fn port_status(&self, port: u8) -> Result<u32, OperationError> {
        LocalController::port_status(self, port)
    }
    fn acknowledge_port(&mut self, port: u8, changes: u32) -> Result<(), OperationError> {
        self.host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller not initialized"))?
            .acknowledge_port(port, changes)
            .map_err(usb_error)
    }
    fn reset_port(&mut self, port: u8, warm: bool) -> Result<(), OperationError> {
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller not initialized"))?;
        device_future(host.reset_port(port, warm))
    }
    fn enable_slot(&mut self) -> Result<u8, OperationError> {
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller not initialized"))?;
        device_future(host.enable_slot())
    }
    fn address_device(&mut self, slot: u8, port: u8, bsr: bool) -> Result<(), OperationError> {
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller not initialized"))?;
        device_future(host.address_device(slot, port, bsr))
    }
}
fn device_future<T>(
    future: impl Future<Output = crab_usb::err::Result<T>>,
) -> Result<T, OperationError> {
    block_until(future, Duration::from_secs(5))
        .map_err(|message| OperationError {
            category: ErrorCategory::Timeout,
            message,
            os_code: None,
            completion_code: None,
        })?
        .map_err(usb_error)
}
fn usb_error(error: crab_usb::usb_if::err::USBError) -> OperationError {
    use crab_usb::usb_if::err::USBError;
    let completion_code = match &error {
        USBError::TransferError(crab_usb::usb_if::err::TransferError::Other(detail)) => detail
            .downcast_ref::<crab_usb::CommandFailure>()
            .map(|failure| failure.code),
        _ => None,
    };
    let category = match error {
        USBError::InvalidParameter | USBError::NotInitialized => ErrorCategory::Validation,
        USBError::NotSupported => ErrorCategory::Unsupported,
        USBError::NotFound => ErrorCategory::Disconnect,
        USBError::Timeout => ErrorCategory::Timeout,
        _ => ErrorCategory::Controller,
    };
    OperationError {
        category,
        message: format!("{error:?}"),
        os_code: None,
        completion_code,
    }
}

impl crate::domain::services::usb::ConfigurationBackend for LocalController {
    fn descriptor(
        &mut self,
        slot: u8,
        request: crate::domain::models::descriptors::DescriptorRequest,
    ) -> Result<Vec<u8>, OperationError> {
        let adapter = self
            .adapter
            .ok_or_else(|| OperationError::validation("DMA adapter unavailable"))?;
        let epoch = adapter.allocation_epoch();
        let bytes = device_future(
            self.host
                .as_mut()
                .ok_or_else(|| OperationError::validation("controller not initialized"))?
                .descriptor(
                    slot,
                    request.kind,
                    request.index,
                    request.language,
                    request.length,
                ),
        )?;
        for lease in adapter.retired_streaming_since(epoch) {
            // SAFETY: this synchronous descriptor future completed successfully;
            // its private streaming map was created/dropped during this call.
            // IRQ dispatch allocates/frees no maps and no other command ran.
            unsafe { adapter.reclaim_lease_after_quiescence(lease) }.map_err(|message| {
                OperationError {
                    category: ErrorCategory::RecoveryRequired,
                    message,
                    os_code: None,
                    completion_code: None,
                }
            })?;
        }
        Ok(bytes)
    }
    fn ep0_packet_size(&mut self, slot: u8, size: u16) -> Result<(), OperationError> {
        device_future(
            self.host
                .as_mut()
                .ok_or_else(|| OperationError::validation("controller not initialized"))?
                .ep0_packet_size(slot, size),
        )
    }
    fn set_configuration(&mut self, slot: u8, value: u8) -> Result<(), OperationError> {
        device_future(
            self.host
                .as_mut()
                .ok_or_else(|| OperationError::validation("controller not initialized"))?
                .set_configuration(slot, value),
        )
    }
    fn set_interface(
        &mut self,
        slot: u8,
        interface: u8,
        alternate: u8,
    ) -> Result<(), OperationError> {
        device_future(
            self.host
                .as_mut()
                .ok_or_else(|| OperationError::validation("controller not initialized"))?
                .set_interface(slot, interface, alternate),
        )
    }
    fn setup_endpoints(
        &mut self,
        slot: u8,
        interface: u8,
        alternate: u8,
        bytes: &[u8],
    ) -> Result<Vec<crate::domain::services::usb::EndpointSnapshot>, OperationError> {
        let infos = device_future(
            self.host
                .as_mut()
                .ok_or_else(|| OperationError::validation("controller not initialized"))?
                .setup_endpoints(slot, interface, alternate, bytes),
        )?;
        Ok(infos
            .into_iter()
            .map(|info| crate::domain::services::usb::EndpointSnapshot {
                address: info.address.raw(),
                transfer_type: format!("{:?}", info.transfer_type).to_lowercase(),
                direction: format!("{:?}", info.direction).to_lowercase(),
                max_packet_size: info.max_packet_size,
                interval: info.interval,
            })
            .collect())
    }
}

use crate::domain::models::requests::{
    BackendCompletion, Direction, OwnedRequest, RequestMetadata, TransferType,
};
struct PendingRequest {
    metadata: RequestMetadata,
    buffer: Vec<u8>,
    raw_id: crab_usb::usb_if::endpoint::RequestId,
    lease: Option<super::iova::Lease>,
    terminal_error: Option<OperationError>,
}
impl LocalController {
    fn check_event_pump(&self) -> Result<(), OperationError> {
        if self.closing {
            return Err(OperationError::validation(
                "controller teardown in progress; retry close",
            ));
        }
        let pump = self
            .pump
            .as_ref()
            .ok_or_else(|| OperationError::validation("event pump unavailable"))?;
        for notice in pump.notices.try_iter() {
            match notice {
                super::events::PumpNotice::Drained => {}
                other => {
                    return Err(recovery_error(
                        format!("event source failed: {other:?}"),
                        None,
                    ));
                }
            }
        }
        Ok(())
    }
}
fn recovery_error(message: String, code: Option<u32>) -> OperationError {
    OperationError {
        category: ErrorCategory::RecoveryRequired,
        message,
        os_code: None,
        completion_code: code,
    }
}
impl crate::domain::services::requests::RequestBackend for LocalController {
    fn submit_request(&mut self, request: OwnedRequest) -> Result<u64, OperationError> {
        self.submit_owned(request, None)
    }
    fn poll_request(&mut self, token: u64) -> Result<Option<BackendCompletion>, OperationError> {
        self.check_event_pump()?;
        let pending = self
            .pending
            .get_mut(&token)
            .ok_or_else(|| OperationError::validation("unknown backend request"))?;
        if let Some(error) = pending.terminal_error.clone() {
            return Ok(Some(BackendCompletion {
                actual_length: None,
                completion_code: error.completion_code,
                data: None,
                error: Some(error),
                quiescent: false,
            }));
        }
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller unavailable"))?;
        let Some(observed) = host
            .reclaim_request(
                pending.metadata.device.slot,
                pending.metadata.endpoint,
                pending.raw_id,
            )
            .map_err(usb_error)?
        else {
            return Ok(None);
        };
        let code = observed.completion_code;
        let actual = match observed.result {
            Ok(result)
                if result.actual_length <= pending.buffer.len()
                    && code.is_some_and(|code| matches!(code, 1 | 13)) =>
            {
                result.actual_length
            }
            result => {
                let error = recovery_error(
                    format!("request completion requires recovery: {result:?}"),
                    code,
                );
                pending.terminal_error = Some(error.clone());
                return Ok(Some(BackendCompletion {
                    actual_length: None,
                    completion_code: code,
                    data: None,
                    error: Some(error),
                    quiescent: false,
                }));
            }
        };
        if let Some(lease) = pending.lease {
            // SAFETY: real successful TD completion consumed by the sole backend owner;
            // mapping handle is dropped and no hardware may use this request's buffer.
            if let Err(message) =
                unsafe { self.adapter.unwrap().reclaim_lease_after_quiescence(lease) }
            {
                let error = recovery_error(message, code);
                pending.terminal_error = Some(error.clone());
                return Ok(Some(BackendCompletion {
                    actual_length: Some(actual),
                    completion_code: code,
                    data: None,
                    error: Some(error),
                    quiescent: false,
                }));
            }
        }
        let mut pending = self.pending.remove(&token).unwrap();
        pending.buffer.truncate(actual);
        Ok(Some(BackendCompletion {
            actual_length: Some(actual),
            completion_code: code,
            data: Some(pending.buffer),
            error: None,
            quiescent: true,
        }))
    }
}

impl crate::domain::services::recovery::RecoveryBackend for LocalController {
    fn reset_endpoint_recovery(
        &mut self,
        slot: u8,
        endpoint: u8,
        preserve: bool,
    ) -> Result<crate::domain::models::requests::BackendAbort, OperationError> {
        self.recover_endpoint(slot, endpoint, Some(preserve))
    }
    fn abort_endpoint(
        &mut self,
        slot: u8,
        endpoint: u8,
    ) -> Result<crate::domain::models::requests::BackendAbort, OperationError> {
        self.recover_endpoint(slot, endpoint, None)
    }
}
impl LocalController {
    fn recover_endpoint(
        &mut self,
        slot: u8,
        endpoint: u8,
        reset: Option<bool>,
    ) -> Result<crate::domain::models::requests::BackendAbort, OperationError> {
        use crate::domain::services::requests::RequestBackend;
        self.check_event_pump()?;
        if self.ring_rebuilds >= 64 {
            return Err(OperationError::validation(
                "ring rebuild budget reached; close/reacquire controller",
            ));
        }
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller unavailable"))?;
        // Neither publishing Stop Endpoint nor a software cancellation is proof.
        match reset {
            Some(preserve) => device_future(host.reset_endpoint(slot, endpoint, preserve))?,
            None => device_future(host.stop_endpoint(slot, endpoint))?,
        };
        let tokens: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, request)| {
                request.metadata.device.slot == slot && request.metadata.endpoint == endpoint
            })
            .map(|(token, _)| *token)
            .collect();
        let mut completed = Vec::new();
        let mut aborted = Vec::new();
        let mut error = None;
        for token in &tokens {
            match self.poll_request(*token) {
                Ok(Some(result)) if result.quiescent && result.error.is_none() => {
                    completed.push((*token, result))
                }
                Ok(_) => {}
                Err(failure) => {
                    error = Some(failure);
                }
            }
        }
        // Clear all CPU-side raw transfer handles only after confirmed Stop. An
        // allocation/rebase failure cannot leave handles that might access originals.
        if let Err(failure) = self
            .host
            .as_mut()
            .unwrap()
            .discard_stopped_requests(slot, endpoint)
            .map_err(usb_error)
        {
            return Ok(crate::domain::models::requests::BackendAbort {
                completed,
                aborted,
                restart_ready: false,
                error: Some(failure),
            });
        }
        for token in tokens {
            let Some(request) = self.pending.get(&token) else {
                continue;
            };
            if let Some(lease) = request.lease {
                // SAFETY: Stop command completed, Stopped context checked, raw handles
                // discarded. This proof applies to every buffer on the selected endpoint.
                if let Err(message) =
                    unsafe { self.adapter.unwrap().reclaim_lease_after_quiescence(lease) }
                {
                    error = Some(recovery_error(message, None));
                    continue;
                }
            }
            self.pending.remove(&token);
            aborted.push(token);
        }
        let mut restart_ready = false;
        if error.is_none() {
            match device_future(
                self.host
                    .as_mut()
                    .unwrap()
                    .rebuild_stopped_endpoint(slot, endpoint),
            ) {
                Ok(()) => {
                    self.ring_rebuilds += 1;
                    restart_ready = true;
                }
                Err(failure) => {
                    error = Some(failure);
                }
            }
        }
        Ok(crate::domain::models::requests::BackendAbort {
            completed,
            aborted,
            restart_ready,
            error,
        })
    }
}

/// Only this private facade may run individual shutdown stages on a real owner.
struct LocalShutdown<'a>(&'a mut LocalController);
impl super::teardown::ShutdownBackend for LocalShutdown<'_> {
    fn close_step(&mut self, stage: super::teardown::CloseStage) -> Result<(), String> {
        use super::teardown::CloseStage::*;
        let owner = &mut self.0;
        match stage {
            StopSubmissions => {
                owner.closing = true;
            }
            HaltController => {
                if let (Some(mapping), Some(layout)) = (&owner.mapping, &owner.layout) {
                    halt_mapping(mapping, layout, Duration::from_secs(5))?;
                } else if owner.adapter.is_some()
                    || owner.host.is_some()
                    || !owner.pending.is_empty()
                {
                    return Err(
                        "cannot establish DMA stop without validated register mapping".into(),
                    );
                }
                // No mapping/adapter/host means no FAH DMA was allocated or published.
            }
            DisableBusMaster => {
                if let Some(resource) = &owner.resources {
                    set_pci_dma(&resource.device, false)?;
                }
            }
            DisableInterrupts => {
                if let Some(binding) = &owner.binding {
                    unsafe { binding.disable_after_quiescence() }.map_err(|e| e.to_string())?;
                }
            }
            JoinEventPump => {
                if let Some(pump) = owner.pump.take() {
                    let drain = pump
                        .stop()
                        .map_err(|_| "event worker panicked after controller halt")?;
                    drop(drain);
                }
            }
            DropDriver => {
                if let Some(adapter) = owner.adapter {
                    for lease in owner.raw.leases.values() {
                        adapter.raw_retire(*lease)?;
                    }
                }
                owner.raw = Default::default();
                drop(owner.host.take());
                owner.pending.clear();
                drop(owner.binding.take());
            }
            ReclaimDma => {
                if let Some(adapter) = owner.adapter {
                    unsafe { adapter.reclaim_retired_after_quiescence() }?;
                    if adapter.retained_allocations() != 0 {
                        return Err("live DMA handles remain after driver destruction".into());
                    }
                }
            }
            ReleaseAdapter => {
                if let Some(adapter) = owner.adapter.take() {
                    // SAFETY: controller halted, IRQs disabled, worker joined, all CrabUSB
                    // Kernel/DMA handles destroyed, and successful unmap verified above.
                    // This reverses the one Box::leak used for the required static KernelOp.
                    unsafe {
                        drop(Box::from_raw(adapter as *const Adapter as *mut Adapter));
                    }
                }
            }
            ReleaseBar => {
                if let Some(mapping) = owner.mapping.as_mut() {
                    unsafe { mapping.unmap_after_quiescence() }?;
                }
                owner.mapping = None;
                owner.layout = None;
            }
            ReleaseVfio => {
                drop(owner.resources.take());
                owner.target = None;
            }
            Done => {}
        }
        Ok(())
    }
}
fn set_pci_dma(device: &vfio_ioctls::VfioDevice, enabled: bool) -> Result<(), String> {
    if device.get_region_size(7) < 6 || device.get_region_flags(7) & 3 != 3 {
        return Err("PCI configuration region unavailable/read-only".into());
    }
    let offset = device
        .get_region_offset(7)
        .checked_add(4)
        .and_then(|offset| i64::try_from(offset).ok())
        .ok_or("PCI configuration offset overflow")?;
    let mut bytes = [0u8; 2];
    // SAFETY: valid initialized two-byte CPU buffers and checked configuration offset.
    if unsafe { libc::pread(device.as_raw_fd(), bytes.as_mut_ptr().cast(), 2, offset) } != 2 {
        return Err("cannot read PCI command".into());
    }
    let command = u16::from_le_bytes(bytes) | 2;
    let command = if enabled { command | 4 } else { command & !4 };
    let bytes = command.to_le_bytes();
    if unsafe { libc::pwrite(device.as_raw_fd(), bytes.as_ptr().cast(), 2, offset) } != 2 {
        return Err("cannot write PCI command".into());
    }
    let mut observed = [0u8; 2];
    if unsafe { libc::pread(device.as_raw_fd(), observed.as_mut_ptr().cast(), 2, offset) } != 2 {
        return Err("cannot verify PCI command".into());
    }
    if u16::from_le_bytes(observed) & 6 != command & 6 {
        return Err("PCI memory/DMA command readback mismatch".into());
    }
    Ok(())
}
fn halt_mapping(
    mapping: &MappedRegion,
    layout: &resources::RegisterLayout,
    timeout: Duration,
) -> Result<(), String> {
    let command = unsafe {
        mapping
            .pointer()
            .as_ptr()
            .add(layout.operational)
            .cast::<u32>()
    };
    let status = unsafe { command.add(1) };
    let deadline = Instant::now() + timeout;
    // Wait for reset/not-ready to clear before any operational register update.
    loop {
        if unsafe { status.read_volatile() } & (1 << 11) == 0
            && unsafe { command.read_volatile() } & 2 == 0
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("controller not ready during close".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let value = unsafe { command.read_volatile() };
    unsafe {
        command.write_volatile(value & !5);
    }
    loop {
        let value = unsafe { status.read_volatile() };
        if value & 1 != 0 && value & (1 << 11) == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("controller halt timed out; DMA ownership retained".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl LocalController {
    fn submit_owned(
        &mut self,
        request: OwnedRequest,
        fields: Option<crab_usb::ExplicitControlFields>,
    ) -> Result<u64, OperationError> {
        self.check_event_pump()?;
        let token = self.next_request;
        let next = token
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("backend request identity exhausted"))?;
        let OwnedRequest {
            metadata,
            mut buffer,
        } = request;
        let direction = match metadata.direction {
            Direction::In => crab_usb::usb_if::transfer::Direction::In,
            Direction::Out => crab_usb::usb_if::transfer::Direction::Out,
        };
        let transfer_buffer = if metadata.direction == Direction::In {
            crab_usb::usb_if::endpoint::TransferBuffer::from_mut_slice(&mut buffer)
        } else {
            crab_usb::usb_if::endpoint::TransferBuffer::from_slice(&buffer)
        };
        use crab_usb::usb_if::{
            endpoint::TransferRequest,
            host::ControlSetup,
            transfer::{Recipient, Request, RequestType},
        };
        let raw = match metadata.transfer_type {
            TransferType::Control => {
                let setup = if fields.is_some() {
                    crate::domain::models::requests::SetupPacket {
                        request_type: 0x40,
                        request: 0,
                        value: 0,
                        index: 0,
                        length: 0,
                    }
                } else {
                    metadata
                        .setup
                        .ok_or_else(|| OperationError::validation("SETUP missing"))?
                };
                let request_type = match (setup.request_type >> 5) & 3 {
                    0 => RequestType::Standard,
                    1 => RequestType::Class,
                    2 => RequestType::Vendor,
                    _ => return Err(OperationError::validation("reserved request type")),
                };
                let recipient = match setup.request_type & 31 {
                    0 => Recipient::Device,
                    1 => Recipient::Interface,
                    2 => Recipient::Endpoint,
                    3 => Recipient::Other,
                    _ => return Err(OperationError::validation("reserved recipient")),
                };
                TransferRequest::Control {
                    setup: ControlSetup {
                        request_type,
                        recipient,
                        request: Request::from(setup.request),
                        value: setup.value,
                        index: setup.index,
                    },
                    direction,
                    buffer: transfer_buffer,
                }
            }
            TransferType::Bulk => TransferRequest::Bulk {
                direction,
                buffer: transfer_buffer,
            },
            TransferType::Interrupt => TransferRequest::Interrupt {
                direction,
                buffer: transfer_buffer,
            },
        };
        let adapter = self
            .adapter
            .ok_or_else(|| OperationError::validation("DMA adapter unavailable"))?;
        let epoch = adapter.allocation_epoch();
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller unavailable"))?;
        let result = match fields {
            Some(fields) => host.submit_control_plan(metadata.device.slot, raw, fields),
            None => host.submit_request(metadata.device.slot, metadata.endpoint, raw),
        };
        let raw_id = match result {
            Ok(id) => id,
            Err(error) => {
                // For the validated control/bulk/interrupt profile all fallible submission
                // checks precede TRB publication. Any dropped fresh map is unpublished.
                for lease in adapter.retired_streaming_since(epoch) {
                    unsafe { adapter.reclaim_lease_after_quiescence(lease) }
                        .map_err(|message| recovery_error(message, None))?;
                }
                return Err(usb_error(error));
            }
        };
        // Once published, metadata failures must return a retained accepted token.
        let (lease, terminal_error) =
            match host.request_dma_address(metadata.device.slot, metadata.endpoint, raw_id) {
                Ok(Some(iova)) => match adapter.mapping_lease(iova) {
                    Some(lease) => (Some(lease), None),
                    None => (
                        None,
                        Some(recovery_error(
                            "published request has no DMA lease".into(),
                            None,
                        )),
                    ),
                },
                Ok(None) if metadata.requested_length == 0 => (None, None),
                _ => (
                    None,
                    Some(recovery_error(
                        "cannot identify published request mapping".into(),
                        None,
                    )),
                ),
            };
        self.pending.insert(
            token,
            PendingRequest {
                metadata,
                buffer,
                raw_id,
                lease,
                terminal_error,
            },
        );
        self.next_request = next;
        Ok(token)
    }
}

impl crate::domain::services::explicit::ExplicitBackend for LocalController {
    fn raw_operation(
        &mut self,
        session: u64,
        request: crate::domain::models::raw::RawOperation,
    ) -> Result<serde_json::Value, OperationError> {
        self.execute_raw(session, request)
    }
    fn submit_control_plan(
        &mut self,
        plan: crate::domain::models::control_plan::ValidatedControlPlan,
    ) -> Result<u64, OperationError> {
        fn direction(d: Direction) -> crab_usb::usb_if::transfer::Direction {
            match d {
                Direction::In => crab_usb::usb_if::transfer::Direction::In,
                Direction::Out => crab_usb::usb_if::transfer::Direction::Out,
            }
        }
        let fields = crab_usb::ExplicitControlFields {
            setup: plan.wire_setup,
            data_stage: plan.has_data,
            data_direction: direction(plan.data_direction),
            status_direction: direction(plan.status_direction),
        };
        self.submit_owned(
            OwnedRequest {
                metadata: plan.metadata,
                buffer: plan.buffer,
            },
            Some(fields),
        )
    }
    fn noop(&mut self) -> Result<[u32; 4], OperationError> {
        self.check_event_pump()?;
        let host = self
            .host
            .as_mut()
            .ok_or_else(|| OperationError::validation("controller unavailable"))?;
        let raw = device_future(async { Ok(host.noop().await) })?;
        let code = raw[2] >> 24;
        if code != 1 {
            return Err(recovery_error(
                format!("Noop command failed: {code}"),
                Some(code),
            ));
        }
        Ok(raw)
    }
}
