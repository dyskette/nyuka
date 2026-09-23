//! problem+json responses (RFC 9457).
//!
//! `DomainError` maps here and nowhere else. Fields: `type`, `title`,
//! `status`, `detail`, `instance`, and `errors[]` for field-level validation
//! so the frontend's TanStack Form can attach them to inputs.
//!
//! `DomainError::UnsupportedCapability` must name the missing capability, so
//! an install refusal reads as a host gap rather than a site problem
//! (ADR-0004).
//!
//! # Server faults do not describe themselves
//!
//! A 4xx `detail` is written for the caller and says what to change. A 5xx
//! `detail` is a fixed string, because the underlying message is a connection
//! string, a file path, or a SQL fragment — none of which the caller can act
//! on and all of which help an attacker. The real message goes to the log
//! stream with the trace id, and the response carries that id so an operator
//! can find it from a screenshot.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nyuka_domain::DomainError;
use serde::Serialize;

pub const PROBLEM_JSON: &str = "application/problem+json";

/// What the caller gets when something goes wrong.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Problem {
    /// A URI reference identifying the problem kind. Relative on purpose:
    /// this project does not own a domain to publish these under, and RFC 9457
    /// permits a relative reference.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The request path, filled in by the middleware rather than by handlers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Field-level failures, so a form can attach each to its input.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<FieldError>,
    /// The trace id for this request, so a screenshot is enough to find the
    /// log line. Present on server faults, where `detail` deliberately is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Seconds to wait, mirrored into the `Retry-After` header.
    #[serde(skip)]
    pub retry_after: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldError {
    /// A JSON Pointer into the request body, such as `/check_interval_secs`.
    pub field: String,
    pub message: String,
}

impl Problem {
    pub fn new(status: StatusCode, kind: &str, title: &str) -> Self {
        Self {
            kind: format!("/problems/{kind}"),
            title: title.into(),
            status: status.as_u16(),
            detail: None,
            instance: None,
            errors: Vec::new(),
            trace_id: None,
            retry_after: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    pub fn with_errors(mut self, errors: Vec<FieldError>) -> Self {
        self.errors = errors;
        self
    }

    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    pub fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Whether this is a server fault, and therefore must not describe itself.
    pub fn is_server_fault(&self) -> bool {
        self.status >= 500
    }

    // --- the ones handlers reach for -------------------------------------

    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", "Not found")
            .with_detail(format!("No {what} matches that identifier."))
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid request",
        )
        .with_detail(detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", "Conflict").with_detail(detail)
    }

    /// No session, or one that has expired.
    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Authentication required",
        )
        .with_detail("Sign in and try again.")
    }

    /// Authenticated, but not on the allow-list.
    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "Forbidden")
            .with_detail("This account is not permitted to use this server.")
    }

    /// The CSRF header is missing on a state-changing request (ADR-0005).
    pub fn csrf_required(header_name: &str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf-required",
            "Missing CSRF header",
        )
        .with_detail(format!(
            "State-changing requests must carry the {header_name} header."
        ))
    }

    pub fn too_many_requests(retry_after: u64) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limited",
            "Too many requests",
        )
        .with_detail("Slow down and retry after the interval given.")
        .with_retry_after(retry_after)
    }

    pub fn payload_too_large(limit: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload-too-large",
            "Request body too large",
        )
        .with_detail(format!("The limit is {limit} bytes."))
    }

    /// The generic server fault, with nothing about its cause.
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal server error",
        )
        .with_detail("Something went wrong on the server. The failure has been logged.")
    }
}

/// Maps a domain failure onto a response.
///
/// The status choices that are not obvious:
///
/// | Domain error | Status | Why |
/// |---|---|---|
/// | `Source` | 502 | The failure is upstream. A 500 would say this server is broken, sending an operator to the wrong logs. |
/// | `UnsupportedCapability` | 501 | The request was valid and this build cannot serve it. Naming the capability is what makes it read as a host gap rather than a site problem (ADR-0004). |
/// | `InsufficientStorage` | 507 | Distinct from a generic 500 so the UI can say "the disk is full" instead of "try again", which will not help. |
/// | `Storage` | 500 | The caller did nothing wrong and can do nothing about it. |
impl From<DomainError> for Problem {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::NotFound => Problem::not_found("resource"),

            DomainError::Conflict(detail) => Problem::conflict(detail),

            DomainError::Invalid(detail) => Problem::invalid(detail),

            DomainError::Source { message, retryable } => {
                let problem = Problem::new(
                    StatusCode::BAD_GATEWAY,
                    "source-unavailable",
                    "The source could not be reached",
                )
                // Safe to pass through: this text describes the upstream site,
                // not this server's internals, and without it the UI can only
                // say "something failed".
                .with_detail(message);
                if retryable {
                    problem.with_retry_after(60)
                } else {
                    problem
                }
            }

            DomainError::UnsupportedCapability(capability) => Problem::new(
                StatusCode::NOT_IMPLEMENTED,
                "unsupported-capability",
                "Unsupported source capability",
            )
            .with_detail(format!(
                "This build does not provide {capability}, which the source requires."
            )),

            DomainError::InsufficientStorage => Problem::new(
                StatusCode::INSUFFICIENT_STORAGE,
                "insufficient-storage",
                "The library has no space left",
            )
            .with_detail("Free space on the library volume and retry."),

            // Both carry internal strings — a connection string, a path, a SQL
            // fragment. The message is logged; the response is not given it.
            DomainError::Storage(_) | DomainError::Internal(_) => Problem::internal(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let retry_after = self.retry_after;

        let mut response = (status, Json(&self)).into_response();

        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));

        if let Some(seconds) = retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }

        response
    }
}

/// The error type handlers return.
///
/// A newtype rather than `Problem` directly, so a handler that returns the
/// wrong thing fails to compile instead of returning a 200 whose body happens
/// to describe an error.
///
/// Boxed because this rides in the `Err` of every handler's return type, and
/// `Problem` is large enough that carrying it inline taxes every success path
/// too.
#[derive(Debug)]
pub struct ApiError(pub Box<Problem>);

impl ApiError {
    pub fn problem(&self) -> &Problem {
        &self.0
    }
}

impl<E: Into<Problem>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(Box::new(error.into()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (*self.0).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("problem+json")
    }

    fn problem_for(error: DomainError) -> Problem {
        Problem::from(error)
    }

    #[test]
    fn each_domain_error_maps_to_a_distinct_status() {
        for (error, status) in [
            (DomainError::NotFound, 404),
            (DomainError::Conflict("c".into()), 409),
            (DomainError::Invalid("i".into()), 400),
            (
                DomainError::Source {
                    message: "s".into(),
                    retryable: false,
                },
                502,
            ),
            (DomainError::UnsupportedCapability("canvas".into()), 501),
            (DomainError::InsufficientStorage, 507),
            (DomainError::Storage("s".into()), 500),
            (DomainError::Internal("i".into()), 500),
        ] {
            assert_eq!(problem_for(error).status, status);
        }
    }

    /// The control this module exists for. A storage error's message is a
    /// connection string or a path; the caller can do nothing with it and an
    /// attacker can.
    #[test]
    fn a_server_fault_does_not_repeat_its_cause() {
        for error in [
            DomainError::Storage(
                "connection to postgres://nyuka:hunter2@db.internal:5432 refused".into(),
            ),
            DomainError::Internal("/var/lib/nyuka/secret.key: permission denied".into()),
        ] {
            let problem = problem_for(error);
            let rendered = serde_json::to_string(&problem).expect("serialize");
            assert!(!rendered.contains("hunter2"));
            assert!(!rendered.contains("db.internal"));
            assert!(!rendered.contains("secret.key"));
            assert!(problem.is_server_fault());
        }
    }

    /// And the other direction: a 4xx must say what to change, or the caller
    /// is guessing.
    #[test]
    fn a_client_fault_does_repeat_its_cause() {
        let problem = problem_for(DomainError::Invalid(
            "check_interval_secs must be at least 300".into(),
        ));
        assert_eq!(
            problem.detail.as_deref(),
            Some("check_interval_secs must be at least 300")
        );
    }

    /// ADR-0004: an install refusal must read as a host gap, not a site
    /// problem. The capability name is the whole difference.
    #[test]
    fn an_unsupported_capability_is_named() {
        let problem = problem_for(DomainError::UnsupportedCapability(
            "js (embedded web view)".into(),
        ));
        assert_eq!(problem.status, 501);
        assert!(
            problem
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("js (embedded web view)")),
            "an operator cannot act on `unsupported capability` alone"
        );
    }

    #[test]
    fn a_retryable_source_failure_carries_retry_after_and_a_permanent_one_does_not() {
        let retryable = problem_for(DomainError::Source {
            message: "timed out".into(),
            retryable: true,
        });
        assert_eq!(retryable.retry_after, Some(60));

        let permanent = problem_for(DomainError::Source {
            message: "404".into(),
            retryable: false,
        });
        assert!(permanent.retry_after.is_none());
    }

    #[tokio::test]
    async fn the_response_content_type_is_problem_json() {
        let response = Problem::not_found("manga").into_response();
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some(PROBLEM_JSON),
            "a client checking the content type must be able to tell a problem \
             from an ordinary body"
        );
    }

    #[tokio::test]
    async fn retry_after_reaches_the_header_and_not_the_body() {
        let response = Problem::too_many_requests(30).into_response();
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("30")
        );
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let body = body_of(response).await;
        assert!(
            body.get("retry_after").is_none(),
            "Retry-After is a header; duplicating it in the body invites the two to disagree"
        );
    }

    #[tokio::test]
    async fn the_body_carries_the_rfc_9457_members() {
        let response = Problem::invalid("bad")
            .with_instance("/api/v1/follows")
            .with_errors(vec![FieldError {
                field: "/check_interval_secs".into(),
                message: "must be at least 300".into(),
            }])
            .into_response();

        let body = body_of(response).await;
        assert_eq!(body["type"], "/problems/invalid-request");
        assert_eq!(body["title"], "Invalid request");
        assert_eq!(body["status"], 400);
        assert_eq!(body["detail"], "bad");
        assert_eq!(body["instance"], "/api/v1/follows");
        assert_eq!(body["errors"][0]["field"], "/check_interval_secs");
    }

    /// Absent members are omitted, not sent as null: RFC 9457 treats an absent
    /// member as "no information", and a null says something different.
    #[tokio::test]
    async fn absent_members_are_omitted() {
        let body =
            body_of(Problem::new(StatusCode::NOT_FOUND, "not-found", "Not found").into_response())
                .await;
        let object = body.as_object().expect("object");
        assert!(!object.contains_key("detail"));
        assert!(!object.contains_key("instance"));
        assert!(!object.contains_key("errors"));
        assert!(!object.contains_key("trace_id"));
    }

    /// A server fault carries the trace id instead of a detail, so a
    /// screenshot is enough to find the log line.
    #[tokio::test]
    async fn a_server_fault_can_carry_a_trace_id() {
        let body = body_of(
            Problem::internal()
                .with_trace_id("4bf92f3577b34da6a3ce929d0e0e4736")
                .into_response(),
        )
        .await;
        assert_eq!(body["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[tokio::test]
    async fn a_domain_error_converts_through_the_question_mark_operator() {
        fn handler() -> ApiResult<()> {
            Err(DomainError::NotFound)?;
            Ok(())
        }
        let error = handler().expect_err("not found");
        assert_eq!(error.problem().status, 404);
        assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
    }
}
