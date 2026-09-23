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

use std::sync::Arc;

use axum::extract::{Request, State};
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
///
/// With `AUTH_MODE=none` there is nothing to reject: every request is the
/// seeded local user. That substitution happens here rather than in each
/// handler, so no handler has a second code path and none can forget one.
pub async fn require_session(
    State(state): State<Arc<crate::state::AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(local) = state.local_user {
        request.extensions_mut().insert(CurrentUser(local));
        return next.run(request).await;
    }

    let session = request.extensions().get::<Session>().cloned();
    let user_id = match resolve_session_user(session).await {
        Ok(id) => id,
        Err(response) => return *response,
    };

    request.extensions_mut().insert(CurrentUser(user_id));
    next.run(request).await
}

/// Resolves the signed-in user from the session, or the response to send.
///
/// Split out from the middleware so the failure below stays testable: a
/// middleware taking `State<AppState>` cannot be mounted on a bare router,
/// and that assertion is the reason the branch exists at all.
async fn resolve_session_user(session: Option<Session>) -> Result<UserId, Box<Response>> {
    // The session layer runs outside this one, so the extension is present.
    // Its absence means the layers were mounted in the wrong order, which is a
    // server bug and must not read as "not signed in" — that would turn a
    // misconfiguration into a login loop with no error anywhere.
    let Some(session) = session else {
        tracing::error!("the session layer is not mounted outside the authentication guard");
        return Err(Box::new(Problem::internal().into_response()));
    };

    let stored: Option<String> = match session.get(USER_ID_KEY).await {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "reading the session failed");
            return Err(Box::new(Problem::internal().into_response()));
        }
    };

    stored
        .as_deref()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .map(UserId)
        .ok_or_else(|| Box::new(Problem::unauthenticated().into_response()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    /// Without the session layer mounted outside it, the guard must fail as a
    /// server error rather than as "not signed in".
    #[tokio::test]
    async fn a_missing_session_layer_is_a_server_fault_not_a_login_prompt() {
        let response = resolve_session_user(None)
            .await
            .expect_err("no session layer");

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
