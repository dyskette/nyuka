//! The scheduler, against a real PostgreSQL.
//!
//! The property that matters is not that a tick enqueues work — it is that
//! repeated ticks do not enqueue it again. A scheduler that fires `prune_jobs`
//! sixty times an hour instead of once a day looks healthy in every log line
//! it writes, so nothing but a test catches it.
//!
//! Skipped when `DATABASE_URL` is unset.

use std::sync::Arc;
use std::time::Duration;

use nyuka_domain::model::{JobKind, JobState};
use nyuka_jobs::queue::{PostgresQueue, kind_to_str};
use nyuka_jobs::scheduler::{DAY, Periodic, Scheduler, SchedulerConfig, default_schedule};
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use uuid::Uuid;

/// A fresh metrics sink. Tests that do not assert on it still need one,
/// because the pool records into it (ADR-0019).
fn metrics() -> Arc<nyuka_jobs::metrics::JobMetrics> {
    Arc::new(nyuka_jobs::metrics::JobMetrics::new())
}

async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run scheduler tests");
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

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_raw(Statement::from_string(db.get_database_backend(), sql))
        .await
        .expect("statement");
}

/// Inserts a follow, plus the repo/source/manga rows its foreign keys need.
async fn insert_follow(db: &DatabaseConnection, interval_secs: i32, checked: Option<&str>) -> Uuid {
    let repo = Uuid::new_v4();
    let source = Uuid::new_v4();
    let manga = Uuid::new_v4();
    let follow = Uuid::new_v4();
    exec(
        db,
        &format!(
            "INSERT INTO source_repo (id, name, url, created_at) \
             VALUES ('{repo}', 'test', 'https://example.invalid/{repo}', now())"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO source (id, repo_id, external_id, name, version, languages, \
             required_capabilities, installed_at) \
             VALUES ('{source}', '{repo}', 'en.test.{source}', 'Test', 1, '[\"en\"]'::jsonb, \
             '[]'::jsonb, now())"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO manga (id, source_id, external_key, title, authors, artists, tags, \
             status, content_rating, direction, created_at, updated_at) \
             VALUES ('{manga}', '{source}', 'k', 'T', '[]'::jsonb, '[]'::jsonb, '[]'::jsonb, \
             0, 0, 0, now(), now())"
        ),
    )
    .await;
    let last = match checked {
        Some(expr) => expr.to_string(),
        None => "NULL".into(),
    };
    exec(
        db,
        &format!(
            "INSERT INTO follow (id, manga_id, check_interval_secs, last_checked_at, \
             auto_download, created_at) \
             VALUES ('{follow}', '{manga}', {interval_secs}, {last}, false, now())"
        ),
    )
    .await;
    follow
}

async fn count_of(queue: &PostgresQueue, kind: JobKind) -> usize {
    queue
        .list(None, 500)
        .await
        .expect("list")
        .items
        .into_iter()
        .filter(|j| j.kind == kind)
        .count()
}

/// The central property: ticking repeatedly within one period enqueues one row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_ticks_within_a_period_enqueue_once() {
    let Some(db) = fresh_database("nyuka_test_sched_once").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig::default(),
    );

    let first = scheduler.tick().await.expect("first tick");
    assert_eq!(
        first.enqueued.get(kind_to_str(JobKind::PruneJobs)),
        Some(&1),
        "the first tick enqueues the daily maintenance kinds"
    );

    for _ in 0..5 {
        let again = scheduler.tick().await.expect("later tick");
        assert!(
            again.enqueued.is_empty(),
            "a tick inside the same period must enqueue nothing, got {:?}",
            again.enqueued
        );
    }

    for periodic in default_schedule() {
        assert_eq!(
            count_of(&queue, periodic.kind).await,
            1,
            "{} must have exactly one row after six ticks",
            kind_to_str(periodic.kind)
        );
    }
}

/// And the other direction: a period that has turned over does enqueue again.
/// Without this, the test above would also pass on a scheduler that enqueues
/// nothing at all after the first tick, forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_period_enqueues_again() {
    let Some(db) = fresh_database("nyuka_test_sched_rollover").await else {
        return;
    };
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let config = SchedulerConfig {
        // A one-second period, so the rollover happens in test time rather
        // than tomorrow.
        schedule: vec![Periodic {
            kind: JobKind::PruneSessions,
            every: Duration::from_secs(1),
            priority: 20,
            max_attempts: 1,
        }],
        ..Default::default()
    };
    let scheduler = Scheduler::new(queue.clone(), follows(&db), metrics(), config);

    scheduler.tick().await.expect("first tick");
    // Long enough to cross a one-second bucket boundary from anywhere inside
    // the previous one.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = scheduler.tick().await.expect("second tick");

    assert_eq!(
        second.enqueued.get(kind_to_str(JobKind::PruneSessions)),
        Some(&1),
        "a period that has elapsed must enqueue its next run"
    );
    assert_eq!(count_of(&queue, JobKind::PruneSessions).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_never_checked_follow_is_due_and_a_freshly_checked_one_is_not() {
    let Some(db) = fresh_database("nyuka_test_sched_follow").await else {
        return;
    };
    insert_follow(&db, 3600, None).await;
    insert_follow(&db, 3600, Some("now()")).await;
    insert_follow(&db, 3600, Some("now() - interval '2 hours'")).await;

    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig::default(),
    );

    let report = scheduler.tick().await.expect("tick");
    assert_eq!(
        report.follows, 2,
        "the never-checked follow and the overdue one are due; the one checked \
         just now is not"
    );
    assert_eq!(count_of(&queue, JobKind::CheckFollow).await, 2);

    let again = scheduler.tick().await.expect("second tick");
    assert_eq!(
        again.follows, 0,
        "a follow already enqueued this interval must not be enqueued again"
    );
    assert_eq!(count_of(&queue, JobKind::CheckFollow).await, 2);
}

/// Each follow gets its own key, or one due follow would suppress every other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follows_are_keyed_independently() {
    let Some(db) = fresh_database("nyuka_test_sched_follow_keys").await else {
        return;
    };
    for _ in 0..4 {
        insert_follow(&db, 3600, None).await;
    }
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig::default(),
    );

    assert_eq!(scheduler.tick().await.expect("tick").follows, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_follow_batch_bounds_one_tick() {
    let Some(db) = fresh_database("nyuka_test_sched_batch").await else {
        return;
    };
    for _ in 0..5 {
        insert_follow(&db, 3600, None).await;
    }
    let queue = Arc::new(PostgresQueue::new(db.clone()));
    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig {
            follow_batch: 2,
            ..Default::default()
        },
    );

    assert_eq!(
        scheduler.tick().await.expect("tick").follows,
        2,
        "a library with more follows than the batch must not flood the queue"
    );
}

/// Startup work: rows stranded by a process that died without draining.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_recovers_rows_stranded_by_a_hard_kill() {
    let Some(db) = fresh_database("nyuka_test_sched_recover").await else {
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
    let job = queue.claim("doomed").await.expect("claim").expect("a job");

    // Backdate the lock to stand in for a worker killed an hour ago.
    exec(
        &db,
        "UPDATE job SET locked_at = now() - interval '1 hour' WHERE state = 'running'",
    )
    .await;

    let scheduler = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig::default(),
    );
    let report = scheduler.tick().await.expect("tick");

    assert_eq!(report.recovered, 1);
    assert_eq!(
        queue.get(job.id).await.expect("get").state,
        JobState::Queued,
        "a stranded job must become runnable again"
    );
}

/// A lock that is merely young must survive, or the scheduler would yank jobs
/// out from under workers that are still running them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_leaves_a_live_worker_alone() {
    let Some(db) = fresh_database("nyuka_test_sched_live").await else {
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
    let job = queue.claim("busy").await.expect("claim").expect("a job");

    let report = Scheduler::new(
        queue.clone(),
        follows(&db),
        metrics(),
        SchedulerConfig::default(),
    )
    .tick()
    .await
    .expect("tick");

    assert_eq!(report.recovered, 0);
    assert_eq!(
        queue.get(job.id).await.expect("get").state,
        JobState::Running
    );
}

/// The invariant the module note calls load-bearing, checked against the
/// retention the scheduler actually ships with.
#[test]
fn the_shipped_schedule_and_retention_agree() {
    nyuka_jobs::scheduler::check_retention(chrono::Duration::days(7), &default_schedule())
        .expect("the default retention must outlast the longest period");
    assert!(DAY < Duration::from_secs(7 * 24 * 3600));
}
