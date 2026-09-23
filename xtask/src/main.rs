//! Repository tasks that need the workspace's own code.
//!
//! `cargo xtask <task>`, wired through `.cargo/config.toml`. The pattern
//! exists so a task that must import a crate — writing the OpenAPI document
//! means calling the router's own builder — is an ordinary binary rather than
//! a shell script that reimplements it.

#![forbid(unsafe_code)]

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("openapi") => openapi(),
        Some("fetch-sources") => fetch_sources(),
        Some(other) => {
            eprintln!("unknown task `{other}`");
            usage();
            std::process::exit(2);
        }
        None => {
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!("tasks:");
    eprintln!("  openapi          write web/openapi.json from the router's own schema");
    eprintln!("  fetch-sources    download the ADR-0004 regression set of .aix packages");
}

/// Where the document lives.
///
/// Committed, because it is the frontend's contract: the generated TypeScript
/// client is produced from it, and a developer running `npm run generate`
/// should not need a database and a Rust toolchain to do so.
fn openapi_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/openapi.json")
}

fn openapi() -> anyhow::Result<()> {
    // Pretty-printed with a trailing newline, so `git diff` on a schema
    // change shows the changed endpoint rather than one enormous line.
    let json = serde_json::to_string_pretty(&nyuka_api::openapi_document())?;
    let path = openapi_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{json}\n"))?;

    println!("wrote {}", path.display());
    Ok(())
}

/// The community index the regression set is published in.
///
/// Pinned by URL rather than vendored: ADR-0004 commits to a capability, not
/// to specific package versions, so the live run should exercise whatever
/// upstream publishes today. Vendoring would redistribute someone else's
/// compiled code and freeze it at whatever version was copied.
const INDEX_URL: &str =
    "https://raw.githubusercontent.com/Aidoku-Community/sources/gh-pages/index.min.json";

#[derive(serde::Deserialize)]
struct Index {
    #[serde(default)]
    sources: Vec<IndexSource>,
}

#[derive(serde::Deserialize)]
struct IndexSource {
    id: String,
    #[serde(rename = "downloadURL")]
    download_url: String,
}

/// Downloads the regression set into a directory the live test can read.
///
/// Writes to `target/aix` by default, or to `$1`. The live test is driven by
/// `NYUKA_LIVE_AIX_DIR`, so the scheduled job is this task followed by that
/// test.
fn fetch_sources() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/aix"));
    std::fs::create_dir_all(&out)?;
    // Canonical, because the printed command is meant to be pasted and the
    // test runs with its own crate as the working directory.
    let out = out.canonicalize().unwrap_or(out);

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("nyuka-xtask/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let index: Index = client.get(INDEX_URL).send()?.error_for_status()?.json()?;
    let base = url::Url::parse(INDEX_URL)?;

    let wanted = nyuka_aidoku_runtime::REGRESSION_SOURCES;
    let mut fetched = 0;
    let mut missing = Vec::new();

    for id in wanted {
        let Some(entry) = index.sources.iter().find(|s| s.id == *id) else {
            // Upstream removed or renamed it. Reported rather than fatal: the
            // commitment is to a capability, and a source disappearing from
            // the community index is news, not a build failure.
            missing.push(*id);
            continue;
        };

        let url = base.join(&entry.download_url)?;
        let bytes = client
            .get(url.clone())
            .send()?
            .error_for_status()?
            .bytes()?;
        let path = out.join(format!("{id}.aix"));
        std::fs::write(&path, &bytes)?;
        println!("{id:<22} {:>8} bytes", bytes.len());
        fetched += 1;
    }

    if !missing.is_empty() {
        eprintln!();
        eprintln!(
            "warning: {} of {} regression sources are no longer in the index: {}",
            missing.len(),
            wanted.len(),
            missing.join(", ")
        );
        eprintln!("ADR-0004's regression set needs revisiting.");
    }

    println!();
    println!("wrote {fetched} package(s) to {}", out.display());
    println!(
        "run: NYUKA_LIVE_AIX_DIR={} cargo test -p nyuka-aidoku-runtime \\",
        out.display()
    );
    println!("       --test live_sources -- --ignored --nocapture");
    Ok(())
}
