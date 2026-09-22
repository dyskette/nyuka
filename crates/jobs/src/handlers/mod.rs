//! Job handlers, one per `JobKind`.
//!
//! Handlers own the behavior a queue library would not have supplied:
//! per-source semaphores, page fan-out within a single job, cancellation tied
//! to partial-file cleanup, and the trace context stored in
//! `job.payload.trace_context` so `job.run` can link back to the request that
//! enqueued it (ADR-0015).
//!
//! Progress is published as `JobEvent` to the broadcast channel. Every state
//! change must also be observable through a plain fetch — SSE makes the UI
//! live, not correct (ADR-0010).
//!
//! `ENOSPC` is handled explicitly: delete the partial file, fail with
//! `DomainError::InsufficientStorage`, and let `/readyz` report the library as
//! not writable (ADR-0007).
//!
//! # Dispatch
//!
//! [`Registry`] maps a kind to its handler and is itself the [`JobHandler`]
//! the worker pool runs. Handlers are registered rather than matched in a
//! `match` arm so a deployment can run without one — a build with no source
//! runtime still prunes sessions — and so each handler can be tested against
//! fakes for the ports it needs.

pub mod maintenance;

use std::collections::HashMap;
use std::sync::Arc;

use nyuka_domain::model::{Job, JobKind};
use nyuka_domain::{DomainError, Result};

use crate::queue::kind_to_str;
use crate::worker::JobHandler;

/// Handles one kind of job.
#[async_trait::async_trait]
pub trait KindHandler: Send + Sync {
    async fn handle(&self, job: &Job) -> Result<()>;
}

/// Dispatches a job to the handler registered for its kind.
#[derive(Default)]
pub struct Registry {
    handlers: HashMap<JobKind, Arc<dyn KindHandler>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registering a kind twice replaces the earlier handler, so composition
    /// order decides and a duplicate cannot silently run both.
    pub fn register(mut self, kind: JobKind, handler: Arc<dyn KindHandler>) -> Self {
        self.handlers.insert(kind, handler);
        self
    }

    pub fn handles(&self, kind: JobKind) -> bool {
        self.handlers.contains_key(&kind)
    }
}

#[async_trait::async_trait]
impl JobHandler for Registry {
    async fn handle(&self, job: Job) -> Result<()> {
        let kind = kind_to_str(job.kind);
        let Some(handler) = self.handlers.get(&job.kind) else {
            // `Internal` is not retryable, so an unhandled kind fails once and
            // stops. Retrying would burn every attempt on a job that no amount
            // of waiting can make runnable.
            return Err(DomainError::Internal(format!(
                "no handler registered for job kind {kind}"
            )));
        };

        // The span carries the ids an operator correlates on. Extracting the
        // W3C parent from `payload.trace_context` is the composition root's
        // job, because the propagator lives with the exporter (ADR-0015).
        let span = tracing::info_span!(
            "job.run",
            job.id = %job.id,
            job.kind = kind,
            job.attempt = job.attempts,
        );
        let _guard = span.enter();

        handler.handle(&job).await
    }
}

/// Reads the W3C trace context an enqueuing request stored on the payload.
///
/// Returns the carrier rather than a span: the `TraceContextPropagator` lives
/// in the composition root alongside the exporter, and pulling it in here
/// would make this crate depend on the telemetry stack it is only a source of
/// spans for.
pub fn trace_carrier(job: &Job) -> HashMap<String, String> {
    job.payload
        .get("trace_context")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Reads a required field from a job payload.
///
/// A malformed payload is `Invalid`, which is not retryable: the row will
/// never parse, so retrying it only delays the failure.
pub fn payload_field<T: serde::de::DeserializeOwned>(job: &Job, field: &str) -> Result<T> {
    let value = job.payload.get(field).ok_or_else(|| {
        DomainError::Invalid(format!(
            "job {} of kind {} has no `{field}` in its payload",
            job.id,
            kind_to_str(job.kind)
        ))
    })?;
    serde_json::from_value(value.clone())
        .map_err(|e| DomainError::Invalid(format!("job payload field `{field}` is invalid: {e}")))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use chrono::Utc;
    use nyuka_domain::model::{JobId, JobState};
    use uuid::Uuid;

    pub fn job(kind: JobKind, payload: serde_json::Value) -> Job {
        Job {
            id: JobId(Uuid::new_v4()),
            kind,
            state: JobState::Running,
            payload,
            priority: 0,
            run_at: Utc::now(),
            attempts: 1,
            max_attempts: 5,
            last_error: None,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::job;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl KindHandler for Counting {
        async fn handle(&self, _job: &Job) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_job_reaches_the_handler_registered_for_its_kind() {
        let hits = Arc::new(AtomicUsize::new(0));
        let other = Arc::new(AtomicUsize::new(0));
        let registry = Registry::new()
            .register(JobKind::PruneJobs, Arc::new(Counting(hits.clone())))
            .register(JobKind::PruneSessions, Arc::new(Counting(other.clone())));

        registry
            .handle(job(JobKind::PruneJobs, serde_json::json!({})))
            .await
            .expect("handled");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(other.load(Ordering::SeqCst), 0, "no other handler may run");
    }

    #[tokio::test]
    async fn an_unregistered_kind_fails_without_retrying() {
        let registry = Registry::new();
        let err = registry
            .handle(job(JobKind::DownloadChapter, serde_json::json!({})))
            .await
            .expect_err("nothing is registered");
        assert!(
            !err.is_retryable(),
            "retrying a kind no build handles would burn every attempt"
        );
        assert!(err.to_string().contains("download_chapter"));
    }

    #[tokio::test]
    async fn registering_a_kind_twice_keeps_the_last() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let registry = Registry::new()
            .register(JobKind::PruneJobs, Arc::new(Counting(first.clone())))
            .register(JobKind::PruneJobs, Arc::new(Counting(second.clone())));

        registry
            .handle(job(JobKind::PruneJobs, serde_json::json!({})))
            .await
            .expect("handled");
        assert_eq!(first.load(Ordering::SeqCst), 0);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_missing_payload_field_is_not_retryable() {
        let err = payload_field::<String>(&job(JobKind::CheckFollow, serde_json::json!({})), "id")
            .expect_err("no such field");
        assert!(!err.is_retryable(), "a payload cannot fix itself on retry");
    }

    #[test]
    fn a_wrongly_typed_payload_field_is_rejected() {
        let j = job(JobKind::CheckFollow, serde_json::json!({ "id": 7 }));
        assert!(payload_field::<String>(&j, "id").is_err());
    }

    #[test]
    fn a_payload_field_of_the_right_type_reads_back() {
        let j = job(JobKind::CheckFollow, serde_json::json!({ "id": "abc" }));
        assert_eq!(payload_field::<String>(&j, "id").expect("read"), "abc");
    }

    #[test]
    fn the_trace_carrier_survives_a_payload_without_one() {
        assert!(
            trace_carrier(&job(JobKind::PruneJobs, serde_json::json!({}))).is_empty(),
            "a job enqueued outside a request still has to run"
        );
    }

    #[test]
    fn the_trace_carrier_reads_w3c_headers_off_the_payload() {
        let j = job(
            JobKind::PruneJobs,
            serde_json::json!({
                "trace_context": { "traceparent": "00-abc-def-01", "nested": { "x": 1 } }
            }),
        );
        let carrier = trace_carrier(&j);
        assert_eq!(
            carrier.get("traceparent").map(String::as_str),
            Some("00-abc-def-01")
        );
        assert!(
            !carrier.contains_key("nested"),
            "a carrier is string-to-string; a non-string value is dropped rather than stringified"
        );
    }
}
