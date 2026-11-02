//! Bounded device-address reservations. No syscall or physical address inference.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationError {
    InvalidRange,
    InvalidAlignment,
    Exhausted,
    UnknownLease,
    DeviceOwned,
    InvalidTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingState {
    Reserved,
    Mapped,
    Quiesced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub id: u64,
    pub iova: u64,
    pub length: u64,
}

#[derive(Debug)]
struct Entry {
    lease: Lease,
    state: MappingState,
}

#[derive(Debug)]
pub struct IovaAllocator {
    ranges: Vec<(u128, u128)>,
    mask: u64,
    page_size: u64,
    next_id: u64,
    live: BTreeMap<u64, Entry>,
}
impl IovaAllocator {
    /// Ranges use inclusive ends, matching address masks. Internally use u128
    /// so the exclusive end above u64::MAX remains representable.
    pub fn new(ranges: &[(u64, u64)], mask: u64, page_size: u64) -> Result<Self, AllocationError> {
        if !page_size.is_power_of_two() {
            return Err(AllocationError::InvalidAlignment);
        }
        let mut accepted = Vec::new();
        for &(start, end) in ranges {
            if start > end {
                return Err(AllocationError::InvalidRange);
            }
            let end = end.min(mask) as u128 + 1;
            let start = align_up(start as u128, page_size as u128);
            let end = end / page_size as u128 * page_size as u128;
            if start < end {
                accepted.push((start, end));
            }
        }
        accepted.sort_unstable();
        let mut merged: Vec<(u128, u128)> = Vec::new();
        for (start, end) in accepted {
            if let Some(last) = merged.last_mut()
                && start <= last.1
            {
                last.1 = last.1.max(end);
                continue;
            }
            merged.push((start, end));
        }
        if merged.is_empty() {
            return Err(AllocationError::InvalidRange);
        }
        Ok(Self {
            ranges: merged,
            mask,
            page_size,
            next_id: 0,
            live: BTreeMap::new(),
        })
    }
    pub fn reserve(&mut self, length: u64, alignment: u64) -> Result<Lease, AllocationError> {
        if length == 0 {
            return Err(AllocationError::InvalidRange);
        }
        if !alignment.is_power_of_two() {
            return Err(AllocationError::InvalidAlignment);
        }
        let alignment = alignment.max(self.page_size) as u128;
        let length = align_up(length as u128, self.page_size as u128);
        if length > u64::MAX as u128 {
            return Err(AllocationError::Exhausted);
        }
        for &(start, end) in &self.ranges {
            let mut cursor = align_up(start, alignment);
            for entry in self.live.values() {
                let a = entry.lease.iova as u128;
                let b = a + entry.lease.length as u128;
                if b <= cursor || a >= end {
                    continue;
                }
                if cursor + length <= a {
                    break;
                }
                cursor = align_up(b, alignment);
            }
            if cursor + length <= end && cursor + length - 1 <= self.mask as u128 {
                let id = self.next_id;
                self.next_id = self
                    .next_id
                    .checked_add(1)
                    .ok_or(AllocationError::Exhausted)?;
                let lease = Lease {
                    id,
                    iova: cursor as u64,
                    length: length as u64,
                };
                self.live.insert(
                    lease.iova,
                    Entry {
                        lease,
                        state: MappingState::Reserved,
                    },
                );
                return Ok(lease);
            }
        }
        Err(AllocationError::Exhausted)
    }
    fn entry(&mut self, lease: Lease) -> Result<&mut Entry, AllocationError> {
        self.live
            .get_mut(&lease.iova)
            .filter(|e| e.lease == lease)
            .ok_or(AllocationError::UnknownLease)
    }
    pub fn mapped(&mut self, lease: Lease) -> Result<(), AllocationError> {
        let e = self.entry(lease)?;
        if e.state != MappingState::Reserved {
            return Err(AllocationError::InvalidTransition);
        }
        e.state = MappingState::Mapped;
        Ok(())
    }
    /// Metadata only. The caller must first establish real device quiescence.
    pub fn quiesced(&mut self, lease: Lease) -> Result<(), AllocationError> {
        let e = self.entry(lease)?;
        if e.state != MappingState::Mapped {
            return Err(AllocationError::InvalidTransition);
        }
        e.state = MappingState::Quiesced;
        Ok(())
    }
    /// Release a reservation that never mapped, or a quiesced mapping after a
    /// successful complete unmap. Failed/partial unmaps must retain the lease.
    pub fn release(&mut self, lease: Lease, unmap_confirmed: bool) -> Result<(), AllocationError> {
        let e = self.entry(lease)?;
        match e.state {
            MappingState::Reserved => {}
            MappingState::Quiesced if unmap_confirmed => {}
            _ => return Err(AllocationError::DeviceOwned),
        }
        self.live.remove(&lease.iova);
        Ok(())
    }
    pub fn live_count(&self) -> usize {
        self.live.len()
    }
}
fn align_up(value: u128, alignment: u128) -> u128 {
    (value + alignment - 1) & !(alignment - 1)
}
