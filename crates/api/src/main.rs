//! `nyuka-api` — REST, SSE, and the embedded SPA, in one binary.
//!
//! This is the composition root: it is the only place the dependency graph is
//! assembled. Adapters are constructed here and passed to the router and the
//! worker pool as `domain` port implementations.
//!
//! Startup order matters:
//!
//! 1. Load and **validate** configuration, failing fast. Includes the auth
//!    settings — issuer reachable, client id and secret present, redirect URI
//!    matching, allow-list non-empty (ADR-0005).
//! 2. Install the tracing subscriber before anything that logs.
//! 3. Connect the pool and run migrations.
//! 4. Recover stale job locks (ADR-0003).
//! 5. Start the worker pool and the scheduler.
//! 6. Serve, with graceful shutdown on `SIGTERM`.

#![forbid(unsafe_code)]

mod config;
mod error;
mod openapi;
mod routes;
mod sse;
mod static_files;
mod telemetry;

fn main() -> anyhow::Result<()> {
    // TODO(scaffold): the startup sequence above.
    Ok(())
}
