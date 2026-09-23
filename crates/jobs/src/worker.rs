//! The worker pool.
//!
//! Workers poll the queue on a short interval with jitter. `LISTEN`/`NOTIFY`
//! is deliberately not used: notifications are lost when no listener is
//! connected, so a correct implementation polls anyway, and `NOTIFY` adds a
//! commit-time global lock plus a dedicated connection per listener for a
//! latency saving that a job running for seconds to minutes does not notice
//! (ADR-0003).
//!
//! # Shutdown drains rather than drops
//!
//! On `SIGTERM` the pool stops claiming, waits for in-flight work up to a
//! bounded timeout, and returns anything still unfinished to `queued`. Killing
//! a worker mid-download without releasing its row leaves the job invisible
//! until stale-lock recovery notices, which is minutes of nothing happening.
//!
//! ADR-0001 notes that axum 0.9 changes `serve`'s graceful-shutdown contract,
//! so the drain test here is what will catch that upgrade breaking shutdown.

use std::sync::Arc;
use std::time::Duration;

use nyuka_domain::Result;
use nyuka_domain::model::{Job, JobEvent, JobState};
use nyuka_domain::ports::EventBus;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::metrics::{self, JobMetrics};
use crate::queue::PostgresQueue;

/// An [`EventBus`] that discards everything.
///
/// For tests that do not observe events. Named rather than an `Option`, so a
/// call site says which it means instead of leaving "no bus" and "a bus I
/// forgot to pass" looking identical.
pub struct NoEvents;

impl EventBus for NoEvents {
    fn publish(&self, _event: JobEvent) {}
}

/// Runs one job.
#[async_trait::async_trait]
pub trait JobHandler: Send + Sync + 'static {
    /// Returning `Err` schedules a retry when the error is retryable and the
    /// job has attempts left; see `PostgresQueue::fail`.
    async fn handle(&self, job: Job) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Bounded from validated configuration, never implicit.
    pub workers: usize,
    pub poll_interval: Duration,
    /// How long shutdown waits for in-flight work before releasing it.
    pub drain_timeout: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            poll_interval: Duration::from_secs(1),
            drain_timeout: Duration::from_secs(30),
        }
    }
}

/// What a shutdown did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    /// Workers that finished their job and stopped cleanly.
    pub drained: usize,
    /// Jobs released back to `queued` because the timeout expired first.
    pub released: u64,
}

pub struct WorkerPool {
    cancel: CancellationToken,
    tasks: JoinSet<usize>,
    queue: Arc<PostgresQueue>,
    config: WorkerConfig,
}

impl WorkerPool {
    /// Starts the pool. Workers begin claiming immediately.
    pub fn start(
        queue: Arc<PostgresQueue>,
        handler: Arc<dyn JobHandler>,
        metrics: Arc<JobMetrics>,
        events: Arc<dyn EventBus>,
        config: WorkerConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        for index in 0..config.workers.max(1) {
            let queue = queue.clone();
            let handler = handler.clone();
            let metrics = metrics.clone();
            let events = events.clone();
            let cancel = cancel.clone();
            let poll = config.poll_interval;
            tasks.spawn(async move {
                run_worker(index, queue, handler, metrics, events, cancel, poll).await;
                index
            });
        }

        Self {
            cancel,
            tasks,
            queue,
            config,
        }
    }

    /// Stops claiming, waits for in-flight work, then releases what is left.
    pub async fn shutdown(mut self) -> DrainReport {
        // Workers observe this between jobs, so nothing new is claimed.
        self.cancel.cancel();

        let deadline = tokio::time::sleep(self.config.drain_timeout);
        tokio::pin!(deadline);

        let mut drained = 0usize;
        loop {
            tokio::select! {
                joined = self.tasks.join_next() => match joined {
                    Some(Ok(_)) => drained += 1,
                    // A panicking handler must not stop the drain: the point is
                    // to release every other worker's job.
                    Some(Err(_)) => drained += 1,
                    None => break,
                },
                _ = &mut deadline => break,
            }
        }

        // Stop the stragglers before releasing, not after: a worker still
        // running when the release query lands could otherwise mark a job
        // `succeeded` that has already gone back to `queued`, and the job
        // would then run twice.
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}

        // Anything still held is released so the next process picks it up
        // immediately rather than waiting minutes for stale-lock recovery.
        // Safe because a single instance owns every `running` row (ADR-0003);
        // under two instances this would steal the other's work.
        let released = self.queue.recover_stale(Duration::ZERO).await.unwrap_or(0);

        DrainReport { drained, released }
    }

    /// A token that fires when shutdown begins, for handlers that can stop
    /// early.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Resolves on `SIGTERM` or `SIGINT`.
///
/// `SIGTERM` is what a container runtime sends before `SIGKILL`, so it is the
/// one that matters: the interval between them is the whole drain budget, and
/// `drain_timeout` must stay under it.
pub async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            // Without a handler the default disposition kills the process
            // without draining, so this is worth saying out loud.
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM; shutdown will not drain")
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

async fn run_worker(
    index: usize,
    queue: Arc<PostgresQueue>,
    handler: Arc<dyn JobHandler>,
    metrics: Arc<JobMetrics>,
    events: Arc<dyn EventBus>,
    cancel: CancellationToken,
    poll: Duration,
) {
    let name = format!("worker-{index}");
    loop {
        if cancel.is_cancelled() {
            return;
        }

        match queue.claim(&name).await {
            Ok(Some(job)) => {
                let id = job.id;
                // Measured from when the job was created rather than from
                // when it was claimed: ADR-0019 starts the clock when the
                // request was accepted, and the wait in the queue is part of
                // what a person experienced.
                let created_at = job.created_at;
                let interactive = metrics::is_interactive(job.kind, job.priority);

                // The job runs to completion even if shutdown starts: dropping
                // it here is what the drain exists to avoid.
                match handler.handle(job).await {
                    Ok(()) => {
                        let _ = queue.complete(id).await;
                        metrics.record(metrics::Outcome::Succeeded);
                        // Published here rather than in each handler: this is
                        // the one place that sees every transition, so a new
                        // job kind is live without its author remembering to
                        // emit anything.
                        events.publish(JobEvent::JobState {
                            job_id: id,
                            state: JobState::Succeeded,
                        });

                        if interactive
                            && let Ok(elapsed) = (chrono::Utc::now() - created_at).to_std()
                        {
                            metrics.record_time_to_first_page(elapsed);
                        }
                    }
                    Err(e) => {
                        let retryable = e.is_retryable();
                        // Counted only once the job is out of attempts. A
                        // retry that later succeeds is not a failed outcome,
                        // and counting each attempt would make the rate
                        // depend on how many retries are configured.
                        match queue.fail(id, &e.to_string(), retryable).await {
                            // A retry is not an outcome, and announcing one as
                            // `Failed` would make a UI show a job as failed
                            // that is about to run again.
                            Ok(Some(_next_run_at)) => events.publish(JobEvent::JobState {
                                job_id: id,
                                state: JobState::Queued,
                            }),
                            _ => {
                                metrics.record(metrics::classify(&e));
                                events.publish(JobEvent::JobState {
                                    job_id: id,
                                    state: JobState::Failed,
                                });
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                // Jittered, so an idle pool does not wake in lockstep and hit
                // the database with a synchronised burst of claim queries.
                let jitter = Duration::from_millis((index as u64 * 37) % 250);
                tokio::select! {
                    _ = tokio::time::sleep(poll + jitter) => {}
                    _ = cancel.cancelled() => return,
                }
            }
            Err(_) => {
                // A database blip must not spin the worker.
                tokio::select! {
                    _ = tokio::time::sleep(poll) => {}
                    _ = cancel.cancelled() => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_bounded() {
        let c = WorkerConfig::default();
        assert!(
            c.workers > 0,
            "a pool with no workers would silently do nothing"
        );
        assert!(c.drain_timeout > Duration::ZERO, "shutdown must be bounded");
    }
}
