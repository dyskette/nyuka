//! Entities, value objects, ports, and use cases.
//!
//! This crate performs no I/O. Every outward interaction is expressed as a
//! trait in [`ports`], implemented by an adapter crate and composed in
//! `nyuka-api`'s `main.rs`.
//!
//! See `docs/adr/README.md` for the decisions this structure follows.
#![forbid(unsafe_code)]

pub mod error;
pub mod model;
pub mod ports;

pub use error::{DomainError, Result};
