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

    for path in paths.keys() {
        // Substitute any path parameter with a real uuid so routing matches.
        let concrete = path.replace("{id}", &uuid::Uuid::new_v4().to_string());
        let response = h.raw(get(&format!("/api/v1{concrete}"))).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");

        assert!(
            status != StatusCode::NOT_FOUND || !bytes.is_empty(),
            "{path} is in the schema but nothing is mounted there: axum's \
             fallback answered with an empty 404"
        );
    }
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
