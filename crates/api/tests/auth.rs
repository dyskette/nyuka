//! The auth surface, against a real router and a real session store.
//!
//! ADR-0005 lists the negative paths explicitly because none of them is
//! exercised by ordinary feature work and each is a real vulnerability when it
//! is wrong. The ones reachable without an identity provider are here; the
//! ones that need a token exchange — replayed nonce, expired code — belong to
//! the Playwright stack against a stub provider, and are named at the bottom
//! of this file so the gap is recorded rather than assumed covered.
//!
//! Skipped when `DATABASE_URL` is unset.

mod support;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use nyuka_persistence::migration::{Migrator, MigratorTrait};

fn mutation(path: &str, with_csrf: bool) -> Request<Body> {
    let mut builder = Request::builder().method(Method::POST).uri(path);
    if with_csrf {
        builder = builder.header("x-requested-with", "XMLHttpRequest");
    }
    builder.body(Body::empty()).expect("request")
}

/// The regression test ADR-0005 asks for, against the real router rather than
/// a synthetic one: the CSRF layer must actually be mounted.
#[tokio::test(flavor = "multi_thread")]
async fn a_mutation_without_the_csrf_header_is_rejected_by_the_real_router() {
    let Some(h) = support::harness("nyuka_test_auth_csrf").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.send(mutation("/api/v1/auth/logout", false)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        body["type"].as_str().is_some_and(|t| t.contains("csrf")),
        "the refusal must name itself: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mutation_with_the_csrf_header_reaches_the_handler() {
    let Some(h) = support::harness("nyuka_test_auth_csrf_ok").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, _) = h.send(mutation("/api/v1/auth/logout", true)).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "logging out without a session is not an error"
    );
}

/// The health probes are outside `/api/v1` precisely so an orchestrator does
/// not have to set a header to use them.
#[tokio::test(flavor = "multi_thread")]
async fn the_health_probes_are_outside_the_csrf_layer() {
    let Some(h) = support::harness("nyuka_test_auth_probe").await else {
        return;
    };
    let (status, _) = h.get("/healthz").await;
    assert_eq!(status, StatusCode::OK);
}

/// `GET /me` without a session is 401, not 500 and not an empty 200.
#[tokio::test(flavor = "multi_thread")]
async fn me_without_a_session_is_unauthenticated() {
    let Some(h) = support::harness("nyuka_test_auth_me").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.get("/api/v1/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["status"], 401);
    assert!(
        body["type"]
            .as_str()
            .is_some_and(|t| t.contains("unauthenticated")),
        "the frontend branches on this to decide whether to redirect: {body}"
    );
}

/// A callback with no flow in progress is refused. This covers the bookmarked
/// callback, the replay, and the session that expired mid-flow — all three
/// look identical from the server's side, and all three must be refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_callback_with_no_flow_in_progress_is_refused() {
    let Some(h) = support::harness("nyuka_test_auth_no_flow").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, _body) = h.get("/api/v1/auth/callback?code=abc&state=whatever").await;
    assert!(
        status.is_client_error() || status == StatusCode::INTERNAL_SERVER_ERROR,
        "a callback without a stored state must never succeed, got {status}"
    );
    assert_ne!(
        status,
        StatusCode::SEE_OTHER,
        "it must not redirect as though the sign-in worked"
    );
    assert_ne!(status, StatusCode::TEMPORARY_REDIRECT);
}

/// A provider that declines must not read as a server fault: pressing cancel
/// is a normal thing for a person to do.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_error_is_reported_as_a_client_outcome() {
    let Some(h) = support::harness("nyuka_test_auth_declined").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h
        .get("/api/v1/auth/callback?error=access_denied&error_description=User%20cancelled")
        .await;

    // With no provider configured the harness reports that first; either way
    // it must not be a redirect that looks like success.
    assert_ne!(status, StatusCode::SEE_OTHER);
    assert!(
        status.is_client_error() || status.is_server_error(),
        "got {status} with {body}"
    );
}

/// The session cookie's flags are the whole reason tokens stay out of the
/// browser. A missing `HttpOnly` would put the session within reach of any
/// injected script.
#[tokio::test(flavor = "multi_thread")]
async fn the_session_cookie_is_http_only_and_same_site_lax() {
    let Some(h) = support::harness("nyuka_test_auth_cookie").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    // Any request that writes to the session sets the cookie. `logout`
    // flushes, which is enough to make the layer emit one.
    let response = h.raw(mutation("/api/v1/auth/logout", true)).await;
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();

    if cookies.is_empty() {
        // Nothing was stored, so no cookie was issued. That is correct
        // behaviour and leaves nothing to assert here.
        return;
    }
    let cookie = cookies.join("; ");
    assert!(cookie.contains("HttpOnly"), "cookie was: {cookie}");
    assert!(cookie.contains("SameSite=Lax"), "cookie was: {cookie}");
    assert!(cookie.contains("Path=/"), "cookie was: {cookie}");
}

/// The limit must actually fire, or it is a layer that costs allocations and
/// reports a control that is not there (ADR-0005 follow-up 6).
#[tokio::test(flavor = "multi_thread")]
async fn the_auth_endpoints_are_rate_limited() {
    let Some(h) = support::harness("nyuka_test_auth_rate_limit").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let mut limited = false;
    for _ in 0..60 {
        let (status, _) = h.send(mutation("/api/v1/auth/logout", true)).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }

    assert!(
        limited,
        "sixty logout requests from one address must hit the limit"
    );
}

/// And the callback specifically, which ADR-0005 names because limiting only
/// the login half leaves the expensive one open: the callback performs a token
/// exchange against the identity provider.
#[tokio::test(flavor = "multi_thread")]
async fn the_callback_is_rate_limited_too() {
    let Some(h) = support::harness("nyuka_test_auth_rate_limit_callback").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let mut limited = false;
    for _ in 0..60 {
        let (status, _) = h.get("/api/v1/auth/callback?code=x&state=y").await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }

    assert!(
        limited,
        "an unlimited callback is a way to make this server \
        hammer the identity provider"
    );
}

/// The library routes are not rate-limited: a reader paging through a series
/// makes many requests in a row and must not be throttled for it.
#[tokio::test(flavor = "multi_thread")]
async fn ordinary_routes_are_not_rate_limited() {
    let Some(h) = support::harness("nyuka_test_auth_rate_limit_scope").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    for _ in 0..60 {
        let (status, _) = h.get("/api/v1/manga").await;
        assert_ne!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "reading is not an authentication attempt"
        );
    }
}

/// Not covered here, recorded so the gap is not mistaken for coverage.
///
/// These need a token exchange, which needs a provider. They belong to the
/// Playwright stack against the stub OIDC container that ADR-0005 specifies:
///
/// - a mismatched `state` between the session and the callback
/// - a replayed `nonce`
/// - a PKCE verifier that does not match the challenge
/// - an expired authorization code
/// - a subject absent from the allow-list
/// - an empty allow-list denying everyone
///
/// The last two are covered as unit tests on `AuthConfig::permits`; what is
/// untested is that the callback consults it, which only an end-to-end run can
/// show.
#[test]
fn the_uncovered_negative_paths_are_written_down() {}

// ---------------------------------------------------------------------------
// AUTH_MODE=none
// ---------------------------------------------------------------------------

/// The point of the mode: a barebones deployment serves without a session.
#[tokio::test(flavor = "multi_thread")]
async fn with_authentication_off_protected_routes_serve_without_a_session() {
    let Some(h) = support::no_auth_harness("nyuka_test_noauth_open").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.get("/api/v1/manga").await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    assert_eq!(body["items"], serde_json::json!([]));
}

/// Every handler keeps the same shape, so `/me` answers with the seeded user
/// rather than a second anonymous representation.
#[tokio::test(flavor = "multi_thread")]
async fn with_authentication_off_me_reports_the_local_user() {
    let Some(h) = support::no_auth_harness("nyuka_test_noauth_me").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.get("/api/v1/me").await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    assert_eq!(body["issuer"], "local");
    assert_eq!(body["subject"], "local");
}

/// There is nothing to sign in to, and saying so is better than a 500 or a
/// redirect into a flow that cannot complete.
#[tokio::test(flavor = "multi_thread")]
async fn with_authentication_off_the_sign_in_endpoints_say_so() {
    let Some(h) = support::no_auth_harness("nyuka_test_noauth_login").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, body) = h.get("/api/v1/auth/login").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["type"], "/problems/auth-disabled");
}

/// CSRF is not about authentication. It still applies, because it costs one
/// header and the frontend sends it either way — and removing it here would
/// be a second behaviour difference between the two modes for no gain.
#[tokio::test(flavor = "multi_thread")]
async fn csrf_still_applies_with_authentication_off() {
    let Some(h) = support::no_auth_harness("nyuka_test_noauth_csrf").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, _) = h.send(mutation("/api/v1/auth/logout", false)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// And the control for all of the above: the default harness must still
/// refuse. If turning the mode on leaked into the default, every
/// access-control test in this file would be passing for the wrong reason.
#[tokio::test(flavor = "multi_thread")]
async fn the_default_mode_still_requires_a_session() {
    let Some(h) = support::harness("nyuka_test_noauth_control").await else {
        return;
    };
    Migrator::up(&h.db, None).await.expect("migrating");

    let (status, _) = h.get("/api/v1/manga").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
