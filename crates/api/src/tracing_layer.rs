//! The per-request span, and the trace id on the way out (ADR-0015).
//!
//! `axum-tracing-opentelemetry` creates the server span and continues an
//! incoming `traceparent`, which is what lets a browser-originated trace and
//! this server's log lines carry the same `trace_id`.
//!
//! # `try_extract_client_ip` is deliberately off
//!
//! The layer can record `client.address` itself, and it reads `Forwarded` and
//! `X-Forwarded-For` unconditionally to do so. That is the same trust problem
//! the rate limiter has: without a proxy in front, the recorded address would
//! be whatever the client typed, which makes every per-address question an
//! operator asks of the logs answerable only by the attacker.
//!
//! So it stays off, and [`record_client_address`] records the field using
//! [`crate::client_ip`], which believes the header only from a peer in
//! `TRUSTED_PROXIES`.
//!
//! # The probes are excluded
//!
//! An orchestrator calls `/healthz` every few seconds forever. Spanning that
//! buries the requests a person actually made.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Uri};
use axum::middleware::Next;
use axum::response::Response;
use axum_tracing_opentelemetry::middleware::OtelAxumLayer;

use crate::client_ip::client_ip;
use crate::state::AppState;

/// The response header carrying the trace id.
///
/// Set on every response so a screenshot of a failure is enough to find the
/// log line, which is the same reason `Problem` carries one.
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// Paths that get no span.
pub fn is_probe(uri: &Uri) -> bool {
    matches!(uri.path(), "/healthz" | "/readyz")
}

/// The server span layer.
pub fn layer() -> OtelAxumLayer {
    OtelAxumLayer::default()
        .filter(|path| !matches!(path, "/healthz" | "/readyz"))
        // See the module note: the built-in extraction trusts forwarded
        // headers from anyone.
        .try_extract_client_ip(false)
}

/// Fills the span's `trace_id`, records `client.address`, and echoes the
/// trace id on the response.
///
/// # `trace_id` has to be recorded, not just created
///
/// The server span declares `trace_id = Empty` and nothing fills it. The
/// OpenTelemetry layer keeps the real trace context in its own extension
/// rather than as a `tracing` field, so a JSON line carries every other span
/// field and no trace id — every line looks fine on its own and correlation
/// is silently gone. That is the failure ADR-0014 calls the one that makes
/// the whole design worthless, and `tests/trace_correlation.rs` is what
/// caught it.
///
/// Recorded **before** `next.run`, so the handler's own events and any span
/// nested inside inherit it. Recording it afterwards would fill the field for
/// nothing that had already been logged.
pub async fn record_span_fields(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let span = tracing::Span::current();

    let trace_id = find_trace_id();
    if let Some(id) = trace_id.as_deref() {
        span.record("trace_id", id);
    }

    if let Some(ConnectInfo(peer)) = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
    {
        let address = client_ip(*peer, request.headers(), &state.config.trusted_proxies);
        // The semantic convention name, so a future exporter needs no mapping
        // (ADR-0015).
        span.record("client.address", tracing::field::display(address));
    }

    let mut response = next.run(request).await;

    if let Some(id) = trace_id
        && let Ok(value) = HeaderValue::from_str(&id)
    {
        response.headers_mut().insert(TRACE_ID_HEADER, value);
    }

    response
}

/// The current trace id, or `None` when no span is active.
pub fn find_trace_id() -> Option<String> {
    axum_tracing_opentelemetry::tracing_opentelemetry_instrumentation_sdk::find_current_trace_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probes_are_excluded_from_tracing() {
        for path in ["/healthz", "/readyz"] {
            let uri: Uri = path.parse().expect("uri");
            assert!(
                is_probe(&uri),
                "an orchestrator calls {path} forever; spanning it buries the \
                 requests a person made"
            );
        }
    }

    #[test]
    fn ordinary_routes_are_traced() {
        for path in ["/api/v1/manga", "/api/v1/events", "/"] {
            let uri: Uri = path.parse().expect("uri");
            assert!(!is_probe(&uri), "{path} must be traced");
        }
    }

    /// A path that merely starts the same way is not a probe, or
    /// `/healthz-report` would silently lose its spans.
    #[test]
    fn a_path_that_only_looks_like_a_probe_is_still_traced() {
        for path in ["/healthzz", "/healthz/detail", "/api/v1/readyz"] {
            let uri: Uri = path.parse().expect("uri");
            assert!(!is_probe(&uri), "{path} is not a probe");
        }
    }

    #[test]
    fn the_trace_header_is_the_conventional_name() {
        assert_eq!(TRACE_ID_HEADER, "x-trace-id");
    }
}
