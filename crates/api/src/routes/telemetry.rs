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
