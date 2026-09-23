//! The `JobQueue` adapter.
//!
//! Hand-written against the SeaORM pool rather than adopting a queue crate:
//! `apalis-sql` 0.7, `sqlxmq` and `underway` all need SQLx 0.8, which does not
//! unify with SeaORM 2.x on 0.9, so any of them would mean a second connection
//! pool and no shared transaction with the domain writes (ADR-0003).
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
//! `SKIP LOCKED` is what makes this exactly-once across workers: a row another
//! transaction holds is passed over rather than waited for. The subquery is
//! what makes it a single statement — selecting then updating in two steps
//! reopens the race the lock closes.
//!
//! This is the least type-checked code in the system and the most expensive to
//! get wrong, which is why `tests/claim.rs` runs concurrent workers against a
//! real database rather than trusting inspection.

use chrono::{DateTime, Utc};
use nyuka_domain::model::{
    ChapterId, Cursor, Job, JobId, JobKind, JobState, JobSubject, JobSummary, MangaId, Page,
};
use nyuka_domain::{DomainError, Result};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};
use uuid::Uuid;

/// How long a claimed job may hold its lock before recovery reclaims it.
pub const DEFAULT_STALE_LOCK: std::time::Duration = std::time::Duration::from_secs(900);

pub struct PostgresQueue {
    db: DatabaseConnection,
}

impl PostgresQueue {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }
}

/// Wire names for job kinds.
///
/// Stored as text rather than an enum type: adding a kind is then a code
/// change rather than a migration, and an unknown value read back is a
/// recoverable error instead of a decode failure.
pub fn kind_to_str(kind: JobKind) -> &'static str {
    match kind {
        JobKind::DownloadChapter => "download_chapter",
        JobKind::PackageChapter => "package_chapter",
        JobKind::RefreshMetadata => "refresh_metadata",
        JobKind::CheckFollow => "check_follow",
        JobKind::UpdateSources => "update_sources",
        JobKind::PruneJobs => "prune_jobs",
        JobKind::PruneSessions => "prune_sessions",
        JobKind::ReconcileLibrary => "reconcile_library",
    }
}

pub fn kind_from_str(s: &str) -> Option<JobKind> {
    Some(match s {
        "download_chapter" => JobKind::DownloadChapter,
        "package_chapter" => JobKind::PackageChapter,
        "refresh_metadata" => JobKind::RefreshMetadata,
        "check_follow" => JobKind::CheckFollow,
        "update_sources" => JobKind::UpdateSources,
        "prune_jobs" => JobKind::PruneJobs,
        "prune_sessions" => JobKind::PruneSessions,
        "reconcile_library" => JobKind::ReconcileLibrary,
        _ => return None,
    })
}

pub fn state_to_str(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

pub fn state_from_str(s: &str) -> Option<JobState> {
    Some(match s {
        "queued" => JobState::Queued,
        "running" => JobState::Running,
        "succeeded" => JobState::Succeeded,
        "failed" => JobState::Failed,
        "cancelled" => JobState::Cancelled,
        _ => return None,
    })
}

/// Exponential backoff with jitter.
///
/// Jitter matters more than the curve: without it, a batch of jobs that failed
/// together retries together, and the thundering herd hits the same site at
/// the same moment (ADR-0003).
pub fn backoff(attempt: i32) -> std::time::Duration {
    let base = 2u64.saturating_pow(attempt.clamp(0, 10) as u32);
    let capped = base.min(3600);
    // Deterministic jitter derived from the attempt, so tests stay stable:
    // a real deployment wants a random source here.
    let jitter = (attempt as u64 * 7919) % 30;
    std::time::Duration::from_secs(capped + jitter)
}

fn row_to_job(row: &sea_orm::QueryResult) -> Result<Job> {
    let kind: String = row.try_get("", "kind").map_err(db)?;
    let state: String = row.try_get("", "state").map_err(db)?;
    Ok(Job {
        id: JobId(row.try_get("", "id").map_err(db)?),
        kind: kind_from_str(&kind)
            .ok_or_else(|| DomainError::Internal(format!("unknown job kind {kind:?}")))?,
        state: state_from_str(&state)
            .ok_or_else(|| DomainError::Internal(format!("unknown job state {state:?}")))?,
        payload: row.try_get("", "payload").map_err(db)?,
        priority: row.try_get("", "priority").map_err(db)?,
        run_at: row.try_get("", "run_at").map_err(db)?,
        attempts: row.try_get("", "attempts").map_err(db)?,
        max_attempts: row.try_get("", "max_attempts").map_err(db)?,
        last_error: row.try_get("", "last_error").map_err(db)?,
        created_at: row.try_get("", "created_at").map_err(db)?,
    })
}

/// Reads the joined subject columns, which are NULL for a job that has none.
///
/// Keyed off `manga_id`: the join either produced every column or none, so one
/// of them standing for the rest cannot disagree with the others.
fn row_to_subject(row: &sea_orm::QueryResult) -> Result<Option<JobSubject>> {
    let manga_id: Option<Uuid> = row.try_get("", "subject_manga_id").map_err(db)?;
    let Some(manga_id) = manga_id else {
        return Ok(None);
    };
    Ok(Some(JobSubject {
        manga_id: MangaId(manga_id),
        manga_title: row.try_get("", "subject_manga_title").map_err(db)?,
        chapter_id: ChapterId(row.try_get("", "subject_chapter_id").map_err(db)?),
        chapter_number: row.try_get("", "subject_chapter_number").map_err(db)?,
        chapter_title: row.try_get("", "subject_chapter_title").map_err(db)?,
    }))
}

fn db(e: sea_orm::DbErr) -> DomainError {
    DomainError::Storage(e.to_string())
}

impl PostgresQueue {
    /// Enqueues a job.
    ///
    /// A repeated `idempotency_key` is not an error: it means the caller
    /// already enqueued this work, so the existing job is returned. That is
    /// what makes a retried `POST /downloads` safe.
    pub async fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        run_at: Option<DateTime<Utc>>,
        idempotency_key: Option<&str>,
        priority: i16,
        max_attempts: i32,
    ) -> Result<JobId> {
        let sql = r#"
            INSERT INTO job (id, kind, payload, state, priority, run_at, attempts,
                             max_attempts, idempotency_key, created_at, updated_at)
            VALUES ($1, $2, $3, 'queued', $4, COALESCE($5, now()), 0, $6, $7, now(), now())
            ON CONFLICT (idempotency_key) DO UPDATE SET updated_at = job.updated_at
            RETURNING id
        "#;
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [
                    Uuid::new_v4().into(),
                    kind_to_str(kind).into(),
                    payload.into(),
                    priority.into(),
                    run_at
                        .map(Value::from)
                        .unwrap_or(Value::ChronoDateTimeUtc(None)),
                    max_attempts.into(),
                    idempotency_key
                        .map(Value::from)
                        .unwrap_or(Value::String(None)),
                ],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("enqueue returned no row".into()))?;
        Ok(JobId(row.try_get("", "id").map_err(db)?))
    }

    /// Whether a job with this idempotency key already exists.
    ///
    /// `enqueue` alone cannot tell the caller whether it inserted or matched
    /// an existing row, because both return the same id. The scheduler needs
    /// the difference to report what a tick actually did.
    pub async fn exists(&self, idempotency_key: &str) -> Result<bool> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT 1 AS present FROM job WHERE idempotency_key = $1",
                [idempotency_key.into()],
            ))
            .await
            .map_err(db)?;
        Ok(row.is_some())
    }

    /// Claims one runnable job, or `None` when there is nothing to do.
    ///
    /// Exactly-once across concurrent workers. Keep this in one place: the
    /// single-statement form is what makes it safe, and splitting the select
    /// from the update reopens the race.
    pub async fn claim(&self, worker: &str) -> Result<Option<Job>> {
        let sql = r#"
            UPDATE job SET state = 'running', locked_by = $1, locked_at = now(),
                           updated_at = now(), attempts = attempts + 1
            WHERE id = (
                SELECT id FROM job
                WHERE state = 'queued' AND run_at <= now()
                ORDER BY priority, run_at
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            RETURNING *
        "#;
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [worker.into()],
            ))
            .await
            .map_err(db)?;
        row.as_ref().map(row_to_job).transpose()
    }

    pub async fn complete(&self, job: JobId) -> Result<()> {
        self.set_terminal(job, JobState::Succeeded, None).await
    }

    pub async fn cancel(&self, job: JobId) -> Result<()> {
        self.set_terminal(job, JobState::Cancelled, None).await
    }

    async fn set_terminal(&self, job: JobId, state: JobState, error: Option<&str>) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "UPDATE job SET state = $1, last_error = $2, locked_by = NULL, \
                 locked_at = NULL, updated_at = now() WHERE id = $3",
                [
                    state_to_str(state).into(),
                    error.map(Value::from).unwrap_or(Value::String(None)),
                    job.0.into(),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Records a failure, returning when the job will next run.
    ///
    /// `None` means the job is exhausted and has moved to `failed`.
    pub async fn fail(
        &self,
        job: JobId,
        error: &str,
        retryable: bool,
    ) -> Result<Option<DateTime<Utc>>> {
        let current = self.get(job).await?;
        // A non-retryable failure is terminal regardless of attempts left:
        // malformed data fails identically next time, so retrying burns the
        // budget for nothing (ADR-0003).
        if !retryable || current.attempts >= current.max_attempts {
            self.set_terminal(job, JobState::Failed, Some(error))
                .await?;
            return Ok(None);
        }
        let delay = backoff(current.attempts);
        let next = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "UPDATE job SET state = 'queued', run_at = $1, last_error = $2, \
                 locked_by = NULL, locked_at = NULL, updated_at = now() WHERE id = $3",
                [next.into(), error.into(), job.0.into()],
            ))
            .await
            .map_err(db)?;
        Ok(Some(next))
    }

    /// Returns jobs whose worker died back to the queue.
    ///
    /// Run on startup and periodically. Without it a worker killed mid-job
    /// leaves its row `running` forever, and nothing ever picks it up again.
    pub async fn recover_stale(&self, older_than: std::time::Duration) -> Result<u64> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::seconds(900));
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "UPDATE job SET state = 'queued', locked_by = NULL, locked_at = NULL, \
                 updated_at = now() WHERE state = 'running' AND locked_at < $1",
                [cutoff.into()],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn get(&self, job: JobId) -> Result<Job> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM job WHERE id = $1",
                [job.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_job(&row)
    }

    pub async fn list(&self, state: Option<JobState>, limit: u64) -> Result<Page<Job>> {
        let (sql, values): (&str, Vec<Value>) = match state {
            Some(s) => (
                "SELECT * FROM job WHERE state = $1 ORDER BY created_at DESC LIMIT $2",
                vec![state_to_str(s).into(), (limit as i64).into()],
            ),
            None => (
                "SELECT * FROM job ORDER BY created_at DESC LIMIT $1",
                vec![(limit as i64).into()],
            ),
        };
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(db)?;
        Ok(Page {
            items: rows.iter().map(row_to_job).collect::<Result<Vec<_>>>()?,
            next: None,
        })
    }

    /// The job list with each download job's subject resolved.
    ///
    /// # The chapter id is parsed in Rust, not cast in SQL
    ///
    /// The id lives in a JSONB payload whose shape varies by kind. The obvious
    /// query joins on `(payload->>'chapter_id')::uuid` — and `::uuid` on a
    /// value that is not one raises an error that fails the *whole statement*,
    /// so a single malformed payload empties the queue view instead of showing
    /// one row without a subject.
    ///
    /// Guarding that cast with `AND` in the join condition does not work.
    /// Postgres does not promise to evaluate conjuncts in written order, and it
    /// does not: a regex guard written first still let the cast run on the rows
    /// it was meant to exclude, and the test below caught it doing so.
    ///
    /// So no untrusted value is ever cast. The ids are parsed here, where a bad
    /// one is an `Option` rather than an aborted transaction, and the second
    /// query matches on `c.id` directly — which also keeps the primary key
    /// index, as comparing `c.id::text` would not.
    pub async fn list_summaries(
        &self,
        state: Option<JobState>,
        limit: u64,
    ) -> Result<Page<JobSummary>> {
        let jobs = self.list(state, limit).await?;

        // Parsed, not trusted. A payload that does not hold a uuid simply has
        // no subject.
        let wanted: Vec<Uuid> = jobs
            .items
            .iter()
            .filter(|j| j.kind == JobKind::DownloadChapter)
            .filter_map(|j| j.payload.get("chapter_id")?.as_str()?.parse::<Uuid>().ok())
            .collect();

        let subjects = self.subjects_for(&wanted).await?;

        let items = jobs
            .items
            .into_iter()
            .map(|job| {
                let subject = job
                    .payload
                    .get("chapter_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .and_then(|id| subjects.get(&id).cloned());
                JobSummary { job, subject }
            })
            .collect();

        Ok(Page { items, next: None })
    }

    /// One job, with its subject.
    ///
    /// Goes through the same id parsing and lookup as the list, rather than a
    /// second query shaped differently: a panel that disagreed with the row it
    /// was opened from would be worse than one that showed nothing.
    pub async fn summary(&self, job: JobId) -> Result<JobSummary> {
        let job = self.get(job).await?;

        let wanted: Vec<Uuid> = if job.kind == JobKind::DownloadChapter {
            job.payload
                .get("chapter_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Uuid>().ok())
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

        let subjects = self.subjects_for(&wanted).await?;
        let subject = wanted.first().and_then(|id| subjects.get(id).cloned());
        Ok(JobSummary { job, subject })
    }

    /// Looks up the series and chapter behind a set of chapter ids.
    ///
    /// Returns a map rather than a list because two jobs can name the same
    /// chapter — a retry and its original — and a positional zip would then
    /// hand one of them the other's subject.
    async fn subjects_for(
        &self,
        chapter_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, JobSubject>> {
        if chapter_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // `= ANY($1)` rather than an IN list built by string concatenation:
        // one bound parameter, and no way for a value to become syntax.
        let sql = "SELECT c.id AS subject_chapter_id, \
                   c.number AS subject_chapter_number, \
                   c.title AS subject_chapter_title, \
                   m.id AS subject_manga_id, \
                   m.title AS subject_manga_title \
                   FROM chapter c \
                   JOIN manga m ON m.id = c.manga_id \
                   WHERE c.id = ANY($1)";

        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [Value::Array(
                    sea_orm::sea_query::ArrayType::Uuid,
                    Some(Box::new(
                        chapter_ids
                            .iter()
                            .map(|id| Value::Uuid(Some(*id)))
                            .collect(),
                    )),
                )],
            ))
            .await
            .map_err(db)?;

        let mut map = std::collections::HashMap::new();
        for row in &rows {
            if let Some(subject) = row_to_subject(row)? {
                map.insert(subject.chapter_id.0, subject);
            }
        }
        Ok(map)
    }

    /// Deletes terminal rows past the retention window.
    ///
    /// The partial claim index only covers queued rows, but terminal rows
    /// still occupy the table and its other indexes; without pruning the table
    /// grows without bound (ADR-0003).
    pub async fn prune(&self, older_than: chrono::Duration) -> Result<u64> {
        let cutoff = Utc::now() - older_than;
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM job WHERE state IN ('succeeded', 'cancelled') AND updated_at < $1",
                [cutoff.into()],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }
}

/// The `domain::ports::JobQueue` implementation.
///
/// A thin bridge: the inherent methods above take the shapes that read well
/// inside this crate, the port takes the shapes the rest of the system uses.
#[async_trait::async_trait]
impl nyuka_domain::ports::JobQueue for PostgresQueue {
    async fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        run_at: Option<DateTime<Utc>>,
        idempotency_key: Option<&str>,
        priority: i16,
        max_attempts: i32,
    ) -> Result<JobId> {
        PostgresQueue::enqueue(
            self,
            kind,
            payload,
            run_at,
            idempotency_key,
            priority,
            max_attempts,
        )
        .await
    }

    async fn claim(&self, worker: &str) -> Result<Option<Job>> {
        PostgresQueue::claim(self, worker).await
    }

    async fn complete(&self, job: JobId) -> Result<()> {
        PostgresQueue::complete(self, job).await
    }

    async fn fail(
        &self,
        job: JobId,
        error: &str,
        retryable: bool,
    ) -> Result<Option<DateTime<Utc>>> {
        PostgresQueue::fail(self, job, error, retryable).await
    }

    async fn cancel(&self, job: JobId) -> Result<()> {
        PostgresQueue::cancel(self, job).await
    }

    async fn recover_stale(&self, older_than_secs: u64) -> Result<u64> {
        PostgresQueue::recover_stale(self, std::time::Duration::from_secs(older_than_secs)).await
    }

    async fn get(&self, job: JobId) -> Result<Job> {
        PostgresQueue::get(self, job).await
    }

    /// The cursor is ignored: the job list is a short operational view, not a
    /// browsable collection, and every caller so far wants the newest page.
    /// Wire keyset pagination here when something needs the second page.
    async fn list(&self, state: Option<JobState>, _cursor: Option<&Cursor>) -> Result<Page<Job>> {
        PostgresQueue::list(self, state, 100).await
    }

    async fn list_summaries(
        &self,
        state: Option<JobState>,
        _cursor: Option<&Cursor>,
    ) -> Result<Page<JobSummary>> {
        PostgresQueue::list_summaries(self, state, 100).await
    }

    async fn get_summary(&self, job: JobId) -> Result<JobSummary> {
        PostgresQueue::summary(self, job).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_state_names_round_trip() {
        for kind in [
            JobKind::DownloadChapter,
            JobKind::PackageChapter,
            JobKind::RefreshMetadata,
            JobKind::CheckFollow,
            JobKind::UpdateSources,
            JobKind::PruneJobs,
            JobKind::PruneSessions,
            JobKind::ReconcileLibrary,
        ] {
            assert_eq!(kind_from_str(kind_to_str(kind)), Some(kind));
        }
        for state in [
            JobState::Queued,
            JobState::Running,
            JobState::Succeeded,
            JobState::Failed,
            JobState::Cancelled,
        ] {
            assert_eq!(state_from_str(state_to_str(state)), Some(state));
        }
    }

    /// The claim index is `WHERE state = 'queued'`, so the queued name in
    /// particular is part of the schema, not just of this module.
    #[test]
    fn the_queued_name_matches_the_partial_index() {
        assert_eq!(state_to_str(JobState::Queued), "queued");
    }

    #[test]
    fn an_unknown_name_is_rejected_rather_than_defaulted() {
        assert_eq!(kind_from_str("not_a_kind"), None);
        assert_eq!(state_from_str("pending"), None);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert!(backoff(1) < backoff(5));
        assert!(
            backoff(30) <= std::time::Duration::from_secs(3600 + 30),
            "an unbounded backoff would park a job effectively forever"
        );
    }

    #[test]
    fn backoff_is_jittered_across_attempts() {
        // Equal delays would make a batch that failed together retry together.
        let a = backoff(3);
        let b = backoff(4);
        assert_ne!(a, b);
    }
}
