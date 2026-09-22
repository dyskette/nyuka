//! `GET /api/v1/events` (ADR-0010).
//!
//! A `broadcast::Receiver` mapped to an event stream. `KeepAlive` emits a
//! comment roughly every 15 s, and `retry:` is set from configuration so the
//! client's backoff is server-controlled.
//!
//! # Two ways this breaks silently
//!
//! 1. **Compression.** A compressor accumulates input before emitting, so a
//!    `CompressionLayer` over this response holds events even when every proxy
//!    in front is configured correctly. `text/event-stream` must be excluded,
//!    and ADR-0010 requires a test for it — this is the failure most likely to
//!    be reintroduced by a later middleware change.
//! 2. **HTTP/1.1 connection exhaustion.** An SSE stream occupies one of the
//!    browser's six connections per origin, and the seventh request of *any*
//!    kind then queues with no error. Production must run behind TLS so the
//!    browser negotiates HTTP/2; browsers do not speak h2c.
//!
//! `X-Accel-Buffering: no` is set on the response so the app can fix its own
//! streaming if an operator substitutes nginx for Caddy.
