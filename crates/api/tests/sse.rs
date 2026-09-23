//! The event stream, against the real router.
//!
//! ADR-0010 names one test explicitly — that `text/event-stream` responses are
//! not compressed — and calls it the failure most likely to be reintroduced by
//! a later middleware change. It is here, together with the access control
//! that decides who can open a stream at all.
//!
//! Skipped when `DATABASE_URL` is unset.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use nyuka_persistence::migration::{Migrator, MigratorTrait};

fn stream_request(accept_encoding: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri("/api/v1/events");
    if let Some(encoding) = accept_encoding {
        builder = builder.header(header::ACCEPT_ENCODING, encoding);
    }
    builder.body(Body::empty()).expect("request")
}

/// The stream is a credential-bearing channel like any other endpoint. It is
/// also the one that cannot send a header, which is why the cookie exists.
#[tokio::test(flavor = "multi_thread")]
async fn the_event_stream_requires_a_session() {
    let Some(h) = support::harness("nyuka_test_sse_auth").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.send(stream_request(None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body["type"]
            .as_str()
            .is_some_and(|t| t.contains("unauthenticated")),
        "got {body}"
    );
}

/// The test ADR-0010 asks for.
///
/// A compressor accumulates input before emitting output, so compressing this
/// response holds events even when every proxy in front is configured
/// correctly — a "real-time feature stopped working" bug with no error
/// anywhere.
///
/// Exercised against `nyuka_api::compression_layer()`, the same value the real
/// router mounts, over a real `text/event-stream` response. Going through the
/// router instead would need an authenticated session, and an unauthenticated
/// 401 is problem+json — which is legitimately compressible, so it would prove
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn sse_responses_are_never_compressed() {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use axum::routing::get;
    use tower::ServiceExt;

    let app = axum::Router::new()
        .route(
            "/stream",
            get(|| async {
                Sse::new(futures::stream::once(async {
                    Ok::<_, std::convert::Infallible>(
                        // Well over the 32-byte floor below which the
                        // predicate skips compression anyway, so a pass here
                        // means the exclusion did the work.
                        Event::default().event("job.state").data("x".repeat(4096)),
                    )
                }))
                .keep_alive(KeepAlive::new())
            }),
        )
        .layer(nyuka_api::compression_layer());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/stream")
                .header(header::ACCEPT_ENCODING, "gzip, br, deflate")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "the test is only meaningful against a real stream response"
    );
    assert!(
        response.headers().get(header::CONTENT_ENCODING).is_none(),
        "an SSE response must never be compressed; got {:?}",
        response.headers().get(header::CONTENT_ENCODING)
    );
}

/// And the other direction, so the test above cannot pass on a layer that
/// compresses nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_json_response_is_compressed() {
    use axum::routing::get;
    use tower::ServiceExt;

    let app = axum::Router::new()
        .route(
            "/big",
            get(|| async { axum::Json(serde_json::json!({ "padding": "y".repeat(4096) })) }),
        )
        .layer(nyuka_api::compression_layer());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/big")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "if the layer compresses nothing, the exclusion test proves nothing"
    );
}
