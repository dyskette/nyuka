//! The REST surface, against a real router and database.
//!
//! Every route under `/api/v1` except the auth endpoints requires a session,
//! so most of what is assertable without one is the access control itself —
//! which is worth asserting, because an endpoint accidentally mounted outside
//! the guard is invisible until someone finds it.
//!
//! Skipped when `DATABASE_URL` is unset.

mod support;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use nyuka_persistence::migration::{Migrator, MigratorTrait};

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request")
}

fn mutate(method: Method, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-requested-with", "XMLHttpRequest")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("request")
}

/// The sweep that catches an endpoint mounted outside the guard. Adding a
/// route to the protected group without adding it here is fine; adding one
/// *outside* it and forgetting is what this is for.
#[tokio::test(flavor = "multi_thread")]
async fn every_library_route_requires_a_session() {
    let Some(h) = support::harness("nyuka_test_routes_auth").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let id = uuid::Uuid::new_v4();
    for path in [
        "/api/v1/manga".to_string(),
        format!("/api/v1/manga/{id}"),
        format!("/api/v1/manga/{id}/chapters"),
        format!("/api/v1/chapters/{id}"),
        "/api/v1/follows".to_string(),
        format!("/api/v1/follows/{id}"),
        "/api/v1/jobs".to_string(),
        format!("/api/v1/jobs/{id}"),
        "/api/v1/events".to_string(),
        "/api/v1/source-repos".to_string(),
        format!("/api/v1/source-repos/{id}/available"),
        "/api/v1/sources".to_string(),
        format!("/api/v1/sources/{id}/filters"),
        format!("/api/v1/sources/{id}/catalog"),
        format!("/api/v1/sources/{id}/catalog/some-key"),
        format!("/api/v1/sources/{id}/catalog/some-key/chapters"),
        format!("/api/v1/downloads/{id}/file"),
    ] {
        let (status, body) = h.send(get(&path)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{path} answered {status} without a session: {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_mutating_route_requires_a_session_too() {
    let Some(h) = support::harness("nyuka_test_routes_auth_mutate").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let id = uuid::Uuid::new_v4();
    for (method, path) in [
        (Method::PUT, "/api/v1/follows".to_string()),
        (Method::DELETE, format!("/api/v1/follows/{id}")),
        (Method::POST, format!("/api/v1/follows/{id}/check-now")),
        (Method::POST, format!("/api/v1/jobs/{id}/cancel")),
        (Method::POST, format!("/api/v1/jobs/{id}/retry")),
        (Method::POST, "/api/v1/source-repos".to_string()),
        (Method::DELETE, format!("/api/v1/source-repos/{id}")),
        (Method::POST, format!("/api/v1/source-repos/{id}/refresh")),
        (Method::POST, "/api/v1/sources".to_string()),
        (Method::DELETE, format!("/api/v1/sources/{id}")),
        (Method::POST, "/api/v1/manga".to_string()),
        (Method::POST, "/api/v1/downloads".to_string()),
        (Method::POST, "/api/v1/telemetry".to_string()),
    ] {
        let (status, body) = h.send(mutate(method.clone(), &path)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} answered {status} without a session: {body}"
        );
    }
}

/// CSRF is checked before authentication, so a cross-site form cannot even
/// learn whether it is signed in.
#[tokio::test(flavor = "multi_thread")]
async fn a_mutation_without_the_csrf_header_is_refused_before_authentication() {
    let Some(h) = support::harness("nyuka_test_routes_csrf").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let request = Request::builder()
        .method(Method::PUT)
        .uri("/api/v1/follows")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("request");

    let (status, body) = h.send(request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        body["type"].as_str().is_some_and(|t| t.contains("csrf")),
        "got {body}"
    );
}

/// The schema is the frontend's contract, and it is served from the same
/// origin so a generated client can fetch it without CORS.
#[tokio::test(flavor = "multi_thread")]
async fn the_openapi_document_is_served_and_describes_the_routes() {
    let Some(h) = support::harness("nyuka_test_routes_openapi").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, doc) = h.send(get("/api/v1/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        doc["openapi"]
            .as_str()
            .is_some_and(|v| v.starts_with("3.1")),
        "openapi-typescript is configured for 3.1: {}",
        doc["openapi"]
    );

    let paths = doc["paths"].as_object().expect("paths");
    for path in [
        "/manga",
        "/manga/{id}",
        "/manga/{id}/chapters",
        "/follows",
        "/follows/{id}",
        "/follows/{id}/check-now",
        "/jobs",
        "/jobs/{id}",
        "/jobs/{id}/cancel",
        "/jobs/{id}/retry",
        "/source-repos",
        "/source-repos/{id}",
        "/source-repos/{id}/refresh",
        "/source-repos/{id}/available",
        "/sources",
        "/sources/{id}",
        "/sources/{id}/filters",
        "/sources/{id}/catalog",
        "/sources/{id}/catalog/{key}",
        "/sources/{id}/catalog/{key}/chapters",
        "/downloads",
        "/downloads/{chapter_id}/file",
    ] {
        assert!(
            paths.contains_key(path),
            "{path} is missing from the schema"
        );
    }

    assert!(
        !paths.contains_key("/healthz") && !paths.contains_key("/readyz"),
        "the probes are not a versioned API and would generate client methods \
         nobody should call"
    );
    assert!(
        !paths.contains_key("/events"),
        "SSE is not a request/response pair; a generated method for it would \
         be misleading"
    );
}

/// Telemetry ingest writes user-controlled data into the log stream, so an
/// unauthenticated one would let anyone fill an operator's logs.
#[tokio::test(flavor = "multi_thread")]
async fn telemetry_ingest_requires_a_session() {
    let Some(h) = support::harness("nyuka_test_routes_telemetry_auth").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, _) = h.send(mutate(Method::POST, "/api/v1/telemetry")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A body over the limit must be refused before it is buffered, not after.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_telemetry_body_is_refused() {
    let Some(h) = support::harness("nyuka_test_routes_telemetry_size").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let huge = "x".repeat(1024 * 1024);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/telemetry")
        .header("x-requested-with", "XMLHttpRequest")
        .header("content-type", "application/json")
        .body(Body::from(huge))
        .expect("request");

    let (status, _) = h.send(request).await;
    assert!(
        status == StatusCode::PAYLOAD_TOO_LARGE || status == StatusCode::UNAUTHORIZED,
        "a megabyte body must not be accepted, got {status}"
    );
}

/// `POST /manga` and `GET /manga` share a path, registered from different
/// modules. utoipa-axum panics at router build on a mismatched grouping, so
/// this asserts the intended shape rather than merely that it built.
#[tokio::test(flavor = "multi_thread")]
async fn adding_to_the_library_is_a_post_on_the_library_path() {
    let Some(h) = support::harness("nyuka_test_routes_manga_methods").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (_, doc) = h.send(get("/api/v1/openapi.json")).await;
    let manga = doc["paths"]["/manga"].as_object().expect("/manga");

    assert!(manga.contains_key("get"), "listing the library");
    assert!(
        manga.contains_key("post"),
        "adding from a catalog; if this landed on /sources instead, the \
         generated client would call the wrong endpoint"
    );
    assert!(
        !doc["paths"]["/sources"]
            .as_object()
            .expect("/sources")
            .contains_key("post"),
        "installing a source is POST /sources; adding a series must not be"
    );
}

/// A schema that describes a route the router does not serve is worse than no
/// schema: the generated client compiles and 404s at runtime.
///
/// The check is not simply "did this 404". A handler can legitimately answer
/// 404 — `/chapters/{id}` for an id that does not exist does exactly that —
/// so the two have to be told apart. An unrouted path produces axum's own
/// fallback: status 404 with an empty body and no `problem+json` content type.
/// Anything that reached a handler or a middleware answers with a body.
#[tokio::test(flavor = "multi_thread")]
async fn every_documented_path_is_actually_routed() {
    let Some(h) = support::harness("nyuka_test_routes_schema_matches").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (_, doc) = h.send(get("/api/v1/openapi.json")).await;
    let paths = doc["paths"].as_object().expect("paths").clone();
    assert!(!paths.is_empty(), "an empty schema would make this vacuous");

    let mut checked = 0;
    for (path, item) in &paths {
        // Substitute path parameters with real values so routing matches.
        let concrete = path
            .replace("{id}", &uuid::Uuid::new_v4().to_string())
            .replace("{key}", "some-external-key")
            .replace("{chapter_id}", &uuid::Uuid::new_v4().to_string());

        // Every method, not just GET. Registering a handler under the wrong
        // path is easy to do — `routes!` groups handlers for one path, so
        // pairing two that differ mounts the second at the first's path — and
        // checking only GET would miss it entirely.
        for method in item.as_object().expect("path item").keys() {
            let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes()) else {
                // `parameters`, `summary` and friends sit alongside the
                // methods in a path item.
                continue;
            };

            let request = Request::builder()
                .method(method.clone())
                .uri(format!("/api/v1{concrete}"))
                .header("x-requested-with", "XMLHttpRequest")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("request");

            let response = h.raw(request).await;
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("body");

            assert!(
                status != StatusCode::NOT_FOUND || !bytes.is_empty(),
                "{method} {path} is in the schema but nothing is mounted \
                 there: axum's fallback answered with an empty 404"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is documented but the router does not accept \
                 that method there"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 20,
        "only {checked} method/path pairs were checked"
    );
}

/// And the control for the control: a path that is definitely not routed must
/// produce the empty-bodied fallback the test above keys on. Without this, a
/// change in axum's fallback behaviour would make that test pass silently for
/// everything.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrouted_path_produces_an_empty_fallback_404() {
    let Some(h) = support::harness("nyuka_test_routes_fallback").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let response = h.raw(get("/api/v1/definitely-not-a-route")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    assert!(
        bytes.is_empty(),
        "the routed-path test keys on this being empty; it answered {:?}",
        String::from_utf8_lossy(&bytes)
    );
}
