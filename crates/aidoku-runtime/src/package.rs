//! `.aix` loading: a zip containing a manifest, `main.wasm`, and filter and
//! settings JSON.
//!
//! Reads the module's required imports so `SourceRegistry::install` can refuse
//! a package this host cannot run, and extracts the source's declared rate
//! limit for the job engine's semaphore.
