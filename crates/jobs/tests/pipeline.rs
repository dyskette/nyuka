//! Scheduler, queue, worker pool, and registry together, against a real
//! PostgreSQL.
//!
//! Each of those is tested on its own elsewhere. This asserts they are wired:
//! a scheduler tick produces rows a worker claims, a registry routes to a
//! handler, and the handler's effect lands in the database. Every one of those
//! seams has passed its own test while being connected to nothing.
//!
//! Skipped when `DATABASE_URL` is unset.

use std::sync::Arc;
use std::time::Duration;

use nyuka_domain::model::{JobKind, JobState};
use nyuka_jobs::handlers::Registry;
use nyuka_jobs::handlers::maintenance::{PruneJobs, PruneSessions};
use nyuka_jobs::queue::PostgresQueue;
use nyuka_jobs::scheduler::{Periodic, Scheduler, SchedulerConfig};
use nyuka_jobs::worker::{WorkerConfig, WorkerPool};
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use nyuka_persistence::session::{SessionRecord, SessionRepository};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

/// A fresh metrics sink. Tests that do not assert on it still need one,
/// because the pool records into it (ADR-0019).
fn metrics() -> Arc<nyuka_jobs::metrics::JobMetrics> {
    Arc::new(nyuka_jobs::metrics::JobMetrics::new())
}

async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run pipeline tests");
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

/// The follow port, over the same connection the queue uses.
fn follows(db: &DatabaseConnection) -> Arc<dyn nyuka_domain::ports::FollowRepository> {
    Arc::new(nyuka_persistence::repository::Repositories::new(db.clone()))
}

fn session(id: &str, expires_at: chrono::DateTime<chrono::Utc>) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        user_id: None,
        data: vec![],
        expires_at,
    }
}

/// The whole path, on the one kind whose effect is visible in the same
/// database: a scheduled `prune_sessions` actually deletes expired sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_job_runs_and_its_effect_lands() {
    let Some(db) = fresh_database("nyuka_test_pipeline").await else {
        return;
    };
    let sessions = SessionRepository::new(db.clone());
    let now = chrono::Utc::now();
    sessions
        .save(&session("expired", now - chrono::Duration::hours(1)))
        .await
        .expect("save");
    sessions
        .save(&session("live", now + chrono::Duration::hours(1)))
        .await
        .expect("save");

    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let registry = Registry::new()
        .register(
            JobKind::PruneSessions,
            Arc::new(PruneSessions::new(sessions.clone())),
        )
        .register(
            JobKind::PruneJobs,
            Arc::new(PruneJobs::new(queue.clone(), chrono::Duration::days(7))),
        );

    // Only prune_sessions is scheduled, so the assertion below is about this
    // job rather than whatever else a default schedule would have added.
    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig {
            schedule: vec![Periodic {
                kind: JobKind::PruneSessions,
                every: Duration::from_secs(3600),
                priority: 20,
                max_attempts: 1,
            }],
            ..Default::default()
        },
    );
    assert_eq!(scheduler.tick().await.expect("tick").enqueued.len(), 1);

    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(registry),
        metrics(),
        WorkerConfig {
            workers: 1,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(5),
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    pool.shutdown().await;

    let job = queue.list(None, 10).await.expect("list").items.remove(0);
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "the job must have run, not merely been enqueued: {:?}",
        job.last_error
    );
    assert!(
        sessions.load("expired").await.expect("load").is_none(),
        "the expired session must be gone"
    );
    assert!(
        sessions.load("live").await.expect("load").is_some(),
        "a live session must survive the prune"
    );
}

/// A kind nobody registered must fail once and stop, not retry until it has
/// burned every attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unhandled_kind_fails_once_and_stops() {
    let Some(db) = fresh_database("nyuka_test_pipeline_unhandled").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    queue
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

    let pool = WorkerPool::start(
        queue.clone(),
        // Deliberately empty: this stands for a build with no source runtime.
        Arc::new(Registry::new()),
        metrics(),
        WorkerConfig {
            workers: 1,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(5),
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    pool.shutdown().await;

    let job = queue.list(None, 10).await.expect("list").items.remove(0);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(
        job.attempts, 1,
        "the worker must not have retried a job no handler can ever run"
    );
    assert!(
        job.last_error
            .as_deref()
            .is_some_and(|e| e.contains("no handler registered")),
        "the reason must name the problem, got {:?}",
        job.last_error
    );
}

// ---------------------------------------------------------------------------
// The objectives (ADR-0019)
// ---------------------------------------------------------------------------

struct FailingHandler {
    error: fn() -> nyuka_domain::DomainError,
}

#[async_trait::async_trait]
impl nyuka_jobs::worker::JobHandler for FailingHandler {
    async fn handle(&self, _job: nyuka_domain::model::Job) -> nyuka_domain::Result<()> {
        Err((self.error)())
    }
}

async fn run_one(
    db: &DatabaseConnection,
    handler: Arc<dyn nyuka_jobs::worker::JobHandler>,
    metrics: Arc<nyuka_jobs::metrics::JobMetrics>,
    priority: i16,
    max_attempts: i32,
) {
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({}),
            None,
            None,
            priority,
            max_attempts,
        )
        .await
        .expect("enqueue");

    let pool = WorkerPool::start(
        queue,
        handler,
        metrics,
        WorkerConfig {
            workers: 1,
            poll_interval: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(5),
        },
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    pool.shutdown().await;
}

/// The classification has to hold through a real worker, not just in a unit
/// test: the worker is what decides whether a failure is even reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_failure_is_reported_but_excluded_from_the_objective() {
    let Some(db) = fresh_database("nyuka_test_slo_source").await else {
        return;
    };
    let metrics = metrics();

    run_one(
        &db,
        Arc::new(FailingHandler {
            error: || nyuka_domain::DomainError::Source {
                message: "the site removed it".into(),
                retryable: false,
            },
        }),
        metrics.clone(),
        0,
        1,
    )
    .await;

    let snapshot = metrics.take(3600);
    assert_eq!(snapshot.jobs_failed_source, 1);
    assert_eq!(
        snapshot.jobs_failed_service, 0,
        "a site removing a chapter is not this service's fault"
    );
    assert_eq!(
        snapshot.service_success_rate(),
        None,
        "and it must not create an attributable outcome to divide by"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_storage_failure_counts_against_the_objective() {
    let Some(db) = fresh_database("nyuka_test_slo_service").await else {
        return;
    };
    let metrics = metrics();

    run_one(
        &db,
        Arc::new(FailingHandler {
            error: || nyuka_domain::DomainError::Storage("disk".into()),
        }),
        metrics.clone(),
        0,
        1,
    )
    .await;

    let snapshot = metrics.take(3600);
    assert_eq!(snapshot.jobs_failed_service, 1);
    assert_eq!(snapshot.jobs_failed_source, 0);
    assert_eq!(snapshot.service_success_rate(), Some(0.0));
}

/// A retry that will happen is not an outcome. Counting each attempt would
/// make the rate depend on how many retries are configured, which is a knob
/// rather than a measurement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_will_retry_is_not_counted_yet() {
    let Some(db) = fresh_database("nyuka_test_slo_retry").await else {
        return;
    };
    let metrics = metrics();

    run_one(
        &db,
        Arc::new(FailingHandler {
            // Retryable, with attempts remaining.
            error: || nyuka_domain::DomainError::Storage("transient".into()),
        }),
        metrics.clone(),
        0,
        5,
    )
    .await;

    let snapshot = metrics.take(3600);
    assert_eq!(
        snapshot.attributable_total(),
        0,
        "the job is going to run again; it has not had an outcome yet"
    );
}

/// SLO-2 times user-requested downloads from when the request was accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_requested_download_is_timed_and_a_background_one_is_not() {
    let Some(db) = fresh_database("nyuka_test_slo_ttfp").await else {
        return;
    };

    struct Ok_;
    #[async_trait::async_trait]
    impl nyuka_jobs::worker::JobHandler for Ok_ {
        async fn handle(&self, _job: nyuka_domain::model::Job) -> nyuka_domain::Result<()> {
            Ok(())
        }
    }

    let interactive = metrics();
    run_one(
        &db,
        Arc::new(Ok_),
        interactive.clone(),
        nyuka_jobs::metrics::PRIORITY_INTERACTIVE,
        5,
    )
    .await;
    let snapshot = interactive.take(3600);
    assert_eq!(snapshot.jobs_succeeded, 1);
    assert_eq!(
        snapshot.ttfp_samples, 1,
        "someone pressed the button and waited"
    );

    let background = metrics();
    run_one(
        &db,
        Arc::new(Ok_),
        background.clone(),
        nyuka_jobs::metrics::PRIORITY_BACKGROUND,
        5,
    )
    .await;
    let snapshot = background.take(3600);
    assert_eq!(snapshot.jobs_succeeded, 1);
    assert_eq!(
        snapshot.ttfp_samples, 0,
        "nobody is waiting for a follow sweep's download, and timing it would \
         measure the scheduler's pacing"
    );
}
