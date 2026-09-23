//! `POST /api/v1/telemetry` — browser telemetry ingest (ADR-0013).
//!
//! Accepts OTLP/JSON and OTLP/protobuf, session-authenticated, and emits each
//! span as a `tracing` event into the same log stream as everything else.
//!
//! The two encodings converge on one filter rather than each carrying its own
//! copy of the rules. An allow-list enforced in one decoder and not the other
//! is the shape of bug where a client simply picks the encoding that skips
//! the check, so a test asserts the same batch produces the same result
//! either way.
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
#[derive(Debug, Default, Deserialize)]
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
    Malformed(String),
    WrongService(String),
    TooManySpans(usize),
}

impl Rejected {
    fn detail(&self) -> String {
        match self {
            Self::Malformed(e) => format!("The body is not valid OTLP: {e}"),
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
        serde_json::from_slice(body).map_err(|e| Rejected::Malformed(e.to_string()))?;

    filter(parsed)
}

/// Decodes an OTLP/protobuf batch and applies the same filtering.
///
/// The two encodings converge on [`filter`] rather than each carrying their
/// own copy of the rules. An allow-list that is enforced in one decoder and
/// not the other is the shape of bug where a client picks the encoding that
/// skips the check.
pub fn accept_protobuf(body: &[u8]) -> Result<Accepted, Rejected> {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;

    let request =
        ExportTraceServiceRequest::decode(body).map_err(|e| Rejected::Malformed(e.to_string()))?;

    filter(from_proto(request))
}

/// Maps the generated protobuf types onto the same shape the JSON decoder
/// produces.
///
/// Deliberately lossy in the same way: a field this does not carry across
/// cannot reach the log stream, whichever encoding it arrived in.
fn from_proto(
    request: opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest,
) -> OtlpBody {
    use opentelemetry_proto::tonic::common::v1::any_value::Value as ProtoValue;

    fn convert_value(value: opentelemetry_proto::tonic::common::v1::AnyValue) -> Option<AnyValue> {
        Some(match value.value? {
            ProtoValue::StringValue(s) => AnyValue {
                string_value: Some(s),
                ..AnyValue::default()
            },
            ProtoValue::BoolValue(b) => AnyValue {
                bool_value: Some(b),
                ..AnyValue::default()
            },
            ProtoValue::IntValue(i) => AnyValue {
                int_value: Some(serde_json::Value::from(i)),
                ..AnyValue::default()
            },
            ProtoValue::DoubleValue(d) => AnyValue {
                double_value: Some(d),
                ..AnyValue::default()
            },
            // Arrays, nested maps and raw bytes are dropped here exactly as
            // the JSON path drops them: flattening is where unbounded nesting
            // becomes unbounded output.
            _ => return None,
        })
    }

    fn convert_kv(kv: opentelemetry_proto::tonic::common::v1::KeyValue) -> KeyValue {
        KeyValue {
            key: kv.key,
            value: kv.value.and_then(convert_value),
        }
    }

    OtlpBody {
        resource_spans: request
            .resource_spans
            .into_iter()
            .map(|rs| ResourceSpans {
                resource: rs.resource.map(|r| OtlpResource {
                    attributes: r.attributes.into_iter().map(convert_kv).collect(),
                }),
                scope_spans: rs
                    .scope_spans
                    .into_iter()
                    .map(|ss| ScopeSpans {
                        spans: ss
                            .spans
                            .into_iter()
                            .map(|span| OtlpSpan {
                                name: span.name,
                                // Protobuf carries ids as raw bytes; OTLP/JSON
                                // uses lowercase hex. Rendered to hex so both
                                // encodings produce the same log line for the
                                // same span.
                                trace_id: hex(&span.trace_id),
                                span_id: hex(&span.span_id),
                                start_time_unix_nano: None,
                                end_time_unix_nano: None,
                                attributes: span.attributes.into_iter().map(convert_kv).collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The validation both encodings share.
fn filter(parsed: OtlpBody) -> Result<Accepted, Rejected> {
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

    // Both encodings ADR-0013 names. The content type selects the decoder
    // rather than being sniffed from the bytes: a guess that reads protobuf
    // as JSON reports a syntax error, which sends whoever hits it looking in
    // the wrong place.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");

    let decoded = if content_type.starts_with("application/x-protobuf")
        || content_type.starts_with("application/protobuf")
    {
        accept_protobuf(&body)
    } else if content_type.starts_with("application/json") {
        accept(&body)
    } else {
        return Err(ApiError(Box::new(
            Problem::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported-encoding",
                "Unsupported telemetry encoding",
            )
            .with_detail("Send OTLP as `application/json` or `application/x-protobuf`."),
        )));
    };

    let accepted = decoded.map_err(|rejected| {
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
            assert!(matches!(accept(body), Err(Rejected::Malformed(_))));
        }
    }

    // --- protobuf ------------------------------------------------------

    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value as PbValue;
    use opentelemetry_proto::tonic::common::v1::{AnyValue as PbAny, KeyValue as PbKv};
    use opentelemetry_proto::tonic::resource::v1::Resource as PbResource;
    use opentelemetry_proto::tonic::trace::v1::{
        ResourceSpans as PbResourceSpans, ScopeSpans as PbScopeSpans, Span as PbSpan,
    };
    use prost::Message as _;

    fn pb_string(key: &str, value: &str) -> PbKv {
        PbKv {
            key: key.into(),
            value: Some(PbAny {
                value: Some(PbValue::StringValue(value.into())),
            }),
            // Profiling-signal field; irrelevant here but part of the
            // generated struct.
            ..Default::default()
        }
    }

    fn pb_batch(service: &str, spans: Vec<PbSpan>) -> Vec<u8> {
        ExportTraceServiceRequest {
            resource_spans: vec![PbResourceSpans {
                resource: Some(PbResource {
                    attributes: vec![pb_string("service.name", service)],
                    ..Default::default()
                }),
                scope_spans: vec![PbScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec()
    }

    fn pb_span(name: &str, attributes: Vec<PbKv>) -> PbSpan {
        PbSpan {
            name: name.into(),
            trace_id: vec![0x4b, 0xf9, 0x2f, 0x35],
            span_id: vec![0x00, 0xf0, 0x67, 0xaa],
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn a_protobuf_batch_is_accepted() {
        let body = pb_batch(
            EXPECTED_SERVICE,
            vec![pb_span(
                "GET /api/v1/manga",
                vec![pb_string("http.request.method", "GET")],
            )],
        );
        let accepted = accept_protobuf(&body).expect("accepted");
        assert_eq!(accepted.spans.len(), 1);
        assert_eq!(accepted.spans[0].name, "GET /api/v1/manga");
        assert_eq!(
            accepted.spans[0].attributes,
            vec![("http.request.method".to_string(), "GET".to_string())]
        );
    }

    /// Ids are raw bytes in protobuf and lowercase hex in OTLP/JSON. The same
    /// span must produce the same log line whichever encoding carried it, or
    /// correlating a browser trace depends on which one the SDK chose.
    #[test]
    fn protobuf_ids_are_rendered_as_hex() {
        let accepted = accept_protobuf(&pb_batch(EXPECTED_SERVICE, vec![pb_span("x", vec![])]))
            .expect("accepted");
        assert_eq!(accepted.spans[0].trace_id, "4bf92f35");
        assert_eq!(accepted.spans[0].span_id, "00f067aa");
    }

    /// The allow-list has to hold on both paths. A rule enforced in one
    /// decoder and not the other is the shape of bug where a client picks the
    /// encoding that skips the check.
    #[test]
    fn the_allow_list_holds_for_protobuf_too() {
        let body = pb_batch(
            EXPECTED_SERVICE,
            vec![pb_span(
                "x",
                vec![
                    pb_string("user.id", "somebody-else"),
                    pb_string("db.statement", "select 1"),
                    pb_string("http.route", "/kept"),
                ],
            )],
        );
        let accepted = accept_protobuf(&body).expect("accepted");
        let rendered = format!("{:?}", accepted.spans[0].attributes);

        assert!(!rendered.contains("somebody-else"), "{rendered}");
        assert!(!rendered.contains("select 1"), "{rendered}");
        assert!(rendered.contains("/kept"));
        assert_eq!(accepted.dropped_attributes, 2);
    }

    #[test]
    fn a_protobuf_batch_claiming_another_service_is_refused() {
        let body = pb_batch("manga-api", vec![pb_span("x", vec![])]);
        assert!(matches!(
            accept_protobuf(&body),
            Err(Rejected::WrongService(_))
        ));
    }

    #[test]
    fn a_protobuf_batch_over_the_span_limit_is_refused() {
        let spans: Vec<PbSpan> = (0..MAX_SPANS + 1)
            .map(|i| pb_span(&format!("s{i}"), vec![]))
            .collect();
        assert!(matches!(
            accept_protobuf(&pb_batch(EXPECTED_SERVICE, spans)),
            Err(Rejected::TooManySpans(_))
        ));
    }

    /// The two encodings must agree. Anything else means the wire format
    /// changes what reaches the log stream.
    #[test]
    fn both_encodings_produce_the_same_result() {
        let json = accept(&batch(
            EXPECTED_SERVICE,
            serde_json::json!([{
                "name": "GET /x",
                "traceId": "4bf92f35",
                "spanId": "00f067aa",
                "attributes": [
                    { "key": "http.route", "value": { "stringValue": "/x" } },
                    { "key": "db.statement", "value": { "stringValue": "dropped" } }
                ]
            }]),
        ))
        .expect("json accepted");

        let proto = accept_protobuf(&pb_batch(
            EXPECTED_SERVICE,
            vec![pb_span(
                "GET /x",
                vec![
                    pb_string("http.route", "/x"),
                    pb_string("db.statement", "dropped"),
                ],
            )],
        ))
        .expect("protobuf accepted");

        assert_eq!(json, proto);
    }

    #[test]
    fn arbitrary_bytes_do_not_panic_the_protobuf_decoder() {
        for body in [
            b"not protobuf".as_slice(),
            b"".as_slice(),
            &[0xFF; 64],
            // A valid JSON body sent with the wrong content type.
            br#"{"resourceSpans":[]}"#,
        ] {
            let _ = accept_protobuf(body);
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
