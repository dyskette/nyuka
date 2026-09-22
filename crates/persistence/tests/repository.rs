//! Repository behaviour against a real PostgreSQL.
//!
//! The central case is `upsert_chapters` returning only genuinely new rows.
//! That cannot be checked without a database — `ON CONFLICT DO NOTHING ...
//! RETURNING` is what produces it, and only Postgres decides what comes back.

use chrono::Utc;
use nyuka_domain::model::*;
use nyuka_persistence::migration::{Migrator, MigratorTrait};
use nyuka_persistence::repository::Repositories;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use uuid::Uuid;

async fn fresh(name: &str) -> Option<(DatabaseConnection, Repositories, MangaId)> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run repository tests");
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
    let mut parsed = url::Url::parse(&base).expect("url");
    parsed.set_path(name);
    let db = Database::connect(parsed.as_str())
        .await
        .expect("connecting");
    Migrator::fresh(&db).await.expect("migrating");

    // A chapter needs a manga, which needs a source, which needs a repo.
    let repo_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let manga_id = Uuid::new_v4();
    for stmt in [
        Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO source_repo (id, name, url, created_at) VALUES ($1, 'r', 'https://r.test', now())",
            [repo_id.into()],
        ),
        Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO source (id, repo_id, external_id, name, version, languages, \
             required_capabilities, installed_at) \
             VALUES ($1, $2, 'en.test', 'Test', 1, '[]'::jsonb, '[]'::jsonb, now())",
            [source_id.into(), repo_id.into()],
        ),
        Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO manga (id, source_id, external_key, title, authors, artists, tags, \
             status, content_rating, direction, created_at, updated_at) \
             VALUES ($1, $2, 'series-1', 'Series', '[]'::jsonb, '[]'::jsonb, '[]'::jsonb, \
             0, 0, 0, now(), now())",
            [manga_id.into(), source_id.into()],
        ),
    ] {
        db.execute_raw(stmt).await.expect("seeding");
    }

    let repos = Repositories::new(db.clone());
    Some((db, repos, MangaId(manga_id)))
}

fn chapter(key: &str, number: f32) -> SourceChapter {
    SourceChapter {
        key: ExternalKey(key.into()),
        title: Some(format!("Chapter {number}")),
        number: Some(number),
        volume: None,
        published_at: Some(Utc::now()),
        scanlators: vec!["Scans".into()],
        url: None,
        language: Some("en".into()),
        locked: false,
    }
}

/// The behaviour `chapter.new` events depend on: a refresh that finds nothing
/// new must report nothing new.
#[tokio::test(flavor = "multi_thread")]
async fn upsert_returns_only_newly_inserted_chapters() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_upsert").await else {
        return;
    };

    let first = repos
        .upsert_chapters(manga, &[chapter("c1", 1.0), chapter("c2", 2.0)])
        .await
        .expect("first upsert");
    assert_eq!(first.len(), 2, "both chapters are new the first time");

    // The same two again: nothing is new, so nothing must be reported.
    let repeat = repos
        .upsert_chapters(manga, &[chapter("c1", 1.0), chapter("c2", 2.0)])
        .await
        .expect("repeat upsert");
    assert!(
        repeat.is_empty(),
        "a refresh finding nothing new must notify nothing, got {} chapters",
        repeat.len()
    );

    // One new alongside two existing: only the new one comes back.
    let mixed = repos
        .upsert_chapters(
            manga,
            &[chapter("c1", 1.0), chapter("c2", 2.0), chapter("c3", 3.0)],
        )
        .await
        .expect("mixed upsert");
    assert_eq!(
        mixed.len(),
        1,
        "only the genuinely new chapter drives a notification"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_upsert_is_not_an_error() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_empty").await else {
        return;
    };
    assert!(
        repos
            .upsert_chapters(manga, &[])
            .await
            .expect("empty")
            .is_empty()
    );
}

/// Keyset pagination must not skip or repeat a chapter, and must terminate.
#[tokio::test(flavor = "multi_thread")]
async fn chapters_paginate_without_gaps_or_repeats() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_page").await else {
        return;
    };
    let chapters: Vec<_> = (0..120)
        .map(|i| chapter(&format!("c{i}"), i as f32))
        .collect();
    repos.upsert_chapters(manga, &chapters).await.expect("seed");

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = repos
            .list_chapters(manga, cursor.as_ref())
            .await
            .expect("page");
        seen.extend(page.items.iter().map(|c| c.id));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(seen.len() <= 120, "pagination did not terminate");
    }

    assert_eq!(seen.len(), 120, "every chapter must appear exactly once");
    let unique: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 120, "a chapter was returned on two pages");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_is_recorded_and_can_be_forgotten() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_download").await else {
        return;
    };
    let ids = repos
        .upsert_chapters(manga, &[chapter("c1", 1.0)])
        .await
        .expect("seed");
    let chapter_id = ids[0];

    assert!(repos.downloaded(chapter_id).await.expect("read").is_none());

    repos
        .record_download(&DownloadedChapter {
            chapter_id,
            relative_path: "Series/Series v01 c001.cbz".into(),
            size_bytes: 4096,
            checksum: "abc123".into(),
            packaged_at: Utc::now(),
        })
        .await
        .expect("record");

    let found = repos
        .downloaded(chapter_id)
        .await
        .expect("read")
        .expect("present");
    assert_eq!(found.size_bytes, 4096);
    assert_eq!(found.checksum, "abc123");

    // Reconciliation after the file went missing.
    assert_eq!(
        repos.forget_downloads(&[chapter_id]).await.expect("forget"),
        1
    );
    assert!(repos.downloaded(chapter_id).await.expect("read").is_none());
}

/// The WASM `defaults` import round trip. Values are opaque bytes here.
#[tokio::test(flavor = "multi_thread")]
async fn source_settings_round_trip() {
    let Some((db, repos, _)) = fresh("nyuka_test_repo_kv").await else {
        return;
    };
    let source: Uuid = db
        .query_one_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT id FROM source LIMIT 1",
        ))
        .await
        .expect("query")
        .expect("a source")
        .try_get("", "id")
        .expect("id");
    let source = SourceId(source);

    assert!(repos.kv_get(source, "unset").await.expect("get").is_none());
    repos.kv_set(source, "k", vec![1, 2, 3]).await.expect("set");
    assert_eq!(
        repos.kv_get(source, "k").await.expect("get"),
        Some(vec![1, 2, 3])
    );
    // Overwriting must replace rather than fail.
    repos.kv_set(source, "k", vec![9]).await.expect("overwrite");
    assert_eq!(repos.kv_get(source, "k").await.expect("get"), Some(vec![9]));
}
