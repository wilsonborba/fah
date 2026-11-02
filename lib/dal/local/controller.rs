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
/// Owns the only CrabUSB engine, its consumer, mappings and VFIO lifetime.
pub struct LocalController {
    run_id: String,
    target: Option<String>,
    resources: Option<GuestResources>,
    mapping: Option<MappedRegion>,
    layout: Option<resources::RegisterLayout>,
    adapter: Option<&'static Adapter>,
    host: Option<crab_usb::ExplicitHost>,
    binding: Option<Arc<InterruptBinding>>,
    pump: Option<Pump>,
}
impl LocalController {
    pub fn new(run_id: String) -> Self {
        Self {
            run_id,
            target: None,
            resources: None,
            mapping: None,
            layout: None,
            adapter: None,
            host: None,
            binding: None,
            pump: None,
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
            .map_err(|e| os_error(format!("{e:?}")))
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
        let mode = select_mode(&resource.device).map_err(|e| e.to_string())?;
        let binding = Arc::new(
            InterruptBinding::register(resource.device.clone(), mode).map_err(|e| e.to_string())?,
        );
        self.binding = Some(binding.clone());
        // Enable Memory Space and Bus Master only after guest-only ownership and maps.
        // Config writes use checked pwrite rather than the library's log-only wrapper.
        if resource.device.get_region_size(7) < 6 || resource.device.get_region_flags(7) & 3 != 3 {
            return Err("PCI configuration region unavailable or read-only".into());
        }
        let config = resource
            .device
            .get_region_offset(7)
            .checked_add(4)
            .and_then(|offset| i64::try_from(offset).ok())
            .ok_or("PCI configuration offset overflow")?;
        let mut command = [0u8; 2];
        let read = unsafe {
            libc::pread(
                resource.device.as_raw_fd(),
                command.as_mut_ptr().cast(),
                2,
                config,
            )
        };
        if read != 2 {
            return Err("cannot read guest PCI command".into());
        }
        let command = (u16::from_le_bytes(command) | 6).to_le_bytes();
        let written = unsafe {
            libc::pwrite(
                resource.device.as_raw_fd(),
                command.as_ptr().cast(),
                2,
                config,
            )
        };
        if written != 2 {
            return Err("cannot enable guest PCI memory/DMA".into());
        }
        block_until(
            self.host.as_mut().unwrap().initialize(),
            Duration::from_secs(5),
        )?
        .map_err(|e| format!("{e:?}"))?;
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
        self.resources = Some(resource);
        self.target = Some(target.into());
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
        }
    }
    fn close(&mut self) -> Result<(), OperationError> {
        // Coordinated hardware halt and teardown is gated by #17. Keep every resource.
        Err(OperationError {
            category: ErrorCategory::Unsupported,
            message: "coordinated close is unavailable until teardown gate #17".into(),
            os_code: None,
            completion_code: None,
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
