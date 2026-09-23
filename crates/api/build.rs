//! Emits build metadata so `service.version` can carry a git SHA alongside the
//! crate version (ADR-0015).
//!
//! # Why the SHA is not from a vergen git feature
//!
//! vergen 10 moved git support into separate crates (`vergen-git2`,
//! `vergen-gitcl`), and both need a working repository at build time. A
//! release image is built from an exported tree with no `.git`, so either
//! would have to be configured to tolerate its own absence — at which point it
//! is doing less than the six lines below.
//!
//! Resolution order, so every build shape produces something useful:
//!
//! 1. `NYUKA_GIT_SHA` from the environment. This is what a container build
//!    passes in, and it is the only thing that works without a repository.
//! 2. `git rev-parse --short HEAD`, for a local build.
//! 3. `unknown`, which is honest rather than a fabricated zero SHA.

use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    vergen::Emitter::default()
        .add_instructions(&vergen::Build::all_build())?
        .add_instructions(&vergen::Cargo::all_cargo())?
        .emit()?;

    println!("cargo:rustc-env=NYUKA_GIT_SHA={}", git_sha());

    // Without this, a build in a different checkout reuses a cached SHA.
    println!("cargo:rerun-if-env-changed=NYUKA_GIT_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

fn git_sha() -> String {
    if let Ok(sha) = std::env::var("NYUKA_GIT_SHA")
        && !sha.trim().is_empty()
    {
        return sha.trim().to_string();
    }

    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
