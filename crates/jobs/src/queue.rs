//! The `JobQueue` adapter.
//!
//! Hand-written against the SeaORM pool rather than adopting a queue crate.
//! The version analysis (ADR-0003): `apalis-sql` 0.7 needs `sqlx ^0.8.1`,
//! `sqlxmq` 0.6 and `underway` 0.2 likewise and are both stale.
//! `apalis-postgres` 1.0-rc *is* on `sqlx ^0.9` and is the one to revisit when
//! it ships stable — at which point this file becomes code to delete.
//!
//! # The claim query
//!
//! ```sql
//! UPDATE job SET state = 'running', locked_by = $1, locked_at = now()
//! WHERE id = (
//!     SELECT id FROM job
//!     WHERE state = 'queued' AND run_at <= now()
//!     ORDER BY priority, run_at
//!     FOR UPDATE SKIP LOCKED
//!     LIMIT 1
//! )
//! RETURNING *
//! ```
//!
//! Keep it in exactly one function. In SeaORM 2.0 the raw-SQL entry points are
//! `query_one_raw` / `query_all_raw` / `execute_raw`; the unsuffixed names now
//! take SeaQuery statements. `QuerySelect::lock_with_behavior(LockType::Update,
//! LockBehavior::SkipLocked)` expresses the lock through the DSL where that
//! reads better.
//!
//! This is the least type-checked code in the system. ADR-0003 requires a test
//! running N workers against a seeded table asserting every row is claimed
//! exactly once, plus one asserting a worker killed mid-job has its row
//! recovered.
