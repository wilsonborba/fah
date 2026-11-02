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
//! uses ordinary pointer reads. Context DBox reads also use ordinary loads.
//! transfer.rs forms a mutable slice before map_single_array. These paths need
//! correction against VFIO's mapping contract before executable bring-up.
//!
//! An adapter wrapper cannot fix dependency-private reads it never intercepts.
//! The maintained dependency patch must cover those paths (issue #10). This
//! contract does not declare them sound. Hardware construction and VM driver
//! execution remain blocked until that implementation and review are complete.
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
