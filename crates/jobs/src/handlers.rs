//! Job handlers, one per `JobKind`.
//!
//! Handlers own the behavior a queue library would not have supplied:
//! per-source semaphores, page fan-out within a single job, cancellation tied
//! to partial-file cleanup, and the trace context stored in
//! `job.payload.trace_context` so `job.run` can link back to the request that
//! enqueued it (ADR-0015).
//!
//! Progress is published as `JobEvent` to the broadcast channel. Every state
//! change must also be observable through a plain fetch — SSE makes the UI
//! live, not correct (ADR-0010).
//!
//! `ENOSPC` is handled explicitly: delete the partial file, fail with
//! `DomainError::InsufficientStorage`, and let `/readyz` report the library as
//! not writable (ADR-0007).
