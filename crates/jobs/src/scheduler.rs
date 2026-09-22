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
//!
//! # Periods come from idempotency keys, not from timer accuracy
//!
//! Every enqueue carries a key naming the period it belongs to, so "daily"
//! means one row per day rather than one row per tick that happened to look
//! due. A missed tick, a restart, a clock jump, or two ticks landing in the
//! same second all collapse to the same key, and the queue's
//! `ON CONFLICT (idempotency_key)` turns the duplicate into a no-op.
//!
//! > [!IMPORTANT]
//! > This makes job-row retention load-bearing. The key only suppresses a
//! > duplicate while the earlier row still exists, so `prune_jobs` must keep
//! > terminal rows for longer than the longest period here. `check_retention`
//! > asserts that, because the failure is silent: a schedule that quietly
//! > starts firing twice looks exactly like one that works.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nyuka_domain::model::{FollowId, JobKind};
use nyuka_domain::{DomainError, Result};
use sea_orm::{ConnectionTrait, Statement};
use tokio_util::sync::CancellationToken;

use crate::queue::{PostgresQueue, kind_to_str};

/// A job kind the scheduler enqueues on a fixed period.
#[derive(Debug, Clone, Copy)]
pub struct Periodic {
    pub kind: JobKind,
    pub every: Duration,
    /// Maintenance runs behind user-visible work, so its priority is higher
    /// (the queue orders ascending).
    pub priority: i16,
    pub max_attempts: i32,
}

pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);
pub const HOUR: Duration = Duration::from_secs(60 * 60);

/// The default schedule.
pub fn default_schedule() -> Vec<Periodic> {
    vec![
        Periodic {
            kind: JobKind::UpdateSources,
            every: DAY,
            priority: 10,
            max_attempts: 3,
        },
        Periodic {
            kind: JobKind::PruneJobs,
            every: DAY,
            priority: 20,
            max_attempts: 1,
        },
        Periodic {
            kind: JobKind::PruneSessions,
            every: HOUR,
            priority: 20,
            max_attempts: 1,
        },
        Periodic {
            kind: JobKind::ReconcileLibrary,
            every: DAY,
            priority: 20,
            max_attempts: 1,
        },
    ]
}

/// Names the period an instant falls in, so every tick inside one period
/// produces the same key.
///
/// Buckets are absolute rather than relative to the last run: the boundary
/// does not drift with restarts, and a process that was down for a day
/// enqueues one catch-up run rather than one per missed tick.
pub fn period_key(prefix: &str, every: Duration, now: DateTime<Utc>) -> String {
    let secs = every.as_secs().max(1);
    let bucket = now.timestamp().div_euclid(secs as i64);
    format!("{prefix}:{bucket}")
}

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub tick: Duration,
    /// How long a `running` row may sit untouched before it is assumed to
    /// belong to a process that died without draining.
    pub stale_after: Duration,
    /// Ceiling on follows enqueued per tick, so a library with thousands of
    /// follows cannot flood the queue in one pass.
    pub follow_batch: u64,
    pub schedule: Vec<Periodic>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(60),
            stale_after: Duration::from_secs(900),
            follow_batch: 200,
            schedule: default_schedule(),
        }
    }
}

/// What one tick did, for logging and for tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    /// Keyed by job kind.
    pub enqueued: HashMap<&'static str, usize>,
    pub follows: usize,
    pub recovered: u64,
}

pub struct Scheduler {
    queue: Arc<PostgresQueue>,
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(queue: Arc<PostgresQueue>, config: SchedulerConfig) -> Self {
        Self { queue, config }
    }

    /// Ticks until cancelled.
    ///
    /// The first tick runs immediately: recovering rows stranded by a hard
    /// kill is startup work, and waiting a full interval to do it means the
    /// queue looks stalled for that long.
    pub async fn run(self, cancel: CancellationToken) {
        loop {
            match self.tick().await {
                Ok(report) => {
                    if report.follows > 0 || !report.enqueued.is_empty() || report.recovered > 0 {
                        tracing::info!(
                            follows = report.follows,
                            recovered = report.recovered,
                            enqueued = ?report.enqueued,
                            "scheduler tick"
                        );
                    }
                }
                // A tick that fails must not end the scheduler: the next one
                // enqueues the same period key, so nothing is lost.
                Err(e) => tracing::warn!(error = %e, "scheduler tick failed"),
            }

            tokio::select! {
                _ = tokio::time::sleep(self.config.tick) => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    /// One pass. Public so the behaviour is testable without waiting a minute.
    pub async fn tick(&self) -> Result<TickReport> {
        let now = Utc::now();
        let mut report = TickReport {
            recovered: self.queue.recover_stale(self.config.stale_after).await?,
            ..Default::default()
        };

        for periodic in &self.config.schedule {
            let kind = kind_to_str(periodic.kind);
            let key = period_key(kind, periodic.every, now);
            let before = self.queue.exists(&key).await?;
            self.queue
                .enqueue(
                    periodic.kind,
                    serde_json::json!({}),
                    None,
                    Some(&key),
                    periodic.priority,
                    periodic.max_attempts,
                )
                .await?;
            if !before {
                *report.enqueued.entry(kind).or_default() += 1;
            }
        }

        for follow in self.due_follows().await? {
            // Bucketed by the follow's own interval, so a follow checked every
            // six hours is enqueued at most once in six hours even though the
            // scheduler looks at it sixty times an hour.
            let every = Duration::from_secs(follow.check_interval_secs.max(1) as u64);
            let key = period_key(&format!("check_follow:{}", follow.id.0), every, now);
            let before = self.queue.exists(&key).await?;
            self.queue
                .enqueue(
                    JobKind::CheckFollow,
                    serde_json::json!({ "follow_id": follow.id.0 }),
                    None,
                    Some(&key),
                    5,
                    3,
                )
                .await?;
            if !before {
                report.follows += 1;
            }
        }

        Ok(report)
    }

    /// Follows whose interval has elapsed.
    ///
    /// `last_checked_at` is written by the `check_follow` handler on success,
    /// not here: marking a follow checked because a job was *enqueued* would
    /// skip a whole interval whenever that job failed.
    async fn due_follows(&self) -> Result<Vec<DueFollow>> {
        let sql = "SELECT id, check_interval_secs FROM follow \
                   WHERE last_checked_at IS NULL \
                      OR last_checked_at + make_interval(secs => check_interval_secs) <= now() \
                   ORDER BY last_checked_at NULLS FIRST \
                   LIMIT $1";
        let db = self.queue.connection();
        let rows = db
            .query_all_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                sql,
                [(self.config.follow_batch as i64).into()],
            ))
            .await
            .map_err(|e| DomainError::Storage(e.to_string()))?;

        rows.into_iter()
            .map(|row| {
                Ok(DueFollow {
                    id: FollowId(
                        row.try_get("", "id")
                            .map_err(|e| DomainError::Storage(e.to_string()))?,
                    ),
                    check_interval_secs: row
                        .try_get("", "check_interval_secs")
                        .map_err(|e| DomainError::Storage(e.to_string()))?,
                })
            })
            .collect()
    }
}

struct DueFollow {
    id: FollowId,
    check_interval_secs: i32,
}

/// Fails when job-row retention is shorter than the longest schedule period.
///
/// See the module note: once a period's row is pruned, its key stops
/// suppressing duplicates and the schedule silently starts firing early.
pub fn check_retention(retention: chrono::Duration, schedule: &[Periodic]) -> Result<()> {
    let longest = schedule
        .iter()
        .map(|p| p.every)
        .max()
        .unwrap_or(Duration::ZERO);
    let longest = chrono::Duration::from_std(longest).unwrap_or(chrono::Duration::zero());
    if retention <= longest {
        return Err(DomainError::Invalid(format!(
            "job retention of {} hours is not longer than the longest schedule period of {} hours; \
             pruned rows would let that schedule fire more than once per period",
            retention.num_hours(),
            longest.num_hours()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ts: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(ts, 0).expect("timestamp")
    }

    #[test]
    fn every_instant_in_a_period_produces_one_key() {
        let start = at(1_758_499_200); // a day boundary
        assert_eq!(
            period_key("prune_jobs", DAY, start),
            period_key("prune_jobs", DAY, start + chrono::Duration::hours(23)),
            "two ticks the same day must collapse to one enqueue"
        );
    }

    #[test]
    fn the_next_period_produces_a_different_key() {
        let start = at(1_758_499_200);
        assert_ne!(
            period_key("prune_jobs", DAY, start),
            period_key("prune_jobs", DAY, start + chrono::Duration::days(1)),
        );
    }

    /// The point of absolute buckets: a process down across several periods
    /// comes back to one catch-up run, not a backlog of them.
    #[test]
    fn a_long_outage_does_not_accumulate_a_backlog() {
        let start = at(1_758_499_200);
        let keys: Vec<_> = (0..5)
            .map(|h| {
                period_key(
                    "prune_sessions",
                    HOUR,
                    start + chrono::Duration::minutes(h * 3),
                )
            })
            .collect();
        assert!(
            keys.windows(2).all(|w| w[0] == w[1]),
            "ticks within the hour share a key regardless of when the process started"
        );
    }

    #[test]
    fn kinds_do_not_collide_within_a_period() {
        let now = at(1_758_499_200);
        assert_ne!(
            period_key("prune_jobs", DAY, now),
            period_key("update_sources", DAY, now)
        );
    }

    #[test]
    fn a_zero_period_does_not_divide_by_zero() {
        let _ = period_key("x", Duration::ZERO, at(1_758_499_200));
    }

    #[test]
    fn retention_shorter_than_the_longest_period_is_rejected() {
        let err = check_retention(chrono::Duration::hours(12), &default_schedule())
            .expect_err("12h retention cannot protect a daily schedule");
        assert!(err.to_string().contains("more than once per period"));
    }

    #[test]
    fn retention_equal_to_the_longest_period_is_rejected() {
        assert!(
            check_retention(chrono::Duration::days(1), &default_schedule()).is_err(),
            "equal is not enough: the row can be pruned the moment the period turns over"
        );
    }

    #[test]
    fn the_default_retention_window_is_long_enough() {
        check_retention(chrono::Duration::days(7), &default_schedule()).expect("7 days");
    }
}
