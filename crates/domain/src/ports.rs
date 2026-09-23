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

/// Fetching a page image.
///
/// Separate from `SourceItem` because the two answer different questions.
/// `SourceItem::pages` asks the source's WASM module *where* the images are;
/// this fetches them, over the same egress policy the module's own requests
/// go through. Keeping it a port is what lets the download handler be tested
/// without a network — and what stops it from reaching for an HTTP client
/// directly and quietly bypassing that policy.
#[async_trait]
pub trait PageFetcher: Send + Sync {
    /// Fetches one page image.
    ///
    /// Implementations honour `PageRef::headers`: a source that requires a
    /// referer gets a 403 without it, which looks like a dead link.
    async fn fetch(&self, page: &PageRef) -> Result<Vec<u8>>;
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

    /// The package's settings declaration, verbatim.
    ///
    /// Passed through rather than interpreted: it describes a form shape the
    /// package owns, and this server has no schema for it. A client renders
    /// it; the values live in the key-value namespace.
    async fn settings_declaration(&self, source: SourceId) -> Result<serde_json::Value>;
}

// ---------------------------------------------------------------------------
// Storage port (ADR-0007)
// ---------------------------------------------------------------------------

/// Where installed source packages are kept between restarts.
///
/// A source is a WASM module compiled at install time and held in memory. Its
/// package was previously downloaded, compiled, and discarded — so after a
/// restart every installed source was listed and unusable, with no way to
/// recover but reinstalling from the network.
///
/// Implementations write under the server's own data directory, never under
/// the library: the library is what an operator backs up and syncs, and
/// third-party executable code has no business travelling with it.
pub trait PackageStorage: Send + Sync {
    fn write(&self, source: SourceId, bytes: &[u8]) -> Result<()>;

    /// `Ok(None)` when there is no package, which is not an error: a source
    /// installed before packages were kept has a row and no file, and that has
    /// to read as "reinstall this" rather than as a failure to start.
    fn read(&self, source: SourceId) -> Result<Option<Vec<u8>>>;

    /// Succeeds when there was nothing to remove, so uninstall works after a
    /// restart that never loaded the source.
    fn remove(&self, source: SourceId) -> Result<()>;
}

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

/// An open handle to a packaged chapter, so the download endpoint can serve
/// `Range` requests without reading the whole archive into memory.
///
/// There is deliberately no `checksum` here. The checksum is computed once
/// during the write and stored on the `DownloadedChapter` row, which is what
/// the `ETag` comes from; asking an open handle for it would either recompute
/// it on every range request or force the implementation to invent one.
#[async_trait]
pub trait ChapterRead: Send + Sync {
    fn size(&self) -> u64;

    /// Reads up to `len` bytes starting at `offset`.
    ///
    /// Returns fewer bytes at end of file rather than failing, so a caller
    /// clamping a `Range` header does not have to be exact.
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>>;
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
    ///
    /// `priority` orders the claim query ascending, so a lower number runs
    /// first. It is on the port rather than fixed by the implementation
    /// because it is a real decision a caller makes: a download a user is
    /// waiting on must outrank one a follow check discovered.
    async fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        run_at: Option<chrono::DateTime<chrono::Utc>>,
        idempotency_key: Option<&str>,
        priority: i16,
        max_attempts: i32,
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

    /// The same job, with its subject resolved.
    ///
    /// Separate from `get` for the reason `list_summaries` is separate from
    /// `list` (ADR-0020): it joins two more tables, and a caller that only
    /// needs the row should not pay for that.
    async fn get_summary(&self, job: JobId) -> Result<JobSummary>;

    async fn list(&self, state: Option<JobState>, cursor: Option<&Cursor>) -> Result<Page<Job>>;

    /// The same list, with each download job's subject resolved.
    ///
    /// A queue of uuids answers nothing an operator asks of it, so the list
    /// view uses this and the plain `list` stays for callers that only need
    /// the rows.
    async fn list_summaries(
        &self,
        state: Option<JobState>,
        cursor: Option<&Cursor>,
    ) -> Result<Page<JobSummary>>;
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

    /// The library list, with its aggregates, ordered and filtered.
    ///
    /// A separate method rather than a flag on `list`, because the two have
    /// different costs: this one joins the source and counts chapters, and a
    /// caller that only needs the series should not pay for that by default.
    ///
    /// The ordering is part of the request because the cursor is only
    /// meaningful under it — a cursor taken while sorted by title does not
    /// describe a position in a list sorted by recency.
    async fn list_summaries(
        &self,
        query: &MangaQuery,
        cursor: Option<&Cursor>,
    ) -> Result<Page<MangaSummary>>;
}

#[async_trait]
pub trait ChapterRepository: Send + Sync {
    async fn get(&self, id: ChapterId) -> Result<Chapter>;
    async fn list_for_manga(
        &self,
        manga: MangaId,
        cursor: Option<&Cursor>,
    ) -> Result<Page<Chapter>>;

    /// The same list, with each chapter's download state.
    ///
    /// Separate from `list_for_manga` for the reason `list_summaries` is
    /// separate from `list`: it joins a second table, and a caller that only
    /// needs the chapters should not pay for that.
    async fn list_summaries_for_manga(
        &self,
        manga: MangaId,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ChapterSummary>>;
    /// Inserts or updates chapters for a series.
    ///
    /// Takes source-shaped chapters, which carry no local id: one is assigned
    /// on insert.
    ///
    /// Returns **only the chapters that were newly inserted**. That set is what
    /// drives `chapter.new` events and the follows badge, so returning
    /// everything would notify on every refresh and returning nothing would
    /// notify never.
    async fn upsert_many(
        &self,
        manga: MangaId,
        chapters: &[SourceChapter],
    ) -> Result<Vec<ChapterId>>;
    async fn downloaded(&self, id: ChapterId) -> Result<Option<DownloadedChapter>>;
    async fn record_download(&self, download: &DownloadedChapter) -> Result<()>;

    /// Drops the download records for chapters whose files are gone.
    ///
    /// This is what `reconcile_library` does when it finds a missing file.
    /// Deleting the file would be a no-op — it is already absent — and the
    /// row is the thing making the UI offer a read that will fail.
    ///
    /// Takes a slice because a restored-from-empty library means every
    /// chapter at once, and one statement per chapter would be thousands of
    /// round trips.
    async fn forget_downloads(&self, missing: &[ChapterId]) -> Result<u64>;
}

/// Signed-in people, for identity and audit (ADR-0005).
#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn get(&self, id: UserId) -> Result<User>;

    /// Records a sign-in, creating the user on first sight.
    ///
    /// Keyed on `(issuer, subject)`: a subject is only unique within its
    /// issuer, so an operator who changes IdP gets new users rather than
    /// silently handing an existing account to whoever holds the same subject
    /// at the new provider.
    async fn record_sign_in(&self, issuer: &str, subject: &str) -> Result<User>;
}

/// Follows.
#[async_trait]
pub trait FollowRepository: Send + Sync {
    async fn get(&self, id: FollowId) -> Result<Follow>;
    async fn list(&self, cursor: Option<&Cursor>) -> Result<Page<Follow>>;

    /// The same list, with the series each follow watches.
    async fn list_summaries(&self, cursor: Option<&Cursor>) -> Result<Page<FollowSummary>>;

    /// Follows whose check interval has elapsed, oldest first.
    ///
    /// Bounded by `limit`: a library with thousands of follows must not turn
    /// one scheduler tick into thousands of enqueues.
    async fn due(&self, limit: u64) -> Result<Vec<Follow>>;

    async fn upsert(&self, follow: &Follow) -> Result<FollowId>;
    async fn delete(&self, id: FollowId) -> Result<()>;

    /// Stamps `last_checked_at`. Called by the `check_follow` handler after a
    /// successful check, which is what makes a failed check retry on the next
    /// tick instead of waiting out the interval.
    async fn mark_checked(&self, id: FollowId) -> Result<()>;
}

#[async_trait]
pub trait SourceRepository: Send + Sync {
    async fn get(&self, id: SourceId) -> Result<InstalledSource>;
    async fn list(&self) -> Result<Vec<InstalledSource>>;
    async fn upsert(&self, source: &InstalledSource) -> Result<SourceId>;
    async fn remove(&self, id: SourceId) -> Result<()>;

    /// The configured repositories, which is what `update_sources` walks.
    async fn list_repos(&self) -> Result<Vec<SourceRepo>>;

    /// What a repository offers, for the browse-and-install view.
    async fn list_entries(&self, repo: SourceRepoId) -> Result<Vec<SourceEntry>>;

    async fn get_entry(&self, repo: SourceRepoId, external: &ExternalKey) -> Result<SourceEntry>;

    /// Replaces a repository's entries wholesale.
    ///
    /// Wholesale, not merged: a source dropped upstream must disappear rather
    /// than linger as an entry whose download URL now 404s. One transaction,
    /// so a failed refresh cannot leave the catalog half-replaced.
    async fn replace_entries(&self, repo: SourceRepoId, entries: &[SourceEntry]) -> Result<u64>;
    async fn get_repo(&self, id: SourceRepoId) -> Result<SourceRepo>;
    async fn upsert_repo(&self, repo: &SourceRepo) -> Result<SourceRepoId>;
    async fn remove_repo(&self, id: SourceRepoId) -> Result<()>;

    /// Stamps a successful index refresh.
    async fn mark_repo_refreshed(&self, id: SourceRepoId) -> Result<()>;

    /// The per-source key-value namespace the WASM `defaults` host import
    /// reads and writes.
    ///
    /// Opaque bytes, not JSON: the guest postcard-encodes these values and
    /// this layer does not interpret them (ADR-0004). Typing them as JSON
    /// would mean decoding and re-encoding a payload only the guest
    /// understands, and the column is `bytea` either way.
    async fn kv_get(&self, source: SourceId, key: &str) -> Result<Option<Vec<u8>>>;
    async fn kv_set(&self, source: SourceId, key: &str, value: Vec<u8>) -> Result<()>;

    /// Every key a source has stored.
    ///
    /// Needed because the settings endpoint reports what is set, and a source
    /// writes keys of its own choosing — the declaration lists what it offers,
    /// not what it has written.
    async fn kv_list(&self, source: SourceId) -> Result<Vec<(String, Vec<u8>)>>;
}
