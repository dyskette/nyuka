//! A router wired to a real database, for route tests.
//!
//! The source ports are stand-ins: nothing here exercises a WASM source, and
//! constructing a real runtime per test would make these slow for no coverage.

#![allow(dead_code)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use nyuka_api::config::{Config, RawConfig};
use nyuka_api::state::{AppState, BroadcastBus, EVENT_CAPACITY};
use nyuka_domain::model::{
    Capability, Cursor, ExternalKey, InstalledSource, Page, SourceChapter, SourceId, SourceManga,
    SourcePage, SourceRepoId,
};
use nyuka_domain::ports::{SourceCatalog, SourceItem, SourceRegistry};
use nyuka_domain::{DomainError, Result};
use nyuka_packaging::adapter::LibraryStoreAdapter;
use nyuka_persistence::repository::Repositories;
use nyuka_persistence::session::SessionRepository;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use std::net::SocketAddr;
use tower::ServiceExt;

pub struct Harness {
    pub router: axum::Router,
    pub db: DatabaseConnection,
    library: tempfile::TempDir,
}

impl Harness {
    pub fn library_path(&self) -> &std::path::Path {
        self.library.path()
    }

    pub async fn get(&self, path: &str) -> (StatusCode, serde_json::Value) {
        self.send(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    /// The whole response, for tests that assert on headers.
    ///
    /// `ConnectInfo` is inserted because the real service is built with
    /// `into_make_service_with_connect_info`, and the rate limiter keys on the
    /// peer address. A `oneshot` router has no connection behind it, so
    /// without this every rate-limited route answers 500 here while working in
    /// production — the test would be measuring the harness.
    pub async fn raw(&self, mut request: Request<Body>) -> axum::response::Response {
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            std::net::IpAddr::from([127, 0, 0, 1]),
            40000,
        )));
        self.router
            .clone()
            .oneshot(request)
            .await
            .expect("response")
    }

    pub async fn send(&self, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = self.raw(request).await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }
}

/// A database of its own per test, so migration state cannot leak between them.
pub async fn fresh_database(name: &str) -> Option<DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok().or_else(|| {
        eprintln!("skipping: set DATABASE_URL to run api route tests");
        None
    })?;
    let admin = Database::connect(&base).await.expect("connecting");
    for sql in [
        format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        format!("CREATE DATABASE {name}"),
    ] {
        admin
            .execute_raw(Statement::from_string(admin.get_database_backend(), sql))
            .await
            .expect("preparing the test database");
    }
    let mut parsed = url::Url::parse(&base).expect("DATABASE_URL is a url");
    parsed.set_path(name);
    Some(
        Database::connect(parsed.as_str())
            .await
            .expect("connecting"),
    )
}

pub fn config(library_root: &std::path::Path) -> Config {
    raw_config(library_root)
        .validate()
        .expect("valid test config")
}

/// The barebones configuration: a database, a library, and no identity
/// provider.
pub fn no_auth_config(library_root: &std::path::Path) -> Config {
    RawConfig {
        auth_mode: "none".into(),
        ..raw_config(library_root)
    }
    .validate()
    .expect("valid test config")
}

fn raw_config(library_root: &std::path::Path) -> RawConfig {
    RawConfig {
        database_url: "postgres://u:p@localhost:5432/nyuka".into(),
        library_root: library_root.display().to_string(),
        oidc_issuer_url: "https://auth.example.test".into(),
        oidc_client_id: "nyuka".into(),
        oidc_client_secret: "shh".into(),
        oidc_redirect_url: "https://nyuka.example.test/api/v1/auth/callback".into(),
        auth_allowed_subjects: "tester".into(),
        session_key: "0".repeat(64),
        ..RawConfig::default()
    }
}

/// Builds a router over a fresh, **unmigrated** database.
///
/// Unmigrated on purpose: readiness has to be observable before the schema is
/// current, and a test that migrated first could not see that state.
pub async fn harness(name: &str) -> Option<Harness> {
    build(name, false).await
}

/// A router with `AUTH_MODE=none`, as a barebones deployment runs it.
pub async fn no_auth_harness(name: &str) -> Option<Harness> {
    build(name, true).await
}

async fn build(name: &str, no_auth: bool) -> Option<Harness> {
    let db = fresh_database(name).await?;
    let library = tempfile::tempdir().expect("tempdir");
    let repositories = Arc::new(Repositories::new(db.clone()));
    let store = nyuka_packaging::store::LibraryStore::open(library.path()).expect("library opens");
    let (event_tx, _) = tokio::sync::broadcast::channel(EVENT_CAPACITY);

    // Seeded the way `main` does, so the id in the router is a real row and
    // `/me` can look it up.
    //
    // This variant migrates first, unlike the default one. `main` always
    // migrates before seeding, and a router built with authentication off
    // has no reason to be observed against an empty schema — the case the
    // default harness leaves unmigrated for is readiness, which does not use
    // this path.
    let local_user = if no_auth {
        use nyuka_persistence::migration::MigratorTrait;
        nyuka_persistence::migration::Migrator::up(&db, None)
            .await
            .expect("migrating");
        Some(
            repositories
                .record_sign_in("local", "local")
                .await
                .expect("seeding the local user")
                .id,
        )
    } else {
        None
    };

    let state = Arc::new(AppState {
        config: if no_auth {
            no_auth_config(library.path())
        } else {
            config(library.path())
        },
        db: db.clone(),
        manga: repositories.clone(),
        chapters: repositories.clone(),
        follows: repositories.clone(),
        sources: repositories.clone(),
        registry: Arc::new(NoRegistry),
        catalog: Arc::new(NoSource),
        items: Arc::new(NoSource),
        library: Arc::new(LibraryStoreAdapter::new(store)),
        queue: Arc::new(nyuka_jobs::queue::PostgresQueue::new(db.clone())),
        events: Arc::new(BroadcastBus::new(event_tx.clone())),
        users: repositories,
        local_user,
        limiter: Arc::new(nyuka_jobs::limiter::SourceLimiter::new(4)),
        sessions: SessionRepository::new(db.clone()),
        // No identity provider is reachable from a test, and discovery would
        // have to contact one. The auth routes report that plainly rather than
        // pretending to work.
        oidc: None,
        event_tx,
    });

    Some(Harness {
        router: nyuka_api::router(state),
        db,
        library,
    })
}

// --- stand-ins for the source ports ----------------------------------------

pub struct NoRegistry;

#[async_trait::async_trait]
impl SourceRegistry for NoRegistry {
    async fn refresh_repo(&self, _repo: SourceRepoId) -> Result<()> {
        Err(DomainError::NotFound)
    }
    async fn install(&self, _repo: SourceRepoId, _entry: &ExternalKey) -> Result<InstalledSource> {
        Err(DomainError::NotFound)
    }
    async fn uninstall(&self, _source: SourceId) -> Result<()> {
        Ok(())
    }
    fn supported_capabilities(&self) -> &[Capability] {
        &[]
    }
    async fn settings_declaration(&self, _source: SourceId) -> Result<serde_json::Value> {
        Err(DomainError::NotFound)
    }
}

pub struct NoSource;

#[async_trait::async_trait]
impl SourceCatalog for NoSource {
    async fn list(
        &self,
        _source: SourceId,
        _query: Option<&str>,
        _filters: &serde_json::Value,
        _cursor: Option<&Cursor>,
    ) -> Result<Page<SourceManga>> {
        Err(DomainError::NotFound)
    }
    async fn filters(&self, _source: SourceId) -> Result<serde_json::Value> {
        Err(DomainError::NotFound)
    }
}

#[async_trait::async_trait]
impl SourceItem for NoSource {
    async fn details(&self, _source: SourceId, _key: &ExternalKey) -> Result<SourceManga> {
        Err(DomainError::NotFound)
    }
    async fn chapters(&self, _source: SourceId, _key: &ExternalKey) -> Result<Vec<SourceChapter>> {
        Ok(vec![])
    }
    async fn pages(
        &self,
        _source: SourceId,
        _manga: &ExternalKey,
        _chapter: &ExternalKey,
    ) -> Result<Vec<SourcePage>> {
        Ok(vec![])
    }
}
