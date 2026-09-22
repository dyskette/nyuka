//! The embedded SPA (ADR-0006).
//!
//! `rust-embed` over `web/dist`, served from the same origin as the API.
//!
//! Route precedence is load-bearing: `/api/v1/*` matches first, then exact
//! asset paths, then an HTML fallback for client-side routing. **An unmatched
//! path under `/api/v1/` must return problem+json `404`, never `index.html`**
//! — otherwise a typo in a fetch surfaces as an HTML parse error instead of a
//! clear 404. ADR-0006 requires a test for both directions.
//!
//! Caching: content-hashed assets get
//! `Cache-Control: public, max-age=31536000, immutable`; `index.html` gets
//! `no-cache`. Getting these backwards produces a stale UI that survives a
//! redeploy, so that is also a test.
