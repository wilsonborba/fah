//! VFIO DMA bridge. Raw allocations remain quarantined until confirmed unmap.
use super::iova::{IovaAllocator, Lease};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Implementations register only the supplied owned memory and retain it until unmap.
/// A map error must leave no mapping. An unmap error must retain the mapping.
pub trait MappingProvider: Send + Sync + 'static {
    /// # Safety
    /// The allocation must remain valid and comply with the mapping access contract.
    unsafe fn map(&self, iova: u64, size: usize, cpu: *mut u8) -> Result<(), String>;
    fn unmap(&self, iova: u64, size: usize) -> Result<(), String>;
}

#[cfg(feature = "hosted-direct")]
pub struct VfioMappingProvider {
    pub container: Arc<vfio_ioctls::VfioContainer>,
}
#[cfg(feature = "hosted-direct")]
impl MappingProvider for VfioMappingProvider {
    unsafe fn map(&self, iova: u64, size: usize, cpu: *mut u8) -> Result<(), String> {
        // SAFETY: The adapter retains the allocation until successful unmap.
        unsafe { self.container.vfio_dma_map(iova, size, cpu) }.map_err(|e| e.to_string())
    }
    fn unmap(&self, iova: u64, size: usize) -> Result<(), String> {
        self.container
            .vfio_dma_unmap(iova, size)
            .map_err(|e| e.to_string())
    }
}

struct Allocation {
    cpu: usize,
    layout: Layout,
    lease: Lease,
    retired: bool,
}
struct State {
    allocator: IovaAllocator,
    allocations: BTreeMap<u64, Allocation>,
    error: Option<String>,
}
pub struct DmaAdapter<P: MappingProvider> {
    provider: P,
    page_size: usize,
    mask: u64,
    state: Mutex<State>,
}

impl<P: MappingProvider> DmaAdapter<P> {
    pub fn new(
        provider: P,
        ranges: &[(u64, u64)],
        mask: u64,
        page_size: usize,
    ) -> Result<Self, String> {
        let allocator =
            IovaAllocator::new(ranges, mask, page_size as u64).map_err(|e| format!("{e:?}"))?;
        Ok(Self {
            provider,
            page_size,
            mask,
            state: Mutex::new(State {
                allocator,
                allocations: BTreeMap::new(),
                error: None,
            }),
        })
    }
    pub fn retained_allocations(&self) -> usize {
        self.state.lock().unwrap().allocations.len()
    }
    pub fn last_error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
    fn allocate(&self, size: usize, alignment: usize, mask: u64) -> Result<(usize, u64), String> {
        if size == 0 || !alignment.is_power_of_two() {
            return Err("invalid allocation layout".into());
        }
        let rounded = size
            .checked_add(self.page_size - 1)
            .ok_or("size overflow")?
            / self.page_size
            * self.page_size;
        let layout = Layout::from_size_align(rounded, alignment.max(self.page_size))
            .map_err(|e| e.to_string())?;
        let mut state = self.state.lock().map_err(|_| "DMA state poisoned")?;
        let lease = state
            .allocator
            .reserve(rounded as u64, layout.align() as u64)
            .map_err(|e| format!("{e:?}"))?;
        if (lease.iova as u128 + lease.length as u128 - 1) > mask.min(self.mask) as u128 {
            state
                .allocator
                .release(lease, false)
                .map_err(|e| format!("{e:?}"))?;
            return Err("device mask exceeded".into());
        }
        // SAFETY: layout is nonzero and valid; no reference to this storage is formed.
        let cpu = unsafe { alloc_zeroed(layout) };
        if cpu.is_null() {
            state
                .allocator
                .release(lease, false)
                .map_err(|e| format!("{e:?}"))?;
            return Err("allocation failed".into());
        }
        // SAFETY: Raw allocation is owned here and retained on every successful map.
        if let Err(e) = unsafe { self.provider.map(lease.iova, rounded, cpu) } {
            unsafe { dealloc(cpu, layout) };
            state
                .allocator
                .release(lease, false)
                .map_err(|e| format!("{e:?}"))?;
            return Err(e);
        }
        state
            .allocator
            .mapped(lease)
            .map_err(|e| format!("{e:?}"))?;
        state.allocations.insert(
            lease.iova,
            Allocation {
                cpu: cpu as usize,
                layout,
                lease,
                retired: false,
            },
        );
        Ok((cpu as usize, lease.iova))
    }
    fn retire(&self, iova: u64) {
        let mut state = self.state.lock().unwrap();
        if let Some(a) = state.allocations.get_mut(&iova) {
            a.retired = true;
        } else {
            state.error = Some("unknown DMA retirement".into());
        }
    }
    /// Release only retired storage after the backend has stopped using it.
    ///
    /// # Safety
    /// All relevant DMA must be quiescent. No user may access retired pointers.
    /// A software cancellation flag is insufficient evidence.
    pub unsafe fn reclaim_retired_after_quiescence(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "DMA state poisoned")?;
        let retired: Vec<u64> = state
            .allocations
            .iter()
            .filter(|(_, a)| a.retired)
            .map(|(k, _)| *k)
            .collect();
        for iova in retired {
            let a = state.allocations.get(&iova).unwrap();
            let (lease, cpu, layout) = (a.lease, a.cpu, a.layout);
            if let Err(e) = self.provider.unmap(iova, layout.size()) {
                state.error = Some(e.clone());
                return Err(e);
            }
            state
                .allocator
                .quiesced(lease)
                .map_err(|e| format!("{e:?}"))?;
            state
                .allocator
                .release(lease, true)
                .map_err(|e| format!("{e:?}"))?;
            state.allocations.remove(&iova);
            // SAFETY: Caller established quiescence and unmap has succeeded.
            unsafe { dealloc(cpu as *mut u8, layout) };
        }
        Ok(())
    }
    fn record(&self, e: String) {
        self.state.lock().unwrap().error = Some(e);
    }
}
// No Drop frees mapped storage. Uncertain ownership intentionally retains memory.

#[cfg(feature = "hosted-direct")]
mod integration {
    use super::*;
    use crab_usb::{DmaAddr, DmaDirection, DmaError, DmaHandle, DmaMapHandle, DmaOp, KernelOp};
    use std::{num::NonZeroUsize, ptr::NonNull, time::Duration};
    impl<P: MappingProvider> DmaOp for DmaAdapter<P> {
        fn page_size(&self) -> usize {
            self.page_size
        }
        unsafe fn alloc_coherent(&self, mask: u64, layout: Layout) -> Option<DmaHandle> {
            match self.allocate(layout.size(), layout.align(), mask) {
                Ok((cpu, iova)) => Some(unsafe {
                    DmaHandle::new(NonNull::new(cpu as *mut u8)?, DmaAddr::from(iova), layout)
                }),
                Err(e) => {
                    self.record(e);
                    None
                }
            }
        }
        unsafe fn dealloc_coherent(&self, handle: DmaHandle) {
            self.retire(handle.dma_addr().as_u64());
        }
        unsafe fn map_single(
            &self,
            mask: u64,
            addr: NonNull<u8>,
            size: NonZeroUsize,
            align: usize,
            _direction: DmaDirection,
        ) -> Result<DmaMapHandle, DmaError> {
            // Always bounce. Arbitrary user buffers may share unaligned pages.
            let layout = Layout::from_size_align(size.get(), align)?;
            let (cpu, iova) = self.allocate(size.get(), align, mask).map_err(|e| {
                self.record(e);
                DmaError::NoMemory
            })?;
            Ok(unsafe {
                DmaMapHandle::new(addr, iova.into(), layout, NonNull::new(cpu as *mut u8))
            })
        }
        unsafe fn unmap_single(&self, handle: DmaMapHandle) {
            self.retire(handle.dma_addr().as_u64());
        }
        fn confirm_write(&self, h: &DmaMapHandle, offset: usize, size: usize, d: DmaDirection) {
            assert!(offset <= h.size() && size <= h.size() - offset);
            if matches!(d, DmaDirection::ToDevice | DmaDirection::Bidirectional) {
                let Some(bounce) = h.alloc_virt() else {
                    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                    return; // Coherent allocation: CPU address already names shared storage.
                };
                for i in offset..offset + size {
                    unsafe {
                        bounce
                            .as_ptr()
                            .add(i)
                            .write_volatile(h.as_ptr().as_ptr().add(i).read());
                    }
                }
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
            }
        }
        fn prepare_read(&self, h: &DmaMapHandle, offset: usize, size: usize, d: DmaDirection) {
            assert!(offset <= h.size() && size <= h.size() - offset);
            if matches!(d, DmaDirection::FromDevice | DmaDirection::Bidirectional) {
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                let Some(bounce) = h.alloc_virt() else {
                    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                    return; // Coherent allocation: CPU address already names shared storage.
                };
                for i in offset..offset + size {
                    unsafe {
                        h.as_ptr()
                            .as_ptr()
                            .add(i)
                            .write(bounce.as_ptr().add(i).read_volatile());
                    }
                }
            }
        }
    }
    impl<P: MappingProvider> KernelOp for DmaAdapter<P> {
        fn delay(&self, d: Duration) {
            std::thread::sleep(d);
        }
    }
}
