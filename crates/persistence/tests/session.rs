//! Session storage against a real PostgreSQL.
//!
//! The expiry behaviour in particular cannot be checked without a database:
//! `load` filters on `expires_at > now()` in SQL, so only the database can say
//! whether that works.

use chrono::{Duration, Utc};
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use nyuka_persistence::session::{SessionRecord, SessionRepository};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use uuid::Uuid;

async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run session tests");
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

fn record(id: &str, ttl: Duration) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        user_id: None,
        data: SessionRepository::encode(&serde_json::json!({"csrf": "abc"})).expect("encode"),
        expires_at: Utc::now() + ttl,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_round_trips() {
    let Some(db) = fresh_database("nyuka_test_session_rt").await else {
        return;
    };
    let repo = SessionRepository::new(db);

    let r = record("sess-1", Duration::hours(1));
    repo.save(&r).await.expect("save");
    let loaded = repo.load("sess-1").await.expect("load").expect("present");
    assert_eq!(loaded.id, r.id);
    assert_eq!(loaded.data, r.data);
    assert!(loaded.user_id.is_none());
}

/// Saving the same id must replace rather than fail, since every request that
/// touches the session writes it back.
#[tokio::test(flavor = "multi_thread")]
async fn saving_an_existing_id_replaces_it() {
    let Some(db) = fresh_database("nyuka_test_session_upsert").await else {
        return;
    };
    let repo = SessionRepository::new(db);

    repo.save(&record("sess-1", Duration::hours(1)))
        .await
        .expect("save");
    let mut updated = record("sess-1", Duration::hours(2));
    updated.data = SessionRepository::encode(&serde_json::json!({"csrf": "xyz"})).expect("encode");
    repo.save(&updated).await.expect("re-save");

    let loaded = repo.load("sess-1").await.expect("load").expect("present");
    assert_eq!(loaded.data, updated.data);
}

/// An expired session must read as absent even before the cleanup job runs —
/// which, between runs, is most of the time.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_session_does_not_load() {
    let Some(db) = fresh_database("nyuka_test_session_expiry").await else {
        return;
    };
    let repo = SessionRepository::new(db);

    repo.save(&record("stale", Duration::seconds(-1)))
        .await
        .expect("save");
    assert!(
        repo.load("stale").await.expect("load").is_none(),
        "an expired session must not be usable even if cleanup is behind"
    );

    // And it is still present until cleanup removes it.
    let removed = repo.delete_expired().await.expect("cleanup");
    assert_eq!(removed, 1);
    assert!(repo.load("stale").await.expect("load").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_takes_effect_immediately() {
    let Some(db) = fresh_database("nyuka_test_session_logout").await else {
        return;
    };
    let repo = SessionRepository::new(db);

    repo.save(&record("sess-1", Duration::hours(1)))
        .await
        .expect("save");
    repo.delete("sess-1").await.expect("delete");
    assert!(repo.load("sess-1").await.expect("load").is_none());
}

/// The operator action for the IdP-revocation gap: an account disabled
/// upstream keeps working here until its sessions are removed (ADR-0005).
#[tokio::test(flavor = "multi_thread")]
async fn every_session_for_a_user_can_be_revoked() {
    let Some(db) = fresh_database("nyuka_test_session_revoke").await else {
        return;
    };
    let user = Uuid::new_v4();
    let other = Uuid::new_v4();

    // A session's user must exist, so seed both.
    let repo = SessionRepository::new(db.clone());
    for id in [user, other] {
        db.execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO app_user (id, issuer, subject, created_at, last_seen_at) \
             VALUES ($1, 'https://idp.test', $2, now(), now())",
            [id.into(), id.to_string().into()],
        ))
        .await
        .expect("seeding a user");
    }

    for (id, owner) in [("a", user), ("b", user), ("c", other)] {
        let mut r = record(id, Duration::hours(1));
        r.user_id = Some(owner);
        repo.save(&r).await.expect("save");
    }

    let revoked = repo.delete_for_user(user).await.expect("revoke");
    assert_eq!(revoked, 2);
    assert!(repo.load("a").await.expect("load").is_none());
    assert!(repo.load("b").await.expect("load").is_none());
    assert!(
        repo.load("c").await.expect("load").is_some(),
        "another user's sessions must be untouched"
    );
}
