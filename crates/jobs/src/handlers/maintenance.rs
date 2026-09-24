//! The three handlers that keep the system from degrading on its own:
//! `prune_jobs`, `prune_sessions`, and `reconcile_library`.
//!
//! None of them talk to a source, so a build with no source runtime still
//! runs them. They are the handlers whose absence is invisible until it is
//! expensive: an unpruned `job` table, a session store that only grows, and a
//! library that offers reads of files that are no longer there.

use std::sync::Arc;

use nyuka_domain::model::{Chapter, ChapterId, Job, MangaId};
use nyuka_domain::ports::{ChapterRepository, LibraryStore, MangaRepository};
use nyuka_domain::{DomainError, Result};
use nyuka_persistence::session::SessionRepository;

use super::KindHandler;
use crate::queue::PostgresQueue;

/// Deletes terminal job rows past the retention window.
///
/// > [!IMPORTANT]
/// > Retention is not a free knob. The scheduler suppresses duplicate periodic
/// > jobs with an idempotency key that only works while the earlier row still
/// > exists, so shortening this below the longest schedule period makes those
/// > jobs fire more than once per period. `scheduler::check_retention` rejects
/// > that pairing at startup.
pub struct PruneJobs {
    queue: Arc<PostgresQueue>,
    retention: chrono::Duration,
}

impl PruneJobs {
    pub fn new(queue: Arc<PostgresQueue>, retention: chrono::Duration) -> Self {
        Self { queue, retention }
    }
}

#[async_trait::async_trait]
impl KindHandler for PruneJobs {
    async fn handle(&self, _job: &Job) -> Result<()> {
        let deleted = self.queue.prune(self.retention).await?;
        tracing::info!(deleted, "pruned terminal job rows");
        Ok(())
    }
}

/// Deletes expired sessions (ADR-0005).
///
/// Expiry is already enforced on read, so this reclaims space rather than
/// closing a security hole. Leaving it out does not let an expired session log
/// anyone in; it only lets the table grow.
pub struct PruneSessions {
    sessions: SessionRepository,
}

impl PruneSessions {
    pub fn new(sessions: SessionRepository) -> Self {
        Self { sessions }
    }
}

#[async_trait::async_trait]
impl KindHandler for PruneSessions {
    async fn handle(&self, _job: &Job) -> Result<()> {
        let deleted = self
            .sessions
            .delete_expired()
            .await
            .map_err(|e| DomainError::Storage(e.to_string()))?;
        tracing::info!(deleted, "pruned expired sessions");
        Ok(())
    }
}

/// What one reconciliation pass found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reconciled {
    pub checked: usize,
    /// Rows whose file is gone, so the UI must stop offering the read.
    pub missing: usize,
    /// Download records actually deleted. Equal to `missing` unless a
    /// concurrent writer removed one first.
    pub forgotten: u64,
}

/// Drops `downloaded_chapter` rows whose file is no longer on disk (ADR-0007).
///
/// The library root is a mount an operator can move, remount empty, or restore
/// from an older backup. When that happens the database still claims every
/// chapter is downloaded, and the UI offers a read that fails at the last
/// step. Reconciling turns that into an honest "not downloaded".
///
/// > [!WARNING]
/// > A library root that is temporarily unmounted looks exactly like one that
/// > was emptied. `run` therefore refuses to reconcile when the root itself is
/// > unreadable — see [`ReconcileLibrary::handle`] — because the alternative is
/// > deleting the entire download index during a five-minute NFS outage.
pub struct ReconcileLibrary {
    manga: Arc<dyn MangaRepository>,
    chapters: Arc<dyn ChapterRepository>,
    library: Arc<dyn LibraryStore>,
}

impl ReconcileLibrary {
    pub fn new(
        manga: Arc<dyn MangaRepository>,
        chapters: Arc<dyn ChapterRepository>,
        library: Arc<dyn LibraryStore>,
    ) -> Self {
        Self {
            manga,
            chapters,
            library,
        }
    }

    async fn reconcile_manga(&self, id: MangaId, report: &mut Reconciled) -> Result<()> {
        let mut cursor = None;
        loop {
            let page = self.chapters.list_for_manga(id, cursor.as_ref()).await?;
            let mut missing = Vec::new();
            for chapter in &page.items {
                if let Some(id) = self.missing_download(chapter, report).await? {
                    missing.push(id);
                }
            }
            // One statement per page rather than per chapter: a library
            // restored from an empty volume means every chapter is missing,
            // and that would otherwise be thousands of round trips.
            report.forgotten += self.chapters.forget_downloads(&missing).await?;

            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    /// Returns the chapter's id when its packaged file is gone.
    async fn missing_download(
        &self,
        chapter: &Chapter,
        report: &mut Reconciled,
    ) -> Result<Option<ChapterId>> {
        let Some(download) = self.chapters.downloaded(chapter.id).await? else {
            return Ok(None);
        };
        report.checked += 1;
        if self.library.exists(&download.relative_path).await? {
            return Ok(None);
        }
        report.missing += 1;
        tracing::warn!(
            chapter.id = %chapter.id,
            path = %download.relative_path,
            "packaged chapter is missing from the library; clearing its download record"
        );
        Ok(Some(chapter.id))
    }
}

#[async_trait::async_trait]
impl KindHandler for ReconcileLibrary {
    async fn handle(&self, _job: &Job) -> Result<()> {
        // Prove the root is readable before concluding anything is missing.
        // `free_bytes` fails on an unreadable root, which is the distinction
        // between "the mount is gone" and "the files are gone" — and treating
        // the first as the second would clear every download record.
        self.library.free_bytes().await.map_err(|e| {
            DomainError::Storage(format!(
                "library root is unreadable, so nothing can be declared missing: {e}"
            ))
        })?;

        let mut report = Reconciled::default();
        let mut cursor = None;
        loop {
            let page = self.manga.list(cursor.as_ref()).await?;
            for manga in &page.items {
                self.reconcile_manga(manga.id, &mut report).await?;
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        tracing::info!(
            checked = report.checked,
            missing = report.missing,
            forgotten = report.forgotten,
            "reconciled the library"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::testing::job;
    use nyuka_domain::model::{
        Chapter, ChapterId, ChapterSummary, ContentRating, Cursor, DownloadedChapter, ExternalKey,
        JobKind, Manga, MangaStatus, Page, PageRef, ReadingDirection, SourceChapter, SourceId,
    };
    use nyuka_domain::ports::ChapterRead;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn manga(id: MangaId) -> Manga {
        Manga {
            id,
            source_id: SourceId(Uuid::nil()),
            external_key: ExternalKey("k".into()),
            title: "T".into(),
            authors: vec![],
            artists: vec![],
            description: None,
            tags: vec![],
            cover_url: None,
            url: None,
            language: None,
            status: MangaStatus::Unknown,
            content_rating: ContentRating::Unknown,
            direction: ReadingDirection::Unknown,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn chapter(id: ChapterId, manga_id: MangaId) -> Chapter {
        Chapter {
            id,
            manga_id,
            external_key: ExternalKey("c".into()),
            title: None,
            number: None,
            volume: None,
            language: None,
            published_at: None,
        }
    }

    struct FakeManga(Vec<Manga>);

    #[async_trait::async_trait]
    impl MangaRepository for FakeManga {
        async fn get(&self, _id: MangaId) -> Result<Manga> {
            Err(DomainError::NotFound)
        }
        async fn find_by_external(
            &self,
            _source: SourceId,
            _key: &ExternalKey,
        ) -> Result<Option<Manga>> {
            Ok(None)
        }
        async fn upsert(&self, _manga: &Manga) -> Result<MangaId> {
            Err(DomainError::NotFound)
        }
        async fn list_summaries(
            &self,
            _query: &nyuka_domain::model::MangaQuery,
            _cursor: Option<&Cursor>,
        ) -> Result<nyuka_domain::model::Page<nyuka_domain::model::MangaSummary>> {
            Ok(nyuka_domain::model::Page {
                items: vec![],
                next: None,
            })
        }
        async fn list(&self, _cursor: Option<&Cursor>) -> Result<nyuka_domain::model::Page<Manga>> {
            Ok(nyuka_domain::model::Page {
                items: self.0.clone(),
                next: None,
            })
        }

        async fn delete(&self, _id: MangaId) -> Result<()> {
            unimplemented!("not exercised by this handler")
        }
    }

    #[derive(Default)]
    struct FakeChapters {
        chapters: Vec<Chapter>,
        downloads: Vec<DownloadedChapter>,
        forgotten: Mutex<Vec<ChapterId>>,
    }

    #[async_trait::async_trait]
    impl ChapterRepository for FakeChapters {
        async fn get(&self, _id: ChapterId) -> Result<Chapter> {
            Err(DomainError::NotFound)
        }
        async fn list_for_manga(
            &self,
            manga: MangaId,
            _cursor: Option<&Cursor>,
        ) -> Result<nyuka_domain::model::Page<Chapter>> {
            Ok(nyuka_domain::model::Page {
                items: self
                    .chapters
                    .iter()
                    .filter(|c| c.manga_id == manga)
                    .cloned()
                    .collect(),
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
        async fn downloaded(&self, id: ChapterId) -> Result<Option<DownloadedChapter>> {
            Ok(self.downloads.iter().find(|d| d.chapter_id == id).cloned())
        }
        async fn record_download(&self, _download: &DownloadedChapter) -> Result<()> {
            Ok(())
        }
        async fn forget_downloads(&self, missing: &[ChapterId]) -> Result<u64> {
            self.forgotten
                .lock()
                .expect("lock")
                .extend_from_slice(missing);
            Ok(missing.len() as u64)
        }

        async fn downloaded_paths_for_manga(&self, _manga: MangaId) -> Result<Vec<String>> {
            unimplemented!("not exercised by this handler")
        }
    }

    #[derive(Default)]
    struct FakeLibrary {
        present: HashSet<String>,
        deleted: Mutex<Vec<String>>,
        root_readable: bool,
    }

    #[async_trait::async_trait]
    impl LibraryStore for FakeLibrary {
        async fn write_chapter(
            &self,
            _manga: &Manga,
            _chapter: &Chapter,
            _pages: Vec<(PageRef, Vec<u8>)>,
        ) -> Result<DownloadedChapter> {
            Err(DomainError::NotFound)
        }
        async fn open_chapter(&self, _path: &str) -> Result<Box<dyn ChapterRead>> {
            Err(DomainError::NotFound)
        }
        async fn delete_chapter(&self, path: &str) -> Result<()> {
            self.deleted.lock().expect("lock").push(path.to_string());
            Ok(())
        }
        async fn exists(&self, path: &str) -> Result<bool> {
            Ok(self.present.contains(path))
        }
        async fn free_bytes(&self) -> Result<u64> {
            if self.root_readable {
                Ok(1 << 30)
            } else {
                Err(DomainError::Storage("root is not mounted".into()))
            }
        }
    }

    fn download(chapter_id: ChapterId, path: &str) -> DownloadedChapter {
        DownloadedChapter {
            chapter_id,
            relative_path: path.into(),
            size_bytes: 1,
            checksum: "x".into(),
            packaged_at: chrono::Utc::now(),
        }
    }

    struct Fixture {
        handler: ReconcileLibrary,
        chapters: Arc<FakeChapters>,
        library: Arc<FakeLibrary>,
        gone: ChapterId,
    }

    fn fixture(root_readable: bool, present: &[&str]) -> Fixture {
        let m = MangaId(Uuid::new_v4());
        let here = ChapterId(Uuid::new_v4());
        let gone = ChapterId(Uuid::new_v4());
        let library = Arc::new(FakeLibrary {
            present: present.iter().map(|s| s.to_string()).collect(),
            deleted: Mutex::new(vec![]),
            root_readable,
        });
        let chapters = Arc::new(FakeChapters {
            chapters: vec![chapter(here, m), chapter(gone, m)],
            downloads: vec![download(here, "T/here.cbz"), download(gone, "T/gone.cbz")],
            forgotten: Mutex::new(vec![]),
        });
        Fixture {
            handler: ReconcileLibrary::new(
                Arc::new(FakeManga(vec![manga(m)])),
                chapters.clone(),
                library.clone(),
            ),
            chapters,
            library,
            gone,
        }
    }

    #[tokio::test]
    async fn a_missing_file_clears_its_download_record_and_a_present_one_survives() {
        let f = fixture(true, &["T/here.cbz"]);
        f.handler
            .handle(&job(JobKind::ReconcileLibrary, serde_json::json!({})))
            .await
            .expect("reconciled");

        assert_eq!(
            *f.chapters.forgotten.lock().expect("lock"),
            vec![f.gone],
            "only the chapter whose file is gone may be forgotten"
        );
        assert!(
            f.library.deleted.lock().expect("lock").is_empty(),
            "the file is already absent; deleting it would be a no-op standing in \
             for the work that matters, which is clearing the row"
        );
    }

    /// The failure this handler could cause is worse than the one it fixes, so
    /// it is checked directly: an unreadable root must abort the pass rather
    /// than declare the whole library missing.
    #[tokio::test]
    async fn an_unmounted_library_root_aborts_instead_of_clearing_everything() {
        let f = fixture(false, &[]);
        let err = f
            .handler
            .handle(&job(JobKind::ReconcileLibrary, serde_json::json!({})))
            .await
            .expect_err("an unreadable root must not reconcile");

        assert!(err.to_string().contains("unreadable"));
        assert!(
            f.chapters.forgotten.lock().expect("lock").is_empty(),
            "not one record may be cleared while the root cannot be read"
        );
        assert!(
            err.is_retryable(),
            "a remount should fix this, so the job must come back"
        );
    }
}
