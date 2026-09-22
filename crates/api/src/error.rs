//! problem+json responses (RFC 9457).
//!
//! `DomainError` maps here and nowhere else. Fields: `type`, `title`,
//! `status`, `detail`, `instance`, and `errors[]` for field-level validation
//! so the frontend's TanStack Form can attach them to inputs.
//!
//! `DomainError::UnsupportedCapability` must name the missing capability, so
//! an install refusal reads as a host gap rather than a site problem
//! (ADR-0004).
