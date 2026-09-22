//! Runs the migrations against a real PostgreSQL.
//!
//! Skipped when `DATABASE_URL` is unset, so a checkout without a database
//! still builds. CI provides one, and the schema-drift check (ADR-0002) runs
//! in the same job — that check is the control that substitutes for the
//! compile-time SQL verification SeaORM does not give.

use nyuka_persistence::migration::{Migrator, MigratorTrait};
// SeaORM 2.0 moved raw SQL to the `*_raw` entry points: `execute`, `query_one`
// and friends now take SeaQuery statements (ADR-0002).
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

fn base_url() -> Option<String> {
    match std::env::var("DATABASE_URL") {
        Ok(u) => Some(u),
        Err(_) => {
            eprintln!("skipping: set DATABASE_URL to run migration tests");
            None
        }
    }
}

/// Gives a test its own freshly created database.
///
/// Sharing one database across tests does not work: they run concurrently and
/// each applies the schema, so they drop each other's tables mid-run. The
/// failure looks like a migration bug and is not one, which is worth the
/// isolation to avoid.
async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = base_url()?;
    let admin = Database::connect(&base).await.expect("connecting");

    // Identifiers come from test names in this file, not from input.
    admin
        .execute_raw(Statement::from_string(
            admin.get_database_backend(),
            format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        ))
        .await
        .expect("dropping any leftover database");
    admin
        .execute_raw(Statement::from_string(
            admin.get_database_backend(),
            format!("CREATE DATABASE {name}"),
        ))
        .await
        .expect("creating the test database");

    let mut parsed = url::Url::parse(&base).expect("DATABASE_URL is a url");
    parsed.set_path(name);
    Some(
        Database::connect(parsed.as_str())
            .await
            .expect("connecting to the test database"),
    )
}

/// Up, down, and up again.
///
/// Running `up` once proves very little: a migration that cannot be reversed,
/// or that is not idempotent across a rollback, only shows up on the second
/// pass — which is exactly the situation a production rollback creates.
#[tokio::test]
async fn migrations_apply_reverse_and_reapply() {
    let Some(db) = fresh_database("nyuka_test_reapply").await else {
        return;
    };
    Migrator::fresh(&db).await.expect("fresh apply");
    Migrator::down(&db, None).await.expect("reverting");
    Migrator::up(&db, None).await.expect("re-applying");

    let pending = Migrator::get_pending_migrations(&db)
        .await
        .expect("listing pending migrations");
    assert!(pending.is_empty(), "migrations left pending after up");
}

/// The partial index and the autovacuum tuning are raw SQL, so nothing in the
/// type system says they were applied. ADR-0003 depends on both.
#[tokio::test]
async fn the_job_table_is_tuned_for_churn() {
    let Some(db) = fresh_database("nyuka_test_job_tuning").await else {
        return;
    };
    Migrator::fresh(&db).await.expect("fresh apply");
    let backend = db.get_database_backend();

    let index = db
        .query_one_raw(Statement::from_string(
            backend,
            "SELECT indexdef FROM pg_indexes WHERE indexname = 'idx_job_claimable'",
        ))
        .await
        .expect("querying indexes")
        .expect("idx_job_claimable exists");
    let def: String = index.try_get("", "indexdef").expect("indexdef");
    assert!(
        def.contains("WHERE") && def.contains("queued"),
        "the claim index must be partial, or it bloats with terminal rows: {def}"
    );

    let opts = db
        .query_one_raw(Statement::from_string(
            backend,
            "SELECT reloptions::text AS o FROM pg_class WHERE relname = 'job'",
        ))
        .await
        .expect("querying reloptions")
        .expect("job table exists");
    let o: Option<String> = opts.try_get("", "o").ok().flatten();
    let o = o.unwrap_or_default();
    assert!(
        o.contains("autovacuum_vacuum_scale_factor"),
        "job is high-churn and needs a lower scale factor: {o:?}"
    );
}

/// The idempotency key is what makes a retried POST /downloads safe, and it
/// only works if the database enforces it.
#[tokio::test]
async fn the_idempotency_key_is_unique_and_nullable() {
    let Some(db) = fresh_database("nyuka_test_idempotency").await else {
        return;
    };
    Migrator::fresh(&db).await.expect("fresh apply");
    let backend = db.get_database_backend();

    let insert = |key: &str| {
        Statement::from_string(
            backend,
            format!(
                "INSERT INTO job (id, kind, payload, state, priority, run_at, attempts, \
                 max_attempts, idempotency_key, created_at, updated_at) \
                 VALUES (gen_random_uuid(), 'download_chapter', '{{}}'::jsonb, 'queued', 0, \
                 now(), 0, 5, {key}, now(), now())"
            ),
        )
    };

    db.execute_raw(insert("'k1'")).await.expect("first insert");
    let duplicate = db.execute_raw(insert("'k1'")).await;
    assert!(
        duplicate.is_err(),
        "a repeated idempotency key must be rejected, or a retried request enqueues twice"
    );

    // Most jobs have no key, so NULLs must not collide with each other.
    db.execute_raw(insert("NULL"))
        .await
        .expect("first null key");
    db.execute_raw(insert("NULL"))
        .await
        .expect("a second null key must be allowed");
}
