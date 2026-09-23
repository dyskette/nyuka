//! The middleware that requires a session.
//!
//! Applied to `/api/v1` as a whole rather than per handler, with the auth
//! endpoints themselves mounted outside it — a login route behind a login
//! check has no way to let anyone in.
//!
//! # Identity comes from the session and nowhere else
//!
//! Not from a header, not from a query parameter, not from anything a proxy
//! put there. `TRUSTED_PROXIES` governs the client *address* for rate limiting
//! and logging; it does not confer identity. An application that trusts
//! `X-Forwarded-User` is fully compromised the moment it becomes reachable by
//! any other route (ADR-0005).

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::UserId;
use tower_sessions::Session;

use crate::error::Problem;
use crate::session_store::USER_ID_KEY;

/// The authenticated user, inserted into request extensions by
/// [`require_session`] so a handler can take it without re-reading the
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentUser(pub UserId);

/// Rejects a request that carries no authenticated session.
pub async fn require_session(mut request: Request, next: Next) -> Response {
    // The session layer runs outside this one, so the extension is present.
    // Its absence means the layers were mounted in the wrong order, which is a
    // server bug and must not read as "not signed in" — that would turn a
    // misconfiguration into a login loop with no error anywhere.
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        tracing::error!("the session layer is not mounted outside the authentication guard");
        return Problem::internal().into_response();
    };

    let stored: Option<String> = match session.get(USER_ID_KEY).await {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return Problem::internal().into_response();
        }
    };

    let Some(user_id) = stored
        .as_deref()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
    else {
        return Problem::unauthenticated()
            .with_instance(request.uri().path().to_string())
            .into_response();
    };

    request
        .extensions_mut()
        .insert(CurrentUser(UserId(user_id)));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    /// Without the session layer mounted outside it, the guard must fail as a
    /// server error rather than as "not signed in".
    #[tokio::test]
    async fn a_missing_session_layer_is_a_server_fault_not_a_login_prompt() {
        let app = Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(require_session));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a 401 here would turn a misconfiguration into a login loop with \
             no error anywhere"
        );

        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(
            !body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("session layer"),
            "the cause belongs in the log, not in the response"
        );
    }
}
