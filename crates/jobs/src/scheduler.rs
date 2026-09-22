//! The scheduler: one Tokio task ticking each minute.
//!
//! Enqueues `check_follow` for due follows, `update_sources` daily, and the
//! maintenance kinds — `prune_jobs` (job-row retention, or the partial index
//! bloats), `prune_sessions` (ADR-0005), and `reconcile_library` (marks
//! `downloaded_chapter` rows whose files have gone missing, so the UI does not
//! offer a read that will fail — ADR-0007).
//!
//! Deferred downloads need no scheduler support: they are rows with a future
//! `run_at`.
