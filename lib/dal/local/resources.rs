//! Validated local PCI region metadata. Parsing metadata never acquires a device.
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionError {
    NotMappable,
    InvalidBounds,
    InvalidAlignment,
    UnmappedHole,
    UnsupportedPermissions,
}

#[derive(Debug, Clone)]
pub struct RegionMetadata {
    pub size: u64,
    pub file_offset: u64,
    pub readable: bool,
    pub writable: bool,
    pub mappable: bool,
    /// None means the whole region; Some(empty) means no available mapping.
    pub sparse: Option<Vec<Range<u64>>>,
}
impl RegionMetadata {
    pub fn mapping(&self, range: Range<u64>, page: u64) -> Result<Range<u64>, RegionError> {
        if !self.mappable {
            return Err(RegionError::NotMappable);
        }
        if !self.readable || !self.writable {
            return Err(RegionError::UnsupportedPermissions);
        }
        if !page.is_power_of_two() || !self.file_offset.is_multiple_of(page) {
            return Err(RegionError::InvalidAlignment);
        }
        if range.start >= range.end || range.end > self.size {
            return Err(RegionError::InvalidBounds);
        }
        let end = range
            .end
            .checked_add(page - 1)
            .ok_or(RegionError::InvalidBounds)?
            / page
            * page;
        let start = range.start / page * page;
        if end > self.size || self.file_offset.checked_add(end).is_none() {
            return Err(RegionError::InvalidBounds);
        }
        if let Some(areas) = &self.sparse {
            for a in areas {
                if a.start >= a.end
                    || a.end > self.size
                    || !a.start.is_multiple_of(page)
                    || !a.end.is_multiple_of(page)
                {
                    return Err(RegionError::InvalidBounds);
                }
            }
            let mut areas = areas.clone();
            areas.sort_by_key(|a| a.start);
            let mut covered = start;
            for a in areas {
                if a.end <= covered {
                    continue;
                }
                if a.start > covered {
                    break;
                }
                covered = a.end;
                if covered >= end {
                    break;
                }
            }
            if covered < end {
                return Err(RegionError::UnmappedHole);
            }
        }
        Ok(start..end)
    }
}

#[cfg(feature = "hosted-direct")]
pub fn vfio_region(device: &vfio_ioctls::VfioDevice, index: u32) -> RegionMetadata {
    use vfio_ioctls::VfioRegionInfoCap;
    let flags = device.get_region_flags(index);
    // Linux UAPI VFIO_REGION_INFO_FLAG_{READ,WRITE,MMAP}.
    let sparse = device
        .get_region_caps(index)
        .into_iter()
        .find_map(|cap| match cap {
            VfioRegionInfoCap::SparseMmap(s) => Some(
                s.areas
                    .into_iter()
                    .map(|a| a.offset..a.offset.saturating_add(a.size))
                    .collect(),
            ),
            _ => None,
        });
    RegionMetadata {
        size: device.get_region_size(index),
        file_offset: device.get_region_offset(index),
        readable: flags & 1 != 0,
        writable: flags & 2 != 0,
        mappable: flags & 4 != 0,
        sparse,
    }
}

#[cfg(feature = "hosted-direct")]
pub mod mapping {
    use super::*;
    use std::{fs::File, os::fd::AsRawFd, ptr::NonNull, sync::Arc};
    pub struct MappedRegion {
        address: NonNull<u8>,
        length: usize,
        _file: Arc<File>,
    }
    impl MappedRegion {
        /// # Safety
        /// Caller must own the device and serialize all MMIO access. Metadata
        /// must describe this FD. No derived pointer may outlive this mapping.
        /// Driver consumers must not access unmapped holes or unknown registers.
        pub unsafe fn new(
            file: Arc<File>,
            meta: &RegionMetadata,
            range: Range<u64>,
            page: u64,
        ) -> Result<Self, String> {
            let selected = meta.mapping(range, page).map_err(|e| format!("{e:?}"))?;
            let length =
                usize::try_from(selected.end - selected.start).map_err(|_| "mapping too large")?;
            let offset = i64::try_from(
                meta.file_offset
                    .checked_add(selected.start)
                    .ok_or("offset overflow")?,
            )
            .map_err(|_| "file offset too large")?;
            // SAFETY: The caller supplies the owned FD and validated metadata.
            let address = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    offset,
                )
            };
            if address == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let address = NonNull::new(address.cast()).ok_or("null mapping")?;
            Ok(Self {
                address,
                length,
                _file: file,
            })
        }
        pub fn pointer(&self) -> NonNull<u8> {
            self.address
        }
        pub fn length(&self) -> usize {
            self.length
        }
    }
    impl Drop for MappedRegion {
        fn drop(&mut self) {
            // SAFETY: Only this owner releases this exact mapping. Raw consumers
            // must already be stopped under the constructor contract.
            unsafe {
                libc::munmap(self.address.as_ptr().cast(), self.length);
            }
        }
    }
}

/// Guard against accidental host execution. The trusted VM launcher supplies
/// a QEMU fw_cfg marker; this is not a security boundary against a hostile root.
#[cfg(feature = "hosted-direct")]
pub struct GuestResources {
    pub device: std::sync::Arc<vfio_ioctls::VfioDevice>,
    pub container: std::sync::Arc<vfio_ioctls::VfioContainer>,
}

pub fn validate_guest_target(
    marker: &str,
    expected: &str,
    vendor: u16,
    device: u16,
    bdf: &str,
) -> Result<(), String> {
    if expected.is_empty() || marker.trim_end_matches('\0') != expected {
        return Err("guest marker mismatch".into());
    }
    let bytes = bdf.as_bytes();
    if bytes.len() != 12
        || bytes[4] != b':'
        || bytes[7] != b':'
        || bytes[10] != b'.'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10) || b.is_ascii_hexdigit())
    {
        return Err("invalid PCI address".into());
    }
    if u8::from_str_radix(&bdf[8..10], 16).map_err(|_| "invalid device")? > 31
        || u8::from_str_radix(&bdf[11..12], 16).map_err(|_| "invalid function")? > 7
    {
        return Err("invalid PCI device or function".into());
    }
    if !matches!((vendor, device), (0x1b36, 0x000d) | (0x1033, 0x0194)) {
        return Err("target is not an approved emulated xHCI model".into());
    }
    Ok(())
}

#[cfg(feature = "hosted-direct")]
impl GuestResources {
    /// Open only a controller already assigned to vfio-pci inside the guest.
    /// No explicit bind/unbind/reset is issued. VFIO acquisition itself has
    /// device effects and is permitted only inside the marked guest.
    pub fn open(bdf: &str, expected_run_id: &str) -> Result<Self, String> {
        use std::{fs, path::PathBuf, sync::Arc};
        let marker = fs::read_to_string("/sys/firmware/qemu_fw_cfg/by_name/opt/fah-lab/raw")
            .map_err(|_| "missing trusted guest marker")?;
        // Validate the path before any lookup using input from the CLI.
        validate_guest_target(&marker, expected_run_id, 0x1b36, 0x000d, bdf)?;
        let path = PathBuf::from("/sys/bus/pci/devices").join(bdf);
        let read_hex = |name: &str| -> Result<u16, String> {
            let value = fs::read_to_string(path.join(name)).map_err(|e| e.to_string())?;
            u16::from_str_radix(value.trim().trim_start_matches("0x"), 16)
                .map_err(|e| e.to_string())
        };
        validate_guest_target(
            &marker,
            expected_run_id,
            read_hex("vendor")?,
            read_hex("device")?,
            bdf,
        )?;
        let driver = fs::read_link(path.join("driver")).map_err(|e| e.to_string())?;
        if driver.file_name().and_then(|s| s.to_str()) != Some("vfio-pci") {
            return Err("guest controller is not assigned to vfio-pci".into());
        }
        let container = Arc::new(vfio_ioctls::VfioContainer::new(None).map_err(|e| e.to_string())?);
        // VFIO validates group viability and enables translated IOMMU access.
        let device = Arc::new(
            vfio_ioctls::VfioDevice::new(&path, container.clone(), true)
                .map_err(|e| e.to_string())?,
        );
        Ok(Self { device, container })
    }
}
