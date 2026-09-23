//! The `download_chapter` handler: fetch a chapter's pages and package them.
//!
//! # Why fetching and packaging are one job
//!
//! `JobKind` names both `download_chapter` and `package_chapter`, but
//! `LibraryStore::write_chapter` takes page bytes in memory and there is no
//! intermediate store for a separate packaging step to read from. Splitting
//! them would mean inventing a staging format for raw pages, which is a second
//! on-disk contract to keep compatible for no gain — ADR-0007 already makes
//! placement atomic, so a chapter is either fully in the library or not there
//! at all. See the note in `lib.rs` about `package_chapter` having no handler.
//!
//! # Memory is bounded on purpose
//!
//! A chapter is held in memory until the archive is written, so a long chapter
//! of large pages is a large allocation. `DownloadConfig` caps the page count
//! and the total bytes, and the fetcher caps each page. Without those a source
//! decides how much memory this process uses.
//!
//! # Cancellation needs no cleanup
//!
//! A cancelled or killed download leaves at most a `.part` file in the
//! library's staging directory, because the archive is built there and renamed
//! into place in one step. `LibraryStore::clean_staging` at startup is the
//! whole recovery story; there is no partial chapter to find and no resume
//! state to reconcile.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use nyuka_domain::model::{Chapter, Job, JobEvent, Manga, PageContent, PageRef, SourcePage};
use nyuka_domain::ports::{
    ChapterRepository, EventBus, LibraryStore, MangaRepository, PageFetcher, SourceItem,
};
use nyuka_domain::{DomainError, Result};
use uuid::Uuid;

use super::{KindHandler, payload_field};
use crate::limiter::SourceLimiter;

#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// In-flight page fetches for one chapter. The per-source limiter is the
    /// real rate control; this bounds how much of one chapter is in memory
    /// while waiting.
    pub page_concurrency: usize,
    pub max_pages: usize,
    pub max_chapter_bytes: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            page_concurrency: 4,
            max_pages: 500,
            max_chapter_bytes: 512 * 1024 * 1024,
        }
    }
}

pub struct DownloadChapter {
    manga: Arc<dyn MangaRepository>,
    chapters: Arc<dyn ChapterRepository>,
    source: Arc<dyn SourceItem>,
    fetcher: Arc<dyn PageFetcher>,
    library: Arc<dyn LibraryStore>,
    events: Arc<dyn EventBus>,
    limiter: Arc<SourceLimiter>,
    config: DownloadConfig,
}

#[allow(clippy::too_many_arguments)]
impl DownloadChapter {
    pub fn new(
        manga: Arc<dyn MangaRepository>,
        chapters: Arc<dyn ChapterRepository>,
        source: Arc<dyn SourceItem>,
        fetcher: Arc<dyn PageFetcher>,
        library: Arc<dyn LibraryStore>,
        events: Arc<dyn EventBus>,
        limiter: Arc<SourceLimiter>,
        config: DownloadConfig,
    ) -> Self {
        Self {
            manga,
            chapters,
            source,
            fetcher,
            library,
            events,
            limiter,
            config,
        }
    }

    /// Whether this chapter is already in the library.
    ///
    /// Checked against the filesystem, not just the row: a re-enqueued job
    /// must not refetch a chapter that is already there, and a row whose file
    /// has gone must not be treated as a download.
    async fn already_downloaded(&self, chapter: &Chapter) -> Result<bool> {
        let Some(existing) = self.chapters.downloaded(chapter.id).await? else {
            return Ok(false);
        };
        self.library.exists(&existing.relative_path).await
    }

    /// Turns the source's page list into fetchable references.
    ///
    /// Text and zip pages are refused by name rather than skipped. Skipping
    /// them would package a chapter that is missing its content and record it
    /// as a complete download.
    fn to_refs(&self, pages: Vec<SourcePage>) -> Result<Vec<PageRef>> {
        if pages.len() > self.config.max_pages {
            return Err(DomainError::Source {
                message: format!(
                    "chapter lists {} pages, over the {} page limit",
                    pages.len(),
                    self.config.max_pages
                ),
                retryable: false,
            });
        }

        pages
            .into_iter()
            .map(|page| match page.content {
                PageContent::Url { url, headers } => Ok(PageRef {
                    index: page.index,
                    url,
                    headers,
                }),
                PageContent::Text(_) => Err(DomainError::UnsupportedCapability(
                    "text chapters are not packaged yet".into(),
                )),
                PageContent::Zip { .. } => Err(DomainError::UnsupportedCapability(
                    "zip-backed pages are not packaged yet".into(),
                )),
            })
            .collect()
    }

    async fn fetch_all(
        &self,
        job: &Job,
        source_id: nyuka_domain::model::SourceId,
        refs: Vec<PageRef>,
    ) -> Result<Vec<(PageRef, Vec<u8>)>> {
        let total = refs.len() as u32;
        let done = AtomicU64::new(0);
        let bytes = AtomicU64::new(0);

        let mut stream = futures::stream::iter(refs.into_iter().map(|page| {
            let fetcher = self.fetcher.clone();
            let limiter = self.limiter.clone();
            let done = &done;
            let bytes = &bytes;
            async move {
                // One permit per request, not one per chapter: holding a
                // single permit while fetching pages concurrently would let a
                // source with a limit of one see four requests at once.
                let _permit = limiter.acquire(source_id).await;
                let image = fetcher.fetch(&page).await?;
                done.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(image.len() as u64, Ordering::Relaxed);
                Ok::<_, DomainError>((page, image))
            }
        }))
        .buffer_unordered(self.config.page_concurrency.max(1));

        let mut fetched = Vec::with_capacity(total as usize);
        while let Some(result) = stream.next().await {
            fetched.push(result?);

            let so_far = bytes.load(Ordering::Relaxed);
            if so_far > self.config.max_chapter_bytes {
                return Err(DomainError::Source {
                    message: format!(
                        "chapter exceeded the {} byte limit after {} pages",
                        self.config.max_chapter_bytes,
                        fetched.len()
                    ),
                    retryable: false,
                });
            }

            // Progress is advisory: ADR-0010 requires every state change to be
            // visible through a plain fetch too, so a dropped event costs the
            // UI liveness, never correctness.
            self.events.publish(JobEvent::JobProgress {
                job_id: job.id,
                done: done.load(Ordering::Relaxed) as u32,
                total,
                bytes: so_far,
            });
        }

        Ok(fetched)
    }
}

#[async_trait::async_trait]
impl KindHandler for DownloadChapter {
    async fn handle(&self, job: &Job) -> Result<()> {
        let chapter_id = nyuka_domain::model::ChapterId(payload_field::<Uuid>(job, "chapter_id")?);
        let chapter = self.chapters.get(chapter_id).await?;
        let manga: Manga = self.manga.get(chapter.manga_id).await?;

        if self.already_downloaded(&chapter).await? {
            tracing::debug!(chapter.id = %chapter.id, "chapter is already in the library");
            return Ok(());
        }

        let refs = {
            // The page list is itself a request to the source, so it takes a
            // permit like any other.
            let _permit = self.limiter.acquire(manga.source_id).await;
            let pages = self
                .source
                .pages(manga.source_id, &manga.external_key, &chapter.external_key)
                .await?;
            self.to_refs(pages)?
        };

        if refs.is_empty() {
            return Err(DomainError::Source {
                message: format!("chapter {chapter_id} listed no pages"),
                retryable: true,
            });
        }

        let pages = self.fetch_all(job, manga.source_id, refs).await?;
        let download = self.library.write_chapter(&manga, &chapter, pages).await?;

        // Record only after the archive is in place. The reverse order would
        // leave a row claiming a download that a crash prevented.
        self.chapters.record_download(&download).await?;
        self.events.publish(JobEvent::ChapterDownloaded {
            manga_id: manga.id,
            chapter_id,
        });

        tracing::info!(
            chapter.id = %chapter_id,
            path = %download.relative_path,
            bytes = download.size_bytes,
            "packaged a chapter"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::testing::job;
    use nyuka_domain::model::{
        ChapterId, ChapterSummary, ContentRating, Cursor, DownloadedChapter, ExternalKey, JobKind,
        MangaId, MangaStatus, Page, ReadingDirection, SourceChapter, SourceId, SourceManga,
    };
    use nyuka_domain::ports::ChapterRead;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    const SOURCE: SourceId = SourceId(Uuid::from_u128(1));

    /// A local newtype, because a port cannot be implemented for `Arc<T>` from
    /// another crate. Wrapping lets a test hold the same fake the handler does
    /// and read what it recorded.
    struct Shared<T>(Arc<T>);

    impl<T> Clone for Shared<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<T> std::ops::Deref for Shared<T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }

    fn manga_row(id: MangaId) -> Manga {
        Manga {
            id,
            source_id: SOURCE,
            external_key: ExternalKey("series".into()),
            title: "Test Series".into(),
            authors: vec![],
            artists: vec![],
            description: None,
            tags: vec![],
            cover_url: None,
            url: None,
            language: None,
            status: MangaStatus::Unknown,
            content_rating: ContentRating::Unknown,
            direction: ReadingDirection::RightToLeft,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn chapter_row(id: ChapterId, manga_id: MangaId) -> Chapter {
        Chapter {
            id,
            manga_id,
            external_key: ExternalKey("ch-21".into()),
            title: None,
            number: Some(21.0),
            volume: None,
            language: None,
            published_at: None,
        }
    }

    struct Repos {
        manga: Manga,
        chapter: Chapter,
        downloaded: Mutex<Option<DownloadedChapter>>,
        recorded: Mutex<Vec<DownloadedChapter>>,
    }

    #[async_trait::async_trait]
    impl MangaRepository for Shared<Repos> {
        async fn get(&self, _id: MangaId) -> Result<Manga> {
            Ok(self.manga.clone())
        }
        async fn find_by_external(
            &self,
            _source: SourceId,
            _key: &ExternalKey,
        ) -> Result<Option<Manga>> {
            Ok(None)
        }
        async fn upsert(&self, _manga: &Manga) -> Result<MangaId> {
            Ok(self.manga.id)
        }
        async fn list_summaries(
            &self,
            _cursor: Option<&Cursor>,
        ) -> Result<Page<nyuka_domain::model::MangaSummary>> {
            Ok(Page {
                items: vec![],
                next: None,
            })
        }
        async fn list(&self, _cursor: Option<&Cursor>) -> Result<Page<Manga>> {
            Ok(Page {
                items: vec![self.manga.clone()],
                next: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl ChapterRepository for Shared<Repos> {
        async fn get(&self, _id: ChapterId) -> Result<Chapter> {
            Ok(self.chapter.clone())
        }
        async fn list_for_manga(
            &self,
            _manga: MangaId,
            _cursor: Option<&Cursor>,
        ) -> Result<Page<Chapter>> {
            Ok(Page {
                items: vec![self.chapter.clone()],
                next: None,
            })
        }
        async fn list_summaries_for_manga(
            &self,
            _manga: MangaId,
            _cursor: Option<&Cursor>,
        ) -> Result<Page<ChapterSummary>> {
            unimplemented!("the panel's chapter list is not exercised here")
        }
        async fn upsert_many(
            &self,
            _manga: MangaId,
            _chapters: &[SourceChapter],
        ) -> Result<Vec<ChapterId>> {
            Ok(vec![])
        }
        async fn downloaded(&self, _id: ChapterId) -> Result<Option<DownloadedChapter>> {
            Ok(self.downloaded.lock().expect("lock").clone())
        }
        async fn record_download(&self, download: &DownloadedChapter) -> Result<()> {
            self.recorded.lock().expect("lock").push(download.clone());
            Ok(())
        }
        async fn forget_downloads(&self, _missing: &[ChapterId]) -> Result<u64> {
            Ok(0)
        }
    }

    struct FakeSource(Vec<SourcePage>);

    #[async_trait::async_trait]
    impl SourceItem for FakeSource {
        async fn details(&self, _s: SourceId, _k: &ExternalKey) -> Result<SourceManga> {
            Err(DomainError::NotFound)
        }
        async fn chapters(&self, _s: SourceId, _k: &ExternalKey) -> Result<Vec<SourceChapter>> {
            Ok(vec![])
        }
        async fn pages(
            &self,
            _s: SourceId,
            _m: &ExternalKey,
            _c: &ExternalKey,
        ) -> Result<Vec<SourcePage>> {
            Ok(self.0.clone())
        }
    }

    /// Records how many fetches overlap, which is the only way to see whether
    /// the limiter is actually in the path.
    #[derive(Default)]
    struct CountingFetcher {
        calls: AtomicUsize,
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        bytes_each: usize,
        fail_on: Option<u32>,
    }

    #[async_trait::async_trait]
    impl PageFetcher for Shared<CountingFetcher> {
        async fn fetch(&self, page: &PageRef) -> Result<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            // Long enough that overlapping fetches would be seen overlapping.
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);

            if self.fail_on == Some(page.index) {
                return Err(DomainError::Source {
                    message: "page is gone".into(),
                    retryable: false,
                });
            }
            Ok(vec![0xAB; self.bytes_each.max(1)])
        }
    }

    #[derive(Default)]
    struct FakeLibrary {
        present: Mutex<Vec<String>>,
        written: Mutex<Vec<usize>>,
    }

    #[async_trait::async_trait]
    impl LibraryStore for Shared<FakeLibrary> {
        async fn write_chapter(
            &self,
            _manga: &Manga,
            chapter: &Chapter,
            pages: Vec<(PageRef, Vec<u8>)>,
        ) -> Result<DownloadedChapter> {
            self.written.lock().expect("lock").push(pages.len());
            let path = format!("Test Series/c{}.cbz", chapter.external_key.0);
            self.present.lock().expect("lock").push(path.clone());
            Ok(DownloadedChapter {
                chapter_id: chapter.id,
                relative_path: path,
                size_bytes: pages.iter().map(|(_, b)| b.len() as u64).sum(),
                checksum: "sha".into(),
                packaged_at: chrono::Utc::now(),
            })
        }
        async fn open_chapter(&self, _path: &str) -> Result<Box<dyn ChapterRead>> {
            Err(DomainError::NotFound)
        }
        async fn delete_chapter(&self, _path: &str) -> Result<()> {
            Ok(())
        }
        async fn exists(&self, path: &str) -> Result<bool> {
            Ok(self.present.lock().expect("lock").iter().any(|p| p == path))
        }
        async fn free_bytes(&self) -> Result<u64> {
            Ok(1 << 30)
        }
    }

    #[derive(Default)]
    struct RecordingBus {
        events: Mutex<Vec<JobEvent>>,
    }

    impl EventBus for Shared<RecordingBus> {
        fn publish(&self, event: JobEvent) {
            self.events.lock().expect("lock").push(event);
        }
    }

    fn url_pages(n: u32) -> Vec<SourcePage> {
        (0..n)
            .map(|i| SourcePage {
                index: i,
                content: PageContent::Url {
                    url: format!("https://example.invalid/{i}.jpg"),
                    headers: vec![],
                },
                description: None,
            })
            .collect()
    }

    struct Fixture {
        handler: DownloadChapter,
        repos: Shared<Repos>,
        fetcher: Shared<CountingFetcher>,
        library: Shared<FakeLibrary>,
        bus: Shared<RecordingBus>,
    }

    async fn fixture(
        pages: Vec<SourcePage>,
        fetcher: CountingFetcher,
        permits: u32,
        config: DownloadConfig,
    ) -> Fixture {
        let manga_id = MangaId(Uuid::new_v4());
        let repos = Shared(Arc::new(Repos {
            manga: manga_row(manga_id),
            chapter: chapter_row(ChapterId(Uuid::new_v4()), manga_id),
            downloaded: Mutex::new(None),
            recorded: Mutex::new(vec![]),
        }));
        let fetcher = Shared(Arc::new(fetcher));
        let library = Shared(Arc::new(FakeLibrary::default()));
        let bus = Shared(Arc::new(RecordingBus::default()));
        let limiter = Arc::new(SourceLimiter::new(permits));
        limiter.register(SOURCE, None).await;

        Fixture {
            handler: DownloadChapter::new(
                Arc::new(repos.clone()),
                Arc::new(repos.clone()),
                Arc::new(FakeSource(pages)),
                Arc::new(fetcher.clone()),
                Arc::new(library.clone()),
                Arc::new(bus.clone()),
                limiter,
                config,
            ),
            repos,
            fetcher,
            library,
            bus,
        }
    }

    fn download_job(chapter: ChapterId) -> Job {
        job(
            JobKind::DownloadChapter,
            serde_json::json!({ "chapter_id": chapter.0 }),
        )
    }

    #[tokio::test]
    async fn a_chapter_is_fetched_packaged_and_recorded() {
        let f = fixture(
            url_pages(3),
            CountingFetcher {
                bytes_each: 16,
                ..Default::default()
            },
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        f.handler
            .handle(&download_job(chapter_id))
            .await
            .expect("downloaded");

        assert_eq!(f.fetcher.calls.load(Ordering::SeqCst), 3);
        assert_eq!(*f.library.written.lock().expect("lock"), vec![3]);
        assert_eq!(f.repos.recorded.lock().expect("lock").len(), 1);

        let events = f.bus.events.lock().expect("lock");
        assert!(
            matches!(events.last(), Some(JobEvent::ChapterDownloaded { .. })),
            "the last event must announce the finished chapter"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, JobEvent::JobProgress { .. }))
                .count(),
            3,
            "one progress event per page"
        );
    }

    /// The row alone is not enough: the file has to be there too.
    #[tokio::test]
    async fn a_chapter_already_in_the_library_is_not_refetched() {
        let f = fixture(
            url_pages(3),
            CountingFetcher::default(),
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;
        let path = "Test Series/cch-21.cbz".to_string();
        f.library.present.lock().expect("lock").push(path.clone());
        *f.repos.downloaded.lock().expect("lock") = Some(DownloadedChapter {
            chapter_id,
            relative_path: path,
            size_bytes: 1,
            checksum: "sha".into(),
            packaged_at: chrono::Utc::now(),
        });

        f.handler
            .handle(&download_job(chapter_id))
            .await
            .expect("no work to do");
        assert_eq!(
            f.fetcher.calls.load(Ordering::SeqCst),
            0,
            "a chapter already on disk must not be fetched again"
        );
    }

    /// And the other direction: a row whose file has gone must download again,
    /// or a restored-from-empty library could never be repaired.
    #[tokio::test]
    async fn a_download_row_whose_file_is_gone_is_fetched_again() {
        let f = fixture(
            url_pages(2),
            CountingFetcher::default(),
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;
        *f.repos.downloaded.lock().expect("lock") = Some(DownloadedChapter {
            chapter_id,
            relative_path: "Test Series/vanished.cbz".into(),
            size_bytes: 1,
            checksum: "sha".into(),
            packaged_at: chrono::Utc::now(),
        });

        f.handler
            .handle(&download_job(chapter_id))
            .await
            .expect("downloaded");
        assert_eq!(f.fetcher.calls.load(Ordering::SeqCst), 2);
    }

    /// The per-source limit is the whole reason this crate has a limiter: a
    /// source that bans for concurrent requests must see one at a time.
    #[tokio::test]
    async fn one_permit_means_pages_are_fetched_one_at_a_time() {
        let f = fixture(
            url_pages(6),
            CountingFetcher::default(),
            1,
            DownloadConfig {
                // Deliberately higher than the permit count, so a missing
                // permit would show up as overlap.
                page_concurrency: 6,
                ..Default::default()
            },
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        f.handler
            .handle(&download_job(chapter_id))
            .await
            .expect("downloaded");
        assert_eq!(
            f.fetcher.peak.load(Ordering::SeqCst),
            1,
            "with one permit no two page fetches may overlap"
        );
    }

    /// And its converse, so the test above cannot pass on a handler that
    /// fetches serially and ignores the limiter entirely.
    #[tokio::test]
    async fn more_permits_let_pages_overlap() {
        let f = fixture(
            url_pages(6),
            CountingFetcher::default(),
            4,
            DownloadConfig {
                page_concurrency: 6,
                ..Default::default()
            },
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        f.handler
            .handle(&download_job(chapter_id))
            .await
            .expect("downloaded");
        assert!(
            f.fetcher.peak.load(Ordering::SeqCst) > 1,
            "four permits must actually produce concurrency"
        );
    }

    #[tokio::test]
    async fn a_failed_page_writes_nothing_and_records_nothing() {
        let f = fixture(
            url_pages(4),
            CountingFetcher {
                fail_on: Some(2),
                ..Default::default()
            },
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        let err = f
            .handler
            .handle(&download_job(chapter_id))
            .await
            .expect_err("one page failed");
        assert!(err.to_string().contains("page is gone"));
        assert!(
            f.library.written.lock().expect("lock").is_empty(),
            "a partial chapter must never reach the library"
        );
        assert!(f.repos.recorded.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_text_page_is_refused_by_name_rather_than_skipped() {
        let mut pages = url_pages(2);
        pages.push(SourcePage {
            index: 2,
            content: PageContent::Text("# chapter".into()),
            description: None,
        });
        let f = fixture(
            pages,
            CountingFetcher::default(),
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        let err = f
            .handler
            .handle(&download_job(chapter_id))
            .await
            .expect_err("text pages are not supported");
        assert!(matches!(err, DomainError::UnsupportedCapability(_)));
        assert_eq!(
            f.fetcher.calls.load(Ordering::SeqCst),
            0,
            "the chapter is refused before anything is fetched"
        );
        assert!(f.library.written.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn too_many_pages_is_refused_before_any_fetch() {
        let f = fixture(
            url_pages(10),
            CountingFetcher::default(),
            4,
            DownloadConfig {
                max_pages: 5,
                ..Default::default()
            },
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        let err = f
            .handler
            .handle(&download_job(chapter_id))
            .await
            .expect_err("over the page limit");
        assert!(
            !err.is_retryable(),
            "the chapter will be the same next time"
        );
        assert_eq!(f.fetcher.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_chapter_over_the_byte_limit_stops_and_writes_nothing() {
        let f = fixture(
            url_pages(8),
            CountingFetcher {
                bytes_each: 1024,
                ..Default::default()
            },
            4,
            DownloadConfig {
                max_chapter_bytes: 2048,
                ..Default::default()
            },
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        let err = f
            .handler
            .handle(&download_job(chapter_id))
            .await
            .expect_err("over the byte limit");
        assert!(err.to_string().contains("byte limit"));
        assert!(
            f.fetcher.calls.load(Ordering::SeqCst) < 8,
            "the limit must stop the fetch, not merely report it afterwards"
        );
        assert!(f.library.written.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_chapter_with_no_pages_is_retryable() {
        let f = fixture(
            vec![],
            CountingFetcher::default(),
            4,
            DownloadConfig::default(),
        )
        .await;
        let chapter_id = f.repos.chapter.id;

        let err = f
            .handler
            .handle(&download_job(chapter_id))
            .await
            .expect_err("no pages");
        assert!(
            err.is_retryable(),
            "a source that listed nothing now may list pages later"
        );
    }

    #[tokio::test]
    async fn a_payload_without_a_chapter_id_fails_permanently() {
        let f = fixture(
            url_pages(1),
            CountingFetcher::default(),
            4,
            DownloadConfig::default(),
        )
        .await;
        let err = f
            .handler
            .handle(&job(JobKind::DownloadChapter, serde_json::json!({})))
            .await
            .expect_err("no chapter_id");
        assert!(!err.is_retryable());
    }
}
