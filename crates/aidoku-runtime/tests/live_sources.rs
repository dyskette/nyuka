//! Regression check against real community sources.
//!
//! Ignored by default: it reaches the network and depends on sites that change
//! without notice. That is exactly why it cannot be the primary test — a
//! failure here does not distinguish "the host regressed" from "the site
//! changed its markup", which is what the conformance fixture exists to settle
//! (ADR-0004).
//!
//! It is still worth having as end-to-end evidence that the host runs real
//! packages, not just the fixture.
//!
//! ```sh
//! NYUKA_LIVE_AIX_DIR=/path/to/aix cargo test -p nyuka-aidoku-runtime \
//!     --test live_sources -- --ignored --nocapture
//! ```

use std::sync::Arc;

use nyuka_aidoku_runtime::engine::{Limits, Runtime};
use nyuka_aidoku_runtime::package::{self, WasmtimeImports};
use nyuka_aidoku_runtime::source::invoke;
use nyuka_aidoku_runtime::state::MemoryDefaults;

/// Every source in the directory must load, run, and return at least one entry.
#[test]
#[ignore = "reaches the network; set NYUKA_LIVE_AIX_DIR"]
fn real_sources_return_entries() {
    let Ok(dir) = std::env::var("NYUKA_LIVE_AIX_DIR") else {
        panic!("set NYUKA_LIVE_AIX_DIR to a directory of .aix packages");
    };
    let runtime = Runtime::new(Limits::default()).expect("engine");

    let mut checked = 0usize;
    let mut failures = Vec::new();

    let entries = std::fs::read_dir(&dir).expect("reading the .aix directory");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("aix") {
            continue;
        }
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bytes = std::fs::read(&path).expect("reading the package");

        let package = match package::load(&bytes, &WasmtimeImports(&runtime)) {
            Ok(p) => p,
            Err(e) => {
                // A capability refusal is a correct outcome, not a failure:
                // roughly 16% of the catalog needs canvas or a web view.
                eprintln!("  skip {name}: {e}");
                continue;
            }
        };
        checked += 1;

        let module = runtime.compile(&package.wasm).expect("module compiles");
        let mut source = match invoke(&runtime, &module, Arc::new(MemoryDefaults::default())) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{name}: instantiate failed: {e}"));
                continue;
            }
        };

        match source.search(None, 1, &[]) {
            Ok(result) => {
                let n = result.entries.len();
                eprintln!(
                    "  {name}: {n} entries, rate limit {:?}",
                    source.declared_rate_limit()
                );
                if n == 0 {
                    failures.push(format!("{name}: returned zero entries"));
                }
            }
            Err(e) => failures.push(format!("{name}: search failed: {e}")),
        }
    }

    assert!(checked > 0, "no runnable .aix packages found in {dir}");
    assert!(
        failures.is_empty(),
        "{} of {checked} sources failed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
