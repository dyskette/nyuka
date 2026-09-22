//! Repository adapters.
//!
//! Every method carries `#[tracing::instrument]` with the stable database
//! semantic conventions — `db.system.name`, `db.operation.name`,
//! `db.collection.name`, and `db.query.summary` as the span name (ADR-0015).
//! Skip arguments that may hold secrets or large payloads.
