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
//!
//! # Why configuration is validated before the subscriber exists
//!
//! Step 1 runs before step 2, so a configuration failure is printed to stderr
//! rather than logged. That is deliberate: the subscriber's own settings come
//! from the configuration being validated, so logging the failure would mean
//! using the thing that is broken to report that it is broken.

#![forbid(unsafe_code)]

use anyhow::Context;
use nyuka_api::config::RawConfig;

fn main() -> anyhow::Result<()> {
    // Step 1, before any of the runtime exists. A configuration error here is
    // the single most common startup failure and it must read plainly.
    let raw = RawConfig::load().context("reading configuration")?;
    let config = match raw.validate() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime")?
        .block_on(nyuka_api::run(config))
}
