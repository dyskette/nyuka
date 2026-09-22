//! Emits build metadata so `service.version` can carry a git SHA alongside the
//! crate version (ADR-0015).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // TODO(scaffold): wire vergen's build and cargo emitters, plus a git SHA.
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}
