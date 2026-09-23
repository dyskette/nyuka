//! `/api/v1/manga` — the local library.
//!
//! Reads only. A series enters the library through the catalog (adding one
//! from a source) or through a follow check, never by a client posting a body
//! it composed: the source owns this metadata, and accepting a client's
//! version of it would let anyone overwrite a title with anything.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use nyuka_domain::model::{ChapterId, Cursor, MangaId, MangaQuery, MangaSort, SortDir, SourceId};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{
    ChapterDto, ChapterSummaryDto, MangaDto, MangaSummaryDto, Paged, Pagination, status_from_name,
};
use crate::state::AppState;

/// The library listing's filters and ordering.
///
/// Every field is optional and every unrecognised value is refused rather than
/// ignored. Ignoring `sort=titel` returns the default ordering and looks like
/// the sort silently not working; ignoring `status=onging` returns every
/// series and looks like the filter matching everything.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct LibraryQuery {
    /// Free text matched against the title.
    pub q: Option<String>,
    /// One of `unknown`, `ongoing`, `completed`, `cancelled`, `hiatus`.
    pub status: Option<String>,
    pub source_id: Option<Uuid>,
    /// One of `title`, `added`, `updated`, `chapters`. Defaults to `updated`.
    pub sort: Option<String>,
    /// `asc` or `desc`. Defaults to `desc`.
    pub dir: Option<String>,
    pub cursor: Option<String>,
}

impl LibraryQuery {
    /// Validates the request into a domain query.
    fn parse(&self) -> ApiResult<MangaQuery> {
        let sort = match self.sort.as_deref() {
            None => MangaSort::default(),
            Some("title") => MangaSort::Title,
            Some("added") => MangaSort::Added,
            Some("updated") => MangaSort::Updated,
            Some("chapters") => MangaSort::Chapters,
            Some(other) => {
                return Err(ApiError(Box::new(Problem::invalid(format!(
                    "`{other}` is not a sort order; expected one of title, \
                     added, updated, chapters"
                )))));
            }
        };

        let dir = match self.dir.as_deref() {
            None => SortDir::default(),
            Some("asc") => SortDir::Asc,
            Some("desc") => SortDir::Desc,
            Some(other) => {
                return Err(ApiError(Box::new(Problem::invalid(format!(
                    "`{other}` is not a direction; expected asc or desc"
                )))));
            }
        };

        let status = match self.status.as_deref() {
            None => None,
            Some(name) => Some(status_from_name(name).ok_or_else(|| {
                ApiError(Box::new(Problem::invalid(format!(
                    "`{name}` is not a publication status; expected one of \
                     unknown, ongoing, completed, cancelled, hiatus"
                ))))
            })?),
        };

        Ok(MangaQuery {
            q: self.q.clone(),
            status,
            source_id: self.source_id.map(SourceId),
            sort,
            dir,
        })
    }
}

/// `GET /api/v1/manga`
#[utoipa::path(
    operation_id = "listLibrary",
    get,
    path = "/manga",
    tag = "library",
    params(LibraryQuery),
    responses(
        (status = OK, body = Paged<MangaSummaryDto>),
        (status = BAD_REQUEST, description = "Unknown sort, direction, or status"),
    ),
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LibraryQuery>,
) -> ApiResult<Json<Paged<MangaSummaryDto>>> {
    // Summaries rather than bare series: the list view shows chapter and
    // download counts, and fetching those per row would be one query per
    // series on a page of fifty.
    //
    // Ordering and filtering are the server's job for the same reason: a
    // client can only order what it holds, which is one page.
    Ok(Json(
        state
            .manga
            .list_summaries(&query.parse()?, query.cursor.clone().map(Cursor).as_ref())
            .await?
            .into(),
    ))
}

/// `GET /api/v1/manga/{id}`
#[utoipa::path(
    operation_id = "getManga",
    get,
    path = "/manga/{id}",
    tag = "library",
    params(("id" = Uuid, Path, description = "Series id")),
    responses(
        (status = OK, body = MangaDto),
        (status = NOT_FOUND, description = "No such series"),
    ),
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<MangaDto>> {
    let manga = state
        .manga
        .get(MangaId(id))
        .await
        .map_err(|e| not_found(e, "series"))?;
    Ok(Json(manga.into()))
}

/// `GET /api/v1/manga/{id}/chapters`
#[utoipa::path(
    operation_id = "listMangaChapters",
    get,
    path = "/manga/{id}/chapters",
    tag = "library",
    params(("id" = Uuid, Path, description = "Series id"), Pagination),
    responses(
        (status = OK, body = Paged<ChapterSummaryDto>),
        (status = NOT_FOUND, description = "No such series"),
    ),
)]
pub async fn chapters(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(page): Query<Pagination>,
) -> ApiResult<Json<Paged<ChapterSummaryDto>>> {
    // Checked first so a client asking about a series that does not exist gets
    // a 404 rather than an empty page, which reads as "no chapters yet".
    state
        .manga
        .get(MangaId(id))
        .await
        .map_err(|e| not_found(e, "series"))?;

    Ok(Json(
        state
            .chapters
            .list_summaries_for_manga(MangaId(id), page.cursor().as_ref())
            .await?
            .into(),
    ))
}

/// `GET /api/v1/chapters/{id}`
#[utoipa::path(
    operation_id = "getChapter",
    get,
    path = "/chapters/{id}",
    tag = "library",
    params(("id" = Uuid, Path, description = "Chapter id")),
    responses(
        (status = OK, body = ChapterDto),
        (status = NOT_FOUND, description = "No such chapter"),
    ),
)]
pub async fn chapter(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ChapterDto>> {
    let chapter = state
        .chapters
        .get(ChapterId(id))
        .await
        .map_err(|e| not_found(e, "chapter"))?;
    Ok(Json(chapter.into()))
}

/// Names what was not found.
///
/// `DomainError::NotFound` carries nothing, so the generic mapping produces
/// "No resource matches that identifier" — true but useless when a request
/// touches two of them.
pub(crate) fn not_found(error: nyuka_domain::DomainError, what: &str) -> ApiError {
    match error {
        nyuka_domain::DomainError::NotFound => ApiError(Box::new(Problem::not_found(what))),
        other => ApiError::from(other),
    }
}
