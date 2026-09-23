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

use crate::error::ApiResult;
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
    pub cursor: Option<String>,
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
            // Filters are a separate concern from search and are not wired
            // through the query string: they are a source-defined structure,
            // and flattening one into query parameters would make the shape
            // depend on whichever source the client last asked about.
            &serde_json::Value::Null,
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

    let details = state
        .items
        .details(source, &key)
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

    // Chapters the details call already returned are stored now rather than
    // waiting for a refresh, so the series is readable immediately. A source
    // that did not send them gets them on the first follow check or manual
    // refresh — asking again here would double the request rate for a list
    // nobody is looking at yet.
    if let Some(chapters) = details.chapters {
        state.chapters.upsert_many(id, &chapters).await?;
    }

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
