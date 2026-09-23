//! The shared application state.
//!
//! Handlers see ports, not adapters. The concrete types are chosen once in
//! `main` and everything downstream is written against `domain` traits, which
//! is what keeps the hexagon's arrows pointing inward.

use std::sync::Arc;

use nyuka_domain::ports::{
    ChapterRepository, EventBus, FollowRepository, JobQueue, LibraryStore, MangaRepository,
    SourceCatalog, SourceItem, SourceRegistry, SourceRepository, UserRepository,
};
use sea_orm::DatabaseConnection;
use tokio::sync::broadcast;

use crate::config::Config;

/// How many events a slow SSE client may fall behind before it is dropped.
///
/// A `broadcast` channel drops the *oldest* events for a lagging receiver
/// rather than blocking the sender, so this bounds memory instead of letting
/// one stalled browser tab stall the job engine (ADR-0010).
pub const EVENT_CAPACITY: usize = 256;

pub struct AppState {
    pub config: Config,
    /// Held for `/readyz` and for nothing else. Handlers use the ports.
    pub db: DatabaseConnection,

    pub manga: Arc<dyn MangaRepository>,
    pub chapters: Arc<dyn ChapterRepository>,
    pub follows: Arc<dyn FollowRepository>,
    pub sources: Arc<dyn SourceRepository>,
    pub registry: Arc<dyn SourceRegistry>,
    pub catalog: Arc<dyn SourceCatalog>,
    pub items: Arc<dyn SourceItem>,
    pub library: Arc<dyn LibraryStore>,
    pub queue: Arc<dyn JobQueue>,
    pub events: Arc<dyn EventBus>,
    pub users: Arc<dyn UserRepository>,

    /// Shared with the worker pool.
    ///
    /// The API holds it so a freshly installed source's declared limit takes
    /// effect immediately: registering it only at the next restart would let
    /// the first download of a new source ignore what it asked for, which is
    /// the fastest route to the IP ban ADR-0004 warns about.
    pub limiter: Arc<nyuka_jobs::limiter::SourceLimiter>,
    pub sessions: nyuka_persistence::session::SessionRepository,

    /// The discovered OIDC client.
    ///
    /// `Option` because a route test builds a router without reaching an
    /// identity provider. `main` always has one — an unreachable issuer fails
    /// the boot (ADR-0005 follow-up 5) — so `None` here means "this router was
    /// assembled for a test", and the auth routes say so rather than
    /// pretending to work.
    pub oidc: Option<Arc<crate::auth::OidcClient>>,

    /// The sender side of the SSE fan-out. Subscribers come from here.
    pub event_tx: broadcast::Sender<nyuka_domain::model::JobEvent>,
}

/// An `EventBus` over a `tokio::sync::broadcast` channel.
///
/// `send` failing means nobody is listening, which is the normal state of a
/// server with no browser tab open. Treating it as an error would log once per
/// event for as long as the UI is closed.
pub struct BroadcastBus {
    tx: broadcast::Sender<nyuka_domain::model::JobEvent>,
}

impl BroadcastBus {
    pub fn new(tx: broadcast::Sender<nyuka_domain::model::JobEvent>) -> Self {
        Self { tx }
    }
}

impl EventBus for BroadcastBus {
    fn publish(&self, event: nyuka_domain::model::JobEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyuka_domain::model::{JobEvent, JobId, JobState};
    use uuid::Uuid;

    fn event() -> JobEvent {
        JobEvent::JobState {
            job_id: JobId(Uuid::new_v4()),
            state: JobState::Succeeded,
        }
    }

    /// The normal state of a server with no browser tab open.
    #[test]
    fn publishing_with_no_subscribers_is_not_an_error() {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        let bus = BroadcastBus::new(tx);
        // Dropping the only receiver is what a closed tab does.
        bus.publish(event());
    }

    #[test]
    fn a_subscriber_receives_what_is_published() {
        let (tx, mut rx) = broadcast::channel(EVENT_CAPACITY);
        BroadcastBus::new(tx).publish(event());
        assert!(rx.try_recv().is_ok());
    }

    /// A stalled tab must cost bounded memory, not unbounded. `broadcast`
    /// drops the oldest events for a lagging receiver rather than blocking the
    /// publisher, which is what keeps one browser from stalling the job engine.
    #[test]
    fn a_lagging_subscriber_is_dropped_rather_than_blocking_the_publisher() {
        let (tx, mut rx) = broadcast::channel(2);
        let bus = BroadcastBus::new(tx);
        for _ in 0..10 {
            bus.publish(event());
        }
        assert!(
            matches!(
                rx.try_recv(),
                Err(broadcast::error::TryRecvError::Lagged(_))
            ),
            "a receiver that fell behind must be told it lagged, not silently fed stale events"
        );
    }
}
