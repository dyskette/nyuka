//! Entities and value objects.
//!
//! Identifiers are newtypes so a `MangaId` cannot be passed where a
//! `ChapterId` is expected. Persistence has its own row structs and maps to
//! these at the repository boundary (ADR-0002).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_newtype!(/// A configured source repository (an index of installable sources).
    SourceRepoId);
id_newtype!(/// An installed source (one `.aix` package).
    SourceId);
id_newtype!(/// A series in the local library.
    MangaId);
id_newtype!(/// A chapter of a library series.
    ChapterId);
id_newtype!(/// A follow: a series being watched for new chapters.
    FollowId);
id_newtype!(/// A unit of background work.
    JobId);
id_newtype!(/// An authenticated principal.
    UserId);

/// A source's own identifier for an item, opaque to this application.
/// Attacker-influenced: never interpolate into a filesystem path without the
/// sanitization in `nyuka-packaging` (ADR-0007).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalKey(pub String);

/// A cursor for keyset pagination. Opaque to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);

/// One page of results plus the cursor that continues it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` when this is the last page.
    pub next: Option<Cursor>,
}

/// Reading direction, mapped to ComicInfo's `Manga` enum on export
/// (ADR-0007). `Unknown` is written rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReadingDirection {
    #[default]
    Unknown,
    LeftToRight,
    RightToLeft,
}

/// Publication status, as a source reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MangaStatus {
    #[default]
    Unknown,
    Ongoing,
    Completed,
    Cancelled,
    Hiatus,
}

/// How explicit a series is, as a source reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContentRating {
    #[default]
    Unknown,
    Safe,
    Suggestive,
    Nsfw,
}

/// A series as a **source** describes it.
///
/// Deliberately not [`Manga`]: a catalog result has no local identity yet. It
/// is keyed only by the source's own [`ExternalKey`], and acquires a
/// [`MangaId`] when a use case adds it to the library. Returning `Manga` from
/// a provider port would force an adapter to invent an id for something that
/// is not in the library, which is how source data and library data start
/// blurring together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceManga {
    pub key: ExternalKey,
    pub title: String,
    pub cover: Option<String>,
    pub authors: Vec<String>,
    pub artists: Vec<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub status: MangaStatus,
    pub content_rating: ContentRating,
    pub direction: ReadingDirection,
    /// Present when the source returned chapters alongside the details.
    pub chapters: Option<Vec<SourceChapter>>,
}

/// A chapter as a source describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceChapter {
    pub key: ExternalKey,
    pub title: Option<String>,
    pub number: Option<f32>,
    pub volume: Option<f32>,
    pub published_at: Option<DateTime<Utc>>,
    pub scanlators: Vec<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub locked: bool,
}

/// What a page holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PageContent {
    /// An image to fetch, with any headers the source requires.
    Url {
        url: String,
        headers: Vec<(String, String)>,
    },
    /// Markdown, for sources that publish text chapters.
    Text(String),
    /// A path inside a zip archive.
    Zip { archive: String, path: String },
}

/// One page of a chapter, as a source describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcePage {
    pub index: u32,
    pub content: PageContent,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manga {
    pub id: MangaId,
    pub source_id: SourceId,
    pub external_key: ExternalKey,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub cover_url: Option<String>,
    pub language: Option<String>,
    pub direction: ReadingDirection,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub id: ChapterId,
    pub manga_id: MangaId,
    pub external_key: ExternalKey,
    pub title: Option<String>,
    pub number: Option<f32>,
    pub volume: Option<f32>,
    pub language: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

/// A chapter that has been packaged into the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedChapter {
    pub chapter_id: ChapterId,
    /// Relative to the configured library root. `LibraryStore` never hands
    /// out absolute paths (ADR-0007).
    pub relative_path: String,
    pub size_bytes: u64,
    /// Computed during the write; also serves as the `ETag`.
    pub checksum: String,
    pub packaged_at: DateTime<Utc>,
}

/// A single page image to fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRef {
    pub index: u32,
    pub url: String,
    /// Some sources require specific headers (referer, user-agent).
    pub headers: Vec<(String, String)>,
}

/// A source's declared rate limit, from the `net::set_rate_limit` host import.
/// The job engine takes the stricter of this and the configured cap — ignoring
/// it is the fastest route to an IP ban (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    pub permits: u32,
    pub period_seconds: u32,
}

/// Host capabilities a source requires. `SourceRegistry` refuses installation
/// when any required capability is unimplemented (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    Net,
    Html,
    Defaults,
    Std,
    /// `js::context_*` — a JS engine. Tier 2.
    JsContext,
    /// `js::webview_*` — needs a real browser engine. Not implemented in v1.
    JsWebView,
    /// 2D drawing with fonts and text. Not implemented in v1.
    Canvas,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSource {
    pub id: SourceId,
    pub repo_id: SourceRepoId,
    pub name: String,
    pub version: String,
    pub languages: Vec<String>,
    pub required_capabilities: Vec<Capability>,
    pub declared_rate_limit: Option<RateLimit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    DownloadChapter,
    PackageChapter,
    RefreshMetadata,
    CheckFollow,
    UpdateSources,
    /// Deletes terminal job rows past the retention window (ADR-0003).
    PruneJobs,
    /// Deletes expired sessions (ADR-0005).
    PruneSessions,
    /// Marks `downloaded_chapter` rows whose files have gone missing
    /// (ADR-0007).
    ReconcileLibrary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    pub payload: serde_json::Value,
    pub priority: i16,
    pub run_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Progress and state changes published to the SSE stream (ADR-0010).
///
/// Every variant must also be observable through a plain fetch. SSE makes the
/// UI live; it never makes it correct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobEvent {
    JobProgress {
        job_id: JobId,
        done: u32,
        total: u32,
        bytes: u64,
    },
    JobState {
        job_id: JobId,
        state: JobState,
    },
    ChapterNew {
        manga_id: MangaId,
        chapter_id: ChapterId,
    },
    ChapterDownloaded {
        chapter_id: ChapterId,
    },
    SourceUpdated {
        source_id: SourceId,
    },
}
