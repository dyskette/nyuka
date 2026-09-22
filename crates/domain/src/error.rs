//! Domain-level errors.
//!
//! Adapters map their own failures into these; `nyuka-api` maps these into
//! problem+json responses. No adapter error type leaks past this boundary.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DomainError>;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("not found")]
    NotFound,

    #[error("conflict: {0}")]
    Conflict(String),

    /// A rule was violated by the caller's input.
    #[error("invalid: {0}")]
    Invalid(String),

    /// The source declined or failed. Carries whether a retry could succeed,
    /// because the job engine's backoff decision depends on it (ADR-0003).
    #[error("source error: {message}")]
    Source { message: String, retryable: bool },

    /// A source needs a host capability this build does not implement
    /// (ADR-0004). Distinct from `Source` so the API can name the missing
    /// capability instead of reporting a generic failure.
    #[error("unsupported source capability: {0}")]
    UnsupportedCapability(String),

    #[error("storage error: {0}")]
    Storage(String),

    /// Out of disk. Separate from `Storage` because the packaging handler must
    /// clean up the partial write and `/readyz` must report the library as not
    /// writable (ADR-0007).
    #[error("insufficient storage")]
    InsufficientStorage,

    #[error("internal error: {0}")]
    Internal(String),
}

impl DomainError {
    /// Whether the job engine should schedule another attempt.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Source { retryable, .. } => *retryable,
            Self::Storage(_) => true,
            Self::NotFound
            | Self::Conflict(_)
            | Self::Invalid(_)
            | Self::UnsupportedCapability(_)
            | Self::InsufficientStorage
            | Self::Internal(_) => false,
        }
    }
}
