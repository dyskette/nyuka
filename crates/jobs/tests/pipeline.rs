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
    let queue = Arc::new(PostgresQueue::new(db));
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
