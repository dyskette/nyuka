//! SeaORM entities and the repository adapters implementing `domain` ports.
//!
//! Entities are an implementation detail of this crate: they are mapped to
//! domain entities at the repository boundary and never exposed outward
//! (ADR-0002).
//!
//! # The drift check is not optional
//!
//! SeaORM type-checks queries against *entity definitions*, not against the
//! live schema. A migration that adds a non-null column without updating its
//! entity still compiles and fails at runtime. The CI job that runs migrations
//! on an ephemeral database, regenerates entities, and fails on any diff is
//! the control that makes this decision safe. See ADR-0002.
#![forbid(unsafe_code)]

pub mod connect;
/// SeaORM entities.
///
/// **Generated, not written.** `sea-orm-cli generate entity` produces these
/// from a migrated database, and CI regenerates and diffs them (ADR-0002).
/// Editing one by hand makes the drift check fail against something it cannot
/// reproduce — change the migration instead.
pub mod entity;
pub mod migration;
pub mod repository;
pub mod session;
