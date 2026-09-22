//! The OpenAPI 3.1 document (utoipa 5.x).
//!
//! Routes are registered through `utoipa-axum`'s `OpenApiRouter`, so adding a
//! route and registering its schema are one call and drift is harder to
//! introduce. `cargo xtask openapi` writes `web/openapi.json`, which is
//! committed; CI fails when it is stale (ADR-0008, ADR-0009).
//!
//! Note for the frontend: utoipa emits OpenAPI 3.1 nullability as a type array
//! or `oneOf: [{type: "null"}, {$ref}]`. That is correct 3.1 and is handled by
//! `openapi-typescript`; it is also a known friction point for some other
//! generators (ADR-0009).
