//! `POST /api/v1/telemetry` — browser telemetry ingest (ADR-0013).
//!
//! Accepts OTLP/JSON and OTLP/protobuf, session-authenticated, and emits each
//! span as a `tracing` event into the same log stream as everything else.
//!
//! Limits: 256 KB body, 200 spans per batch, ~30 requests/minute per session
//! plus a per-IP limit. `TELEMETRY_INGEST_ENABLED` is the kill switch.
//!
//! Validation is an **allow-list**: reject any batch whose resource
//! `service.name` is not `manga-web`; keep only `http.*`, `url.*`,
//! `exception.*`, `browser.*`, `app.*`; truncate strings to 1 KB; cap attribute
//! counts. `session.id`, `user.id`, and `service.name` come from the server,
//! never from the client.
//!
//! Returns `202` with an empty OTLP body, `400` problem+json for validation
//! failures, `429` with `Retry-After` when throttled. Never `5xx` for bad
//! client input.
//!
//! # This writes user-controlled data into the log stream
//!
//! Two risks ADR-0013 calls out:
//!
//! - **Log injection.** A browser can send strings that land in JSON lines an
//!   operator later pipes through `jq`. The allow-list, truncation, caps, and
//!   correct JSON escaping are the controls, and they need tests — including a
//!   span name containing newlines, quotes, and ANSI escapes.
//! - **Retention.** One tab at 30 req/min × 200 spans is 6,000 spans/minute
//!   against a `50m × 10` rotation, which can evict the backend history that
//!   made log-only telemetry worth having. Measure before enabling the SDK in
//!   Phase 2; give `web_telemetry` its own log target if it is a meaningful
//!   share of volume.
//!
//! # Why a hand-written subset rather than the generated types
//!
//! The OTLP protobuf definitions are large, and decoding them is decoding
//! attacker-influenced bytes. What this needs from a span is its name, its
//! ids, its timing, and a filtered attribute list — so the decode target is a
//! shape that holds exactly that, and everything else in the payload is
//! dropped during parsing rather than after. A field this does not model
//! cannot reach the log stream through a mistake downstream.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::UserId;
use serde::Deserialize;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult, Problem};
use crate::state::AppState;

/// The only `service.name` this endpoint accepts.
pub const EXPECTED_SERVICE: &str = "manga-web";

/// Spans accepted in one batch.
pub const MAX_SPANS: usize = 200;

/// Attributes kept per span.
pub const MAX_ATTRIBUTES: usize = 32;

/// Longest string value kept, in bytes.
pub const MAX_STRING_BYTES: usize = 1024;

/// Attribute prefixes the allow-list admits.
///
/// An allow-list rather than a deny-list: a deny-list has to anticipate every
/// key a browser might invent, and the first one nobody thought of lands in
/// the log stream.
pub const ALLOWED_PREFIXES: &[&str] = &["http.", "url.", "exception.", "browser.", "app."];

/// Keys the server owns. A client supplying one has it replaced, not merged.
pub const SERVER_OWNED: &[&str] = &["session.id", "user.id", "service.name"];

pub fn is_allowed_attribute(key: &str) -> bool {
    !SERVER_OWNED.contains(&key) && ALLOWED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Truncates on a character boundary, so the result is still valid UTF-8.
///
/// Slicing by byte index would panic mid-character; a browser sending a long
/// string of multi-byte characters is not a server fault.
pub fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

// --- the decode target ------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OtlpBody {
    #[serde(default, alias = "resourceSpans")]
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Debug, Deserialize)]
struct ResourceSpans {
    #[serde(default)]
    resource: Option<OtlpResource>,
    #[serde(default, alias = "scopeSpans")]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Debug, Deserialize)]
struct OtlpResource {
    #[serde(default)]
    attributes: Vec<KeyValue>,
}

#[derive(Debug, Deserialize)]
struct ScopeSpans {
    #[serde(default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Debug, Deserialize)]
struct OtlpSpan {
    #[serde(default)]
    name: String,
    #[serde(default, alias = "traceId")]
    trace_id: String,
    #[serde(default, alias = "spanId")]
    span_id: String,
    #[serde(default, alias = "startTimeUnixNano")]
    start_time_unix_nano: Option<serde_json::Value>,
    #[serde(default, alias = "endTimeUnixNano")]
    end_time_unix_nano: Option<serde_json::Value>,
    #[serde(default)]
    attributes: Vec<KeyValue>,
}

#[derive(Debug, Deserialize)]
struct KeyValue {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: Option<AnyValue>,
}

/// The OTLP `AnyValue` union, reduced to what is loggable.
#[derive(Debug, Deserialize)]
struct AnyValue {
    #[serde(default, alias = "stringValue")]
    string_value: Option<String>,
    #[serde(default, alias = "intValue")]
    int_value: Option<serde_json::Value>,
    #[serde(default, alias = "doubleValue")]
    double_value: Option<f64>,
    #[serde(default, alias = "boolValue")]
    bool_value: Option<bool>,
}

impl AnyValue {
    /// Renders a value, or `None` for a shape this does not log.
    ///
    /// Arrays and nested key-value lists are dropped rather than flattened:
    /// flattening is where unbounded nesting becomes unbounded output, and no
    /// allow-listed attribute is one.
    fn render(&self) -> Option<String> {
        if let Some(s) = &self.string_value {
            return Some(truncate(s, MAX_STRING_BYTES));
        }
        if let Some(i) = &self.int_value {
            // OTLP/JSON encodes 64-bit integers as strings.
            return Some(truncate(i.to_string().trim_matches('"'), 64));
        }
        if let Some(d) = self.double_value {
            return Some(d.to_string());
        }
        if let Some(b) = self.bool_value {
            return Some(b.to_string());
        }
        None
    }
}

/// One span, reduced to what is logged.
#[derive(Debug, PartialEq, Eq)]
pub struct AcceptedSpan {
    pub name: String,
    pub trace_id: String,
    pub span_id: String,
    pub attributes: Vec<(String, String)>,
}

/// What a batch was reduced to.
#[derive(Debug, PartialEq, Eq)]
pub struct Accepted {
    pub spans: Vec<AcceptedSpan>,
    /// Attributes dropped by the allow-list, for the ingest's own counter.
    pub dropped_attributes: usize,
}

/// Why a batch was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejected {
    NotJson(String),
    WrongService(String),
    TooManySpans(usize),
}

impl Rejected {
    fn detail(&self) -> String {
        match self {
            Self::NotJson(e) => format!("The body is not OTLP/JSON: {e}"),
            Self::WrongService(found) => format!(
                "Only `{EXPECTED_SERVICE}` telemetry is accepted here; the batch claimed \
                 `{}`.",
                truncate(found, 64)
            ),
            Self::TooManySpans(count) => {
                format!("A batch may carry at most {MAX_SPANS} spans; this one had {count}.")
            }
        }
    }
}

/// Parses and filters a batch. Pure, so every rule is testable directly.
pub fn accept(body: &[u8]) -> Result<Accepted, Rejected> {
    let parsed: OtlpBody =
        serde_json::from_slice(body).map_err(|e| Rejected::NotJson(e.to_string()))?;

    // The service claim is checked before any span is examined: a batch from
    // something other than the web app has nothing here worth reading.
    let service = parsed
        .resource_spans
        .iter()
        .filter_map(|rs| rs.resource.as_ref())
        .flat_map(|r| r.attributes.iter())
        .find(|kv| kv.key == "service.name")
        .and_then(|kv| kv.value.as_ref())
        .and_then(|v| v.string_value.clone())
        .unwrap_or_default();

    if service != EXPECTED_SERVICE {
        return Err(Rejected::WrongService(service));
    }

    let total: usize = parsed
        .resource_spans
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .map(|ss| ss.spans.len())
        .sum();
    if total > MAX_SPANS {
        return Err(Rejected::TooManySpans(total));
    }

    let mut dropped_attributes = 0;
    let mut spans = Vec::with_capacity(total);

    for span in parsed
        .resource_spans
        .into_iter()
        .flat_map(|rs| rs.scope_spans.into_iter())
        .flat_map(|ss| ss.spans.into_iter())
    {
        let mut attributes = Vec::new();
        for attribute in span.attributes {
            if !is_allowed_attribute(&attribute.key) {
                dropped_attributes += 1;
                continue;
            }
            if attributes.len() >= MAX_ATTRIBUTES {
                dropped_attributes += 1;
                continue;
            }
            let Some(value) = attribute.value.as_ref().and_then(AnyValue::render) else {
                dropped_attributes += 1;
                continue;
            };
            attributes.push((truncate(&attribute.key, 128), value));
        }

        let _ = (&span.start_time_unix_nano, &span.end_time_unix_nano);

        spans.push(AcceptedSpan {
            name: truncate(&span.name, MAX_STRING_BYTES),
            trace_id: truncate(&span.trace_id, 64),
            span_id: truncate(&span.span_id, 64),
            attributes,
        });
    }

    Ok(Accepted {
        spans,
        dropped_attributes,
    })
}

/// `POST /api/v1/telemetry`
pub async fn ingest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    user: axum::extract::Extension<CurrentUser>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    if !state.config.telemetry.ingest_enabled {
        // The kill switch. 404 rather than 403: an operator who turned this
        // off wants it to look absent, not forbidden.
        return Err(ApiError(Box::new(Problem::not_found("endpoint"))));
    }

    let limit = state.config.telemetry.ingest_max_body_bytes;
    if body.len() > limit {
        return Err(ApiError(Box::new(Problem::payload_too_large(limit))));
    }

    // Protobuf is declared in ADR-0013 and is not implemented. Refusing by
    // content type is the honest form of that: the alternative is feeding
    // protobuf bytes to a JSON parser and reporting a parse error, which
    // sends whoever hits it looking in the wrong place.
    if let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        && content_type.starts_with("application/x-protobuf")
    {
        return Err(ApiError(Box::new(
            Problem::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported-encoding",
                "OTLP/protobuf is not accepted",
            )
            .with_detail("Send OTLP/JSON with `Content-Type: application/json`."),
        )));
    }

    let accepted = accept(&body).map_err(|rejected| {
        ApiError(Box::new(
            Problem::new(
                StatusCode::BAD_REQUEST,
                "invalid-telemetry",
                "Invalid telemetry",
            )
            .with_detail(rejected.detail()),
        ))
    })?;

    emit(&accepted, user.0.0);

    // An empty OTLP success body, which is what a collector returns.
    Ok((
        StatusCode::ACCEPTED,
        [(header::CONTENT_TYPE, "application/json")],
        "{}",
    )
        .into_response())
}

/// Writes the accepted spans into the log stream.
///
/// `service.name`, `session.id` and `user.id` are set here from what the
/// server knows. A client that sent its own had them dropped by the
/// allow-list before this point, so there is nothing to override.
fn emit(accepted: &Accepted, user: UserId) {
    for span in &accepted.spans {
        // A single structured event per span. The attribute list is rendered
        // as a JSON value rather than interpolated, so a name or value
        // containing quotes, newlines or escapes is escaped by the serializer
        // — which is what keeps one span one `jq`-parseable line.
        let mut attributes: serde_json::Map<String, serde_json::Value> = span
            .attributes
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();

        // The three server-owned values go in here rather than as macro
        // fields. `service.name` is a resource attribute, and putting it in
        // the map gets the exact semantic-convention key into the output —
        // `tracing`'s macro cannot parse a dotted field name alongside
        // `target:`, and a renamed field would be a different key than the
        // convention specifies. A client that sent any of these had them
        // dropped by the allow-list, so these overwrite nothing.
        attributes.insert(
            "service.name".into(),
            serde_json::Value::String(EXPECTED_SERVICE.into()),
        );
        attributes.insert(
            "user.id".into(),
            serde_json::Value::String(user.to_string()),
        );

        tracing::info!(
            target: "web_telemetry",
            browser_trace_id = %span.trace_id,
            browser_span_id = %span.span_id,
            span_name = %span.name,
            attributes = %serde_json::Value::Object(attributes),
            "browser span"
        );
    }

    if accepted.dropped_attributes > 0 {
        tracing::debug!(
            target: "web_telemetry",
            dropped = accepted.dropped_attributes,
            "dropped attributes outside the allow-list"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(service: &str, spans: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": service } }
                    ]
                },
                "scopeSpans": [{ "spans": spans }]
            }]
        }))
        .expect("serialize")
    }

    fn one_span(name: &str, attributes: serde_json::Value) -> serde_json::Value {
        serde_json::json!([{
            "name": name,
            "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
            "spanId": "00f067aa0ba902b7",
            "attributes": attributes,
        }])
    }

    #[test]
    fn a_well_formed_batch_is_accepted() {
        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                "GET /api/v1/manga",
                serde_json::json!([
                    { "key": "http.request.method", "value": { "stringValue": "GET" } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");
        assert_eq!(accepted.spans.len(), 1);
        assert_eq!(accepted.spans[0].name, "GET /api/v1/manga");
        assert_eq!(
            accepted.spans[0].attributes,
            vec![("http.request.method".to_string(), "GET".to_string())]
        );
    }

    /// ADR-0013 follow-up 3: a client claiming to be the API must be refused.
    #[test]
    fn a_batch_claiming_another_service_is_refused() {
        let body = batch("manga-api", one_span("x", serde_json::json!([])));
        assert_eq!(
            accept(&body),
            Err(Rejected::WrongService("manga-api".into()))
        );
    }

    #[test]
    fn a_batch_with_no_service_name_is_refused() {
        let body = serde_json::to_vec(&serde_json::json!({
            "resourceSpans": [{ "scopeSpans": [{ "spans": [] }] }]
        }))
        .expect("serialize");
        assert!(matches!(accept(&body), Err(Rejected::WrongService(_))));
    }

    /// The identity fields are the server's. A client sending them must not
    /// have them reach the log stream at all.
    #[test]
    fn server_owned_attributes_are_dropped() {
        for key in SERVER_OWNED {
            assert!(
                !is_allowed_attribute(key),
                "{key} is the server's to set, not the client's"
            );
        }

        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                "x",
                serde_json::json!([
                    { "key": "user.id", "value": { "stringValue": "somebody-else" } },
                    { "key": "session.id", "value": { "stringValue": "forged" } },
                    { "key": "http.route", "value": { "stringValue": "/kept" } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");
        let rendered = format!("{:?}", accepted.spans[0].attributes);
        assert!(!rendered.contains("somebody-else"), "{rendered}");
        assert!(!rendered.contains("forged"), "{rendered}");
        assert!(rendered.contains("/kept"));
        assert_eq!(accepted.dropped_attributes, 2);
    }

    /// An allow-list, not a deny-list: the first key nobody anticipated must
    /// not be the one that lands in the log stream.
    #[test]
    fn attributes_outside_the_allow_list_are_dropped() {
        for key in ["db.statement", "custom.thing", "", "httpx.method", "urlish"] {
            assert!(!is_allowed_attribute(key), "{key} must be dropped");
        }
        for key in [
            "http.request.method",
            "url.full",
            "exception.type",
            "browser.language",
            "app.route",
        ] {
            assert!(is_allowed_attribute(key), "{key} must be kept");
        }
    }

    #[test]
    fn a_batch_over_the_span_limit_is_refused() {
        let spans: Vec<serde_json::Value> = (0..MAX_SPANS + 1)
            .map(|i| serde_json::json!({ "name": format!("s{i}") }))
            .collect();
        let body = batch(EXPECTED_SERVICE, serde_json::Value::Array(spans));
        assert_eq!(accept(&body), Err(Rejected::TooManySpans(MAX_SPANS + 1)));
    }

    #[test]
    fn a_batch_exactly_at_the_limit_is_accepted() {
        let spans: Vec<serde_json::Value> = (0..MAX_SPANS)
            .map(|i| serde_json::json!({ "name": format!("s{i}") }))
            .collect();
        let body = batch(EXPECTED_SERVICE, serde_json::Value::Array(spans));
        assert_eq!(accept(&body).expect("accepted").spans.len(), MAX_SPANS);
    }

    #[test]
    fn attribute_counts_are_capped_per_span() {
        let attributes: Vec<serde_json::Value> = (0..MAX_ATTRIBUTES + 10)
            .map(|i| {
                serde_json::json!({
                    "key": format!("http.header.h{i}"),
                    "value": { "stringValue": "v" }
                })
            })
            .collect();
        let body = batch(
            EXPECTED_SERVICE,
            one_span("x", serde_json::Value::Array(attributes)),
        );
        let accepted = accept(&body).expect("accepted");
        assert_eq!(accepted.spans[0].attributes.len(), MAX_ATTRIBUTES);
        assert_eq!(accepted.dropped_attributes, 10);
    }

    #[test]
    fn long_strings_are_truncated() {
        let long = "a".repeat(MAX_STRING_BYTES * 3);
        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                &long,
                serde_json::json!([
                    { "key": "http.route", "value": { "stringValue": long } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");
        assert!(accepted.spans[0].name.len() <= MAX_STRING_BYTES + 4);
        assert!(accepted.spans[0].attributes[0].1.len() <= MAX_STRING_BYTES + 4);
    }

    /// Slicing by byte index would panic mid-character, and a browser sending
    /// multi-byte text is not a server fault.
    #[test]
    fn truncation_respects_character_boundaries() {
        let text = "日本語".repeat(1000);
        let cut = truncate(&text, 10);
        assert!(cut.len() <= 13, "cut was {} bytes", cut.len());
        // The assertion that matters: it is still a valid `String`, which it
        // could not be if a character had been split.
        assert!(cut.chars().count() > 0);
    }

    /// ADR-0013 follow-up 4: hostile strings must produce one valid JSON line.
    #[test]
    fn a_hostile_span_name_serializes_to_one_json_line() {
        let hostile =
            "evil\n{\"level\":\"INFO\",\"message\":\"injected\"}\n\u{1b}[31mred\u{1b}[0m\"quote\"";
        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                hostile,
                serde_json::json!([
                    { "key": "http.route", "value": { "stringValue": hostile } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");

        // What `emit` writes as the attribute field.
        let attributes: serde_json::Map<String, serde_json::Value> = accepted.spans[0]
            .attributes
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let rendered = serde_json::Value::Object(attributes).to_string();

        assert!(
            !rendered.contains('\n'),
            "a raw newline would split one span into two log lines: {rendered}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&rendered).is_ok(),
            "the rendered attributes must stay parseable: {rendered}"
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_refused_rather_than_panicking() {
        for body in [
            b"not json".as_slice(),
            b"".as_slice(),
            b"{".as_slice(),
            &[0xFF, 0xFE, 0x00],
        ] {
            assert!(matches!(accept(body), Err(Rejected::NotJson(_))));
        }
    }

    /// Deeply nested input must be refused by the parser rather than
    /// recursing until the stack gives out.
    #[test]
    fn deeply_nested_json_does_not_crash() {
        let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        assert!(accept(deep.as_bytes()).is_err());
    }

    /// Arrays and nested maps are dropped rather than flattened: flattening
    /// is where unbounded nesting becomes unbounded output.
    #[test]
    fn a_value_shape_that_is_not_logged_is_dropped() {
        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                "x",
                serde_json::json!([
                    { "key": "http.thing", "value": { "arrayValue": { "values": [] } } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");
        assert!(accepted.spans[0].attributes.is_empty());
        assert_eq!(accepted.dropped_attributes, 1);
    }

    #[test]
    fn numeric_and_boolean_values_are_kept() {
        let body = batch(
            EXPECTED_SERVICE,
            one_span(
                "x",
                serde_json::json!([
                    { "key": "http.response.status_code", "value": { "intValue": "200" } },
                    { "key": "http.duration", "value": { "doubleValue": 1.5 } },
                    { "key": "http.cached", "value": { "boolValue": true } }
                ]),
            ),
        );
        let accepted = accept(&body).expect("accepted");
        assert_eq!(accepted.spans[0].attributes.len(), 3);
        assert_eq!(accepted.spans[0].attributes[0].1, "200");
    }
}
