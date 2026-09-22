//! The in-process job engine: queue adapter, worker pool, scheduler, handlers.
//!
//! # This assumes exactly one API instance
//!
//! The per-source semaphore, the broadcast fan-out to SSE, and stale-lock
//! recovery all depend on it. Running a second instance is not a scaling knob
//! — it invalidates all three at once. See ADR-0003, and ADR-0010 for the SSE
//! half.
//! # `package_chapter` has no handler
//!
//! `JobKind` names it, and nothing enqueues or handles it. Packaging happens
//! inside `download_chapter`: `LibraryStore::write_chapter` takes page bytes
//! in memory, so a separate packaging step would need an intermediate store
//! for raw pages — a second on-disk format to version and keep compatible,
//! bought for nothing, since ADR-0007 already makes placement atomic. The kind
//! is left in place rather than removed because a future source type that
//! streams to disk would want it. Decide before adding one.
#![forbid(unsafe_code)]

pub mod handlers;
pub mod limiter;
pub mod queue;
pub mod scheduler;
pub mod worker;
