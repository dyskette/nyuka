//! `GET /healthz` and `GET /readyz`.
//!
//! The two answer different questions and a deployment breaks when they are
//! conflated:
//!
//! - **`/healthz` is liveness.** "Is this process running?" It must not touch
//!   the database. A liveness probe that checks a dependency restarts the
//!   process when the *dependency* fails, which turns a brief database blip
//!   into a crash loop that outlasts it.
//! - **`/readyz` is readiness.** "Should this process receive traffic?" It
//!   checks the database, whether migrations are current, and whether the
//!   library root is writable.
//!
//! A process on an out-of-date schema is reported separately from an
//! unreachable database, because reporting them together makes an incident
//! harder to read (ADR-0002).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub version: String,
}

/// Liveness. Deliberately checks nothing.
pub async fn healthz(State(state): State<Arc<AppState>>) -> Json<Health> {
    Json(Health {
        status: "ok",
        version: state.config.telemetry.service_version.clone(),
    })
}

#[derive(Debug, Serialize)]
pub struct Readiness {
    pub ready: bool,
    pub database: Check,
    pub migrations: Check,
    pub library: Check,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Check {
    fn ok() -> Self {
        Self {
            ok: true,
            detail: None,
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: Some(detail.into()),
        }
    }
}

/// Readiness. Returns `503` when not ready, so a load balancer acts on the
/// status rather than having to parse the body.
pub async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let db = nyuka_persistence::connect::readiness(&state.db).await;

    let database = if db.reachable {
        Check::ok()
    } else {
        Check::failed("the database is unreachable")
    };

    let migrations = if db.migrations_current {
        Check::ok()
    } else {
        Check::failed(format!(
            "{} migration(s) have not been applied: {}",
            db.pending.len(),
            db.pending.join(", ")
        ))
    };

    // Free space rather than a write probe: a probe would create and delete a
    // file on every check, and an unreadable root fails this call anyway,
    // which is the case that matters (ADR-0007).
    let library = match state.library.free_bytes().await {
        Ok(bytes) if bytes > 0 => Check::ok(),
        Ok(_) => Check::failed("the library volume has no space left"),
        Err(e) => Check::failed(format!("the library root is not usable: {e}")),
    };

    let ready = database.ok && migrations.ok && library.ok;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(Readiness {
            ready,
            database,
            migrations,
            library,
        }),
    )
        .into_response()
}
