//! The worker pool.
//!
//! Bounded from validated configuration, never implicit. Workers poll on a
//! short interval with jitter; `LISTEN`/`NOTIFY` is deliberately not used —
//! notifications are lost with no listener, so a correct implementation polls
//! anyway, and `NOTIFY` adds a commit-time global lock plus a dedicated
//! connection per listener (ADR-0003).
//!
//! WASM invocations go to `spawn_blocking` so a slow or looping source cannot
//! occupy an async worker thread.
//!
//! On `SIGTERM`: stop claiming, drain in-flight work up to a bounded timeout,
//! return undrained jobs to `queued`, exit. ADR-0003 requires a test for this,
//! partly because axum 0.9 changes `serve`'s graceful-shutdown contract and
//! that test is what will catch it.
