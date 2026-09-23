//! Repository behaviour against a real PostgreSQL.
//!
//! The central case is `upsert_chapters` returning only genuinely new rows.
//! That cannot be checked without a database — `ON CONFLICT DO NOTHING ...
//! RETURNING` is what produces it, and only Postgres decides what comes back.

use chrono::Utc;
use nyuka_domain::DomainError;
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

// ---------------------------------------------------------------------------
// Manga and follows
// ---------------------------------------------------------------------------

/// The source id the harness seeds. Needed to upsert a second series under it.
async fn seeded_source(repos: &Repositories, manga: MangaId) -> SourceId {
    repos
        .get_manga(manga)
        .await
        .expect("seeded manga")
        .source_id
}

fn sample_manga(source: SourceId, key: &str) -> Manga {
    Manga {
        // Ignored on insert — the repository assigns one — and ignored on
        // conflict, where the existing row keeps its own.
        id: MangaId(Uuid::nil()),
        source_id: source,
        external_key: ExternalKey(key.into()),
        title: "Alpha".into(),
        authors: vec!["Author".into()],
        artists: vec!["Artist".into()],
        description: Some("A description".into()),
        tags: vec!["Action".into(), "Drama".into()],
        cover_url: Some("https://example.test/c.jpg".into()),
        url: Some("https://example.test/alpha".into()),
        language: Some("en".into()),
        status: MangaStatus::Ongoing,
        content_rating: ContentRating::Suggestive,
        direction: ReadingDirection::RightToLeft,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// Every field the domain carries must survive a write and a read. This is the
/// test that would have caught the model being narrower than the schema.
#[tokio::test(flavor = "multi_thread")]
async fn a_manga_round_trips_through_every_column() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_manga").await else {
        return;
    };
    let source = seeded_source(&repos, seed).await;
    let written = sample_manga(source, "alpha");

    let id = repos.upsert_manga(&written).await.expect("upsert");
    let read = repos.get_manga(id).await.expect("get");

    assert_eq!(read.source_id, written.source_id);
    assert_eq!(read.external_key, written.external_key);
    assert_eq!(read.title, written.title);
    assert_eq!(read.authors, written.authors);
    assert_eq!(read.artists, written.artists);
    assert_eq!(read.description, written.description);
    assert_eq!(read.tags, written.tags);
    assert_eq!(read.cover_url, written.cover_url);
    assert_eq!(read.url, written.url);
    assert_eq!(read.language, written.language);
    assert_eq!(read.status, MangaStatus::Ongoing);
    assert_eq!(read.content_rating, ContentRating::Suggestive);
    assert_eq!(read.direction, ReadingDirection::RightToLeft);
}

/// A refresh updates what the source owns and keeps the row's identity. If
/// `created_at` moved, every metadata refresh would reshuffle the library's
/// "recently added" ordering.
#[tokio::test(flavor = "multi_thread")]
async fn upserting_the_same_key_updates_in_place_and_keeps_created_at() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_manga_upsert").await else {
        return;
    };
    let source = seeded_source(&repos, seed).await;

    let id = repos
        .upsert_manga(&sample_manga(source, "alpha"))
        .await
        .expect("insert");
    let first = repos.get_manga(id).await.expect("get");

    let mut changed = sample_manga(source, "alpha");
    changed.title = "Alpha (revised)".into();
    changed.status = MangaStatus::Completed;
    let again = repos.upsert_manga(&changed).await.expect("update");

    assert_eq!(again, id, "the same key must not create a second row");
    let second = repos.get_manga(id).await.expect("get");
    assert_eq!(second.title, "Alpha (revised)");
    assert_eq!(second.status, MangaStatus::Completed);
    assert_eq!(
        second.created_at, first.created_at,
        "a refresh must not make a series look newly added"
    );
    assert!(second.updated_at >= first.updated_at);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manga_is_found_by_its_source_and_key() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_manga_find").await else {
        return;
    };
    let source = seeded_source(&repos, seed).await;
    let id = repos
        .upsert_manga(&sample_manga(source, "alpha"))
        .await
        .expect("insert");

    assert_eq!(
        repos
            .find_manga_by_external(source, &ExternalKey("alpha".into()))
            .await
            .expect("find")
            .map(|m| m.id),
        Some(id)
    );
    assert!(
        repos
            .find_manga_by_external(source, &ExternalKey("nope".into()))
            .await
            .expect("find")
            .is_none(),
        "a key no source uses must be absent, not an error"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_manga_is_not_found_rather_than_a_storage_error() {
    let Some((_db, repos, _seed)) = fresh("nyuka_test_repo_manga_missing").await else {
        return;
    };
    assert!(matches!(
        repos.get_manga(MangaId(Uuid::new_v4())).await,
        Err(DomainError::NotFound)
    ));
}

async fn follow_for(repos: &Repositories, manga: MangaId, interval: i32) -> FollowId {
    repos
        .upsert_follow(&Follow {
            id: FollowId(Uuid::nil()),
            manga_id: manga,
            check_interval_secs: interval,
            last_checked_at: None,
            auto_download: true,
            created_at: Utc::now(),
        })
        .await
        .expect("upsert follow")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_round_trips_and_is_unique_per_series() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_follow").await else {
        return;
    };
    let id = follow_for(&repos, manga, 3600).await;
    let read = repos.get_follow(id).await.expect("get");
    assert_eq!(read.manga_id, manga);
    assert_eq!(read.check_interval_secs, 3600);
    assert!(read.auto_download);
    assert!(read.last_checked_at.is_none());

    // A second follow for the same series updates rather than duplicating.
    let again = follow_for(&repos, manga, 7200).await;
    assert_eq!(again, id);
    assert_eq!(
        repos.get_follow(id).await.expect("get").check_interval_secs,
        7200
    );
}

/// The query the scheduler runs every minute. It has to be right in SQL,
/// because loading every follow to filter in Rust would scale with the library
/// rather than with the work.
#[tokio::test(flavor = "multi_thread")]
async fn due_follows_respects_each_follows_own_interval() {
    let Some((db, repos, manga)) = fresh("nyuka_test_repo_due").await else {
        return;
    };
    let id = follow_for(&repos, manga, 3600).await;

    assert_eq!(
        repos.due_follows(10).await.expect("due").len(),
        1,
        "a follow that has never been checked is due"
    );

    repos.mark_follow_checked(id).await.expect("mark");
    assert!(
        repos.due_follows(10).await.expect("due").is_empty(),
        "a follow checked just now is not due for another hour"
    );

    db.execute_raw(Statement::from_string(
        db.get_database_backend(),
        "UPDATE follow SET last_checked_at = now() - interval '2 hours'",
    ))
    .await
    .expect("backdate");
    assert_eq!(
        repos.due_follows(10).await.expect("due").len(),
        1,
        "an hour-interval follow last checked two hours ago is due again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn due_follows_is_bounded_by_the_limit() {
    let Some((db, repos, manga)) = fresh("nyuka_test_repo_due_limit").await else {
        return;
    };
    let source = seeded_source(&repos, manga).await;
    for i in 0..5 {
        let other = repos
            .upsert_manga(&sample_manga(source, &format!("series-{i}")))
            .await
            .expect("manga");
        follow_for(&repos, other, 3600).await;
    }
    let _ = db;

    assert_eq!(repos.due_follows(2).await.expect("due").len(), 2);
    assert_eq!(repos.due_follows(50).await.expect("due").len(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_follow_is_gone() {
    let Some((_db, repos, manga)) = fresh("nyuka_test_repo_follow_delete").await else {
        return;
    };
    let id = follow_for(&repos, manga, 3600).await;
    repos.delete_follow(id).await.expect("delete");
    assert!(matches!(
        repos.get_follow(id).await,
        Err(DomainError::NotFound)
    ));
    // Deleting again is the desired state, not a failure.
    repos.delete_follow(id).await.expect("idempotent delete");
}

// ---------------------------------------------------------------------------
// Sources and repositories
// ---------------------------------------------------------------------------

fn sample_source(repo: SourceRepoId, external: &str) -> InstalledSource {
    InstalledSource {
        id: SourceId(Uuid::nil()),
        repo_id: repo,
        external_id: ExternalKey(external.into()),
        name: "Example".into(),
        version: 7,
        languages: vec!["en".into(), "ja".into()],
        required_capabilities: vec![Capability::Net, Capability::Html],
        declared_rate_limit: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_round_trips_through_every_column() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_source").await else {
        return;
    };
    let repo_id = repos
        .get_source(seeded_source(&repos, seed).await)
        .await
        .expect("seeded source")
        .repo_id;

    let written = sample_source(repo_id, "en.example");
    let id = repos.upsert_source(&written).await.expect("upsert");
    let read = repos.get_source(id).await.expect("get");

    assert_eq!(read.repo_id, repo_id);
    assert_eq!(read.external_id, written.external_id);
    assert_eq!(read.name, "Example");
    assert_eq!(
        read.version, 7,
        "a numeric manifest version must stay numeric through the column"
    );
    assert_eq!(read.languages, vec!["en".to_string(), "ja".to_string()]);
    assert_eq!(
        read.required_capabilities,
        vec![Capability::Net, Capability::Html]
    );
    assert!(read.declared_rate_limit.is_none());
}

/// Reinstalling must keep the row's id, or every `manga` row pointing at the
/// source would be orphaned by an upgrade.
#[tokio::test(flavor = "multi_thread")]
async fn reinstalling_a_source_keeps_its_id() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_source_reinstall").await else {
        return;
    };
    let repo_id = repos
        .get_source(seeded_source(&repos, seed).await)
        .await
        .expect("seeded source")
        .repo_id;

    let first = repos
        .upsert_source(&sample_source(repo_id, "en.example"))
        .await
        .expect("install");

    let mut upgraded = sample_source(repo_id, "en.example");
    upgraded.version = 8;
    upgraded.name = "Example (renamed)".into();
    let second = repos.upsert_source(&upgraded).await.expect("upgrade");

    assert_eq!(second, first);
    let read = repos.get_source(first).await.expect("get");
    assert_eq!(read.version, 8);
    assert_eq!(read.name, "Example (renamed)");
}

/// A rate limit is only learned once the module runs, so an upsert that does
/// not know one must not erase one already recorded.
#[tokio::test(flavor = "multi_thread")]
async fn an_upsert_without_a_rate_limit_keeps_the_recorded_one() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_source_rate").await else {
        return;
    };
    let repo_id = repos
        .get_source(seeded_source(&repos, seed).await)
        .await
        .expect("seeded source")
        .repo_id;

    let mut with_limit = sample_source(repo_id, "en.example");
    with_limit.declared_rate_limit = Some(RateLimit {
        permits: 2,
        period_seconds: 10,
    });
    let id = repos.upsert_source(&with_limit).await.expect("install");
    assert_eq!(
        repos
            .get_source(id)
            .await
            .expect("get")
            .declared_rate_limit
            .map(|r| r.permits),
        Some(2)
    );

    repos
        .upsert_source(&sample_source(repo_id, "en.example"))
        .await
        .expect("reinstall");
    assert_eq!(
        repos
            .get_source(id)
            .await
            .expect("get")
            .declared_rate_limit
            .map(|r| r.permits),
        Some(2),
        "a reinstall that has not yet seen the module declare a limit must \
         not drop the one already known"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_round_trips_and_is_keyed_on_its_url() {
    let Some((_db, repos, _seed)) = fresh("nyuka_test_repo_source_repo").await else {
        return;
    };
    let written = SourceRepo {
        id: SourceRepoId(Uuid::nil()),
        name: "Community".into(),
        url: "https://example.test/index.json".into(),
        last_refreshed_at: None,
        created_at: Utc::now(),
    };
    let id = repos.upsert_source_repo(&written).await.expect("insert");
    let read = repos.get_source_repo(id).await.expect("get");
    assert_eq!(read.name, "Community");
    assert_eq!(read.url, written.url);
    assert!(read.last_refreshed_at.is_none());

    let renamed = SourceRepo {
        name: "Community (renamed)".into(),
        ..written
    };
    assert_eq!(
        repos.upsert_source_repo(&renamed).await.expect("update"),
        id,
        "the same url must not create a second repository"
    );
    assert_eq!(
        repos.get_source_repo(id).await.expect("get").name,
        "Community (renamed)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_successful_refresh_is_stamped() {
    let Some((_db, repos, _seed)) = fresh("nyuka_test_repo_refresh_stamp").await else {
        return;
    };
    let id = repos
        .upsert_source_repo(&SourceRepo {
            id: SourceRepoId(Uuid::nil()),
            name: "R".into(),
            url: "https://example.test/r.json".into(),
            last_refreshed_at: None,
            created_at: Utc::now(),
        })
        .await
        .expect("insert");

    assert!(
        repos
            .get_source_repo(id)
            .await
            .expect("get")
            .last_refreshed_at
            .is_none(),
        "a repository nobody has fetched must read as never refreshed"
    );

    repos.mark_repo_refreshed(id).await.expect("stamp");
    assert!(
        repos
            .get_source_repo(id)
            .await
            .expect("get")
            .last_refreshed_at
            .is_some()
    );
}

/// Uninstalling cascades. This is the destructive half of the operation and
/// the reason the method carries a warning.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_source_removes_its_series() {
    let Some((_db, repos, seed)) = fresh("nyuka_test_repo_source_delete").await else {
        return;
    };
    let source = seeded_source(&repos, seed).await;
    assert!(repos.get_manga(seed).await.is_ok());

    repos.delete_source(source).await.expect("delete");

    assert!(matches!(
        repos.get_manga(seed).await,
        Err(DomainError::NotFound)
    ));
    assert!(matches!(
        repos.get_source(source).await,
        Err(DomainError::NotFound)
    ));
}
