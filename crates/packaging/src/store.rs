//! The `LibraryStore` implementation over a local volume.
//!
//! # Atomic placement
//!
//! Build the archive in a temp directory, fsync, then `rename`.
//!
//! `rename` is atomic **only within a single filesystem**, so the temp
//! directory must live *inside the library root* — not in `/tmp`. Otherwise
//! the rename silently degrades to a cross-mount copy and a crash mid-copy
//! leaves a truncated archive that looks like a completed download. ADR-0007
//! requires a test asserting the temp directory's location.
//!
//! A useful consequence: because nothing partial is ever visible in the
//! library, cancelling a running download only has to discard the temp
//! directory.
