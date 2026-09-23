//! `/api/v1/follows` — series watched for new chapters.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::{Follow, FollowId, JobKind, MangaId};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{FollowDto, FollowSummaryDto, Paged, Pagination};
use crate::routes::library::not_found;
use crate::state::AppState;

/// The shortest interval a follow may use.
///
/// Not a matter of taste: a follow is a repeated request to someone else's
/// site, and a five-minute floor is what keeps an enthusiastic user from
/// turning their library into a crawler that gets the deployment banned
/// (ADR-0004).
pub const MIN_CHECK_INTERVAL_SECS: i32 = 300;

/// A month. Beyond this the follow is indistinguishable from not following.
pub const MAX_CHECK_INTERVAL_SECS: i32 = 60 * 60 * 24 * 31;

#[derive(Debug, Deserialize, ToSchema)]
pub struct FollowRequest {
    pub manga_id: Uuid,
    /// Defaults to six hours when absent.
    pub check_interval_secs: Option<i32>,
    #[serde(default)]
    pub auto_download: bool,
}

pub const DEFAULT_CHECK_INTERVAL_SECS: i32 = 6 * 60 * 60;

/// Validates an interval, naming the field so a form can attach the message.
fn validate_interval(secs: i32) -> Result<i32, ApiError> {
    if !(MIN_CHECK_INTERVAL_SECS..=MAX_CHECK_INTERVAL_SECS).contains(&secs) {
        return Err(ApiError(Box::new(
            Problem::invalid(format!(
                "check_interval_secs must be between {MIN_CHECK_INTERVAL_SECS} and \
                 {MAX_CHECK_INTERVAL_SECS} seconds"
            ))
            .with_errors(vec![crate::error::FieldError {
                field: "/check_interval_secs".into(),
                message: format!(
                    "Must be at least {MIN_CHECK_INTERVAL_SECS} seconds, so a library does \
                     not become a crawler."
                ),
            }]),
        )));
    }
    Ok(secs)
}

/// `GET /api/v1/follows`
#[utoipa::path(
    operation_id = "listFollows",
    get,
    path = "/follows",
    tag = "follows",
    params(Pagination),
    responses((status = OK, body = Paged<FollowSummaryDto>)),
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Pagination>,
) -> ApiResult<Json<Paged<FollowSummaryDto>>> {
    Ok(Json(
        state
            .follows
            .list_summaries(page.cursor().as_ref())
            .await?
            .into(),
    ))
}

/// `GET /api/v1/follows/{id}`
#[utoipa::path(
    operation_id = "getFollow",
    get,
    path = "/follows/{id}",
    tag = "follows",
    params(("id" = Uuid, Path, description = "Follow id")),
    responses(
        (status = OK, body = FollowDto),
        (status = NOT_FOUND, description = "No such follow"),
    ),
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<FollowDto>> {
    let follow = state
        .follows
        .get(FollowId(id))
        .await
        .map_err(|e| not_found(e, "follow"))?;
    Ok(Json(follow.into()))
}

/// `PUT /api/v1/follows` — follow a series, or change how it is followed.
///
/// `PUT` rather than `POST` because the operation is keyed on `manga_id` and
/// is idempotent: following an already-followed series updates it instead of
/// creating a second follow, which is what a user pressing the button twice
/// expects.
#[utoipa::path(
    operation_id = "upsertFollow",
    put,
    path = "/follows",
    tag = "follows",
    request_body = FollowRequest,
    responses(
        (status = OK, body = FollowDto),
        (status = BAD_REQUEST, description = "Invalid interval"),
        (status = NOT_FOUND, description = "No such series"),
    ),
)]
pub async fn upsert(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FollowRequest>,
) -> ApiResult<Json<FollowDto>> {
    let interval = validate_interval(
        body.check_interval_secs
            .unwrap_or(DEFAULT_CHECK_INTERVAL_SECS),
    )?;

    // A follow's foreign key would reject this anyway, but as a constraint
    // violation rather than a 404 naming the series.
    state
        .manga
        .get(MangaId(body.manga_id))
        .await
        .map_err(|e| not_found(e, "series"))?;

    let id = state
        .follows
        .upsert(&Follow {
            // Ignored on insert and on conflict; the repository assigns or
            // keeps one.
            id: FollowId(Uuid::nil()),
            manga_id: MangaId(body.manga_id),
            check_interval_secs: interval,
            // Never reset by an update: doing so would make editing a follow
            // trigger an immediate check, and editing several would send a
            // burst at the source.
            last_checked_at: None,
            auto_download: body.auto_download,
            created_at: chrono::Utc::now(),
        })
        .await?;

    Ok(Json(state.follows.get(id).await?.into()))
}

/// `DELETE /api/v1/follows/{id}`
#[utoipa::path(
    operation_id = "deleteFollow",
    delete,
    path = "/follows/{id}",
    tag = "follows",
    params(("id" = Uuid, Path, description = "Follow id")),
    responses((status = NO_CONTENT, description = "Removed, or already absent")),
)]
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    // Already-absent is the desired state, so this does not 404: a client
    // retrying a delete it already made should not see a failure.
    state.follows.delete(FollowId(id)).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/follows/{id}/check-now`
#[utoipa::path(
    operation_id = "checkFollowNow",
    post,
    path = "/follows/{id}/check-now",
    tag = "follows",
    params(("id" = Uuid, Path, description = "Follow id")),
    responses(
        (status = ACCEPTED, description = "A check was queued"),
        (status = NOT_FOUND, description = "No such follow"),
    ),
)]
pub async fn check_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let follow = state
        .follows
        .get(FollowId(id))
        .await
        .map_err(|e| not_found(e, "follow"))?;

    // Keyed on the minute, so a user pressing the button repeatedly queues one
    // check rather than one per press — the scheduler's own key is bucketed by
    // the follow's interval and would not suppress a manual request.
    let bucket = chrono::Utc::now().timestamp() / 60;
    let job = state
        .queue
        .enqueue(
            JobKind::CheckFollow,
            serde_json::json!({ "follow_id": follow.id.0 }),
            None,
            Some(&format!("check_follow:manual:{}:{bucket}", follow.id.0)),
            // Above the scheduler's sweep: someone is waiting for this one.
            0,
            3,
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        [(
            axum::http::header::LOCATION,
            format!("/api/v1/jobs/{}", job.0),
        )],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interval_below_the_floor_is_refused_with_a_field_error() {
        let error = validate_interval(60).expect_err("below the floor");
        assert_eq!(error.problem().status, 400);
        assert_eq!(
            error.problem().errors.first().map(|e| e.field.as_str()),
            Some("/check_interval_secs"),
            "a form needs the pointer to attach the message to an input"
        );
    }

    #[test]
    fn an_absurdly_long_interval_is_refused() {
        assert!(validate_interval(MAX_CHECK_INTERVAL_SECS + 1).is_err());
    }

    #[test]
    fn the_boundaries_themselves_are_accepted() {
        assert!(validate_interval(MIN_CHECK_INTERVAL_SECS).is_ok());
        assert!(validate_interval(MAX_CHECK_INTERVAL_SECS).is_ok());
    }

    /// The floor exists so a library does not become a crawler.
    #[test]
    fn the_floor_is_five_minutes() {
        assert_eq!(MIN_CHECK_INTERVAL_SECS, 300);
        const { assert!(DEFAULT_CHECK_INTERVAL_SECS > MIN_CHECK_INTERVAL_SECS) };
    }
}
