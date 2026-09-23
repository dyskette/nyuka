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
pub mod rate_limit;
pub mod routes;
pub mod session_store;
pub mod sse;
pub mod state;
pub mod static_files;
pub mod telemetry;
pub mod tracing_layer;

use std::sync::Arc;

use crate::rate_limit::TrustedIpKeyExtractor;
use anyhow::Context;
use axum::routing::{get, post};
use axum::{Json, Router};
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
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

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
    let (oidc, local_user) = if config.auth.mode.is_disabled() {
        warn_authentication_is_off(&config);

        // One row, created once. Every request is this user, so `/me`, the
        // session's `user_id` and the telemetry `user.id` field all keep
        // their shapes — there is no anonymous branch for a handler to get
        // wrong. Switching to OIDC later leaves it behind as an inert row.
        let user = repositories
            .record_sign_in(LOCAL_ISSUER, LOCAL_SUBJECT)
            .await
            .context("seeding the local user")?;
        (None, Some(user.id))
    } else {
        let client = crate::auth::OidcClient::discover(&config.auth)
            .await
            .context("discovering the OIDC provider")?;
        (Some(Arc::new(client)), None)
    };

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
                limiter.clone(),
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
    // Shared by the pool that records outcomes and the scheduler that
    // reports them (ADR-0019).
    let metrics = Arc::new(nyuka_jobs::metrics::JobMetrics::new());

    let pool = WorkerPool::start(
        queue.clone(),
        Arc::new(handlers),
        metrics.clone(),
        // The same bus the SSE endpoint subscribes to, so a job finishing
        // reaches an open browser (ADR-0010).
        events.clone(),
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
            metrics,
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
        local_user,
        limiter,
        sessions,
        oidc,
        event_tx,
    });

    // Step 6.
    // `into_make_service_with_connect_info` rather than a bare router: the
    // rate limiter keys on the peer address, and without `ConnectInfo` in the
    // extensions it cannot extract one.
    let app = router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
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

/// Every route that appears in the OpenAPI document.
///
/// One list, two consumers: `router` mounts it, and `openapi_document` reads
/// the schema out of it. A second list would be a second thing to keep in
/// step, which is the drift the generated client exists downstream of.
///
/// `OpenApiRouter` rather than `Router`, so registering a handler and
/// describing it are the same call.
fn documented_routes() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(openapi::ApiDoc::openapi())
        .routes(utoipa_axum::routes!(routes::library::list))
        .routes(utoipa_axum::routes!(routes::library::get))
        .routes(utoipa_axum::routes!(routes::library::chapters))
        .routes(utoipa_axum::routes!(routes::library::chapter))
        .routes(utoipa_axum::routes!(
            routes::follows::list,
            routes::follows::upsert
        ))
        .routes(utoipa_axum::routes!(
            routes::follows::get,
            routes::follows::delete
        ))
        .routes(utoipa_axum::routes!(routes::follows::check_now))
        .routes(utoipa_axum::routes!(
            routes::jobs::list,
            routes::jobs::trigger
        ))
        .routes(utoipa_axum::routes!(routes::jobs::get))
        .routes(utoipa_axum::routes!(routes::jobs::cancel))
        .routes(utoipa_axum::routes!(routes::jobs::retry))
        .routes(utoipa_axum::routes!(
            routes::sources::list_repos,
            routes::sources::add_repo
        ))
        .routes(utoipa_axum::routes!(routes::sources::delete_repo))
        .routes(utoipa_axum::routes!(routes::sources::refresh_repo))
        .routes(utoipa_axum::routes!(routes::sources::available))
        // Both live on `/sources`, so they belong in one `routes!` call.
        .routes(utoipa_axum::routes!(
            routes::sources::list,
            routes::sources::install
        ))
        // `POST /manga` sits with the catalog rather than with the library
        // routes because it is how a catalog entry becomes a library entry.
        .routes(utoipa_axum::routes!(routes::catalog::add))
        .routes(utoipa_axum::routes!(routes::sources::uninstall))
        .routes(utoipa_axum::routes!(routes::sources::filters))
        .routes(utoipa_axum::routes!(
            routes::sources::settings,
            routes::sources::put_setting
        ))
        .routes(utoipa_axum::routes!(routes::catalog::browse))
        .routes(utoipa_axum::routes!(routes::catalog::details))
        .routes(utoipa_axum::routes!(routes::catalog::chapters))
        .routes(utoipa_axum::routes!(routes::downloads::request))
        .routes(utoipa_axum::routes!(routes::downloads::file))
}

/// The OpenAPI document, built without needing application state.
///
/// `cargo xtask openapi` writes this to `web/openapi.json`, and CI fails when
/// the committed copy is stale. Building it from `documented_routes` rather
/// than from a hand-maintained list is what makes that check mean something.
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    documented_routes().split_for_parts().1
}

/// The identity the seeded local user is recorded under.
///
/// `(issuer, subject)` is the natural key, so these constants are what make
/// the seed idempotent across restarts and what keeps it from ever colliding
/// with a real provider's subject.
pub const LOCAL_ISSUER: &str = "local";
pub const LOCAL_SUBJECT: &str = "local";

/// Says out loud what `AUTH_MODE=none` means for this deployment.
///
/// Two messages rather than one: a loopback bind is a reasonable barebones
/// setup and gets a short note, while a non-loopback bind is the case where
/// the consequence is worth spelling out. Installing a source makes this
/// server fetch and execute third-party WebAssembly, so an open instance is
/// not only a readable library.
fn warn_authentication_is_off(config: &Config) {
    if config.bind_addr.ip().is_loopback() {
        tracing::warn!(
            bind = %config.bind_addr,
            "authentication is disabled (AUTH_MODE=none); the server is bound to \
             loopback, so only this host can reach it"
        );
        return;
    }

    tracing::warn!(
        bind = %config.bind_addr,
        "authentication is disabled (AUTH_MODE=none) and this server is listening \
         on a non-loopback address. Anyone who can reach this port can browse the \
         library, queue downloads, and install sources — which makes this server \
         fetch and execute third-party WebAssembly. Put it behind a VPN, a \
         firewall, or an authenticating proxy, or set AUTH_MODE=oidc."
    );
}

/// The response compression layer.
///
/// A function rather than an inline `CompressionLayer::new()` so the test that
/// guards it exercises the configuration this router actually uses. Written
/// inline, a later change here would not be covered by anything.
///
/// `DefaultPredicate` excludes `text/event-stream`, and that exclusion is
/// load-bearing rather than incidental: a compressor accumulates input before
/// emitting, so compressing an SSE response holds events even when every proxy
/// in front is configured correctly. ADR-0010 calls replacing this predicate
/// the failure most likely to be reintroduced, and
/// `sse_responses_are_never_compressed` is what catches it.
pub fn compression_layer() -> CompressionLayer {
    CompressionLayer::new()
}

/// Builds the router.
///
/// Layer order is load-bearing and reads bottom-up in `.layer()` terms: the
/// session layer must be *outside* the CSRF check, or a rejected mutation
/// would still have loaded and saved a session.
pub fn router(state: Arc<AppState>) -> Router {
    // Enforced by a layer as well as in the handler. The layer rejects before
    // the body is buffered, which is the difference between refusing a
    // 50 MB upload and allocating it first; the handler's own check is what
    // reports the limit as problem+json.
    let telemetry_body_limit = state.config.telemetry.ingest_max_body_bytes;

    // Built here rather than passed in: the governor holds per-key state, so
    // one config per router is what makes the limit apply across requests.
    let auth_governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(TrustedIpKeyExtractor::new(
                state.config.trusted_proxies.clone(),
            ))
            .per_second(rate_limit::AUTH_PER_SECOND)
            .burst_size(rate_limit::AUTH_BURST)
            .finish()
            // The builder only returns `None` for a zero period or burst, and
            // both are constants above.
            .expect("the auth rate limit constants are valid"),
    );

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

    // Everything that needs a signed-in user. The auth endpoints are
    // deliberately not here: a login route behind a login check cannot let
    // anyone in.
    //
    // `OpenApiRouter` rather than `Router`, so registering a handler and
    // describing it are the same call — the drift this guards against is a
    // schema that says one thing while the server does another.
    let (protected, api_doc) = documented_routes().split_for_parts();

    let protected = protected
        // SSE is not in the OpenAPI document: it is not a request/response
        // pair, and a generated client method for it would be misleading.
        .route("/events", get(sse::events))
        // Nor is telemetry ingest: it speaks OTLP, and a generated client
        // method for it would describe a shape the browser SDK already owns.
        .route(
            "/telemetry",
            post(routes::telemetry::ingest).layer(RequestBodyLimitLayer::new(telemetry_body_limit)),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));

    // Rate-limited as a group. The callback is included deliberately: it
    // performs a token exchange against the identity provider, so leaving it
    // open is a way to make this server hammer someone else's (ADR-0005
    // follow-up 6).
    let auth_routes = Router::new()
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        .layer(GovernorLayer::new(auth_governor));

    let api = Router::new()
        .merge(auth_routes)
        .route("/me", get(auth::me))
        .route(
            "/openapi.json",
            get(move || {
                let doc = api_doc.clone();
                async move { Json(doc) }
            }),
        )
        .merge(protected)
        // Applied to `/api/v1` only. The health probes below are outside it,
        // and an orchestrator's probe cannot set a header.
        .layer(axum::middleware::from_fn(csrf::require_csrf_header));

    Router::new()
        .nest(
            "/api/v1",
            // The API's own fallback, so an unknown endpoint answers
            // problem+json rather than falling through to the SPA shell. This
            // is the sharp edge ADR-0006 names: without it a mistyped fetch
            // returns `index.html` with 200, and the client reports an HTML
            // parse error instead of a missing endpoint.
            //
            // The bare prefix needs its own route: `/api/v1/` reduces to an
            // inner path of `/`, which the nested fallback does not catch, so
            // without this it falls through to the SPA shell — the exact
            // failure the fallback exists to prevent.
            api.route("/", get(static_files::api_not_found_bare))
                .fallback(static_files::api_not_found_bare),
        )
        // Deliberately outside `/api/v1`: a probe is not a versioned API, and
        // an orchestrator should not have to track the API version to know
        // whether the process is alive.
        .route("/healthz", get(routes::health::healthz))
        .route("/readyz", get(routes::health::readyz))
        .layer(session_layer)
        .layer(compression_layer())
        // Outside compression and sessions, so the span covers the whole
        // request including whatever those layers do. `client.address` is
        // recorded from inside it.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            tracing_layer::record_span_fields,
        ))
        .layer(tracing_layer::layer())
        // Last, so it only sees what nothing above matched. A deep link like
        // `/library/123` is a client-side route, not a missing resource, and
        // gets the shell with 200.
        .fallback(static_files::serve)
        .with_state(state)
}
