//! `/api/v1/jobs` — the background work queue, as an operator sees it.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::{JobId, JobState};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{JobDto, Paged};
use crate::routes::library::not_found;
use crate::state::AppState;

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct JobQuery {
    /// One of `queued`, `running`, `succeeded`, `failed`, `cancelled`.
    pub state: Option<String>,
    pub cursor: Option<String>,
}

/// `GET /api/v1/jobs`
#[utoipa::path(
    get,
    path = "/jobs",
    tag = "jobs",
    params(JobQuery),
    responses(
        (status = OK, body = Paged<JobDto>),
        (status = BAD_REQUEST, description = "Unknown state filter"),
    ),
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<JobQuery>,
) -> ApiResult<Json<Paged<JobDto>>> {
    let filter = match query.state.as_deref() {
        None => None,
        // An unknown filter is refused rather than ignored: silently returning
        // every job for `state=suceeded` looks like the filter worked.
        Some(name) => Some(nyuka_jobs::queue::state_from_str(name).ok_or_else(|| {
            ApiError(Box::new(Problem::invalid(format!(
                "`{name}` is not a job state; expected one of queued, running, \
                 succeeded, failed, cancelled"
            ))))
        })?),
    };

    let cursor = query.cursor.map(nyuka_domain::model::Cursor);
    Ok(Json(
        state.queue.list(filter, cursor.as_ref()).await?.into(),
    ))
}

/// `GET /api/v1/jobs/{id}`
#[utoipa::path(
    get,
    path = "/jobs/{id}",
    tag = "jobs",
    params(("id" = Uuid, Path, description = "Job id")),
    responses(
        (status = OK, body = JobDto),
        (status = NOT_FOUND, description = "No such job"),
    ),
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<JobDto>> {
    let job = state
        .queue
        .get(JobId(id))
        .await
        .map_err(|e| not_found(e, "job"))?;
    Ok(Json(job.into()))
}

/// `POST /api/v1/jobs/{id}/cancel`
///
/// Cancelling a `running` job stops it from being retried; it does not
/// interrupt the attempt in flight. That is an honest limit rather than an
/// oversight — see ADR-0003's open question on cancellation semantics.
#[utoipa::path(
    post,
    path = "/jobs/{id}/cancel",
    tag = "jobs",
    params(("id" = Uuid, Path, description = "Job id")),
    responses(
        (status = OK, body = JobDto),
        (status = CONFLICT, description = "The job has already finished"),
        (status = NOT_FOUND, description = "No such job"),
    ),
)]
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<JobDto>> {
    let job = state
        .queue
        .get(JobId(id))
        .await
        .map_err(|e| not_found(e, "job"))?;

    if is_terminal(job.state) {
        return Err(ApiError(Box::new(Problem::conflict(format!(
            "This job is already {}, so there is nothing to cancel.",
            crate::routes::dto::state_name(job.state)
        )))));
    }

    state.queue.cancel(JobId(id)).await?;
    Ok(Json(state.queue.get(JobId(id)).await?.into()))
}

/// `POST /api/v1/jobs/{id}/retry`
#[utoipa::path(
    post,
    path = "/jobs/{id}/retry",
    tag = "jobs",
    params(("id" = Uuid, Path, description = "Job id")),
    responses(
        (status = ACCEPTED, description = "Queued again"),
        (status = CONFLICT, description = "The job has not finished"),
        (status = NOT_FOUND, description = "No such job"),
    ),
)]
pub async fn retry(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let job = state
        .queue
        .get(JobId(id))
        .await
        .map_err(|e| not_found(e, "job"))?;

    // Retrying a job that is still queued or running would either duplicate it
    // or reset the attempt counter under a worker's feet.
    if !is_terminal(job.state) {
        return Err(ApiError(Box::new(Problem::conflict(format!(
            "This job is {} and has not finished yet.",
            crate::routes::dto::state_name(job.state)
        )))));
    }

    // A fresh job rather than a reset of the old row, so the failed attempt
    // stays readable. An operator retrying something wants to compare, and a
    // reset erases what they were comparing against.
    let queued = state
        .queue
        .enqueue(
            job.kind,
            job.payload.clone(),
            None,
            // Keyed on the job being retried, so a double-click queues one.
            Some(&format!("retry:{}", job.id.0)),
            job.priority,
            job.max_attempts,
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        [(
            axum::http::header::LOCATION,
            format!("/api/v1/jobs/{}", queued.0),
        )],
    )
        .into_response())
}

/// Whether a job has reached a state it will not leave on its own.
fn is_terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Succeeded | JobState::Failed | JobState::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_exactly_the_ones_a_worker_will_not_move() {
        assert!(is_terminal(JobState::Succeeded));
        assert!(is_terminal(JobState::Failed));
        assert!(is_terminal(JobState::Cancelled));
        assert!(
            !is_terminal(JobState::Queued),
            "a queued job is still going to run"
        );
        assert!(
            !is_terminal(JobState::Running),
            "retrying a running job would reset the attempt counter under a worker"
        );
    }

    /// Silently returning everything for a typo looks like the filter worked.
    #[test]
    fn an_unknown_state_filter_is_not_a_valid_state() {
        assert!(nyuka_jobs::queue::state_from_str("suceeded").is_none());
        assert!(nyuka_jobs::queue::state_from_str("succeeded").is_some());
    }
}
