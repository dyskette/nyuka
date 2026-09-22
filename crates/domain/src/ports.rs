//! Ports: the traits adapters implement.
//!
//! Interface before implementation. Each port is owned by `domain` and
//! implemented outward — `nyuka-persistence`, `nyuka-jobs`,
//! `nyuka-aidoku-runtime`, `nyuka-packaging` — and wired together in
//! `nyuka-api`'s `main.rs`.
//!
//! All futures are `Send`: the runtime is multi-threaded and these are awaited
//! from both Axum handlers and job workers (ADR-0001, ADR-0003).

use async_trait::async_trait;

use crate::Result;
use crate::model::*;

// ---------------------------------------------------------------------------
// Provider ports (ADR-0004)
// ---------------------------------------------------------------------------

/// Browsing and searching a source's catalog.
#[async_trait]
pub trait SourceCatalog: Send + Sync {
    async fn list(
        &self,
        source: SourceId,
        query: Option<&str>,
        filters: &serde_json::Value,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SourceManga>>;

    /// The filter schema the source declares, rendered by the frontend's
    /// `DynamicForm`.
    async fn filters(&self, source: SourceId) -> Result<serde_json::Value>;
}

/// Reading one item's details, chapters, and pages.
#[async_trait]
pub trait SourceItem: Send + Sync {
    async fn details(&self, source: SourceId, key: &ExternalKey) -> Result<SourceManga>;

    async fn chapters(&self, source: SourceId, key: &ExternalKey) -> Result<Vec<SourceChapter>>;

    /// Pages for a chapter.
    ///
    /// Takes both keys because a source needs the series to resolve a chapter;
    /// `get_page_list` is given a `Manga` and a `Chapter`, not a chapter alone.
    async fn pages(
        &self,
        source: SourceId,
        manga: &ExternalKey,
        chapter: &ExternalKey,
    ) -> Result<Vec<SourcePage>>;
}

/// Installing, updating, and removing sources.
#[async_trait]
pub trait SourceRegistry: Send + Sync {
    /// Refreshes a repository's index.
    async fn refresh_repo(&self, repo: SourceRepoId) -> Result<()>;

    /// Installs a source from a repository entry.
    ///
    /// Must read the package's required imports and return
    /// [`DomainError::UnsupportedCapability`] when this host does not provide
    /// one, naming the capability. A source that installs and then fails
    /// mid-download is the worst outcome, because it looks like a site problem
    /// rather than a host gap (ADR-0004).
    ///
    /// [`DomainError::UnsupportedCapability`]: crate::DomainError::UnsupportedCapability
    async fn install(&self, repo: SourceRepoId, entry: &ExternalKey) -> Result<InstalledSource>;

    async fn uninstall(&self, source: SourceId) -> Result<()>;

    /// Capabilities this host build actually implements. Compared against a
    /// package's requirements at install time.
    fn supported_capabilities(&self) -> &[Capability];
}

// ---------------------------------------------------------------------------
// Storage port (ADR-0007)
// ---------------------------------------------------------------------------

/// Writing and reading packaged chapters.
///
/// Never returns an absolute path: callers receive a relative path or a
/// stream, so a second library root stays addable without changing callers.
#[async_trait]
pub trait LibraryStore: Send + Sync {
    /// Packages pages into a CBZ with an embedded `ComicInfo.xml` and places
    /// it atomically.
    ///
    /// Implementations build in a temp directory **inside the library root**,
    /// fsync, then rename. `rename` is only atomic within one filesystem — a
    /// temp dir in `/tmp` silently degrades to a cross-mount copy, and a crash
    /// mid-copy leaves a truncated archive that looks complete.
    async fn write_chapter(
        &self,
        manga: &Manga,
        chapter: &Chapter,
        pages: Vec<(PageRef, Vec<u8>)>,
    ) -> Result<DownloadedChapter>;

    async fn open_chapter(&self, path: &str) -> Result<Box<dyn ChapterRead>>;

    async fn delete_chapter(&self, path: &str) -> Result<()>;

    async fn exists(&self, path: &str) -> Result<bool>;

    /// Backs `disk.library_free_bytes` in the metrics snapshot and the
    /// library-writable check in `/readyz`.
    async fn free_bytes(&self) -> Result<u64>;
}

/// A readable, seekable handle to a packaged chapter, so the download
/// endpoint can serve `Range` requests.
pub trait ChapterRead: Send + Sync {
    fn size(&self) -> u64;
    fn checksum(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Queue port (ADR-0003)
// ---------------------------------------------------------------------------

/// The job queue.
#[async_trait]
pub trait JobQueue: Send + Sync {
    /// Enqueues within the caller's transaction.
    ///
    /// `idempotency_key` is unique: a retried `POST /downloads` with the same
    /// key must not enqueue twice. This is transactional with the domain
    /// writes that caused it, which is the whole reason the queue lives in
    /// Postgres rather than Redis.
    async fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        run_at: Option<chrono::DateTime<chrono::Utc>>,
        idempotency_key: Option<&str>,
    ) -> Result<JobId>;

    /// Claims one runnable job with `FOR UPDATE SKIP LOCKED`.
    ///
    /// Must be exactly-once across concurrent workers. This is the least
    /// type-checked code in the system and the most expensive to get wrong —
    /// ADR-0003 requires a concurrency test asserting each row is claimed once.
    async fn claim(&self, worker: &str) -> Result<Option<Job>>;

    async fn complete(&self, job: JobId) -> Result<()>;

    /// Records a failure. Returns `Some(next_run_at)` when attempts remain,
    /// `None` when the job is exhausted.
    async fn fail(
        &self,
        job: JobId,
        error: &str,
        retryable: bool,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>>;

    async fn cancel(&self, job: JobId) -> Result<()>;

    /// Returns jobs stuck in `Running` with a stale lock to `Queued`. Called
    /// on startup and periodically.
    async fn recover_stale(&self, older_than_secs: u64) -> Result<u64>;

    async fn get(&self, job: JobId) -> Result<Job>;

    async fn list(&self, state: Option<JobState>, cursor: Option<&Cursor>) -> Result<Page<Job>>;
}

/// Publishes job events to subscribers. Backed by `tokio::sync::broadcast`
/// in-process; the SSE handler is the only subscriber (ADR-0010).
pub trait EventBus: Send + Sync {
    fn publish(&self, event: JobEvent);
}

// ---------------------------------------------------------------------------
// Repository ports (ADR-0002)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait MangaRepository: Send + Sync {
    async fn get(&self, id: MangaId) -> Result<Manga>;
    async fn find_by_external(&self, source: SourceId, key: &ExternalKey) -> Result<Option<Manga>>;
    async fn upsert(&self, manga: &Manga) -> Result<MangaId>;
    async fn list(&self, cursor: Option<&Cursor>) -> Result<Page<Manga>>;
}

#[async_trait]
pub trait ChapterRepository: Send + Sync {
    async fn get(&self, id: ChapterId) -> Result<Chapter>;
    async fn list_for_manga(
        &self,
        manga: MangaId,
        cursor: Option<&Cursor>,
    ) -> Result<Page<Chapter>>;
    /// Returns the chapters that were newly inserted, which is what drives
    /// `chapter.new` events and the follows badge.
    async fn upsert_many(&self, manga: MangaId, chapters: &[Chapter]) -> Result<Vec<ChapterId>>;
    async fn downloaded(&self, id: ChapterId) -> Result<Option<DownloadedChapter>>;
    async fn record_download(&self, download: &DownloadedChapter) -> Result<()>;
}

#[async_trait]
pub trait SourceRepository: Send + Sync {
    async fn get(&self, id: SourceId) -> Result<InstalledSource>;
    async fn list(&self) -> Result<Vec<InstalledSource>>;
    /// The per-source key-value namespace the WASM `defaults` host import
    /// reads and writes.
    async fn kv_get(&self, source: SourceId, key: &str) -> Result<Option<serde_json::Value>>;
    async fn kv_set(&self, source: SourceId, key: &str, value: serde_json::Value) -> Result<()>;
}
