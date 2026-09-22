//! The in-process job engine: queue adapter, worker pool, scheduler, handlers.
//!
//! # This assumes exactly one API instance
//!
//! The per-source semaphore, the broadcast fan-out to SSE, and stale-lock
//! recovery all depend on it. Running a second instance is not a scaling knob
//! — it invalidates all three at once. See ADR-0003, and ADR-0010 for the SSE
//! half.
#![forbid(unsafe_code)]

pub mod handlers;
pub mod limiter;
pub mod queue;
pub mod scheduler;
pub mod worker;
