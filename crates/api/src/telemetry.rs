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
//!   ├─ RedactionLayer       masks authorization|cookie|set-cookie|token|
//!   │                       secret|password
//!   └─ fmt::layer().json()  flatten_event, span list, RFC 3339, service.*
//! ```
//!
//! Every line carries `trace_id` and `span_id`. Sampling affects export only —
//! **logs are never sampled**, because for a low-traffic single-tenant service
//! full fidelity beats aggregate queryability.
//!
//! Two tests are load-bearing here: one asserting the redaction layer drops an
//! authorization header, a cookie, and a token-bearing query string; and one
//! asserting `trace_id` is present on every line of a request and the job it
//! enqueues. Silent loss of correlation makes the whole design worthless.
