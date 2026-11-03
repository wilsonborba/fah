//! Opaque lifecycle namespaces, not authentication credentials.
//! The random process seed plus checked counter is unique within one process.
//! Independent executable starts obtain independent64-bit namespace seeds.
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
pub fn fresh() -> Option<u64> {
    static SEED: OnceLock<u64> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .ok()?;
    let seed = SEED.get_or_init(|| RandomState::new().hash_one(std::process::id()));
    Some(seed.wrapping_add(sequence))
}
