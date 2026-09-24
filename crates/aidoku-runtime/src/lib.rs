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
pub const HOST_ABI_VERSION: u32 = 2;

pub mod adapter;
pub mod engine;
pub mod error;
/// The sources v1 is accountable for, as a regression set (ADR-0004).
///
/// The commitment itself is capability-defined — any source whose required
/// imports are tier 1 — because the install check already enforces exactly
/// that boundary. These seven are the ones the spike proved end to end, and
/// they are what the scheduled live run exercises.
///
/// Both source shapes are here on purpose. The `abs:` bug that made every
/// HTML-scraping source return zero entries was invisible in JSON sources, so
/// a regression set of only the first kind would have passed through it.
pub const REGRESSION_SOURCES: &[&str] = &[
    // JSON API
    "en.dankefurslesen",
    "en.hivescans",
    "en.guya",
    // HTML scrape
    "ar.aasq",
    "en.asurascans",
    "en.flamecomics",
    "en.weebcentral",
];

pub mod fetcher;
pub mod imports;
pub mod models;
pub mod package;
pub mod registry;
pub mod resource;
pub mod settings;
pub mod source;
pub mod state;

#[cfg(test)]
mod regression_set_tests {
    use super::REGRESSION_SOURCES;
    use crate::package::supported_capabilities;
    use nyuka_domain::model::Capability;

    /// Both shapes, because the `abs:` bug was invisible in one of them.
    #[test]
    fn the_regression_set_covers_json_and_html_sources() {
        assert!(REGRESSION_SOURCES.contains(&"en.guya"), "a JSON API source");
        assert!(
            REGRESSION_SOURCES.contains(&"en.asurascans"),
            "an HTML-scraping source"
        );
        assert_eq!(REGRESSION_SOURCES.len(), 7);
    }

    #[test]
    fn the_regression_set_has_no_duplicates() {
        let unique: std::collections::HashSet<_> = REGRESSION_SOURCES.iter().collect();
        assert_eq!(unique.len(), REGRESSION_SOURCES.len());
    }

    /// The v1 commitment is tier 1, and this is what the install check
    /// enforces. If a capability is ever added to this list, the commitment
    /// in ADR-0004 has widened and the ADR needs to say so.
    #[test]
    fn the_default_build_provides_exactly_tier_one() {
        let provided = supported_capabilities();
        assert!(provided.contains(&Capability::Net));
        assert!(provided.contains(&Capability::Html));
        assert!(provided.contains(&Capability::Defaults));
        assert!(provided.contains(&Capability::Std));

        assert!(
            !provided.contains(&Capability::Canvas),
            "canvas is deferred; those sources are refused at install by name"
        );
        assert!(
            !provided.contains(&Capability::JsWebView),
            "a webview is not in v1"
        );

        #[cfg(not(feature = "js"))]
        assert!(
            !provided.contains(&Capability::JsContext),
            "js context is feature-gated and off by default: enabling it puts \
             a script engine in every deployment for two sources"
        );

        #[cfg(not(feature = "js"))]
        assert_eq!(
            provided.len(),
            4,
            "the default build provides exactly tier 1; a fifth capability \
             means the ADR-0004 commitment widened without the ADR saying so"
        );
    }
}
