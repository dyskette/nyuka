//! Per-source concurrency limiting.
//!
//! One semaphore per source, bounding page fetches inside a download handler.
//!
//! This is correct **only because there is one process** (ADR-0003). Two
//! processes each hold their own semaphore, so the configured cap produces
//! twice the request rate toward the site, and the consequence is an IP ban on
//! the sites the application exists to read. Running a second instance is not a
//! scaling knob; it is a redesign.

use std::collections::HashMap;
use std::sync::Arc;

use nyuka_aidoku_runtime::imports::net::RateLimit;
use nyuka_domain::model::SourceId;
use tokio::sync::{Mutex, Semaphore};

/// Concurrency budgets, one per source.
#[derive(Clone)]
pub struct SourceLimiter {
    /// The operator's ceiling, applied when a source declares nothing.
    default_permits: u32,
    limits: Arc<Mutex<HashMap<SourceId, Arc<Semaphore>>>>,
}

impl SourceLimiter {
    pub fn new(default_permits: u32) -> Self {
        Self {
            default_permits: default_permits.max(1),
            limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers a source's declared budget.
    ///
    /// The **stricter** of the source's limit and the operator's cap wins: a
    /// source that asks for less than the cap knows something about the site
    /// that the operator does not, and exceeding it is what gets the
    /// deployment banned (ADR-0004).
    pub async fn register(&self, source: SourceId, declared: Option<RateLimit>) {
        let permits = match declared {
            Some(limit) => limit.permits.min(self.default_permits).max(1),
            None => self.default_permits,
        };
        self.limits
            .lock()
            .await
            .insert(source, Arc::new(Semaphore::new(permits as usize)));
    }

    /// Waits for a slot against a source.
    ///
    /// The returned permit releases on drop, so a handler that panics mid-fetch
    /// does not leak the slot and stall the source forever.
    pub async fn acquire(&self, source: SourceId) -> tokio::sync::OwnedSemaphorePermit {
        let semaphore = {
            let mut map = self.limits.lock().await;
            map.entry(source)
                .or_insert_with(|| Arc::new(Semaphore::new(self.default_permits as usize)))
                .clone()
        };
        semaphore
            .acquire_owned()
            .await
            .expect("source semaphores are never closed")
    }

    /// Permits currently available, for the metrics snapshot.
    pub async fn available(&self, source: SourceId) -> usize {
        self.limits
            .lock()
            .await
            .get(&source)
            .map(|s| s.available_permits())
            .unwrap_or(self.default_permits as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn source() -> SourceId {
        SourceId(Uuid::new_v4())
    }

    #[tokio::test]
    async fn a_stricter_declared_limit_wins() {
        let limiter = SourceLimiter::new(4);
        let s = source();
        limiter
            .register(
                s,
                Some(RateLimit {
                    permits: 2,
                    period: Duration::from_secs(2),
                }),
            )
            .await;
        assert_eq!(limiter.available(s).await, 2, "the source asked for less");
    }

    #[tokio::test]
    async fn a_looser_declared_limit_does_not_raise_the_cap() {
        let limiter = SourceLimiter::new(4);
        let s = source();
        limiter
            .register(
                s,
                Some(RateLimit {
                    permits: 50,
                    period: Duration::from_secs(1),
                }),
            )
            .await;
        assert_eq!(
            limiter.available(s).await,
            4,
            "a source must not be able to raise the operator's ceiling"
        );
    }

    #[tokio::test]
    async fn a_source_declaring_nothing_gets_the_default() {
        let limiter = SourceLimiter::new(3);
        let s = source();
        limiter.register(s, None).await;
        assert_eq!(limiter.available(s).await, 3);
    }

    #[tokio::test]
    async fn concurrency_is_actually_bounded() {
        let limiter = SourceLimiter::new(2);
        let s = source();
        limiter.register(s, None).await;

        let a = limiter.acquire(s).await;
        let b = limiter.acquire(s).await;
        assert_eq!(limiter.available(s).await, 0);

        // A third must wait rather than proceed.
        let blocked = tokio::time::timeout(Duration::from_millis(50), limiter.acquire(s)).await;
        assert!(blocked.is_err(), "the cap was exceeded");

        drop(a);
        drop(b);
        assert_eq!(limiter.available(s).await, 2, "permits must return on drop");
    }

    /// Sources must not contend with each other: a slow site should not stall
    /// downloads from a different one.
    #[tokio::test]
    async fn sources_are_limited_independently() {
        let limiter = SourceLimiter::new(1);
        let (a, b) = (source(), source());
        let _held = limiter.acquire(a).await;
        let other = tokio::time::timeout(Duration::from_millis(50), limiter.acquire(b)).await;
        assert!(other.is_ok(), "one source's backlog must not block another");
    }
}
