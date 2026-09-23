//! Shutdown behaviour of the worker pool, against a real PostgreSQL.
//!
//! ADR-0003 asks for exactly this: that `SIGTERM` stops claiming, drains
//! in-flight work, and returns whatever it could not finish to `queued`.
//! ADR-0001 adds the other reason to keep it — axum 0.9 changes `serve`'s
//! graceful-shutdown contract, and this is the test that will fail when that
//! upgrade breaks the drain.
//!
//! Skipped when `DATABASE_URL` is unset.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nyuka_domain::model::{Job, JobKind, JobState};
use nyuka_domain::{DomainError, Result};
use nyuka_jobs::queue::PostgresQueue;
use nyuka_jobs::worker::{JobHandler, NoEvents, WorkerConfig, WorkerPool};
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

/// A fresh metrics sink. Tests that do not assert on it still need one,
/// because the pool records into it (ADR-0019).
fn metrics() -> Arc<nyuka_jobs::metrics::JobMetrics> {
    Arc::new(nyuka_jobs::metrics::JobMetrics::new())
}

async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run drain tests");
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

/// A handler that takes a known amount of time, so the drain window is not a
/// guess.
struct SlowHandler {
    duration: Duration,
    started: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl JobHandler for SlowHandler {
    async fn handle(&self, _job: Job) -> Result<()> {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.duration).await;
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn states(queue: &PostgresQueue) -> Vec<JobState> {
    let page = queue.list(None, 100).await.expect("list");
    page.items.into_iter().map(|j| j.state).collect()
}

fn count(states: &[JobState], want: JobState) -> usize {
    states.iter().filter(|s| **s == want).count()
}

async fn enqueue_n(queue: &PostgresQueue, n: usize) {
    for i in 0..n {
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
}

/// The drain itself: work already claimed finishes, work not yet claimed is
/// left alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_finishes_in_flight_work_and_stops_claiming() {
    let Some(db) = fresh_database("nyuka_test_drain_finish").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db));
    enqueue_n(&queue, 6).await;

    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(SlowHandler {
            duration: Duration::from_millis(400),
            started: started.clone(),
            finished: finished.clone(),
        }),
        metrics(),
        Arc::new(NoEvents),
        WorkerConfig {
            workers: 2,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(10),
        },
    );

    // Long enough for both workers to claim, short enough that neither has
    // finished when shutdown begins.
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        2,
        "both workers should be busy before shutdown"
    );
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the test is only meaningful while work is still in flight"
    );

    let report = pool.shutdown().await;

    assert_eq!(
        finished.load(Ordering::SeqCst),
        2,
        "in-flight work must run to completion, not be dropped"
    );
    assert_eq!(report.drained, 2, "both workers stopped cleanly");
    assert_eq!(report.released, 0, "nothing was left holding a job");
    assert_eq!(
        started.load(Ordering::SeqCst),
        2,
        "no job may be claimed after shutdown begins"
    );

    let states = states(&queue).await;
    assert_eq!(count(&states, JobState::Succeeded), 2);
    assert_eq!(
        count(&states, JobState::Queued),
        4,
        "unclaimed jobs stay queued for the next process"
    );
    assert_eq!(count(&states, JobState::Running), 0, "no row left running");
}

/// The other direction: when the handler outlasts the drain budget, the job
/// must come back rather than sit `running` until stale-lock recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_that_outlasts_the_drain_returns_to_queued() {
    let Some(db) = fresh_database("nyuka_test_drain_timeout").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db));
    enqueue_n(&queue, 1).await;

    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(SlowHandler {
            // Far past the budget, so the timeout is what ends the drain.
            duration: Duration::from_secs(30),
            started: started.clone(),
            finished: finished.clone(),
        }),
        metrics(),
        Arc::new(NoEvents),
        WorkerConfig {
            workers: 1,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_millis(200),
        },
    );

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(started.load(Ordering::SeqCst), 1);

    let report = pool.shutdown().await;

    assert_eq!(report.drained, 0, "the worker did not finish in time");
    assert_eq!(report.released, 1, "its job must be handed back");
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the handler was cut off, so it must not report success"
    );

    let job = queue.list(None, 10).await.expect("list").items.remove(0);
    assert_eq!(
        job.state,
        JobState::Queued,
        "an undrained job must be runnable again immediately"
    );
    assert_eq!(
        job.attempts, 1,
        "the interrupted attempt still counts, so a job that always outlasts \
         the drain eventually reaches max_attempts instead of looping forever"
    );
}

struct PanicHandler;

#[async_trait::async_trait]
impl JobHandler for PanicHandler {
    async fn handle(&self, _job: Job) -> Result<()> {
        panic!("handler bug");
    }
}

/// A panicking handler must not take the drain down with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_handler_does_not_block_shutdown() {
    let Some(db) = fresh_database("nyuka_test_drain_panic").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db));
    enqueue_n(&queue, 1).await;

    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(PanicHandler),
        metrics(),
        Arc::new(NoEvents),
        WorkerConfig {
            workers: 1,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(5),
        },
    );

    tokio::time::sleep(Duration::from_millis(150)).await;
    let report = tokio::time::timeout(Duration::from_secs(5), pool.shutdown())
        .await
        .expect("shutdown must not hang on a panicking worker");

    assert_eq!(
        report.released, 1,
        "the panicking worker's job must be recovered, not stranded"
    );
    let job = queue.list(None, 10).await.expect("list").items.remove(0);
    assert_eq!(job.state, JobState::Queued);
}

struct FailHandler {
    retryable: bool,
}

#[async_trait::async_trait]
impl JobHandler for FailHandler {
    async fn handle(&self, _job: Job) -> Result<()> {
        Err(DomainError::Source {
            message: "upstream said no".into(),
            retryable: self.retryable,
        })
    }
}

/// The worker must route a handler error through the queue's retry decision
/// rather than deciding for itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retryable_failure_is_rescheduled_and_a_permanent_one_is_not() {
    let Some(db) = fresh_database("nyuka_test_drain_failure").await else {
        return;
    };

    for retryable in [true, false] {
        let queue = Arc::new(PostgresQueue::new(db.clone()));
        db.execute_raw(Statement::from_string(
            db.get_database_backend(),
            "TRUNCATE job",
        ))
        .await
        .expect("clearing between cases");
        enqueue_n(&queue, 1).await;

        let pool = WorkerPool::start(
            queue.clone(),
            Arc::new(FailHandler { retryable }),
            metrics(),
            Arc::new(NoEvents),
            WorkerConfig {
                workers: 1,
                poll_interval: Duration::from_millis(20),
                drain_timeout: Duration::from_secs(5),
            },
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        pool.shutdown().await;

        let job = queue.list(None, 10).await.expect("list").items.remove(0);
        assert_eq!(job.attempts, 1);
        assert!(
            job.last_error
                .as_deref()
                .is_some_and(|e| e.contains("upstream said no")),
            "the failure reason must reach the row so an operator can see it"
        );
        if retryable {
            assert_eq!(job.state, JobState::Queued, "a retryable failure retries");
            assert!(
                job.run_at > chrono::Utc::now(),
                "the retry must be backed off, not immediate"
            );
        } else {
            assert_eq!(
                job.state,
                JobState::Failed,
                "a permanent failure must not burn the remaining attempts"
            );
        }
    }
}
