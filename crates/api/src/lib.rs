//! `nyuka-api` — REST, SSE, and the embedded SPA.
//!
//! # Why this is a library with a thin binary on top
//!
//! Two reasons, both practical rather than stylistic:
//!
//! - An integration test under `tests/` can import a library and cannot import
//!   a binary. Route tests need to build the router and drive it with `tower`'s
//!   `ServiceExt`, which means the router has to live here.
//! - In a binary crate, anything `main` does not yet reach is dead code, and
//!   CI denies warnings. That turns incremental construction into a choice
//!   between blanket `allow(dead_code)` and wiring everything at once. A
//!   library's public items are its API, so the question does not arise.

#![forbid(unsafe_code)]

pub mod auth;
pub mod client_ip;
pub mod config;
pub mod csrf;
pub mod defaults;
pub mod error;
pub mod openapi;
pub mod routes;
pub mod session_store;
pub mod sse;
pub mod state;
pub mod static_files;
pub mod telemetry;

use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::routing::{get, post};
use nyuka_aidoku_runtime::adapter::SourceRuntime;
use nyuka_aidoku_runtime::engine::{Limits, Runtime};
use nyuka_aidoku_runtime::fetcher::{FetcherConfig, VettedFetcher};
use nyuka_aidoku_runtime::imports::net::VettingResolver;
use nyuka_aidoku_runtime::registry::{AidokuRegistry, RegistryConfig};
use nyuka_domain::model::JobKind;
use nyuka_jobs::handlers::Registry as HandlerRegistry;
use nyuka_jobs::handlers::download::{DownloadChapter, DownloadConfig};
use nyuka_jobs::handlers::follow::{CheckFollow, RefreshMetadata};
use nyuka_jobs::handlers::maintenance::{PruneJobs, PruneSessions, ReconcileLibrary};
use nyuka_jobs::handlers::sources::UpdateSources;
use nyuka_jobs::limiter::SourceLimiter;
use nyuka_jobs::queue::PostgresQueue;
use nyuka_jobs::scheduler::{Scheduler, SchedulerConfig};
use nyuka_jobs::worker::{WorkerConfig, WorkerPool, shutdown_signal};
use nyuka_packaging::adapter::LibraryStoreAdapter;
use nyuka_persistence::connect::PoolConfig;
use nyuka_persistence::repository::Repositories;
use nyuka_persistence::session::SessionRepository;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::session_store::PostgresSessionStore;
use crate::state::{AppState, BroadcastBus, EVENT_CAPACITY};
use tower_sessions::cookie::{Key, SameSite};
use tower_sessions::{Expiry, SessionManagerLayer};

pub async fn run(config: Config) -> anyhow::Result<()> {
    // Step 2. Nothing above this line may log.
    let _telemetry = telemetry::init(&config.telemetry).context("installing the subscriber")?;

    tracing::info!(
        service.name = %config.telemetry.service_name,
        service.version = %config.telemetry.service_version,
        bind = %config.bind_addr,
        "starting"
    );

    // Step 3.
    let db = nyuka_persistence::connect::connect_and_migrate(PoolConfig {
        url: config.database_url.expose().to_string(),
        slow_statement_threshold: config.telemetry.db_slow_statement,
        ..PoolConfig::default()
    })
    .await
    .context("connecting to the database and applying migrations")?;

    let repositories = Arc::new(Repositories::new(db.clone()));
    let sessions = SessionRepository::new(db.clone());
    let queue = Arc::new(PostgresQueue::new(db.clone()));

    // Anything left over from a process that was killed rather than drained.
    let recovered = queue
        .recover_stale(config.jobs.stale_lock)
        .await
        .context("recovering stale job locks")?;
    if recovered > 0 {
        tracing::warn!(recovered, "returned stranded jobs to the queue");
    }

    let store = nyuka_packaging::store::LibraryStore::open(&config.library_root)
        .with_context(|| format!("opening the library at {}", config.library_root.display()))?;
    // A crash mid-write leaves a `.part` file nothing will ever claim.
    match store.clean_staging() {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, "cleaned abandoned files from library staging"),
        Err(e) => tracing::warn!(error = %e, "could not clean library staging"),
    }
    let library = Arc::new(LibraryStoreAdapter::new(store));

    // --- sources ---------------------------------------------------------
    let resolver = Arc::new(VettingResolver::new());
    let wasm = Arc::new(
        Runtime::new(Limits {
            max_memory_bytes: config.sources.wasm_max_memory_bytes,
            timeout: config.sources.wasm_timeout,
        })
        // wasmtime's error type is its own, not a `std::error::Error`, so
        // `Context` does not apply to it.
        .map_err(|e| anyhow::anyhow!("building the wasm engine: {e}"))?,
    );
    let defaults = Arc::new(defaults::RepositoryDefaults::new(repositories.clone()));
    let source_runtime = Arc::new(SourceRuntime::new(wasm, defaults));
    let registry = Arc::new(
        AidokuRegistry::new(
            source_runtime.clone(),
            repositories.clone(),
            resolver.clone(),
            RegistryConfig::default(),
        )
        .context("building the source registry")?,
    );
    let fetcher = Arc::new(
        VettedFetcher::new(FetcherConfig::default(), resolver)
            .context("building the page fetcher")?,
    );

    // Registering every installed source's limit before any work starts is
    // what stops the first download of a run from ignoring it.
    let limiter = Arc::new(SourceLimiter::new(config.jobs.source_max_concurrency));
    for source in repositories.list_sources().await.unwrap_or_default() {
        limiter
            .register(source.id, source.declared_rate_limit)
            .await;
    }

    // Discovered at startup, not at first sign-in: an unreachable or
    // misconfigured issuer must fail the boot where an operator is watching
    // (ADR-0005 follow-up 5).
    let oidc = Some(Arc::new(
        crate::auth::OidcClient::discover(&config.auth)
            .await
            .context("discovering the OIDC provider")?,
    ));

    let (event_tx, _) = broadcast::channel(EVENT_CAPACITY);
    let events = Arc::new(BroadcastBus::new(event_tx.clone()));

    // --- jobs ------------------------------------------------------------
    let handlers = HandlerRegistry::new()
        .register(
            JobKind::DownloadChapter,
            Arc::new(DownloadChapter::new(
                repositories.clone(),
                repositories.clone(),
                source_runtime.clone(),
                fetcher,
                library.clone(),
                events.clone(),
                limiter.clone(),
                DownloadConfig::default(),
            )),
        )
        .register(
            JobKind::CheckFollow,
            Arc::new(CheckFollow::new(
                repositories.clone(),
                repositories.clone(),
                repositories.clone(),
                source_runtime.clone(),
                events.clone(),
                limiter.clone(),
                queue.clone(),
            )),
        )
        .register(
            JobKind::RefreshMetadata,
            Arc::new(RefreshMetadata::new(
                repositories.clone(),
                repositories.clone(),
                source_runtime.clone(),
                events.clone(),
                limiter,
            )),
        )
        .register(
            JobKind::UpdateSources,
            Arc::new(UpdateSources::new(repositories.clone(), registry.clone())),
        )
        .register(
            JobKind::PruneJobs,
            Arc::new(PruneJobs::new(queue.clone(), config.jobs.retention)),
        )
        .register(
            JobKind::PruneSessions,
            Arc::new(PruneSessions::new(sessions.clone())),
        )
        .register(
            JobKind::ReconcileLibrary,
            Arc::new(ReconcileLibrary::new(
                repositories.clone(),
                repositories.clone(),
                library.clone(),
            )),
        );

    // Step 5.
    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(handlers),
        WorkerConfig {
            workers: config.jobs.workers,
            poll_interval: config.jobs.poll_interval,
            drain_timeout: config.jobs.drain_timeout,
        },
    );

    let scheduler_cancel = CancellationToken::new();
    let scheduler = tokio::spawn(
        Scheduler::new(
            queue.clone(),
            repositories.clone(),
            SchedulerConfig {
                stale_after: config.jobs.stale_lock,
                ..SchedulerConfig::default()
            },
        )
        .run(scheduler_cancel.clone()),
    );

    let bind_addr = config.bind_addr;
    let state = Arc::new(AppState {
        config,
        db,
        manga: repositories.clone(),
        chapters: repositories.clone(),
        follows: repositories.clone(),
        sources: repositories.clone(),
        registry,
        catalog: source_runtime.clone(),
        items: source_runtime,
        library,
        queue,
        events,
        users: repositories,
        sessions,
        oidc,
        event_tx,
    });

    // Step 6.
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;

    // Order matters on the way down too: stop scheduling before draining, or
    // the scheduler enqueues work the workers will never claim.
    tracing::info!("shutting down");
    scheduler_cancel.cancel();
    let _ = scheduler.await;

    let report = pool.shutdown().await;
    tracing::info!(
        drained = report.drained,
        released = report.released,
        "worker pool stopped"
    );

    Ok(())
}

/// Builds the router.
///
/// Layer order is load-bearing and reads bottom-up in `.layer()` terms: the
/// session layer must be *outside* the CSRF check, or a rejected mutation
/// would still have loaded and saved a session.
pub fn router(state: Arc<AppState>) -> Router {
    let session_layer = SessionManagerLayer::new(PostgresSessionStore::new(state.sessions.clone()))
        // `HttpOnly` so an XSS payload cannot read it; `SameSite=Lax` so a
        // cross-site form does not carry it on a POST; `Path=/` so the cookie
        // reaches both the API and the SPA (ADR-0005).
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_path("/".to_string())
        // Not `Secure` when the bind address is loopback: a `Secure` cookie is
        // dropped by the browser over plain HTTP, which would make local
        // development impossible to sign in to. Production runs behind TLS.
        .with_secure(!state.config.bind_addr.ip().is_loopback())
        .with_expiry(Expiry::OnInactivity(time::Duration::seconds(
            state.config.auth.session_ttl.as_secs() as i64,
        )))
        // Signed, so a forged or tampered cookie is rejected before it costs a
        // database lookup — which also stops the endpoint being used as a
        // session-id oracle. This is what makes `SESSION_KEY` load-bearing,
        // and why changing it logs everyone out.
        .with_signed(Key::from(state.config.auth.session_key.expose().as_bytes()));

    let api = Router::new()
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
        // Applied to `/api/v1` only. The health probes below are outside it,
        // and an orchestrator's probe cannot set a header.
        .layer(axum::middleware::from_fn(csrf::require_csrf_header));

    Router::new()
        .nest("/api/v1", api)
        // Deliberately outside `/api/v1`: a probe is not a versioned API, and
        // an orchestrator should not have to track the API version to know
        // whether the process is alive.
        .route("/healthz", get(routes::health::healthz))
        .route("/readyz", get(routes::health::readyz))
        .layer(session_layer)
        .with_state(state)
}
