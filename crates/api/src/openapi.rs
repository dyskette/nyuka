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
//!
//! # The document describes `/api/v1`, and nothing else
//!
//! The health probes are not in it. They are not a versioned API, an
//! orchestrator does not read a schema to call them, and including them would
//! generate a typed client method nobody should use.

use utoipa::OpenApi;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};

/// Describes how a client authenticates.
///
/// The session cookie is the only credential. Named here so a generated client
/// knows to send credentials rather than looking for a bearer token that does
/// not exist (ADR-0005).
pub struct SessionCookie;

impl utoipa::Modify for SessionCookie {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "session",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                    "id",
                    "An opaque, signed, HttpOnly session cookie issued by \
                     `GET /api/v1/auth/callback`. There is no bearer token: \
                     tokens from the identity provider never reach the browser.",
                ))),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "nyuka",
        description = "A self-hosted manga library server.",
        license(name = "AGPL-3.0-or-later"),
    ),
    modifiers(&SessionCookie),
    tags(
        (name = "library", description = "Series and chapters already in the library"),
        (name = "follows", description = "Series watched for new chapters"),
        (name = "jobs", description = "Background work"),
    ),
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_declares_the_session_cookie_and_no_bearer_scheme() {
        let doc = serde_json::to_value(ApiDoc::openapi()).expect("serialize");
        let schemes = doc["components"]["securitySchemes"]
            .as_object()
            .expect("security schemes");

        let session = &schemes["session"];
        assert_eq!(session["type"], "apiKey");
        assert_eq!(session["in"], "cookie");

        // Inspected structurally rather than by searching the rendered text:
        // the scheme's own description says "there is no bearer token", so a
        // substring search matches this module's prose and passes regardless
        // of what is declared.
        for (name, scheme) in schemes {
            assert_ne!(
                scheme["scheme"], "bearer",
                "`{name}` advertises a bearer token; tokens never reach the \
                 browser, so a client looking for one would find nothing"
            );
            assert_ne!(scheme["type"], "http", "`{name}` declares HTTP auth");
        }
    }

    #[test]
    fn the_document_is_openapi_3_1() {
        let rendered = serde_json::to_value(ApiDoc::openapi()).expect("serialize");
        assert!(
            rendered["openapi"]
                .as_str()
                .is_some_and(|v| v.starts_with("3.1")),
            "openapi-typescript is configured for 3.1: {}",
            rendered["openapi"]
        );
    }
}
