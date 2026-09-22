//! The shared `Engine` and per-invocation `Store`.
//!
//! Configuration, per ADR-0004:
//!
//! - One `Store` per invocation, carrying its own resource table.
//! - A shared `Engine` with the on-disk compilation cache enabled, keyed to
//!   include `HOST_ABI_VERSION`.
//! - **Epoch-based interruption** for timeouts, not fuel: fuel modifies
//!   compiled code and costs throughput, and what is needed here is a
//!   wall-clock bound rather than deterministic instruction counting.
//! - Memory capped in the 64–128 MB range.
//! - No WASI filesystem, no WASI sockets. Network access exists only through
//!   the `net` host import.
//! - Invocations run on `spawn_blocking`.
