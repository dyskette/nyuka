//! The health probes, against a real router and a real database.
//!
//! These exist because the two probes are easy to write and easy to get
//! backwards, and getting them backwards is only visible in production: a
//! liveness probe that checks the database turns a brief outage into a crash
//! loop, and a readiness probe that checks nothing sends traffic to a process
//! that cannot serve it.
//!
//! Skipped when `DATABASE_URL` is unset.

mod support;

use axum::http::StatusCode;
use nyuka_persistence::migration::{Migrator, MigratorTrait};

/// Liveness answers "is this process running", and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn healthz_is_ok_even_before_migrations_have_run() {
    let Some(h) = support::harness("nyuka_test_health_live").await else {
        return;
    };
    // Deliberately not migrated: liveness must not depend on schema state.
    let (status, body) = h.get("/healthz").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(
        body["version"].as_str().is_some_and(|v| !v.is_empty()),
        "the probe is also how an operator confirms which build is running"
    );
}

/// And the other side of that pair: readiness must notice.
#[tokio::test(flavor = "multi_thread")]
async fn readyz_refuses_traffic_until_migrations_are_applied() {
    let Some(h) = support::harness("nyuka_test_health_pending").await else {
        return;
    };
    let (status, body) = h.get("/readyz").await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a load balancer acts on the status, not the body"
    );
    assert_eq!(body["ready"], false);
    assert_eq!(body["migrations"]["ok"], false);
    assert_eq!(
        body["database"]["ok"], true,
        "an out-of-date schema is a different failure from an unreachable \
         database, and reporting them together makes an incident harder to read"
    );
    assert!(
        body["migrations"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("migration")),
        "the detail must say what is outstanding"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn readyz_is_ok_once_the_schema_is_current() {
    let Some(h) = support::harness("nyuka_test_health_ready").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.get("/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ready"], true);
    assert_eq!(body["database"]["ok"], true);
    assert_eq!(body["migrations"]["ok"], true);
    assert_eq!(body["library"]["ok"], true);
}

/// `/readyz` reports the library because a process that cannot write the
/// library cannot do the one thing it exists for.
#[tokio::test(flavor = "multi_thread")]
async fn readyz_refuses_traffic_when_the_library_root_is_gone() {
    let Some(h) = support::harness("nyuka_test_health_library").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");
    std::fs::remove_dir_all(h.library_path()).expect("remove the library out from under it");

    let (status, body) = h.get("/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["library"]["ok"], false);
    assert_eq!(
        body["database"]["ok"], true,
        "a missing library must not be reported as a database failure"
    );
}
