//! Subscriber setup (ADR-0014, ADR-0015).
//!
//! ```text
//! Registry
//!   ├─ EnvFilter            RUST_LOG, default "info,nyuka_api=debug,sea_orm=warn,hyper=warn"
//!   ├─ ErrorLayer           SpanTrace capture for error reports
//!   ├─ OpenTelemetryLayer   TracerProvider with NO exporter in v1 — it still
//!   │                       creates real span/trace ids and honours
//!   │                       propagated context, which is what makes the JSON
//!   │                       lines joinable. Exporter only under `otlp`.
//!   └─ fmt::layer().json()  flatten_event, span list, RFC 3339, service.*
//!        └─ RedactingWriter  masks authorization|cookie|set-cookie|token|
//!                            secret|password on the way out
//! ```
//!
//! Sampling affects export only — **logs are never sampled**, because for a
//! low-traffic single-tenant service full fidelity beats aggregate
//! queryability.
//!
//! # `trace_id` on a line is not automatic
//!
//! This module used to claim every line carried one. It did not. The
//! OpenTelemetry layer keeps the trace context in its own span extension
//! rather than as a `tracing` field, so the JSON formatter never sees it: the
//! lines carry every other span field and no trace id, and each one looks
//! perfectly fine on its own.
//!
//! Two things are needed, and both were missing:
//!
//! 1. The global `TraceContextPropagator`, installed below. Without it an
//!    incoming `traceparent` is ignored — not rejected, ignored — and every
//!    request starts a fresh trace, so a browser span and the server's lines
//!    become unrelated.
//! 2. Recording `trace_id` onto the server span, which
//!    `tracing_layer::record_span_fields` does before the handler runs.
//!
//! `tests/trace_correlation.rs` is what found both. ADR-0014 asks for that
//! test and calls silent loss of correlation the failure that makes this
//! design worthless; it was right.
//!
//! Two tests are load-bearing here: one asserting the redaction layer drops an
//! authorization header, a cookie, and a token-bearing query string; and one
//! asserting `trace_id` is present on every line of a request and the job it
//! enqueues. Silent loss of correlation makes the whole design worthless.
//!
//! # Redaction happens at the writer, and that is not a detail
//!
//! The obvious designs do not work, and both fail *silently*, which is the
//! worst property a masking control can have:
//!
//! - A `Layer` sees values as they are recorded but cannot rewrite what a
//!   downstream formatter prints. A layer named `RedactionLayer` would be a
//!   layer that does not redact.
//! - A [`FormatFields`] wrapper only sees **span** fields. `Format<Json>`
//!   serializes event fields straight from `event.field_map()` and never
//!   consults the field formatter, so `tracing::info!(authorization = …)`
//!   would pass through untouched. This was tried first, and the test below
//!   is what caught it.
//!
//! The writer is the one place that sees the finished line, whatever produced
//! it, so that is where the masking goes. It also means the control survives a
//! change of formatter instead of quietly lapsing.
//!
//! Cost: one JSON parse and re-serialize per line. That is the right trade
//! here — this service is single-tenant and low-traffic, and it already chose
//! full log fidelity over throughput.
//!
//! If a line cannot be parsed back as JSON its fields are replaced wholesale.
//! Emitting it unredacted because the parse failed would make the control fail
//! open, which is the one outcome worse than losing a log line. That is also
//! why `LOG_FORMAT` accepts only `json`: a text formatter would put every line
//! down that path.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_error::ErrorLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::TelemetryConfig;

/// What a redacted value is replaced with.
pub const MASK: &str = "***";

/// Field-name fragments whose values must never be printed.
///
/// Matched as substrings, case-insensitively, so `http.request.header.
/// authorization` and `db.password` are both caught without enumerating every
/// prefix any library might choose. The cost is the occasional false positive
/// — a field called `token_count` is masked — which is the right direction for
/// this to err in.
const SENSITIVE: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "token",
    "secret",
    "password",
    "api-key",
    "api_key",
];

/// Whether a field's value must be masked.
pub fn is_sensitive(field: &str) -> bool {
    let lowered = field.to_ascii_lowercase();
    SENSITIVE.iter().any(|needle| lowered.contains(needle))
}

/// Masks a query string's sensitive parameters, keeping the shape readable.
///
/// A URL with `?access_token=…` is a secret in a field named `url`, which
/// name-based masking cannot see. Used where URLs are recorded.
pub fn redact_query(url: &str) -> String {
    let Some((path, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let masked: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_sensitive(key) => format!("{key}={MASK}"),
            _ => pair.to_string(),
        })
        .collect();
    format!("{path}?{}", masked.join("&"))
}

/// Wraps a writer, masking sensitive keys in each JSON line it carries.
///
/// Lines are buffered until a newline so a writer that is handed a partial
/// line cannot emit half an object. `fmt` writes one complete line per event,
/// but relying on that would make this break quietly if it ever stopped being
/// true.
pub struct RedactingWriter<W: std::io::Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: std::io::Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    fn drain_lines(&mut self) -> std::io::Result<()> {
        while let Some(at) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=at).collect();
            let redacted = redact_line(&line[..line.len() - 1]);
            self.inner.write_all(redacted.as_bytes())?;
            self.inner.write_all(b"\n")?;
        }
        Ok(())
    }
}

impl<W: std::io::Write> std::io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.drain_lines()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // A trailing fragment with no newline is still a log line, and
        // dropping it would lose the last record of a crash.
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            let redacted = redact_line(&line);
            self.inner.write_all(redacted.as_bytes())?;
            self.inner.write_all(b"\n")?;
        }
        self.inner.flush()
    }
}

impl<W: std::io::Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let _ = std::io::Write::flush(self);
    }
}

/// Installs [`RedactingWriter`] over another `MakeWriter`.
#[derive(Debug, Clone, Default)]
pub struct Redacting<M>(pub M);

impl<'a, M: tracing_subscriber::fmt::MakeWriter<'a>> tracing_subscriber::fmt::MakeWriter<'a>
    for Redacting<M>
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new(self.0.make_writer())
    }
}

/// Masks the sensitive keys in one formatted JSON line.
///
/// Split out from the writer so the fail-closed path is directly testable
/// without standing up a subscriber.
pub fn redact_line(line: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(line) else {
        return dropped();
    };
    if text.trim().is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut value) => {
            redact_value(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| dropped())
        }
        // Fail closed. Printing the unparsed text would emit exactly the
        // values this exists to mask, in the one case nobody is watching.
        Err(_) => dropped(),
    }
}

fn dropped() -> String {
    r#"{"redaction":"line could not be parsed and was dropped"}"#.to_string()
}

fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if is_sensitive(key) {
                    *child = serde_json::Value::String(MASK.to_string());
                } else {
                    redact_value(child);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

/// Keeps the tracer provider alive and flushes it on drop.
///
/// Dropping this at the end of `main` is what gives an exporter its last
/// chance to send. Without it a crash-free shutdown still loses the final
/// spans, which are usually the interesting ones.
pub struct TelemetryGuard {
    provider: SdkTracerProvider,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(e) = self.provider.shutdown() {
            // Not `tracing`: the subscriber is being torn down.
            eprintln!("telemetry shutdown failed: {e}");
        }
    }
}

/// The default filter, used when `RUST_LOG` is unset.
///
/// `sqlx=warn` is here because SeaORM's migrator surfaces PostgreSQL notices
/// at INFO — "relation already exists, skipping" on every boot — which buries
/// the startup lines that matter.
pub const DEFAULT_FILTER: &str = "info,nyuka_api=debug,sea_orm=warn,sqlx=warn,hyper=warn";

/// Installs the subscriber. Call once, before anything that logs.
pub fn init(config: &TelemetryConfig) -> anyhow::Result<TelemetryGuard> {
    // Without this, an incoming `traceparent` is ignored and every request
    // starts a new trace. Nothing errors — the header is simply not read — so
    // a browser-originated span and the server's lines become two unrelated
    // traces, which is the correlation ADR-0013 exists to establish.
    // `tests/trace_correlation.rs` is what caught its absence.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attribute(opentelemetry::KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            config.service_version.clone(),
        ))
        .build();

    // No span processor: spans are created and carry real ids, and nothing is
    // exported. That is the whole v1 design — the ids are what make the JSON
    // lines joinable, and an exporter is a separate decision (ADR-0014).
    let provider = SdkTracerProvider::builder().with_resource(resource).build();

    let tracer = provider.tracer(config.service_name.clone());

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(filter)
        .with(ErrorLayer::default())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(true)
                .with_span_list(true)
                .with_writer(Redacting(std::io::stdout))
                .boxed(),
        )
        .try_init()?;

    Ok(TelemetryGuard { provider })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Emits through the real subscriber stack, so a test cannot pass against
    /// a mechanism the subscriber does not actually use.
    fn emit(record: impl FnOnce()) -> String {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(true)
                .with_span_list(true)
                .with_writer(Redacting(buffer.clone())),
        );
        tracing::subscriber::with_default(subscriber, record);

        let bytes = buffer.0.lock().expect("lock").clone();
        String::from_utf8(bytes).expect("utf8")
    }

    #[test]
    fn sensitive_names_are_recognised_case_insensitively_and_as_substrings() {
        for name in [
            "authorization",
            "Authorization",
            "http.request.header.authorization",
            "cookie",
            "set-cookie",
            "session_token",
            "client_secret",
            "db.password",
            "x-api-key",
        ] {
            assert!(is_sensitive(name), "{name} must be masked");
        }
    }

    #[test]
    fn ordinary_names_are_left_alone() {
        for name in ["http.route", "job.id", "manga.title", "user.id", "status"] {
            assert!(!is_sensitive(name), "{name} must not be masked");
        }
    }

    /// The first of the two load-bearing tests: an authorization header, a
    /// cookie, and a token must not reach the log stream.
    #[test]
    fn the_formatter_masks_credentials() {
        let line = emit(|| {
            tracing::info!(
                authorization = "Bearer eyJhbGciOi.super-secret",
                cookie = "id=abc123; other=1",
                session_token = "tok_live_9f8e7d",
                http.route = "/api/v1/manga",
                "handled a request"
            );
        });

        assert!(!line.contains("super-secret"), "line was: {line}");
        assert!(!line.contains("abc123"), "line was: {line}");
        assert!(!line.contains("tok_live_9f8e7d"), "line was: {line}");
        assert!(line.contains(MASK));
        assert!(
            line.contains("/api/v1/manga"),
            "masking must not swallow the fields that make a line useful"
        );
    }

    /// Nested structures are where a header map usually lives.
    #[test]
    fn masking_reaches_into_nested_values() {
        let mut value = serde_json::json!({
            "request": {
                "headers": { "authorization": "Bearer abc", "accept": "application/json" },
                "cookies": [{ "set-cookie": "sid=xyz" }]
            }
        });
        redact_value(&mut value);
        let rendered = value.to_string();

        assert!(!rendered.contains("Bearer abc"));
        assert!(!rendered.contains("sid=xyz"));
        assert!(rendered.contains("application/json"));
    }

    /// A URL carrying a token is a secret in a field named `url`, which
    /// name-based masking cannot see.
    #[test]
    fn a_token_bearing_query_string_is_masked() {
        assert_eq!(
            redact_query("/auth/callback?code=abc&access_token=secret&state=xyz"),
            "/auth/callback?code=abc&access_token=***&state=xyz"
        );
    }

    #[test]
    fn a_query_string_without_secrets_is_untouched() {
        let url = "/api/v1/manga?cursor=abc&limit=50";
        assert_eq!(redact_query(url), url);
    }

    #[test]
    fn a_url_without_a_query_is_untouched() {
        assert_eq!(redact_query("/api/v1/manga"), "/api/v1/manga");
    }

    /// Span fields go through a different path in the formatter than event
    /// fields do. Both have to be masked, and only the writer sees both.
    #[test]
    fn credentials_on_a_span_are_masked_too() {
        let line = emit(|| {
            let span = tracing::info_span!(
                "request",
                authorization = "Bearer span-level-secret",
                http.route = "/api/v1/jobs"
            );
            let _guard = span.enter();
            tracing::info!("inside the span");
        });

        assert!(!line.contains("span-level-secret"), "line was: {line}");
        assert!(line.contains("/api/v1/jobs"));
    }

    /// The control must fail closed. Emitting the unparsed text in the one
    /// case nobody is watching is worse than losing the line.
    #[test]
    fn an_unparseable_line_is_dropped_rather_than_printed() {
        let out = redact_line(b"not json at all: Bearer super-secret");
        assert!(!out.contains("super-secret"));
        assert!(out.contains("could not be parsed"));
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "the replacement must still be valid JSON, or it breaks every line after it"
        );
    }

    #[test]
    fn invalid_utf8_is_dropped() {
        assert!(redact_line(&[0xFF, 0xFE]).contains("could not be parsed"));
    }

    /// A writer handed a partial line must not emit half an object.
    #[test]
    fn a_line_split_across_writes_is_redacted_once_whole() {
        use std::io::Write;

        let buffer = Buffer::default();
        {
            let mut writer = RedactingWriter::new(buffer.clone());
            let line = br#"{"authorization":"Bearer split-secret","ok":1}"#;
            let (head, tail) = line.split_at(20);
            writer.write_all(head).expect("head");
            writer.write_all(tail).expect("tail");
            writer.write_all(b"\n").expect("newline");
        }

        let out = String::from_utf8(buffer.0.lock().expect("lock").clone()).expect("utf8");
        assert!(!out.contains("split-secret"), "out was: {out}");
        assert!(out.contains(MASK));
        assert!(out.contains("\"ok\":1"));
    }

    /// A fragment with no trailing newline is still a log line, and losing it
    /// loses the last record before a crash.
    #[test]
    fn a_trailing_fragment_is_flushed_on_drop() {
        use std::io::Write;

        let buffer = Buffer::default();
        {
            let mut writer = RedactingWriter::new(buffer.clone());
            writer
                .write_all(br#"{"message":"no newline here"}"#)
                .expect("write");
        }

        let out = String::from_utf8(buffer.0.lock().expect("lock").clone()).expect("utf8");
        assert!(out.contains("no newline here"), "out was: {out}");
    }

    #[test]
    fn the_default_filter_quiets_the_noisy_crates() {
        assert!(DEFAULT_FILTER.contains("sea_orm=warn"));
        assert!(
            DEFAULT_FILTER.contains("sqlx=warn"),
            "sqlx notices at INFO bury the startup lines that matter"
        );
        assert!(DEFAULT_FILTER.contains("hyper=warn"));
        assert!(DEFAULT_FILTER.contains("nyuka_api=debug"));
    }
}
