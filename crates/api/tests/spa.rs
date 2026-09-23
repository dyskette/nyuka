//! The embedded SPA and its route precedence (ADR-0006).
//!
//! ADR-0006 names two tests explicitly, and both guard failures that are
//! invisible until someone hits them:
//!
//! - The fallback is a sharp edge. Mounted before the API routes, or widened,
//!   it swallows API 404s: a mistyped fetch returns `index.html` with `200`
//!   and the client reports an HTML parse error rather than a missing
//!   endpoint.
//! - Cache headers backwards produce a stale UI that survives a redeploy.
//!
//! Skipped when `DATABASE_URL` is unset.

mod support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request")
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The first half of ADR-0006 follow-up 1.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_api_path_returns_problem_json_not_the_shell() {
    let Some(h) = support::harness("nyuka_test_spa_api_404").await else {
        return;
    };

    let response = h.raw(get("/api/v1/does-not-exist")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("application/problem+json"),
        "a mistyped fetch must report a missing endpoint, not an HTML parse \
         error; content type was {content_type:?}"
    );

    let body = body_text(response).await;
    assert!(
        !body.contains("<!doctype html") && !body.contains("<html"),
        "the SPA shell reached an API path: {body}"
    );
}

/// Nested API paths too, which is where a `/*rest` fallback would differ from
/// a bare one.
#[tokio::test(flavor = "multi_thread")]
async fn a_deeply_nested_unknown_api_path_is_also_problem_json() {
    let Some(h) = support::harness("nyuka_test_spa_api_404_nested").await else {
        return;
    };

    for path in [
        "/api/v1/manga/not-a-uuid/nonsense",
        "/api/v1/sources/x/y/z",
        "/api/v1/",
    ] {
        let response = h.raw(get(path)).await;
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = body_text(response).await;
        assert!(
            !body.contains("<html"),
            "{path} fell through to the SPA shell (content type {content_type:?})"
        );
    }
}

/// The second half of ADR-0006 follow-up 1: a client-side route is not a
/// missing resource.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_non_api_path_returns_the_shell() {
    let Some(h) = support::harness("nyuka_test_spa_deep_link").await else {
        return;
    };

    for path in ["/library", "/library/123", "/settings/sources"] {
        let response = h.raw(get(path)).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} is a deep link the router handles once the shell loads"
        );
        let body = body_text(response).await;
        assert!(
            body.contains("<html") || body.contains("<!doctype"),
            "{path}: {body}"
        );
    }
}

/// ADR-0006 follow-up 2. Getting these backwards produces a stale UI that
/// survives a redeploy.
#[tokio::test(flavor = "multi_thread")]
async fn index_html_is_revalidated_and_hashed_assets_are_immutable() {
    let Some(h) = support::harness("nyuka_test_spa_cache").await else {
        return;
    };

    let shell = h.raw(get("/index.html")).await;
    let cache = shell
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        cache, "no-cache",
        "index.html's URL never changes and its contents change every deploy"
    );

    // Only meaningful against a real frontend build; a placeholder has no
    // hashed assets, and asserting against one would be asserting nothing.
    let Some(asset) = hashed_asset() else {
        eprintln!("skipping the immutable half: no frontend build is embedded");
        return;
    };

    let response = h.raw(get(&format!("/{asset}"))).await;
    assert_eq!(response.status(), StatusCode::OK, "serving {asset}");
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some(nyuka_api::static_files::IMMUTABLE_CACHE)
    );
}

/// The first content-hashed asset in the embedded build, if there is one.
fn hashed_asset() -> Option<String> {
    nyuka_api::static_files::Assets::iter()
        .map(|p| p.to_string())
        .find(|p| nyuka_api::static_files::is_content_hashed(p))
}

/// An `ETag` that never matches is an `ETag` that saves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_matching_etag_gets_a_304() {
    let Some(h) = support::harness("nyuka_test_spa_etag").await else {
        return;
    };

    let first = h.raw(get("/index.html")).await;
    let etag = first
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("an etag")
        .to_string();

    let second = h
        .raw(
            Request::builder()
                .uri("/index.html")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await;

    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert!(
        body_text(second).await.is_empty(),
        "a 304 must not carry the body it just told the client it already has"
    );
}

/// The probes are above the fallback, or an orchestrator would get HTML.
#[tokio::test(flavor = "multi_thread")]
async fn the_health_probes_are_not_swallowed_by_the_fallback() {
    let Some(h) = support::harness("nyuka_test_spa_probes").await else {
        return;
    };

    let response = h.raw(get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_text(response).await.contains("\"status\""),
        "an orchestrator must get JSON, not the SPA shell"
    );
}
