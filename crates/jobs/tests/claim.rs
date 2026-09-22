//! The claim query, under concurrency, against a real PostgreSQL.
//!
//! ADR-0003 calls this the least type-checked code in the system and the most
//! expensive to get wrong: a claim that is not exactly-once means two workers
//! download the same chapter, double the request rate toward a site that bans
//! for exactly that, and race each other writing the same file.
//!
//! Nothing in the type system says `FOR UPDATE SKIP LOCKED` is correct, and a
//! single-threaded test would pass against a completely broken implementation.
//! So these run real concurrent workers.
//!
//! Skipped when `DATABASE_URL` is unset.

use std::collections::HashSet;
use std::sync::Arc;

use nyuka_domain::model::{JobKind, JobState};
use nyuka_jobs::queue::PostgresQueue;
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run claim tests");
        None
    })?;
    let admin = Database::connect(&base).await.expect("connecting");
    for sql in [
        format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        format!("CREATE DATABASE {name}"),
    ] {
        admin
            .execute_raw(Statement::from_string(admin.get_database_backend(), sql))
            .await
            .expect("preparing the test database");
    }
    let mut parsed = url::Url::parse(&base).expect("DATABASE_URL is a url");
    parsed.set_path(name);
    let db = Database::connect(parsed.as_str())
        .await
        .expect("connecting");
    Migrator::fresh(&db).await.expect("migrating");
    Some(db)
}

/// The central property: N jobs, W concurrent workers, every job claimed
/// exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn every_job_is_claimed_exactly_once() {
    let Some(db) = fresh_database("nyuka_test_claim_once").await else {
        return;
    };
    const JOBS: usize = 200;
    const WORKERS: usize = 8;

    let queue = Arc::new(PostgresQueue::new(db));
    for i in 0..JOBS {
        queue
            .enqueue(
                JobKind::DownloadChapter,
                serde_json::json!({ "n": i }),
                None,
                None,
                0,
                5,
            )
            .await
            .expect("enqueue");
    }

    let mut workers = Vec::new();
    for w in 0..WORKERS {
        let queue = queue.clone();
        workers.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            // Drain until the queue is empty. A worker that stops at the first
            // empty claim would leave work behind under contention.
            while let Some(job) = queue.claim(&format!("worker-{w}")).await.expect("claim") {
                mine.push(job.id);
            }
            mine
        }));
    }

    let mut all = Vec::new();
    for w in workers {
        all.extend(w.await.expect("worker finished"));
    }

    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(
        unique.len(),
        all.len(),
        "a job was claimed twice: {} claims but only {} distinct",
        all.len(),
        unique.len()
    );
    assert_eq!(
        all.len(),
        JOBS,
        "every job must be claimed exactly once, got {} of {JOBS}",
        all.len()
    );
}

/// A worker that dies mid-job leaves its row `running`. Without recovery
/// nothing ever picks it up again.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_lock_is_recovered() {
    let Some(db) = fresh_database("nyuka_test_claim_stale").await else {
        return;
    };
    let queue = PostgresQueue::new(db);

    let id = queue
        .enqueue(
            JobKind::CheckFollow,
            serde_json::json!({}),
            None,
            None,
            0,
            5,
        )
        .await
        .expect("enqueue");
    let claimed = queue.claim("doomed-worker").await.expect("claim");
    assert_eq!(claimed.map(|j| j.id), Some(id));

    // Nothing else can claim it while the lock is fresh.
    assert!(
        queue.claim("other").await.expect("claim").is_none(),
        "a running job must not be claimable"
    );

    // Simulate the worker having died some time ago.
    queue
        .connection()
        .execute_raw(Statement::from_string(
            queue.connection().get_database_backend(),
            "UPDATE job SET locked_at = now() - interval '1 hour'",
        ))
        .await
        .expect("ageing the lock");

    let recovered = queue
        .recover_stale(std::time::Duration::from_secs(900))
        .await
        .expect("recover");
    assert_eq!(recovered, 1);

    let reclaimed = queue.claim("healthy-worker").await.expect("claim");
    assert_eq!(
        reclaimed.map(|j| j.id),
        Some(id),
        "a recovered job must become claimable again"
    );
}

/// Deferred downloads are jobs with a future `run_at`, which is the whole
/// scheduling mechanism (ADR-0003).
#[tokio::test(flavor = "multi_thread")]
async fn a_future_job_is_not_claimed_yet() {
    let Some(db) = fresh_database("nyuka_test_claim_future").await else {
        return;
    };
    let queue = PostgresQueue::new(db);

    queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({}),
            Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            None,
            0,
            5,
        )
        .await
        .expect("enqueue");

    assert!(
        queue.claim("worker").await.expect("claim").is_none(),
        "a job scheduled for later must not be claimable now"
    );
}

/// Priority then run_at, so an interactive download overtakes a background
/// follow check.
#[tokio::test(flavor = "multi_thread")]
async fn higher_priority_is_claimed_first() {
    let Some(db) = fresh_database("nyuka_test_claim_priority").await else {
        return;
    };
    let queue = PostgresQueue::new(db);

    // Enqueued in the opposite order to the one expected back.
    let background = queue
        .enqueue(
            JobKind::CheckFollow,
            serde_json::json!({}),
            None,
            None,
            10,
            5,
        )
        .await
        .expect("enqueue");
    let interactive = queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({}),
            None,
            None,
            0,
            5,
        )
        .await
        .expect("enqueue");

    let first = queue.claim("worker").await.expect("claim").expect("a job");
    assert_eq!(
        first.id, interactive,
        "a lower priority number must be claimed first"
    );
    let second = queue.claim("worker").await.expect("claim").expect("a job");
    assert_eq!(second.id, background);
}

/// A retried request must not enqueue twice. The unique constraint is what
/// enforces it; this checks the adapter surfaces that as reuse rather than an
/// error.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_idempotency_key_reuses_the_job() {
    let Some(db) = fresh_database("nyuka_test_claim_idem").await else {
        return;
    };
    let queue = PostgresQueue::new(db);

    let first = queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({ "chapter": 1 }),
            None,
            Some("submit-abc"),
            0,
            5,
        )
        .await
        .expect("first enqueue");
    let second = queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({ "chapter": 1 }),
            None,
            Some("submit-abc"),
            0,
            5,
        )
        .await
        .expect("a retry must succeed, not error");

    assert_eq!(first, second, "a retry must return the existing job");
    let queued = queue.list(Some(JobState::Queued), 100).await.expect("list");
    assert_eq!(queued.items.len(), 1, "only one job may exist for the key");
}

/// Retryability decides the backoff path, and a non-retryable failure must be
/// terminal even with attempts left.
#[tokio::test(flavor = "multi_thread")]
async fn only_retryable_failures_are_rescheduled() {
    let Some(db) = fresh_database("nyuka_test_claim_fail").await else {
        return;
    };
    let queue = PostgresQueue::new(db);

    let transient = queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({}),
            None,
            None,
            0,
            5,
        )
        .await
        .expect("enqueue");
    queue.claim("w").await.expect("claim");
    let next = queue
        .fail(transient, "network blip", true)
        .await
        .expect("fail");
    assert!(next.is_some(), "a retryable failure must be rescheduled");
    assert_eq!(
        queue.get(transient).await.expect("get").state,
        JobState::Queued
    );

    let permanent = queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({}),
            None,
            None,
            0,
            5,
        )
        .await
        .expect("enqueue");
    // Claim past the rescheduled one.
    while let Some(j) = queue.claim("w").await.expect("claim") {
        if j.id == permanent {
            break;
        }
    }
    let next = queue
        .fail(permanent, "malformed data", false)
        .await
        .expect("fail");
    assert!(
        next.is_none(),
        "a non-retryable failure is terminal even with attempts left"
    );
    assert_eq!(
        queue.get(permanent).await.expect("get").state,
        JobState::Failed
    );
}
