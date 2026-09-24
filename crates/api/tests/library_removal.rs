//! Removing a series, and what that does to the files on disk.
//!
//! The files are the destructive half and are opt-in, so both answers are
//! asserted: `?files=true` takes them and the default leaves them.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_orm::{ConnectionTrait, Statement};
use uuid::Uuid;

/// A series with one downloaded chapter, and the file that chapter names.
///
/// Written through SQL rather than the API because adding through `POST
/// /manga` needs a source to answer, and this is about what removal does to
/// rows and files that already exist.
async fn seed(h: &support::Harness) -> (Uuid, std::path::PathBuf) {
    let manga = Uuid::now_v7();
    let chapter = Uuid::now_v7();
    let relative = "example/ch-1.cbz";

    let path = h.library_path().join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("library dir");
    std::fs::write(&path, b"not really a cbz").expect("the chapter file");

    for sql in [
        format!(
            "INSERT INTO source_repo (id, url, name, created_at) \
             VALUES ('{0}', 'https://example.test/index.json', 'Example', now())",
            Uuid::nil()
        ),
        format!(
            "INSERT INTO source \
             (id, repo_id, external_id, name, version, languages, required_capabilities, installed_at) \
             VALUES ('{0}', '{1}', 'en.example', 'Example', 1, '[\"en\"]', '[]', now())",
            Uuid::max(),
            Uuid::nil()
        ),
        format!(
            "INSERT INTO manga (id, source_id, external_key, title, authors, artists, tags, \
             status, content_rating, direction, created_at, updated_at) \
             VALUES ('{manga}', '{0}', 'series-1', 'Example Series', '[]', '[]', '[]', 0, 0, 0, now(), now())",
            Uuid::max()
        ),
        format!(
            "INSERT INTO chapter (id, manga_id, external_key, scanlators, locked, created_at) \
             VALUES ('{chapter}', '{manga}', 'ch-1', '[]', false, now())"
        ),
        format!(
            "INSERT INTO downloaded_chapter (chapter_id, relative_path, size_bytes, checksum, packaged_at) \
             VALUES ('{chapter}', '{relative}', 16, 'abc', now())"
        ),
    ] {
        h.db.execute_raw(Statement::from_string(h.db.get_database_backend(), sql))
            .await
            .expect("seeding");
    }

    (manga, path)
}

fn delete(path: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        // A state-changing request carries it or is refused before anything
        // else runs; a cross-site form cannot set it.
        .header(nyuka_api::csrf::CSRF_HEADER, "fetch")
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
async fn removing_a_series_leaves_its_files_by_default() {
    let Some(h) = support::no_auth_harness("nyuka_test_remove_keep_files").await else {
        return;
    };
    let (manga, file) = seed(&h).await;

    let (status, _) = h.send(delete(&format!("/api/v1/manga/{manga}"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = h.get(&format!("/api/v1/manga/{manga}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the series is gone");
    assert!(
        file.exists(),
        "the reader's copy was taken without being asked"
    );
}

#[tokio::test]
async fn removing_a_series_takes_its_files_when_asked() {
    let Some(h) = support::no_auth_harness("nyuka_test_remove_with_files").await else {
        return;
    };
    let (manga, file) = seed(&h).await;

    let (status, _) = h
        .send(delete(&format!("/api/v1/manga/{manga}?files=true")))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(!file.exists(), "the file was left behind");
}

/// The cascade, which is what makes one statement enough.
#[tokio::test]
async fn removing_a_series_takes_its_chapters_with_it() {
    let Some(h) = support::no_auth_harness("nyuka_test_remove_cascade").await else {
        return;
    };
    let (manga, _) = seed(&h).await;

    let (status, _) = h.send(delete(&format!("/api/v1/manga/{manga}"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let remaining =
        h.db.query_one_raw(Statement::from_string(
            h.db.get_database_backend(),
            "SELECT count(*) AS n FROM chapter".to_string(),
        ))
        .await
        .expect("counting")
        .expect("a row");
    let count: i64 = remaining.try_get("", "n").expect("count");
    assert_eq!(count, 0, "chapters outlived the series they belong to");
}

/// Answered as a miss rather than as success, so a client cannot mistake a
/// stale id for a removal that happened.
#[tokio::test]
async fn removing_a_series_that_is_not_there_is_a_miss() {
    let Some(h) = support::no_auth_harness("nyuka_test_remove_missing").await else {
        return;
    };

    let (status, _) = h
        .send(delete(&format!("/api/v1/manga/{}", Uuid::now_v7())))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
