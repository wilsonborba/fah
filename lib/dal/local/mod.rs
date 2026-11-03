//! Local hardware access and DMA safety contract.
//!
//! # Ownership phases
//!
//! CPU-owned storage may be initialized before publication. Mapped storage must
//! satisfy the VFIO mapping API's stricter access preconditions even before DMA.
//! Published buffers, rings, and contexts remain device-owned until a completion
//! protocol or confirmed quiescence returns ownership. A timeout or cancellation
//! flag does not return ownership. Unmap must succeed before storage is freed.
//!
//! # Required access discipline
//!
//! Do not create Rust references to storage while its mapping contract forbids
//! them. Map raw pointers and lengths, with checked alignment and page bounds.
//! Device-written event fields require explicit I/O access semantics. Read the
//! publication/cycle indicator first, apply the platform's acquire discipline,
//! then read the remaining scalar fields. A multiword TRB is not an atomic read.
//! Volatile accesses prevent elimination but do not provide atomicity or
//! inter-thread synchronization. A compiler barrier does not repair aliasing.
//!
//! CPU publication must initialize all descriptor fields before the final cycle
//! bit is made visible, then order publication before the doorbell. Review both
//! command and transfer rings; do not generalize from one passing test.
//!
//! # Baseline findings and execution gate
//!
//! CrabUSB e83b56b event.rs current_data uses dma-api DArray::read. dma-api 0.7.3
//! used ordinary pointer reads. Context DBox reads also used ordinary loads.
//! The maintained patches now use volatile accesses, cycle-first event reads,
//! control-dword-last TRB publication and raw streaming-map construction.
//! Reference-forming dma-api helpers fail closed under fah-vfio. Streaming
//! buffers use CPU-owned originals and separately mapped bounce storage.
//!
//! An adapter wrapper cannot fix dependency-private reads it never intercepts.
//! The maintained dependency patch must cover those paths (issue #10). This
//! contract does not declare the whole backend sound. Executable bring-up also
//! requires the single-owner lifecycle and quiescence/teardown gates (#11/#17).
//!
//! # Failure and reclamation
//!
//! Keep ownership on failed unmap, uncertain stop, or late completion. Quarantine
//! resources rather than recycling their IOVA. Do not implement Drop as an
//! unconditional unmap/free of potentially active rings. Process-death cleanup
//! must be checked against VFIO/kernel behavior separately from normal close.
//!
//! # Evidence limits
//!
//! Unit tests model ownership and failures. Codegen inspection supports review.
//! Neither codegen, Miri, successful compilation nor runtime tests alone prove
//! correctness of hardware DMA. Coherent x86 caches remove cache maintenance,
//! not ordering, aliasing, or lifetime obligations.

pub mod iova;

#[cfg(feature = "hosted-direct")]
pub mod dma;

pub mod resources;

#[cfg(feature = "hosted-direct")]
pub mod events;

pub mod iommu;

#[cfg(feature = "hosted-direct")]
pub mod controller;

pub mod teardown;
