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
    eprintln!("  openapi    write web/openapi.json from the router's own schema");
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
