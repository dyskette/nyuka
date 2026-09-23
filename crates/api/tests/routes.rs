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
        format!("/api/v1/sources/{id}/settings"),
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
        (Method::POST, "/api/v1/jobs".to_string()),
        (Method::PUT, format!("/api/v1/sources/{id}/settings")),
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
        "/sources/{id}/settings",
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
    // `/sources` also has a POST — installing a source — and the two must be
    // different operations rather than one having landed on the other's path.
    // This previously asserted that `/sources` had *no* POST, which passed
    // only because `install` was never mounted: the assertion was encoding
    // the bug rather than catching it.
    let sources = doc["paths"]["/sources"].as_object().expect("/sources");
    assert!(sources.contains_key("post"), "installing a source");
    assert_ne!(
        manga["post"]["operationId"], sources["post"]["operationId"],
        "adding a series and installing a source are different operations"
    );
    assert_eq!(manga["post"]["operationId"], "addMangaToLibrary");
    assert_eq!(sources["post"]["operationId"], "installSource");
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

            let body: serde_json::Value =
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            let unrouted = status == StatusCode::NOT_FOUND
                && body["type"]
                    .as_str()
                    .is_some_and(|t| t.ends_with("no-such-endpoint"));
            assert!(
                !unrouted,
                "{method} {path} is in the schema but nothing is mounted there"
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

/// The served document and the committed one must be the same document.
///
/// CI regenerates and diffs, which catches a schema changed without
/// regenerating. This catches the other direction — a committed file edited
/// by hand, which the diff would accept if the edit happened to match what
/// the generator produces on that run but not on the next.
#[tokio::test(flavor = "multi_thread")]
async fn the_served_document_matches_the_committed_one() {
    let Some(h) = support::harness("nyuka_test_routes_schema_committed").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (_, served) = h.send(get("/api/v1/openapi.json")).await;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/openapi.json");
    let Ok(committed) = std::fs::read_to_string(path) else {
        panic!("web/openapi.json is missing; run `cargo xtask openapi`");
    };
    let committed: serde_json::Value =
        serde_json::from_str(&committed).expect("committed schema is json");

    assert_eq!(
        served, committed,
        "the committed schema does not match what the server serves; run \
         `cargo xtask openapi` and commit the result"
    );
}

/// And the control for the control: a path that is definitely not routed must
/// answer with the distinct `no-such-endpoint` problem the test above keys on.
/// Without this, a change to that problem type would make the check pass for
/// everything.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrouted_api_path_reports_no_such_endpoint() {
    let Some(h) = support::harness("nyuka_test_routes_fallback").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.send(get("/api/v1/definitely-not-a-route")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["type"]
            .as_str()
            .is_some_and(|t| t.ends_with("no-such-endpoint")),
        "the routed-path test keys on this type: {body}"
    );
    assert_eq!(
        body["instance"], "/api/v1/definitely-not-a-route",
        "the instance must be the URL the client asked for, not the path left \
         after nesting stripped the prefix"
    );
}

/// The two kinds of 404 must be distinguishable. "This URL does not exist"
/// and "the thing you asked for does not exist" call for different actions,
/// and a client that cannot tell them apart retries a typo forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_endpoint_is_a_different_problem_from_a_missing_resource() {
    let Some(h) = support::harness("nyuka_test_routes_404_kinds").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (_, unrouted) = h.send(get("/api/v1/nope")).await;
    assert_ne!(
        unrouted["type"].as_str(),
        Some("/problems/not-found"),
        "an unrouted URL must not answer with the same problem a missing \
         resource does"
    );
    assert_eq!(unrouted["type"], "/problems/no-such-endpoint");
}
