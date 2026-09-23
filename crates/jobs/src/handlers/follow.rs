//! The `check_follow` and `refresh_metadata` handlers.
//!
//! Both ask a source what it knows about a series and write the answer back.
//! They are separate kinds because they are triggered by different things and
//! deserve different rates: `check_follow` runs on the follow's own interval
//! looking for new chapters, `refresh_metadata` runs when a user asks or when
//! a series is first added.

use std::sync::Arc;

use nyuka_domain::model::{ChapterId, Follow, Job, JobEvent, JobKind, Manga, MangaId, SourceManga};
use nyuka_domain::ports::{
    ChapterRepository, EventBus, FollowRepository, JobQueue, MangaRepository, SourceItem,
};
use nyuka_domain::{DomainError, Result};
use uuid::Uuid;

use super::{KindHandler, payload_field};
use crate::limiter::SourceLimiter;

/// Applies what a source reported onto the stored series.
///
/// The local id and `created_at` are kept: a refresh updates what the source
/// owns, never the library's own identity or ordering.
fn merge(existing: &Manga, source: &SourceManga) -> Manga {
    Manga {
        id: existing.id,
        source_id: existing.source_id,
        external_key: existing.external_key.clone(),
        title: source.title.clone(),
        authors: source.authors.clone(),
        artists: source.artists.clone(),
        description: source.description.clone(),
        tags: source.tags.clone(),
        cover_url: source.cover.clone(),
        url: source.url.clone(),
        // Sources do not report a language per series; it comes from the
        // installed source's own language list, which the install records.
        language: existing.language.clone(),
        status: source.status,
        content_rating: source.content_rating,
        direction: source.direction,
        created_at: existing.created_at,
        updated_at: chrono::Utc::now(),
    }
}

/// Shared by both handlers: fetch details and chapters, write them back, and
/// report which chapters are new.
struct Refresher {
    manga: Arc<dyn MangaRepository>,
    chapters: Arc<dyn ChapterRepository>,
    source: Arc<dyn SourceItem>,
    events: Arc<dyn EventBus>,
    limiter: Arc<SourceLimiter>,
}

impl Refresher {
    async fn refresh(&self, manga_id: MangaId) -> Result<Vec<ChapterId>> {
        let existing = self.manga.get(manga_id).await?;

        let (details, chapters) = {
            let _permit = self.limiter.acquire(existing.source_id).await;
            let details = self
                .source
                .details(existing.source_id, &existing.external_key)
                .await?;
            // Some sources return chapters with the details, which saves a
            // request. Asking again when they did would double the rate
            // toward the source for nothing.
            let chapters = match details.chapters.clone() {
                Some(chapters) => chapters,
                None => {
                    self.source
                        .chapters(existing.source_id, &existing.external_key)
                        .await?
                }
            };
            (details, chapters)
        };

        self.manga.upsert(&merge(&existing, &details)).await?;

        // `upsert_many` returns only the chapters it actually inserted, which
        // is what makes this notify on new chapters rather than on every
        // refresh (ADR-0010).
        let new = self.chapters.upsert_many(manga_id, &chapters).await?;
        for chapter_id in &new {
            self.events.publish(JobEvent::ChapterNew {
                manga_id,
                chapter_id: *chapter_id,
            });
        }
        Ok(new)
    }
}

/// Looks for new chapters on a followed series.
pub struct CheckFollow {
    refresher: Refresher,
    follows: Arc<dyn FollowRepository>,
    queue: Arc<dyn JobQueue>,
}

impl CheckFollow {
    pub fn new(
        manga: Arc<dyn MangaRepository>,
        chapters: Arc<dyn ChapterRepository>,
        follows: Arc<dyn FollowRepository>,
        source: Arc<dyn SourceItem>,
        events: Arc<dyn EventBus>,
        limiter: Arc<SourceLimiter>,
        queue: Arc<dyn JobQueue>,
    ) -> Self {
        Self {
            refresher: Refresher {
                manga,
                chapters,
                source,
                events,
                limiter,
            },
            follows,
            queue,
        }
    }

    /// Queues downloads for newly found chapters.
    ///
    /// Keyed on the chapter id, so a follow checked twice before the download
    /// runs enqueues one job rather than two — and so does a user who asks for
    /// the same chapter by hand.
    async fn queue_downloads(&self, follow: &Follow, new: &[ChapterId]) -> Result<()> {
        for chapter_id in new {
            self.queue
                .enqueue(
                    JobKind::DownloadChapter,
                    serde_json::json!({ "chapter_id": chapter_id.0 }),
                    None,
                    Some(&format!("download:{}", chapter_id.0)),
                    // Below a user-requested download: someone waiting on a
                    // chapter should not queue behind a follow sweep.
                    5,
                    5,
                )
                .await?;
        }
        tracing::info!(
            follow.id = %follow.id,
            chapters = new.len(),
            "queued downloads for new chapters"
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl KindHandler for CheckFollow {
    async fn handle(&self, job: &Job) -> Result<()> {
        let follow_id = nyuka_domain::model::FollowId(payload_field::<Uuid>(job, "follow_id")?);
        let follow = match self.follows.get(follow_id).await {
            Ok(follow) => follow,
            // The follow was removed between the scheduler enqueuing this and
            // the worker claiming it. That is not a failure — the work is
            // simply no longer wanted.
            Err(DomainError::NotFound) => {
                tracing::debug!(follow.id = %follow_id, "follow is gone; nothing to check");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let new = self.refresher.refresh(follow.manga_id).await?;

        if follow.auto_download && !new.is_empty() {
            self.queue_downloads(&follow, &new).await?;
        }

        // Stamped only now. Doing it before the refresh would make a source
        // outage look like a successful check and push the next one a whole
        // interval away.
        self.follows.mark_checked(follow_id).await?;
        Ok(())
    }
}

/// Re-reads a series' details and chapter list on demand.
pub struct RefreshMetadata {
    refresher: Refresher,
}

impl RefreshMetadata {
    pub fn new(
        manga: Arc<dyn MangaRepository>,
        chapters: Arc<dyn ChapterRepository>,
        source: Arc<dyn SourceItem>,
        events: Arc<dyn EventBus>,
        limiter: Arc<SourceLimiter>,
    ) -> Self {
        Self {
            refresher: Refresher {
                manga,
                chapters,
                source,
                events,
                limiter,
            },
        }
    }
}

#[async_trait::async_trait]
impl KindHandler for RefreshMetadata {
    async fn handle(&self, job: &Job) -> Result<()> {
        let manga_id = MangaId(payload_field::<Uuid>(job, "manga_id")?);
        let new = self.refresher.refresh(manga_id).await?;
        tracing::info!(manga.id = %manga_id, new_chapters = new.len(), "refreshed metadata");
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nyuka_domain::model::{
        Chapter, ChapterSummary, ContentRating, Cursor, DownloadedChapter, ExternalKey, FollowId,
        MangaStatus, Page, ReadingDirection, SourceChapter, SourceId,
    };
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SOURCE: SourceId = SourceId(Uuid::from_u128(7));

    pub(crate) struct Shared<T>(pub(crate) Arc<T>);

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

    pub(crate) fn stored_manga(id: MangaId) -> Manga {
        Manga {
            id,
            source_id: SOURCE,
            external_key: ExternalKey("series".into()),
            title: "Old Title".into(),
            authors: vec![],
            artists: vec![],
            description: None,
            tags: vec![],
            cover_url: None,
            url: None,
            language: Some("en".into()),
            status: MangaStatus::Unknown,
            content_rating: ContentRating::Unknown,
            direction: ReadingDirection::Unknown,
            created_at: chrono::DateTime::from_timestamp(1_000_000, 0).expect("ts"),
            updated_at: chrono::DateTime::from_timestamp(1_000_000, 0).expect("ts"),
        }
    }

    pub(crate) fn source_details(chapters: Option<Vec<SourceChapter>>) -> SourceManga {
        SourceManga {
            key: ExternalKey("series".into()),
            title: "New Title".into(),
            cover: Some("https://example.invalid/c.jpg".into()),
            authors: vec!["Author".into()],
            artists: vec!["Artist".into()],
            description: Some("desc".into()),
            url: Some("https://example.invalid/series".into()),
            tags: vec!["Action".into()],
            status: MangaStatus::Ongoing,
            content_rating: ContentRating::Safe,
            direction: ReadingDirection::RightToLeft,
            chapters,
        }
    }

    pub(crate) fn source_chapter(key: &str) -> SourceChapter {
        SourceChapter {
            key: ExternalKey(key.into()),
            title: None,
            number: Some(1.0),
            volume: None,
            published_at: None,
            scanlators: vec![],
            url: None,
            language: None,
            locked: false,
        }
    }

    #[derive(Default)]
    pub(crate) struct Repos {
        pub(crate) manga: Mutex<Option<Manga>>,
        pub(crate) upserted: Mutex<Vec<Manga>>,
        pub(crate) new_chapters: Vec<ChapterId>,
        pub(crate) upsert_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MangaRepository for Shared<Repos> {
        async fn get(&self, _id: MangaId) -> Result<Manga> {
            self.manga
                .lock()
                .expect("lock")
                .clone()
                .ok_or(DomainError::NotFound)
        }
        async fn find_by_external(&self, _s: SourceId, _k: &ExternalKey) -> Result<Option<Manga>> {
            Ok(None)
        }
        async fn upsert(&self, manga: &Manga) -> Result<MangaId> {
            self.upserted.lock().expect("lock").push(manga.clone());
            Ok(manga.id)
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
        async fn list(&self, _c: Option<&Cursor>) -> Result<Page<Manga>> {
            Ok(Page {
                items: vec![],
                next: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl ChapterRepository for Shared<Repos> {
        async fn get(&self, _id: ChapterId) -> Result<Chapter> {
            Err(DomainError::NotFound)
        }
        async fn list_for_manga(&self, _m: MangaId, _c: Option<&Cursor>) -> Result<Page<Chapter>> {
            Ok(Page {
                items: vec![],
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
            _m: MangaId,
            _chapters: &[SourceChapter],
        ) -> Result<Vec<ChapterId>> {
            self.upsert_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.new_chapters.clone())
        }
        async fn downloaded(&self, _id: ChapterId) -> Result<Option<DownloadedChapter>> {
            Ok(None)
        }
        async fn record_download(&self, _d: &DownloadedChapter) -> Result<()> {
            Ok(())
        }
        async fn forget_downloads(&self, _m: &[ChapterId]) -> Result<u64> {
            Ok(0)
        }
    }

    pub(crate) struct Follows {
        pub(crate) follow: Mutex<Option<Follow>>,
        pub(crate) checked: Mutex<Vec<FollowId>>,
    }

    #[async_trait::async_trait]
    impl FollowRepository for Shared<Follows> {
        async fn get(&self, _id: FollowId) -> Result<Follow> {
            self.follow
                .lock()
                .expect("lock")
                .clone()
                .ok_or(DomainError::NotFound)
        }
        async fn list(&self, _c: Option<&Cursor>) -> Result<Page<Follow>> {
            Ok(Page {
                items: vec![],
                next: None,
            })
        }
        async fn due(&self, _limit: u64) -> Result<Vec<Follow>> {
            Ok(vec![])
        }
        async fn upsert(&self, follow: &Follow) -> Result<FollowId> {
            Ok(follow.id)
        }
        async fn delete(&self, _id: FollowId) -> Result<()> {
            Ok(())
        }
        async fn mark_checked(&self, id: FollowId) -> Result<()> {
            self.checked.lock().expect("lock").push(id);
            Ok(())
        }
    }

    pub(crate) struct FakeSource {
        pub(crate) details: SourceManga,
        pub(crate) chapters: Vec<SourceChapter>,
        pub(crate) chapter_calls: AtomicUsize,
    }

    impl FakeSource {
        pub(crate) fn returning(details: SourceManga) -> Self {
            Self {
                details,
                chapters: vec![],
                chapter_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SourceItem for Shared<FakeSource> {
        async fn details(&self, _s: SourceId, _k: &ExternalKey) -> Result<SourceManga> {
            Ok(self.details.clone())
        }
        async fn chapters(&self, _s: SourceId, _k: &ExternalKey) -> Result<Vec<SourceChapter>> {
            self.chapter_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.chapters.clone())
        }
        async fn pages(
            &self,
            _s: SourceId,
            _m: &ExternalKey,
            _c: &ExternalKey,
        ) -> Result<Vec<nyuka_domain::model::SourcePage>> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    pub(crate) struct Bus {
        pub(crate) events: Mutex<Vec<JobEvent>>,
    }

    impl EventBus for Shared<Bus> {
        fn publish(&self, event: JobEvent) {
            self.events.lock().expect("lock").push(event);
        }
    }

    fn refresher(repos: Shared<Repos>, source: Shared<FakeSource>, bus: Shared<Bus>) -> Refresher {
        Refresher {
            manga: Arc::new(repos.clone()),
            chapters: Arc::new(repos),
            source: Arc::new(source),
            events: Arc::new(bus),
            limiter: Arc::new(SourceLimiter::new(2)),
        }
    }

    /// A refresh must overwrite what the source owns and keep what the library
    /// owns. Overwriting `created_at` would reshuffle "recently added" on every
    /// check; keeping a stale title would make a refresh pointless.
    #[test]
    fn merge_takes_source_fields_and_keeps_local_identity() {
        let id = MangaId(Uuid::new_v4());
        let existing = stored_manga(id);
        let merged = merge(&existing, &source_details(None));

        assert_eq!(merged.id, id);
        assert_eq!(merged.source_id, existing.source_id);
        assert_eq!(merged.created_at, existing.created_at);
        assert_eq!(
            merged.language, existing.language,
            "language comes from the installed source, not from the series"
        );

        assert_eq!(merged.title, "New Title");
        assert_eq!(merged.artists, vec!["Artist".to_string()]);
        assert_eq!(merged.tags, vec!["Action".to_string()]);
        assert_eq!(merged.status, MangaStatus::Ongoing);
        assert_eq!(merged.content_rating, ContentRating::Safe);
        assert_eq!(merged.direction, ReadingDirection::RightToLeft);
        assert!(merged.updated_at > existing.updated_at);
    }

    /// Asking for chapters a source already handed over doubles the request
    /// rate toward it for nothing.
    #[tokio::test]
    async fn chapters_returned_with_the_details_are_not_fetched_again() {
        let id = MangaId(Uuid::new_v4());
        let repos = Shared(Arc::new(Repos {
            manga: Mutex::new(Some(stored_manga(id))),
            ..Default::default()
        }));
        let source = Shared(Arc::new(FakeSource {
            details: source_details(Some(vec![source_chapter("c1")])),
            chapters: vec![],
            chapter_calls: AtomicUsize::new(0),
        }));

        refresher(repos, source.clone(), Shared(Arc::new(Bus::default())))
            .refresh(id)
            .await
            .expect("refreshed");

        assert_eq!(
            source.chapter_calls.load(Ordering::SeqCst),
            0,
            "the details already carried the chapter list"
        );
    }

    #[tokio::test]
    async fn chapters_are_fetched_when_the_details_omit_them() {
        let id = MangaId(Uuid::new_v4());
        let repos = Shared(Arc::new(Repos {
            manga: Mutex::new(Some(stored_manga(id))),
            ..Default::default()
        }));
        let source = Shared(Arc::new(FakeSource {
            details: source_details(None),
            chapters: vec![source_chapter("c1")],
            chapter_calls: AtomicUsize::new(0),
        }));

        refresher(repos, source.clone(), Shared(Arc::new(Bus::default())))
            .refresh(id)
            .await
            .expect("refreshed");

        assert_eq!(source.chapter_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn only_newly_inserted_chapters_are_announced() {
        let id = MangaId(Uuid::new_v4());
        let new = vec![ChapterId(Uuid::new_v4()), ChapterId(Uuid::new_v4())];
        let repos = Shared(Arc::new(Repos {
            manga: Mutex::new(Some(stored_manga(id))),
            new_chapters: new.clone(),
            ..Default::default()
        }));
        let bus = Shared(Arc::new(Bus::default()));

        let announced = refresher(
            repos,
            Shared(Arc::new(FakeSource {
                details: source_details(Some(vec![
                    source_chapter("c1"),
                    source_chapter("c2"),
                    source_chapter("c3"),
                ])),
                chapters: vec![],
                chapter_calls: AtomicUsize::new(0),
            })),
            bus.clone(),
        )
        .refresh(id)
        .await
        .expect("refreshed");

        assert_eq!(announced, new);
        assert_eq!(
            bus.events.lock().expect("lock").len(),
            2,
            "three chapters were seen but only two were new; announcing all \
             three would notify on every refresh"
        );
    }
}

#[cfg(test)]
mod check_follow_tests {
    use super::tests::*;
    use super::*;
    use crate::handlers::testing::job;
    use nyuka_domain::model::{Cursor, FollowId, JobId, JobState, Page};
    use std::sync::Mutex;

    /// One recorded `enqueue` call.
    struct Enqueued {
        kind: JobKind,
        payload: serde_json::Value,
        idempotency_key: Option<String>,
        priority: i16,
    }

    #[derive(Default)]
    struct RecordingQueue {
        enqueued: Mutex<Vec<Enqueued>>,
    }

    #[async_trait::async_trait]
    impl JobQueue for Shared<RecordingQueue> {
        async fn enqueue(
            &self,
            kind: JobKind,
            payload: serde_json::Value,
            _run_at: Option<chrono::DateTime<chrono::Utc>>,
            idempotency_key: Option<&str>,
            priority: i16,
            _max_attempts: i32,
        ) -> Result<JobId> {
            self.enqueued.lock().expect("lock").push(Enqueued {
                kind,
                payload,
                idempotency_key: idempotency_key.map(str::to_string),
                priority,
            });
            Ok(JobId(Uuid::new_v4()))
        }
        async fn claim(&self, _worker: &str) -> Result<Option<Job>> {
            Ok(None)
        }
        async fn complete(&self, _job: JobId) -> Result<()> {
            Ok(())
        }
        async fn fail(
            &self,
            _job: JobId,
            _error: &str,
            _retryable: bool,
        ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
            Ok(None)
        }
        async fn cancel(&self, _job: JobId) -> Result<()> {
            Ok(())
        }
        async fn recover_stale(&self, _older_than_secs: u64) -> Result<u64> {
            Ok(0)
        }
        async fn get(&self, _job: JobId) -> Result<Job> {
            Err(DomainError::NotFound)
        }
        async fn list(&self, _state: Option<JobState>, _c: Option<&Cursor>) -> Result<Page<Job>> {
            Ok(Page {
                items: vec![],
                next: None,
            })
        }
    }

    struct Setup {
        handler: CheckFollow,
        follows: Shared<Follows>,
        queue: Shared<RecordingQueue>,
        follow_id: FollowId,
    }

    fn setup(auto_download: bool, new: Vec<ChapterId>, follow_exists: bool) -> Setup {
        let manga_id = MangaId(Uuid::new_v4());
        let follow_id = FollowId(Uuid::new_v4());
        let repos = Shared(Arc::new(Repos {
            manga: Mutex::new(Some(stored_manga(manga_id))),
            new_chapters: new,
            ..Default::default()
        }));
        let follows = Shared(Arc::new(Follows {
            follow: Mutex::new(follow_exists.then(|| Follow {
                id: follow_id,
                manga_id,
                check_interval_secs: 3600,
                last_checked_at: None,
                auto_download,
                created_at: chrono::Utc::now(),
            })),
            checked: Mutex::new(vec![]),
        }));
        let queue = Shared(Arc::new(RecordingQueue::default()));
        let source = Shared(Arc::new(FakeSource::returning(source_details(Some(vec![
            source_chapter("c1"),
        ])))));

        Setup {
            handler: CheckFollow::new(
                Arc::new(repos.clone()),
                Arc::new(repos),
                Arc::new(follows.clone()),
                Arc::new(source),
                Arc::new(Shared(Arc::new(Bus::default()))),
                Arc::new(SourceLimiter::new(2)),
                Arc::new(queue.clone()),
            ),
            follows,
            queue,
            follow_id,
        }
    }

    fn check_job(id: FollowId) -> Job {
        job(
            JobKind::CheckFollow,
            serde_json::json!({ "follow_id": id.0 }),
        )
    }

    #[tokio::test]
    async fn auto_download_queues_one_job_per_new_chapter() {
        let new = vec![ChapterId(Uuid::new_v4()), ChapterId(Uuid::new_v4())];
        let s = setup(true, new.clone(), true);

        s.handler
            .handle(&check_job(s.follow_id))
            .await
            .expect("checked");

        let enqueued = s.queue.enqueued.lock().expect("lock");
        assert_eq!(enqueued.len(), 2);
        for (i, call) in enqueued.iter().enumerate() {
            assert_eq!(call.kind, JobKind::DownloadChapter);
            assert_eq!(call.payload["chapter_id"], serde_json::json!(new[i].0));
            assert_eq!(
                call.idempotency_key.as_deref(),
                Some(format!("download:{}", new[i].0).as_str()),
                "keying on the chapter means two checks before the download \
                 runs enqueue one job, not two"
            );
            assert!(
                call.priority > 0,
                "a follow-discovered download must not outrank one a user is waiting on"
            );
        }
    }

    #[tokio::test]
    async fn a_follow_without_auto_download_queues_nothing() {
        let s = setup(false, vec![ChapterId(Uuid::new_v4())], true);
        s.handler
            .handle(&check_job(s.follow_id))
            .await
            .expect("checked");
        assert!(s.queue.enqueued.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_successful_check_is_stamped() {
        let s = setup(true, vec![], true);
        s.handler
            .handle(&check_job(s.follow_id))
            .await
            .expect("checked");
        assert_eq!(
            *s.follows.checked.lock().expect("lock"),
            vec![s.follow_id],
            "the interval restarts only after a check that worked"
        );
    }

    /// The scheduler enqueues, then a user unfollows before the worker gets
    /// there. That is not a failure, and failing it would retry three times
    /// against a follow that no longer exists.
    #[tokio::test]
    async fn a_follow_deleted_before_the_job_ran_is_not_an_error() {
        let s = setup(true, vec![], false);
        s.handler
            .handle(&check_job(s.follow_id))
            .await
            .expect("a removed follow is simply nothing to do");
        assert!(s.follows.checked.lock().expect("lock").is_empty());
        assert!(s.queue.enqueued.lock().expect("lock").is_empty());
    }
}
