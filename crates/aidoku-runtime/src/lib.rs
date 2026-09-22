//! The Aidoku `.aix` host: a `wasmtime` sandbox implementing the Aidoku source
//! ABI, exposed through the `domain` provider ports.
//!
//! This is the largest and riskiest crate in the workspace. Read ADR-0004
//! before changing anything here.
//!
//! # The ABI has no version to pin
//!
//! The upstream `aidoku` crate is `0.3.0` with `publish = false` and no git
//! tags; sources consume it as a git dependency. There is no semver boundary,
//! so three controls substitute for one:
//!
//! 1. [`ABI_SOURCE_COMMIT`] records the exact aidoku-rs commit this host
//!    targets, in code rather than only in a lockfile.
//! 2. [`HOST_ABI_VERSION`] is ours, and is part of the compiled-module cache
//!    key so a bump invalidates cached modules.
//! 3. A conformance `.aix`, built from a fixture crate at that commit,
//!    exercises every implemented import and asserts the postcard round-trip.
//!
//! Values cross the boundary postcard-encoded, so an upstream struct change is
//! a silent wire-format break. If any of the three controls is skipped, this
//! design is unsafe.
//!
//! # Capability tiers
//!
//! | tier | imports                                   | v1                |
//! |------|-------------------------------------------|-------------------|
//! | 1    | `std`, `defaults`, `net`, `html`          | required          |
//! | 2    | `js::context_*` (via `rquickjs`)          | behind `js`       |
//! | 3    | `js::webview_*`, `canvas`                 | **not built**     |
//!
//! Tier 3 returns a distinct unsupported-capability error, and
//! `SourceRegistry::install` refuses packages that require it. FlareSolverr
//! does **not** substitute for `js::webview_*`: it is a detour for challenges
//! on the `net` path only.
#![forbid(unsafe_code)]

/// The aidoku-rs commit this host's ABI is written against.
///
/// The conformance fixture is built against this exact commit, so re-pinning
/// means rebuilding it and getting a passing run — not editing this constant.
pub const ABI_SOURCE_COMMIT: &str = "e1320b0a2e11afb59e4dee374883a2212d325699";

/// Our own ABI generation. Bump on any change to the host import surface or
/// to a postcard-encoded struct. Part of the module cache key.
pub const HOST_ABI_VERSION: u32 = 1;

pub mod engine;
pub mod error;
pub mod imports;
pub mod models;
pub mod package;
pub mod resource;
pub mod source;
pub mod state;
