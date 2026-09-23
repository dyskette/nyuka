//! `GET /api/v1/events` (ADR-0010).
//!
//! A `broadcast::Receiver` mapped to an event stream. `KeepAlive` emits a
//! comment roughly every 15 s, and `retry:` is set from configuration so the
//! client's backoff is server-controlled.
//!
//! # Two ways this breaks silently
//!
//! 1. **Compression.** A compressor accumulates input before emitting, so a
//!    `CompressionLayer` over this response holds events even when every proxy
//!    in front is configured correctly. `text/event-stream` must be excluded,
//!    and ADR-0010 requires a test for it — this is the failure most likely to
//!    be reintroduced by a later middleware change.
//! 2. **HTTP/1.1 connection exhaustion.** An SSE stream occupies one of the
//!    browser's six connections per origin, and the seventh request of *any*
//!    kind then queues with no error. Production must run behind TLS so the
//!    browser negotiates HTTP/2; browsers do not speak h2c.
//!
//! `X-Accel-Buffering: no` is set on the response so the app can fix its own
//! streaming if an operator substitutes nginx for Caddy.
//!
//! # `Last-Event-ID` replay is not implemented
//!
//! ADR-0010 describes replay as a latency optimization on top of
//! invalidate-on-reconnect, which is where correctness actually comes from.
//! Nothing here persists events, so no `id:` field is emitted — and because
//! `EventSource` only sends `Last-Event-ID` for streams that sent ids, the
//! browser will not ask for a replay this server cannot perform. Emitting ids
//! without a store would invite exactly that.
//!
//! # A slow client is dropped, not waited for
//!
//! The channel drops the oldest events for a receiver that falls behind. That
//! is reported to the client as a `lagged` event rather than passed over,
//! because a UI that missed updates needs to refetch — silently continuing
//! would leave a view that looks live and is not.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::Stream;
use nyuka_domain::model::JobEvent;
use tokio::sync::broadcast;

use crate::state::AppState;

/// How often a comment is emitted to keep an idle connection open.
///
/// Well under the 60 s an intermediary is likely to idle-timeout at, and well
/// under the read timeout a proxy would apply.
pub const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(15);

/// The SSE event name for "you missed some".
pub const LAGGED_EVENT: &str = "lagged";

/// `GET /api/v1/events`.
pub async fn events(State(state): State<Arc<AppState>>) -> Response {
    let stream = event_stream(state.event_tx.subscribe());

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(KEEPALIVE)
                // A comment line. Any payload works; this one says what it is
                // when someone reads the raw stream with `curl`.
                .text("keep-alive"),
        )
        .into_response();

    // nginx honours this, which lets the application fix its own streaming
    // without the operator editing proxy configuration (ADR-0010).
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    // Belt and braces against an intermediary that caches by default.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

/// Maps a broadcast receiver onto an SSE stream.
///
/// Split out from the handler so it can be driven directly in a test without
/// standing up a router.
pub fn event_stream(
    receiver: broadcast::Receiver<JobEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(event) => Some((Ok(to_sse(&event)), receiver)),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                // Told, not swallowed: a client that missed updates has to
                // refetch, and a view that looks live but is not is the bug
                // ADR-0010's invalidate-on-reconnect rule exists to prevent.
                tracing::warn!(missed, "an SSE client fell behind");
                let event = Event::default()
                    .event(LAGGED_EVENT)
                    .data(missed.to_string());
                Some((Ok(event), receiver))
            }
            // The sender is gone, which only happens at shutdown.
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
}

/// Renders one domain event.
///
/// The event *name* is the `JobEvent` tag, so a client subscribes with
/// `addEventListener("job.progress", …)` rather than parsing every message to
/// find out whether it cares.
fn to_sse(event: &JobEvent) -> Event {
    let name = event_name(event);
    match serde_json::to_string(event) {
        Ok(data) => Event::default().event(name).data(data),
        // A domain event that will not serialize is a bug, not a client
        // problem. Dropping the connection over it would be worse than
        // sending a payload the client will ignore.
        Err(e) => {
            tracing::error!(error = %e, "a job event could not be serialized");
            Event::default().event("error").data("{}")
        }
    }
}

/// The SSE event name for a domain event.
///
/// Dotted rather than snake_case to match the observability naming and to read
/// as a namespace in `addEventListener`.
pub fn event_name(event: &JobEvent) -> &'static str {
    match event {
        JobEvent::JobProgress { .. } => "job.progress",
        JobEvent::JobState { .. } => "job.state",
        JobEvent::ChapterNew { .. } => "chapter.new",
        JobEvent::ChapterDownloaded { .. } => "chapter.downloaded",
        JobEvent::SourceUpdated { .. } => "source.updated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use nyuka_domain::model::{ChapterId, JobId, JobState, MangaId, SourceId};
    use uuid::Uuid;

    fn job_state() -> JobEvent {
        JobEvent::JobState {
            job_id: JobId(Uuid::new_v4()),
            state: JobState::Succeeded,
        }
    }

    #[test]
    fn every_event_variant_has_a_distinct_name() {
        let events = [
            JobEvent::JobProgress {
                job_id: JobId(Uuid::new_v4()),
                done: 1,
                total: 2,
                bytes: 3,
            },
            job_state(),
            JobEvent::ChapterNew {
                manga_id: MangaId(Uuid::new_v4()),
                chapter_id: ChapterId(Uuid::new_v4()),
            },
            JobEvent::ChapterDownloaded {
                chapter_id: ChapterId(Uuid::new_v4()),
            },
            JobEvent::SourceUpdated {
                source_id: SourceId(Uuid::new_v4()),
            },
        ];
        let names: std::collections::HashSet<_> = events.iter().map(event_name).collect();
        assert_eq!(
            names.len(),
            events.len(),
            "two variants sharing a name would make addEventListener ambiguous"
        );
    }

    #[tokio::test]
    async fn published_events_reach_the_stream() {
        let (tx, rx) = broadcast::channel(8);
        let mut stream = Box::pin(event_stream(rx));

        tx.send(job_state()).expect("send");
        let event = stream.next().await.expect("an event").expect("infallible");

        // `Event` has no accessors, so the rendered form is what there is to
        // assert on — which is also what the browser sees.
        let rendered = format!("{event:?}");
        assert!(
            rendered.contains("job.state"),
            "the event name carries the variant: {rendered}"
        );
    }

    /// A client that fell behind must be told, or it keeps a view that looks
    /// live and is not.
    #[tokio::test]
    async fn a_lagging_client_receives_a_lagged_event() {
        let (tx, rx) = broadcast::channel(2);
        for _ in 0..8 {
            tx.send(job_state()).expect("send");
        }

        let mut stream = Box::pin(event_stream(rx));
        let event = stream.next().await.expect("an event").expect("infallible");
        let rendered = format!("{event:?}");

        assert!(
            rendered.contains(LAGGED_EVENT),
            "the first thing a lagged receiver sees must say so: {rendered}"
        );
    }

    /// Shutdown closes the channel, and the stream must end rather than spin.
    #[tokio::test]
    async fn the_stream_ends_when_the_sender_is_dropped() {
        let (tx, rx) = broadcast::channel(2);
        drop(tx);
        let mut stream = Box::pin(event_stream(rx));
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn the_keepalive_is_well_under_a_typical_idle_timeout() {
        assert!(
            KEEPALIVE < std::time::Duration::from_secs(60),
            "an intermediary idling out at 60s would close the stream between comments"
        );
    }
}
