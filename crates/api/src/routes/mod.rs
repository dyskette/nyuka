//! Route modules under `/api/v1`.
//!
//! Handlers are thin: parse the DTO, call a `domain` use case, map the result.
//! Business logic lives in `domain`, and `#[tracing::instrument]` goes on the
//! use cases rather than on handlers (ADR-0015).
//!
//! ```text
//! auth          GET  /auth/login, /auth/callback, POST /auth/logout, GET /me
//! source-repos  CRUD, POST /{id}/refresh
//! sources       GET, POST, DELETE, GET|PUT /{id}/settings, GET /{id}/filters
//! catalog       GET /sources/{id}/catalog, /manga/{key}, /manga/{key}/chapters
//! manga         GET /manga, /manga/{id}, /manga/{id}/chapters, POST /manga
//! follows       CRUD, POST /{id}/check-now
//! downloads     POST /downloads (Idempotency-Key) -> 202 + Location
//!               GET  /downloads/{chapter_id}/file (streams CBZ, Range)
//! jobs          GET, GET /{id}, POST /{id}/cancel, POST /{id}/retry
//! events        GET /events (SSE)
//! telemetry     POST /telemetry (ADR-0013)
//! health        GET /healthz, /readyz
//! ```
//!
//! Conventions: problem+json, cursor pagination, ETag/If-None-Match on reads,
//! `Retry-After` on 429 and 503, RFC 3339 timestamps, snake_case JSON.
//!
//! CSRF is a middleware invariant, not a per-handler concern: `X-Requested-With`
//! is required on every state-changing method. A future endpoint accepting a
//! form-encoded body, or a handler mounted outside the stack, reopens the hole
//! — ADR-0005 requires a test that a mutation without the header is rejected.

pub mod health;
pub mod telemetry;
