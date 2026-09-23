//! `/api/v1/manga` — the local library.
//!
//! Reads only. A series enters the library through the catalog (adding one
//! from a source) or through a follow check, never by a client posting a body
//! it composed: the source owns this metadata, and accepting a client's
//! version of it would let anyone overwrite a title with anything.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use nyuka_domain::model::{ChapterId, MangaId};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{ChapterDto, MangaDto, MangaSummaryDto, Paged, Pagination};
use crate::state::AppState;

/// `GET /api/v1/manga`
#[utoipa::path(
    operation_id = "listLibrary",
    get,
    path = "/manga",
    tag = "library",
    params(Pagination),
    responses((status = OK, body = Paged<MangaSummaryDto>)),
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Pagination>,
) -> ApiResult<Json<Paged<MangaSummaryDto>>> {
    // Summaries rather than bare series: the list view shows chapter and
    // download counts, and fetching those per row would be one query per
    // series on a page of fifty.
    Ok(Json(
        state
            .manga
            .list_summaries(page.cursor().as_ref())
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
        (status = OK, body = Paged<ChapterDto>),
        (status = NOT_FOUND, description = "No such series"),
    ),
)]
pub async fn chapters(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(page): Query<Pagination>,
) -> ApiResult<Json<Paged<ChapterDto>>> {
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
            .list_for_manga(MangaId(id), page.cursor().as_ref())
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
