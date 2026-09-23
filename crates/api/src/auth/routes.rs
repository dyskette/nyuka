//! `/api/v1/auth/*` and `GET /api/v1/me`.
//!
//! The negative paths here are the point. ADR-0005 lists them explicitly
//! because none is exercised by ordinary feature work, and each is a real
//! vulnerability when it is wrong: a mismatched `state`, a replayed `nonce`, a
//! missing PKCE verifier, a subject absent from the allow-list, and an empty
//! allow-list denying everyone.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use openidconnect::core::CoreAuthenticationFlow;
use openidconnect::{
    AuthorizationCode, CsrfToken, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::auth::{NONCE_KEY, PKCE_VERIFIER_KEY, RETURN_TO_KEY, STATE_KEY, is_safe_return_to};
use crate::error::{ApiError, ApiResult, Problem};
use crate::session_store::USER_ID_KEY;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Where to send the browser afterwards. Validated, never echoed raw.
    pub return_to: Option<String>,
}

/// `GET /api/v1/auth/login` — starts the flow.
pub async fn login(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<LoginQuery>,
) -> ApiResult<Response> {
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        ApiError(Box::new(
            Problem::internal().with_detail("This server is not configured for sign-in."),
        ))
    })?;

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf, nonce) = oidc
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        // `openid` is required; `profile` and `email` are what a `groups`
        // claim usually rides alongside. `offline_access` is deliberately not
        // requested: nothing here needs a refresh token, and asking for one
        // means storing a long-lived credential for no purpose (ADR-0005).
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(challenge)
        .url();

    // Server-side. A `state` the client could rewrite proves nothing.
    session
        .insert(STATE_KEY, csrf.secret())
        .await
        .map_err(session_failed)?;
    session
        .insert(NONCE_KEY, nonce.secret())
        .await
        .map_err(session_failed)?;
    session
        .insert(PKCE_VERIFIER_KEY, verifier.secret())
        .await
        .map_err(session_failed)?;

    // An unsafe target is dropped rather than refused: the sign-in should
    // still work, it just lands on the default page.
    let return_to = query.return_to.filter(|t| is_safe_return_to(t));
    session
        .insert(RETURN_TO_KEY, return_to)
        .await
        .map_err(session_failed)?;

    Ok(Redirect::to(url.as_str()).into_response())
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    /// The provider's own failure, such as `access_denied`.
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// `GET /api/v1/auth/callback` — finishes the flow.
pub async fn callback(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<CallbackQuery>,
) -> ApiResult<Response> {
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        ApiError(Box::new(
            Problem::internal().with_detail("This server is not configured for sign-in."),
        ))
    })?;

    // Take the flow's secrets out of the session before anything else, so a
    // replayed callback finds nothing to validate against no matter which
    // branch below it would have taken.
    let expected_state: Option<String> = session.remove(STATE_KEY).await.map_err(session_failed)?;
    let nonce: Option<String> = session.remove(NONCE_KEY).await.map_err(session_failed)?;
    let verifier: Option<String> = session
        .remove(PKCE_VERIFIER_KEY)
        .await
        .map_err(session_failed)?;
    let return_to: Option<Option<String>> = session
        .remove(RETURN_TO_KEY)
        .await
        .map_err(session_failed)?;

    if let Some(error) = query.error {
        // The provider declined. This is `access_denied` when a user pressed
        // cancel, which is not a server fault.
        return Err(ApiError(Box::new(
            Problem::new(
                axum::http::StatusCode::BAD_REQUEST,
                "sign-in-declined",
                "Sign-in was not completed",
            )
            .with_detail(format!(
                "The identity provider returned `{error}`{}.",
                query
                    .error_description
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            )),
        )));
    }

    let (Some(expected_state), Some(nonce), Some(verifier)) = (expected_state, nonce, verifier)
    else {
        // No flow in progress: a bookmarked callback, a replay, or a session
        // that expired mid-flow. All three look the same and all three are
        // refused.
        return Err(flow_not_started());
    };

    let Some(received_state) = query.state else {
        return Err(flow_not_started());
    };

    // Constant-time: `state` is a secret the comparison is gating on.
    if !constant_time_eq(expected_state.as_bytes(), received_state.as_bytes()) {
        return Err(ApiError(Box::new(
            Problem::new(
                axum::http::StatusCode::BAD_REQUEST,
                "state-mismatch",
                "Sign-in could not be verified",
            )
            .with_detail("The sign-in did not start in this browser. Try again."),
        )));
    }

    let Some(code) = query.code else {
        return Err(flow_not_started());
    };

    let tokens = oidc
        .client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| {
            upstream(format!(
                "the authorization code could not be exchanged: {e}"
            ))
        })?
        .set_pkce_verifier(PkceCodeVerifier::new(verifier))
        .request_async(&oidc.http)
        .await
        .map_err(|e| upstream(format!("the token request failed: {e}")))?;

    let id_token = tokens
        .id_token()
        .ok_or_else(|| upstream("the provider returned no ID token".to_string()))?;

    let verifier = oidc.client.id_token_verifier();
    let claims = id_token
        .claims(&verifier, &Nonce::new(nonce))
        .map_err(|e| upstream(format!("the ID token could not be verified: {e}")))?;

    // Guards against an access token substituted for another user's. Only
    // present when the provider sends it, which is why it is checked rather
    // than required.
    if let Some(expected) = claims.access_token_hash() {
        let actual = openidconnect::AccessTokenHash::from_token(
            tokens.access_token(),
            id_token
                .signing_alg()
                .map_err(|e| upstream(e.to_string()))?,
            id_token
                .signing_key(&verifier)
                .map_err(|e| upstream(e.to_string()))?,
        )
        .map_err(|e| upstream(e.to_string()))?;
        if actual != *expected {
            return Err(upstream(
                "the access token does not match the ID token".to_string(),
            ));
        }
    }

    let subject = claims.subject().as_str();
    let groups = groups_from(claims);

    // Fail-closed, with the config validated at startup.
    if !state.config.auth.permits(subject, &groups) {
        tracing::warn!(
            // Not the subject: it is the person's identity at their provider.
            issuer = %oidc.issuer(),
            "refused a sign-in for a subject that is not on the allow-list"
        );
        return Err(ApiError(Box::new(Problem::forbidden())));
    }

    let user = state
        .users
        .record_sign_in(oidc.issuer(), subject)
        .await
        .map_err(ApiError::from)?;

    // A new id for the authenticated session, so a session fixated before
    // login is not the one that ends up authenticated.
    session.cycle_id().await.map_err(session_failed)?;
    session
        .insert(USER_ID_KEY, user.id.0.to_string())
        .await
        .map_err(session_failed)?;

    let target = return_to
        .flatten()
        .filter(|t| is_safe_return_to(t))
        .unwrap_or_else(|| "/".to_string());

    tracing::info!(user.id = %user.id, "signed in");
    Ok(Redirect::to(&target).into_response())
}

/// `POST /api/v1/auth/logout` — ends the session immediately.
///
/// Deletes the row rather than waiting for expiry, which is the whole reason
/// sessions are server-side.
pub async fn logout(session: Session) -> ApiResult<Response> {
    session.flush().await.map_err(session_failed)?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Serialize)]
pub struct Me {
    pub id: uuid::Uuid,
    pub issuer: String,
    pub subject: String,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /api/v1/me` — who the session belongs to.
pub async fn me(State(state): State<Arc<AppState>>, session: Session) -> ApiResult<Json<Me>> {
    let user_id: Option<String> = session.get(USER_ID_KEY).await.map_err(session_failed)?;
    let user_id = user_id
        .and_then(|id| uuid::Uuid::parse_str(&id).ok())
        .ok_or_else(|| ApiError(Box::new(Problem::unauthenticated())))?;

    let user = state
        .users
        .get(nyuka_domain::model::UserId(user_id))
        .await
        // A session naming a user that no longer exists is not a 404 for the
        // caller — it is a session that is no longer good.
        .map_err(|_| ApiError(Box::new(Problem::unauthenticated())))?;

    Ok(Json(Me {
        id: user.id.0,
        issuer: user.issuer,
        subject: user.subject,
        last_seen_at: user.last_seen_at,
    }))
}

/// Reads the groups claim.
///
/// A missing claim is an empty list rather than an error: for a provider that
/// does not emit `groups`, membership simply does not decide anything, and the
/// subject allow-list still does.
fn groups_from(
    claims: &openidconnect::IdTokenClaims<
        crate::auth::GroupClaims,
        openidconnect::core::CoreGenderClaim,
    >,
) -> Vec<String> {
    claims.additional_claims().groups.clone()
}

fn session_failed(e: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %e, "session storage failed");
    ApiError(Box::new(Problem::internal()))
}

fn upstream(detail: String) -> ApiError {
    ApiError(Box::new(
        Problem::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "provider-unavailable",
            "The identity provider could not complete the sign-in",
        )
        .with_detail(detail),
    ))
}

fn flow_not_started() -> ApiError {
    ApiError(Box::new(
        Problem::new(
            axum::http::StatusCode::BAD_REQUEST,
            "no-sign-in-in-progress",
            "No sign-in is in progress",
        )
        .with_detail("Start again from the sign-in page."),
    ))
}

/// Compares two byte strings without an early return.
///
/// `state` is a secret, and a byte-at-a-time comparison leaks how much of a
/// guess was right through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_strings_compare_equal() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
    }

    #[test]
    fn different_strings_do_not() {
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc123", b"xbc123"));
        assert!(
            !constant_time_eq(b"abc", b"abc123"),
            "a prefix must not compare equal"
        );
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn empty_strings_compare_equal() {
        assert!(constant_time_eq(b"", b""));
    }
}
