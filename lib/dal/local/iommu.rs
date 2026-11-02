//! Linux VFIO type1 metadata parser. No guessed IOVA ranges or no-IOMMU mode.
use std::collections::BTreeSet;
#[derive(Debug, PartialEq, Eq)]
pub struct IommuLimits {
    pub page_sizes: u64,
    pub ranges: Vec<(u64, u64)>,
}
pub fn parse_info(bytes: &[u8]) -> Result<IommuLimits, String> {
    let u32at = |at: usize| -> Result<u32, String> {
        Ok(u32::from_ne_bytes(
            bytes
                .get(at..at + 4)
                .ok_or("truncated IOMMU info")?
                .try_into()
                .unwrap(),
        ))
    };
    let u64at = |at: usize| -> Result<u64, String> {
        Ok(u64::from_ne_bytes(
            bytes
                .get(at..at + 8)
                .ok_or("truncated IOMMU info")?
                .try_into()
                .unwrap(),
        ))
    };
    let size = u32at(0)? as usize;
    if size > bytes.len() || size < 24 {
        return Err("invalid IOMMU info size".into());
    }
    if u32at(4)? & 3 != 3 {
        return Err("IOMMU page sizes and capabilities required".into());
    }
    let page_sizes = u64at(8)?;
    if page_sizes & 4096 == 0 {
        return Err("4KiB translated DMA pages unavailable".into());
    }
    let mut offset = u32at(16)? as usize;
    let mut seen = BTreeSet::new();
    let mut ranges = Vec::new();
    while offset != 0 {
        if offset < 24 || offset + 8 > size || !seen.insert(offset) {
            return Err("invalid or cyclic IOMMU capability".into());
        }
        let id = u16::from_ne_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let version = u16::from_ne_bytes(bytes[offset + 2..offset + 4].try_into().unwrap());
        let next = u32at(offset + 4)? as usize;
        if id == 1 {
            if version != 1 || offset + 16 > size {
                return Err("unsupported IOVA range capability".into());
            }
            let count = u32at(offset + 8)? as usize;
            let length = count
                .checked_mul(16)
                .and_then(|n| n.checked_add(16))
                .ok_or("IOVA range overflow")?;
            if offset.checked_add(length).is_none_or(|end| end > size) {
                return Err("truncated IOVA ranges".into());
            }
            for index in 0..count {
                let at = offset + 16 + index * 16;
                let start = u64at(at)?;
                let end = u64at(at + 8)?;
                if start > end {
                    return Err("reversed IOVA range".into());
                }
                ranges.push((start, end));
            }
        }
        offset = next;
    }
    if ranges.is_empty() {
        return Err("kernel did not report allowed IOVA ranges".into());
    }
    Ok(IommuLimits { page_sizes, ranges })
}
#[cfg(feature = "hosted-direct")]
pub fn query(container: &vfio_ioctls::VfioContainer) -> Result<IommuLimits, String> {
    use std::os::fd::AsRawFd;
    let mut words = vec![0u64; 3];
    for _ in 0..3 {
        let length = words.len() * 8;
        words[0] = length as u64;
        // SAFETY: aligned owned buffer, argsz describes its full writable length.
        // Linux UAPI vfio.h: _IO(';', VFIO_BASE + 12), VFIO_BASE=100.
        if unsafe { libc::ioctl(container.as_raw_fd(), 0x3b70, words.as_mut_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let needed = (words[0] & 0xffff_ffff) as usize;
        if needed > 1024 * 1024 {
            return Err("IOMMU metadata exceeds bounded buffer".into());
        }
        if needed > length {
            words = vec![0u64; needed.div_ceil(8)];
            continue;
        }
        // SAFETY: byte view of initialized u64 buffer, CPU-owned, not DMA mapped.
        return parse_info(unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), length) });
    }
    Err("IOMMU metadata changed during query".into())
}
