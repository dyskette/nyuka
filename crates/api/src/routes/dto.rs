//! Wire types for the REST surface.
//!
//! Domain types are not serialized directly. Two reasons, and the second is
//! the one that bites:
//!
//! - A domain type changes for domain reasons. If it is also the wire format,
//!   every such change is a breaking API change nobody noticed.
//! - `SourceEntry` and `InstalledSource` carry fields a client has no business
//!   seeing, and `Manga` carries a `source_id` that is only meaningful with
//!   the source list alongside it. Mapping here is where that gets decided
//!   once.
//!
//! JSON is snake_case and timestamps are RFC 3339, per the conventions in
//! `routes/mod.rs`.

use nyuka_domain::model::{
    Chapter, ChapterSummary, ContentRating, Cursor, Follow, InstalledSource, Job, JobKind,
    JobState, JobSubject, JobSummary, Manga, MangaStatus, MangaSummary, Page, ReadingDirection,
    SourceEntry, SourceRepo,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// One page of results.
///
/// `next_cursor` is opaque and is the only supported way to ask for the next
/// page. Offsets are deliberately absent: they shift under inserts, and a
/// library that gains a chapter mid-scroll would skip or repeat one.
#[derive(Debug, Serialize, ToSchema)]
pub struct Paged<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T, U: From<T>> From<Page<T>> for Paged<U> {
    fn from(page: Page<T>) -> Self {
        Self {
            items: page.items.into_iter().map(U::from).collect(),
            next_cursor: page.next.map(|c| c.0),
        }
    }
}

/// The cursor query parameter shared by every list endpoint.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct Pagination {
    /// The `next_cursor` from a previous page.
    pub cursor: Option<String>,
}

impl Pagination {
    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor.clone().map(Cursor)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MangaDto {
    pub id: Uuid,
    pub source_id: Uuid,
    pub external_key: String,
    pub title: String,
    pub authors: Vec<String>,
    pub artists: Vec<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_url: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub status: String,
    pub content_rating: String,
    pub reading_direction: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Enum values are rendered as lower snake_case strings rather than as the
/// integers the column holds. The integers are a storage detail whose meaning
/// is pinned by a test; a client should not have to know them.
pub fn status_name(status: MangaStatus) -> &'static str {
    match status {
        MangaStatus::Unknown => "unknown",
        MangaStatus::Ongoing => "ongoing",
        MangaStatus::Completed => "completed",
        MangaStatus::Cancelled => "cancelled",
        MangaStatus::Hiatus => "hiatus",
    }
}

pub fn rating_name(rating: ContentRating) -> &'static str {
    match rating {
        ContentRating::Unknown => "unknown",
        ContentRating::Safe => "safe",
        ContentRating::Suggestive => "suggestive",
        ContentRating::Nsfw => "nsfw",
    }
}

pub fn direction_name(direction: ReadingDirection) -> &'static str {
    match direction {
        ReadingDirection::Unknown => "unknown",
        ReadingDirection::LeftToRight => "left_to_right",
        ReadingDirection::RightToLeft => "right_to_left",
    }
}

impl From<Manga> for MangaDto {
    fn from(m: Manga) -> Self {
        Self {
            id: m.id.0,
            source_id: m.source_id.0,
            external_key: m.external_key.0,
            title: m.title,
            authors: m.authors,
            artists: m.artists,
            description: m.description,
            tags: m.tags,
            cover_url: m.cover_url,
            url: m.url,
            language: m.language,
            status: status_name(m.status).into(),
            content_rating: rating_name(m.content_rating).into(),
            reading_direction: direction_name(m.direction).into(),
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// A library entry with the numbers the list view shows.
#[derive(Debug, Serialize, ToSchema)]
pub struct MangaSummaryDto {
    #[serde(flatten)]
    pub manga: MangaDto,
    pub source_name: String,
    pub chapter_count: i64,
    /// Chapters with a file in the library. The difference from
    /// `chapter_count` is what the progress column shows.
    pub downloaded_count: i64,
}

impl From<MangaSummary> for MangaSummaryDto {
    fn from(s: MangaSummary) -> Self {
        Self {
            manga: s.manga.into(),
            source_name: s.source_name,
            chapter_count: s.chapter_count,
            downloaded_count: s.downloaded_count,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChapterDto {
    pub id: Uuid,
    pub manga_id: Uuid,
    pub external_key: String,
    pub title: Option<String>,
    pub number: Option<f32>,
    pub volume: Option<f32>,
    pub language: Option<String>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A chapter with its local download state.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChapterSummaryDto {
    #[serde(flatten)]
    pub chapter: ChapterDto,
    /// True when a packaged file is recorded for this chapter.
    pub downloaded: bool,
    /// The packaged size, present only when `downloaded`.
    pub size_bytes: Option<i64>,
}

impl From<ChapterSummary> for ChapterSummaryDto {
    fn from(s: ChapterSummary) -> Self {
        Self {
            chapter: s.chapter.into(),
            downloaded: s.downloaded,
            size_bytes: s.size_bytes,
        }
    }
}

impl From<Chapter> for ChapterDto {
    fn from(c: Chapter) -> Self {
        Self {
            id: c.id.0,
            manga_id: c.manga_id.0,
            external_key: c.external_key.0,
            title: c.title,
            number: c.number,
            volume: c.volume,
            language: c.language,
            published_at: c.published_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FollowDto {
    pub id: Uuid,
    pub manga_id: Uuid,
    pub check_interval_secs: i32,
    pub last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub auto_download: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<Follow> for FollowDto {
    fn from(f: Follow) -> Self {
        Self {
            id: f.id.0,
            manga_id: f.manga_id.0,
            check_interval_secs: f.check_interval_secs,
            last_checked_at: f.last_checked_at,
            auto_download: f.auto_download,
            created_at: f.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceRepoDto {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub last_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<SourceRepo> for SourceRepoDto {
    fn from(r: SourceRepo) -> Self {
        Self {
            id: r.id.0,
            name: r.name,
            url: r.url,
            last_refreshed_at: r.last_refreshed_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceEntryDto {
    pub repo_id: Uuid,
    pub external_id: String,
    pub name: String,
    pub version: u32,
    pub icon_url: Option<String>,
    pub languages: Vec<String>,
    pub content_rating: String,
    pub base_url: Option<String>,
    /// Absent when the entry is not installed.
    pub installed_version: Option<u32>,
}

impl From<SourceEntry> for SourceEntryDto {
    fn from(e: SourceEntry) -> Self {
        Self {
            repo_id: e.repo_id.0,
            external_id: e.external_id.0,
            name: e.name,
            version: e.version,
            icon_url: e.icon_url,
            languages: e.languages,
            content_rating: rating_name(e.content_rating).into(),
            base_url: e.base_url,
            // The download URL is deliberately not exposed: a client has no
            // use for it, and publishing it invites someone to fetch a `.aix`
            // through the browser rather than through the install endpoint
            // that performs the capability check.
            installed_version: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstalledSourceDto {
    pub id: Uuid,
    pub repo_id: Uuid,
    pub external_id: String,
    pub name: String,
    pub version: u32,
    pub languages: Vec<String>,
    pub required_capabilities: Vec<String>,
    /// Requests per period the source asked for, if it has declared one.
    pub declared_rate_limit: Option<RateLimitDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RateLimitDto {
    pub permits: u32,
    pub period_seconds: u32,
}

impl From<InstalledSource> for InstalledSourceDto {
    fn from(s: InstalledSource) -> Self {
        Self {
            id: s.id.0,
            repo_id: s.repo_id.0,
            external_id: s.external_id.0,
            name: s.name,
            version: s.version,
            languages: s.languages,
            required_capabilities: s
                .required_capabilities
                .iter()
                .map(|c| format!("{c:?}").to_lowercase())
                .collect(),
            declared_rate_limit: s.declared_rate_limit.map(|r| RateLimitDto {
                permits: r.permits,
                period_seconds: r.period_seconds,
            }),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobDto {
    pub id: Uuid,
    pub kind: String,
    pub state: String,
    pub priority: i16,
    pub run_at: chrono::DateTime<chrono::Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub fn kind_name(kind: JobKind) -> &'static str {
    nyuka_jobs::queue::kind_to_str(kind)
}

pub fn state_name(state: JobState) -> &'static str {
    nyuka_jobs::queue::state_to_str(state)
}

impl From<Job> for JobDto {
    fn from(j: Job) -> Self {
        Self {
            id: j.id.0,
            kind: kind_name(j.kind).into(),
            state: state_name(j.state).into(),
            priority: j.priority,
            run_at: j.run_at,
            attempts: j.attempts,
            max_attempts: j.max_attempts,
            // The payload is not exposed. It carries a trace context and, for
            // some kinds, keys that are the source's business rather than the
            // browser's.
            last_error: j.last_error,
            created_at: j.created_at,
        }
    }
}

/// What a job is about, when that can be resolved.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobSubjectDto {
    pub manga_id: Uuid,
    pub manga_title: String,
    pub chapter_id: Uuid,
    pub chapter_number: Option<f32>,
    pub chapter_title: Option<String>,
}

/// A job with its subject resolved.
///
/// This is why the payload stays unexposed: a client needs to know *which*
/// chapter a download is for, not the opaque arguments the handler runs on.
/// Resolving it here answers the question without publishing the rest.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobSummaryDto {
    #[serde(flatten)]
    pub job: JobDto,
    /// Absent for maintenance work, which is about nothing a reader named.
    pub subject: Option<JobSubjectDto>,
}

impl From<JobSubject> for JobSubjectDto {
    fn from(s: JobSubject) -> Self {
        Self {
            manga_id: s.manga_id.0,
            manga_title: s.manga_title,
            chapter_id: s.chapter_id.0,
            chapter_number: s.chapter_number,
            chapter_title: s.chapter_title,
        }
    }
}

impl From<JobSummary> for JobSummaryDto {
    fn from(s: JobSummary) -> Self {
        Self {
            job: s.job.into(),
            subject: s.subject.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client branches on these strings. Changing one is a breaking API
    /// change, which is easy to do by accident while renaming a variant.
    #[test]
    fn enum_names_are_pinned() {
        assert_eq!(status_name(MangaStatus::Ongoing), "ongoing");
        assert_eq!(status_name(MangaStatus::Unknown), "unknown");
        assert_eq!(rating_name(ContentRating::Nsfw), "nsfw");
        assert_eq!(
            direction_name(ReadingDirection::RightToLeft),
            "right_to_left"
        );
        assert_eq!(kind_name(JobKind::DownloadChapter), "download_chapter");
        assert_eq!(state_name(JobState::Succeeded), "succeeded");
    }

    #[test]
    fn an_empty_page_omits_the_cursor_rather_than_sending_null() {
        let page: Paged<MangaDto> = Page::<Manga> {
            items: vec![],
            next: None,
        }
        .into();
        let json = serde_json::to_value(&page).expect("serialize");
        assert_eq!(json["items"], serde_json::json!([]));
        assert!(
            json.as_object()
                .expect("object")
                .get("next_cursor")
                .is_none(),
            "an absent cursor means `there is no next page`, and a null would \
             be a second way of saying the same thing"
        );
    }

    #[test]
    fn a_page_with_more_carries_the_cursor_through() {
        let page: Paged<MangaDto> = Page::<Manga> {
            items: vec![],
            next: Some(Cursor("abc".into())),
        }
        .into();
        assert_eq!(
            serde_json::to_value(&page).expect("serialize")["next_cursor"],
            "abc"
        );
    }

    /// The `.aix` URL must not reach a client: fetching one through the
    /// browser bypasses the capability check the install endpoint performs.
    #[test]
    fn a_source_entry_does_not_expose_its_download_url() {
        let entry = SourceEntry {
            repo_id: nyuka_domain::model::SourceRepoId(Uuid::new_v4()),
            external_id: nyuka_domain::model::ExternalKey("en.example".into()),
            name: "Example".into(),
            version: 1,
            icon_url: None,
            download_url: "https://example.test/secret-path/en.example.aix".into(),
            languages: vec![],
            content_rating: ContentRating::Safe,
            base_url: None,
            min_app_version: None,
        };
        let json = serde_json::to_string(&SourceEntryDto::from(entry)).expect("serialize");
        assert!(!json.contains("secret-path"), "json was: {json}");
        assert!(!json.contains("download_url"));
    }

    /// The payload carries a trace context and source-specific keys.
    #[test]
    fn a_job_does_not_expose_its_payload() {
        let job = Job {
            id: nyuka_domain::model::JobId(Uuid::new_v4()),
            kind: JobKind::DownloadChapter,
            state: JobState::Running,
            payload: serde_json::json!({ "trace_context": { "traceparent": "00-abc" } }),
            priority: 0,
            run_at: chrono::Utc::now(),
            attempts: 1,
            max_attempts: 5,
            last_error: None,
            created_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&JobDto::from(job)).expect("serialize");
        assert!(!json.contains("traceparent"), "json was: {json}");
    }
}
