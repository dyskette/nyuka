//! `/api/v1/sources/{id}/catalog` — browsing a source, and adding from it.
//!
//! Everything here reaches a third-party site through a WASM module, so every
//! response is slower and less reliable than a database read, and every string
//! in it came from a package this project did not review.
//!
//! # A catalog result is not a library entry
//!
//! `SourceManga` carries no local id, because a catalog result does not have
//! one until someone adds it. Returning `Manga` from these endpoints would
//! mean fabricating an id for something that is not in the library, which is
//! exactly the confusion the split exists to prevent.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use nyuka_domain::model::{Cursor, ExternalKey, Manga, MangaId, SourceId, SourceManga};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{ChapterDto, MangaDto, direction_name, rating_name, status_name};
use crate::routes::library::not_found;
use crate::state::AppState;

/// A catalog result, which has no local identity.
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogItemDto {
    /// The source's own key. This, plus the source id, is how it is added.
    pub external_key: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub authors: Vec<String>,
    pub artists: Vec<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub status: String,
    pub content_rating: String,
    pub reading_direction: String,
    /// Set when this series is already in the library, so the UI can offer
    /// "open" rather than "add".
    pub manga_id: Option<Uuid>,
}

impl From<SourceManga> for CatalogItemDto {
    fn from(m: SourceManga) -> Self {
        Self {
            external_key: m.key.0,
            title: m.title,
            cover_url: m.cover,
            authors: m.authors,
            artists: m.artists,
            description: m.description,
            url: m.url,
            tags: m.tags,
            status: status_name(m.status).into(),
            content_rating: rating_name(m.content_rating).into(),
            reading_direction: direction_name(m.direction).into(),
            manga_id: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogPage {
    pub items: Vec<CatalogItemDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct CatalogQuery {
    /// Free-text search. Absent means the source's default listing.
    pub q: Option<String>,

    /// The source's own filters, as a JSON array of filter values.
    ///
    /// One parameter carrying JSON rather than a parameter per filter. A
    /// source declares its own filters — 121 of the 136 community sources do
    /// — so flattening them into the query string would make the shape of a
    /// request depend on whichever source it was about, and multi-select
    /// carries separate included and excluded lists that no flat encoding
    /// holds without inventing a convention.
    ///
    /// It stays a `GET` so a filtered catalog is a link, which is how every
    /// screen in this application already keeps its state.
    ///
    /// Each value is one of:
    ///
    /// ```json
    /// [
    ///   {"Text":        {"id": "author", "value": "Mori"}},
    ///   {"Sort":        {"id": "sort", "index": 1, "ascending": false}},
    ///   {"Check":       {"id": "completed", "value": 1}},
    ///   {"Select":      {"id": "status", "value": "ongoing"}},
    ///   {"MultiSelect": {"id": "genre", "included": ["action"], "excluded": ["horror"]}}
    /// ]
    /// ```
    pub filters: Option<String>,

    pub cursor: Option<String>,
}

impl CatalogQuery {
    /// Reads the `filters` parameter.
    ///
    /// Absent is `Null`, which the runtime reads as no filters. A value that
    /// is not JSON is refused here rather than passed on: the source would
    /// never see it, so reporting it as a source failure would send someone
    /// to check a third-party site over their own request.
    fn filters(&self) -> ApiResult<serde_json::Value> {
        let Some(raw) = self.filters.as_deref() else {
            return Ok(serde_json::Value::Null);
        };

        serde_json::from_str(raw).map_err(|e| {
            ApiError(Box::new(Problem::invalid(format!(
                "`filters` is not valid JSON: {e}"
            ))))
        })
    }
}

/// `GET /api/v1/sources/{id}/catalog`
#[utoipa::path(
    operation_id = "browseCatalog",
    get,
    path = "/sources/{id}/catalog",
    tag = "catalog",
    params(("id" = Uuid, Path, description = "Source id"), CatalogQuery),
    responses(
        (status = OK, body = CatalogPage),
        (status = NOT_FOUND, description = "No such source"),
        (status = BAD_GATEWAY, description = "The source could not be reached"),
    ),
)]
pub async fn browse(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<CatalogQuery>,
) -> ApiResult<Json<CatalogPage>> {
    let page = state
        .catalog
        .list(
            SourceId(id),
            query.q.as_deref(),
            &query.filters()?,
            query.cursor.map(Cursor).as_ref(),
        )
        .await
        .map_err(|e| not_found(e, "source"))?;

    let items = mark_present(&state, SourceId(id), page.items).await?;
    Ok(Json(CatalogPage {
        items,
        next_cursor: page.next.map(|c| c.0),
    }))
}

/// `GET /api/v1/sources/{id}/catalog/{key}`
#[utoipa::path(
    operation_id = "getCatalogItem",
    get,
    path = "/sources/{id}/catalog/{key}",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Source id"),
        ("key" = String, Path, description = "The source's own key for the series"),
    ),
    responses(
        (status = OK, body = CatalogItemDto),
        (status = NOT_FOUND, description = "No such source or series"),
        (status = BAD_GATEWAY, description = "The source could not be reached"),
    ),
)]
pub async fn details(
    State(state): State<Arc<AppState>>,
    Path((id, key)): Path<(Uuid, String)>,
) -> ApiResult<Json<CatalogItemDto>> {
    let key = ExternalKey(key);
    let details = state
        .items
        .details(SourceId(id), &key)
        .await
        .map_err(|e| not_found(e, "series"))?;

    let mut item = CatalogItemDto::from(details);
    item.manga_id = state
        .manga
        .find_by_external(SourceId(id), &key)
        .await?
        .map(|m| m.id.0);
    Ok(Json(item))
}

/// The chapter list, from whichever call carries it.
///
/// Aidoku sources come in two shapes: some return chapters alongside a
/// series' details, and some expose them only through a separate
/// `get_chapter_list`. Asking again when the details already carried them
/// would double the request rate toward a third party for nothing.
///
/// `add` used to store only what the details happened to include, on the
/// reasoning that a second request was traffic toward a list nobody was
/// looking at yet. They are: the catalog card turns into "Open in library"
/// the moment the add returns, so a series from a source of the second kind
/// opened on an empty chapter list — and nothing would ever fill it, because
/// no endpoint refreshes a single series.
///
/// `Refresher` in the jobs crate resolves the same two shapes the same way.
async fn chapter_list(
    items: &dyn nyuka_domain::ports::SourceItem,
    source: SourceId,
    key: &ExternalKey,
    from_details: Option<Vec<nyuka_domain::model::SourceChapter>>,
) -> nyuka_domain::Result<Vec<nyuka_domain::model::SourceChapter>> {
    match from_details {
        Some(chapters) => Ok(chapters),
        None => items.chapters(source, key).await,
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddRequest {
    pub source_id: Uuid,
    pub external_key: String,
}

/// `POST /api/v1/manga` — add a catalog entry to the library.
///
/// The body names a source and a key, not a series: the metadata comes from
/// the source, never from the client. Accepting a client's version would let
/// anyone write any title into the library.
#[utoipa::path(
    operation_id = "addMangaToLibrary",
    post,
    path = "/manga",
    tag = "catalog",
    request_body = AddRequest,
    responses(
        (status = OK, body = MangaDto, description = "Added, or already present"),
        (status = NOT_FOUND, description = "No such source or series"),
        (status = BAD_GATEWAY, description = "The source could not be reached"),
    ),
)]
pub async fn add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddRequest>,
) -> ApiResult<Json<MangaDto>> {
    let source = SourceId(body.source_id);
    let key = ExternalKey(body.external_key);

    let mut details = state
        .items
        .details(source, &key)
        .await
        .map_err(|e| not_found(e, "series"))?;

    // Read before the series is written, so the add is all or nothing. A
    // source that answers with details and then fails on chapters leaves
    // nothing behind, and pressing Add again is a retry rather than a series
    // that is present in the library and permanently empty.
    let chapters = chapter_list(state.items.as_ref(), source, &key, details.chapters.take())
        .await
        .map_err(|e| not_found(e, "series"))?;

    let now = chrono::Utc::now();
    let id = state
        .manga
        .upsert(&Manga {
            // Assigned on insert; on conflict the existing row keeps its own,
            // which is what makes adding twice idempotent.
            id: MangaId(Uuid::nil()),
            source_id: source,
            external_key: key.clone(),
            title: details.title,
            authors: details.authors,
            artists: details.artists,
            description: details.description,
            tags: details.tags,
            cover_url: details.cover,
            url: details.url,
            // Not per-series: it comes from the installed source's language
            // list, which the install recorded.
            language: None,
            status: details.status,
            content_rating: details.content_rating,
            direction: details.direction,
            created_at: now,
            updated_at: now,
        })
        .await?;

    state.chapters.upsert_many(id, &chapters).await?;

    Ok(Json(state.manga.get(id).await?.into()))
}

/// `GET /api/v1/sources/{id}/catalog/{key}/chapters`
#[utoipa::path(
    operation_id = "listCatalogChapters",
    get,
    path = "/sources/{id}/catalog/{key}/chapters",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Source id"),
        ("key" = String, Path, description = "The source's own key for the series"),
    ),
    responses(
        (status = OK, body = Vec<ChapterDto>),
        (status = NOT_FOUND, description = "No such source or series"),
        (status = BAD_GATEWAY, description = "The source could not be reached"),
    ),
)]
pub async fn chapters(
    State(state): State<Arc<AppState>>,
    Path((id, key)): Path<(Uuid, String)>,
) -> ApiResult<Json<Vec<SourceChapterDto>>> {
    let chapters = state
        .items
        .chapters(SourceId(id), &ExternalKey(key))
        .await
        .map_err(|e| not_found(e, "series"))?;

    Ok(Json(chapters.into_iter().map(Into::into).collect()))
}

/// A chapter as a source describes it, with no local id.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceChapterDto {
    pub external_key: String,
    pub title: Option<String>,
    pub number: Option<f32>,
    pub volume: Option<f32>,
    pub language: Option<String>,
    pub scanlators: Vec<String>,
    pub url: Option<String>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the source says this chapter needs something the reader does
    /// not have, such as a paid account.
    pub locked: bool,
}

impl From<nyuka_domain::model::SourceChapter> for SourceChapterDto {
    fn from(c: nyuka_domain::model::SourceChapter) -> Self {
        Self {
            external_key: c.key.0,
            title: c.title,
            number: c.number,
            volume: c.volume,
            language: c.language,
            scanlators: c.scanlators,
            url: c.url,
            published_at: c.published_at,
            locked: c.locked,
        }
    }
}

/// Marks catalog results that are already in the library.
///
/// One lookup per item rather than one query: the page is bounded by what the
/// source returned, and a batch lookup would need a repository method whose
/// only caller is this function.
async fn mark_present(
    state: &Arc<AppState>,
    source: SourceId,
    items: Vec<SourceManga>,
) -> ApiResult<Vec<CatalogItemDto>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let key = item.key.clone();
        let mut dto = CatalogItemDto::from(item);
        dto.manga_id = state
            .manga
            .find_by_external(source, &key)
            .await?
            .map(|m| m.id.0);
        out.push(dto);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyuka_domain::model::{SourceChapter, SourcePage};
    use nyuka_domain::ports::SourceItem;
    // `Result` in this module is the API's, which is not what the port returns.
    use nyuka_domain::Result as DomainResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SOURCE: SourceId = SourceId(Uuid::from_u128(11));

    fn chapter(key: &str) -> SourceChapter {
        SourceChapter {
            key: ExternalKey(key.into()),
            title: None,
            number: None,
            volume: None,
            published_at: None,
            scanlators: Vec::new(),
            url: None,
            language: None,
            locked: false,
        }
    }

    /// A source whose separate chapter call is counted, so a test can say
    /// whether it was reached rather than only what came back.
    struct CountingSource {
        calls: AtomicUsize,
        chapters: Vec<SourceChapter>,
    }

    #[async_trait::async_trait]
    impl SourceItem for CountingSource {
        async fn details(
            &self,
            _source: SourceId,
            _key: &ExternalKey,
        ) -> DomainResult<SourceManga> {
            Err(nyuka_domain::DomainError::NotFound)
        }

        async fn chapters(
            &self,
            _source: SourceId,
            _key: &ExternalKey,
        ) -> DomainResult<Vec<SourceChapter>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.chapters.clone())
        }

        async fn pages(
            &self,
            _source: SourceId,
            _manga: &ExternalKey,
            _chapter: &ExternalKey,
        ) -> DomainResult<Vec<SourcePage>> {
            Ok(Vec::new())
        }
    }

    fn source(chapters: Vec<SourceChapter>) -> CountingSource {
        CountingSource {
            calls: AtomicUsize::new(0),
            chapters,
        }
    }

    /// The defect: Asura returns details without chapters, so a series added
    /// from it went into the library with an empty chapter list that nothing
    /// would ever fill.
    #[tokio::test]
    async fn asks_the_source_when_the_details_carried_no_chapters() {
        let items = source(vec![chapter("ch-1"), chapter("ch-2")]);

        let chapters = chapter_list(&items, SOURCE, &ExternalKey("series".into()), None)
            .await
            .expect("the chapter list is read");

        assert_eq!(chapters.len(), 2, "the separate call's chapters are used");
        assert_eq!(items.calls.load(Ordering::SeqCst), 1);
    }

    /// The other half: a source that already sent them must not be asked
    /// again. Two requests per add toward a third party, for a list already
    /// in hand, is what the original code was right to avoid.
    #[tokio::test]
    async fn does_not_ask_again_when_the_details_carried_them() {
        let items = source(vec![chapter("from-the-separate-call")]);

        let chapters = chapter_list(
            &items,
            SOURCE,
            &ExternalKey("series".into()),
            Some(vec![chapter("from-the-details")]),
        )
        .await
        .expect("the chapter list is read");

        assert_eq!(chapters[0].key.0, "from-the-details");
        assert_eq!(
            items.calls.load(Ordering::SeqCst),
            0,
            "the source was asked for a list it had already sent"
        );
    }

    /// An empty list is an answer, not an absence. A source that says a
    /// series has no chapters yet must not be asked a second time.
    #[tokio::test]
    async fn an_empty_list_in_the_details_is_still_an_answer() {
        let items = source(vec![chapter("ch-1")]);

        let chapters = chapter_list(
            &items,
            SOURCE,
            &ExternalKey("series".into()),
            Some(Vec::new()),
        )
        .await
        .expect("the chapter list is read");

        assert!(chapters.is_empty());
        assert_eq!(items.calls.load(Ordering::SeqCst), 0);
    }
}
