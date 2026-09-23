//! CSRF protection as a middleware invariant (ADR-0005).
//!
//! # Why a header is enough here, and exactly when it stops being enough
//!
//! The SPA is served from the same origin as the API, so there is no CORS
//! boundary to lean on and `SameSite=Lax` alone does not cover state-changing
//! requests. What does cover them is requiring a header a cross-site form
//! cannot set.
//!
//! An HTML form can issue a cross-site `POST` with the victim's cookies
//! attached, but it can only send `application/x-www-form-urlencoded`,
//! `multipart/form-data`, or `text/plain`, and it cannot set custom headers.
//! `fetch` and `XMLHttpRequest` can set them, but a cross-origin call with
//! custom headers triggers a CORS preflight — and this server answers no
//! preflight, so the browser never sends the real request.
//!
//! > [!WARNING]
//! > That argument has two load-bearing premises, and a future change can
//! > break either without touching this file:
//! >
//! > 1. **No endpoint accepts a form-encoded body.** The moment one does, a
//! >    cross-site form can reach it, and the header requirement is the only
//! >    thing left standing.
//! > 2. **No CORS layer is added** that would answer a preflight for this
//! >    origin.
//! >
//! > ADR-0005 asks for a test that a mutation without the header is rejected.
//! > It is at the bottom of this file, and it is the regression test for the
//! > whole scheme rather than for this function.
//!
//! Safe methods are exempt because they are not supposed to change anything.
//! An endpoint that mutates on `GET` is a bug this layer cannot see, which is
//! why that is not a convention but a rule.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::Method;

use crate::error::Problem;

/// The header a cross-site form cannot set.
///
/// `X-Requested-With` is conventional rather than magic — any custom header
/// would do — but it is the one the frontend sends and the one an operator
/// debugging with `curl` will find documented.
pub const CSRF_HEADER: &str = "x-requested-with";

/// Methods that must carry the header.
///
/// Derived from "does not change state" rather than listed by hand, so a
/// method added to HTTP does not silently land on the exempt side.
pub fn requires_csrf_header(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Rejects a state-changing request that does not carry [`CSRF_HEADER`].
pub async fn require_csrf_header(request: Request, next: Next) -> Response {
    if requires_csrf_header(request.method()) && !request.headers().contains_key(CSRF_HEADER) {
        return Problem::csrf_required(CSRF_HEADER)
            .with_instance(request.uri().path().to_string())
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route(
                "/mutate",
                post(|| async { "ok" })
                    .put(|| async { "ok" })
                    .patch(|| async { "ok" })
                    .delete(|| async { "ok" }),
            )
            .route("/read", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(require_csrf_header))
    }

    async fn call(request: HttpRequest<Body>) -> (StatusCode, String) {
        let response = app().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn request(method: Method, path: &str, with_header: bool) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().method(method).uri(path);
        if with_header {
            builder = builder.header(CSRF_HEADER, "XMLHttpRequest");
        }
        builder.body(Body::empty()).expect("request")
    }

    /// The test ADR-0005 asks for. This is the regression test for the whole
    /// scheme, not for one function.
    #[tokio::test]
    async fn a_mutation_without_the_header_is_rejected() {
        let (status, body) = call(request(Method::POST, "/mutate", false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body.contains("csrf-required"),
            "the refusal must name itself, or it reads as a permissions problem"
        );
        assert!(
            body.contains(CSRF_HEADER),
            "and it must name the header, or the caller is guessing"
        );
    }

    #[tokio::test]
    async fn a_mutation_with_the_header_is_allowed() {
        let (status, _) = call(request(Method::POST, "/mutate", true)).await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Every unsafe method, not just POST. A DELETE that slipped past this
    /// would be the most damaging one to miss.
    #[tokio::test]
    async fn every_state_changing_method_is_covered() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(
                requires_csrf_header(&method),
                "{method} changes state and must require the header"
            );
        }
    }

    #[tokio::test]
    async fn a_delete_without_the_header_is_rejected() {
        let (status, _) = call(request(Method::DELETE, "/mutate", false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// Safe methods are exempt. Requiring the header on `GET` would break
    /// `EventSource`, `<img>`, and anchor downloads — the three call shapes
    /// ADR-0005 chose cookies for precisely because they cannot set headers.
    #[tokio::test]
    async fn safe_methods_do_not_require_the_header() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE] {
            assert!(
                !requires_csrf_header(&method),
                "{method} must stay usable from a context that cannot set headers"
            );
        }

        let (status, _) = call(request(Method::GET, "/read", false)).await;
        assert_eq!(status, StatusCode::OK);
    }

    /// The value is never inspected. Checking it against a list would be
    /// security theatre: an attacker who could set the header could set any
    /// value, so presence is the whole signal.
    #[tokio::test]
    async fn the_header_value_is_not_inspected() {
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/mutate")
            .header(CSRF_HEADER, "anything at all")
            .body(Body::empty())
            .expect("request");
        let (status, _) = call(request).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_refusal_names_the_path_it_refused() {
        let (_, body) = call(request(Method::POST, "/mutate", false)).await;
        assert!(body.contains("/mutate"));
    }
}
