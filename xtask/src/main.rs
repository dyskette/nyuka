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
        Some("abi-surface") => abi_surface(),
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
    eprintln!("  abi-surface      regenerate the pinned tier-1 import surface snapshot");
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
    let document = serde_json::to_value(nyuka_api::openapi_document())?;
    let json = serde_json::to_string_pretty(&document)?;
    let path = openapi_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{json}\n"))?;

    check_every_handler_is_routed(&document)?;

    println!("wrote {}", path.display());
    Ok(())
}

/// Fails when a handler carries `#[utoipa::path]` but is never mounted.
///
/// `utoipa-axum` documents only what the router registers, so a handler that
/// is written, annotated, and then left out of `routes!` produces no
/// operation — it is simply absent, with nothing to notice. That is how
/// `POST /sources` (installing a source) existed as dead code: the route test
/// checks that every *documented* path is routed, which cannot see a path
/// that was never documented in the first place.
///
/// Comparing the `operation_id` literals in the source against the generated
/// document catches the other direction. It is a text scan rather than
/// something the type system enforces, which is why it lives here and fails
/// the same command that writes the file.
fn check_every_handler_is_routed(document: &serde_json::Value) -> anyhow::Result<()> {
    let routed: std::collections::HashSet<String> = document["paths"]
        .as_object()
        .map(|paths| {
            paths
                .values()
                .filter_map(|item| item.as_object())
                .flat_map(|item| item.values())
                .filter_map(|op| op.get("operationId"))
                .filter_map(|id| id.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let routes_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/api/src/routes");
    let mut declared = Vec::new();
    for entry in std::fs::read_dir(&routes_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path)?;
        for line in source.lines() {
            let Some(rest) = line.trim().strip_prefix("operation_id = \"") else {
                continue;
            };
            let Some(id) = rest.split('"').next() else {
                continue;
            };
            declared.push((id.to_string(), path.clone()));
        }
    }

    let missing: Vec<String> = declared
        .iter()
        .filter(|(id, _)| !routed.contains(id))
        .map(|(id, path)| {
            format!(
                "{id} (in {})",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })
        .collect();

    anyhow::ensure!(
        missing.is_empty(),
        "these handlers are annotated but never mounted, so they are \
         unreachable and absent from the schema: {}",
        missing.join(", ")
    );

    println!("{} operations, all routed", routed.len());
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

// --- abi-surface -------------------------------------------------------------

/// Rewrites the snapshot of aidoku-rs' tier-1 host imports.
///
/// Reads the cargo git checkout the conformance fixture pins, so the snapshot
/// is a fact about `ABI_SOURCE_COMMIT` rather than a transcription. Run this
/// after re-pinning; the host's own test fails until `PROVIDED` covers it.
fn abi_surface() -> anyhow::Result<()> {
    let commit = nyuka_aidoku_runtime::ABI_SOURCE_COMMIT;
    let root = aidoku_checkout(commit)?;
    let imports = root.join("crates/lib/src/imports");

    let mut found: Vec<String> = Vec::new();
    for file in ["std.rs", "defaults.rs", "net.rs", "html.rs"] {
        let text = std::fs::read_to_string(imports.join(file))?;
        found.extend(extern_imports(&strip_comments(&text)));
    }
    found.sort();
    found.dedup();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/aidoku-runtime/abi/tier1-surface.txt");
    let header = format!(
        "# The tier-1 host import surface of aidoku-rs, at the commit in\n\
         # ABI_SOURCE_COMMIT ({commit}). Generated, not written by hand:\n\
         #\n\
         #   cargo xtask abi-surface\n\
         #\n\
         # `bindings::PROVIDED` must cover every line. ADR-0004 commits this\n\
         # project to tier 1 in full.\n\n"
    );
    std::fs::write(&path, format!("{header}{}\n", found.join("\n")))?;

    println!("wrote {} ({} imports)", path.display(), found.len());
    Ok(())
}

/// The cargo git checkout for the pinned aidoku-rs revision.
fn aidoku_checkout(commit: &str) -> anyhow::Result<PathBuf> {
    let home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let checkouts = home.join("git/checkouts");

    for entry in std::fs::read_dir(&checkouts)? {
        let dir = entry?.path();
        if !dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("aidoku-rs-"))
        {
            continue;
        }
        // Cargo names the directory by the short commit.
        for rev in std::fs::read_dir(&dir)? {
            let rev = rev?.path();
            if rev
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| commit.starts_with(n))
            {
                return Ok(rev);
            }
        }
    }

    anyhow::bail!(
        "no aidoku-rs checkout for {commit} under {}; build the conformance \
         fixture first (cargo build --manifest-path \
         crates/aidoku-runtime/conformance/Cargo.toml --target wasm32-unknown-unknown)",
        checkouts.display()
    )
}

fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `module::name` for every item in a `#[link(wasm_import_module = ...)]`
/// block, resolving `#[link_name]` to the name that crosses the boundary.
fn extern_imports(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;

    while let Some(at) = rest.find("#[link(wasm_import_module = \"") {
        let after = &rest[at + "#[link(wasm_import_module = \"".len()..];
        let Some(quote) = after.find('"') else { break };
        let module = &after[..quote];

        let Some(open) = after.find('{') else { break };
        let Some(close) = after[open..].find("\n}") else {
            break;
        };
        let body = &after[open + 1..open + close];

        let mut link_name: Option<String> = None;
        for line in body.lines() {
            let line = line.trim();
            if let Some(start) = line.find("link_name = \"") {
                let tail = &line[start + "link_name = \"".len()..];
                if let Some(end) = tail.find('"') {
                    link_name = Some(tail[..end].to_string());
                }
                continue;
            }
            if let Some(start) = line.find("fn ") {
                let tail = &line[start + 3..];
                let name: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push(format!("{module}::{}", link_name.take().unwrap_or(name)));
                }
            }
        }

        rest = &after[open + close..];
    }

    out
}
