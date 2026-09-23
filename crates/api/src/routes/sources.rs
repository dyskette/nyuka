//! `/api/v1/source-repos` and `/api/v1/sources`.
//!
//! A repository is an index URL an operator adds; a source is one `.aix`
//! package installed from it. The two are separate resources because they fail
//! separately: a repository can be unreachable while every source installed
//! from it keeps working.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::{ExternalKey, JobKind, SourceId, SourceRepo, SourceRepoId};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::dto::{InstalledSourceDto, SourceEntryDto, SourceRepoDto};
use crate::routes::library::not_found;
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourceRepoRequest {
    pub name: String,
    /// The index URL, such as `https://example.test/index.min.json`.
    pub url: String,
}

/// Rejects a repository URL that is not an absolute `http(s)` URL.
///
/// The scheme check is not decoration. `file://` would make the registry read
/// the local filesystem, and the egress policy — which only sees hosts — has
/// nothing to say about a URL that never resolves one.
fn validate_repo_url(url: &str) -> Result<(), ApiError> {
    let parsed = url::Url::parse(url).map_err(|_| field_error("/url", "Must be a URL."))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(field_error("/url", "Must be an http or https URL."));
    }

    // No separate host check: `url` refuses an empty host for `http` and
    // `https` at parse time, so anything reaching here has one. A URL like
    // `https:///index.json` is not hostless — it parses as the host
    // `index.json`, which is a real if useless name that fails at resolution.
    Ok(())
}

fn field_error(field: &str, message: &str) -> ApiError {
    ApiError(Box::new(Problem::invalid(message.to_string()).with_errors(
        vec![crate::error::FieldError {
            field: field.into(),
            message: message.into(),
        }],
    )))
}

/// `GET /api/v1/source-repos`
#[utoipa::path(
    get,
    path = "/source-repos",
    tag = "sources",
    responses((status = OK, body = Vec<SourceRepoDto>)),
)]
pub async fn list_repos(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<SourceRepoDto>>> {
    // Not paginated: the count is bounded by what an operator added.
    Ok(Json(
        state
            .sources
            .list_repos()
            .await?
            .into_iter()
            .map(SourceRepoDto::from)
            .collect(),
    ))
}

/// `POST /api/v1/source-repos`
#[utoipa::path(
    post,
    path = "/source-repos",
    tag = "sources",
    request_body = SourceRepoRequest,
    responses(
        (status = OK, body = SourceRepoDto),
        (status = BAD_REQUEST, description = "Invalid URL"),
    ),
)]
pub async fn add_repo(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SourceRepoRequest>,
) -> ApiResult<Json<SourceRepoDto>> {
    validate_repo_url(&body.url)?;

    let id = state
        .sources
        .upsert_repo(&SourceRepo {
            id: SourceRepoId(Uuid::nil()),
            name: body.name,
            url: body.url,
            // Not set here: adding the same URL twice must not make an index
            // that was never fetched look current.
            last_refreshed_at: None,
            created_at: chrono::Utc::now(),
        })
        .await?;

    Ok(Json(state.sources.get_repo(id).await?.into()))
}

/// `DELETE /api/v1/source-repos/{id}`
#[utoipa::path(
    delete,
    path = "/source-repos/{id}",
    tag = "sources",
    params(("id" = Uuid, Path, description = "Repository id")),
    responses((status = NO_CONTENT, description = "Removed, or already absent")),
)]
pub async fn delete_repo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    // Cascades to the repository's catalog entries. Sources already installed
    // from it keep working: they are compiled from a package this server
    // already holds, and the index is only needed to install or upgrade.
    state.sources.remove_repo(SourceRepoId(id)).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/source-repos/{id}/refresh`
#[utoipa::path(
    post,
    path = "/source-repos/{id}/refresh",
    tag = "sources",
    params(("id" = Uuid, Path, description = "Repository id")),
    responses(
        (status = ACCEPTED, description = "A refresh was queued"),
        (status = NOT_FOUND, description = "No such repository"),
    ),
)]
pub async fn refresh_repo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    state
        .sources
        .get_repo(SourceRepoId(id))
        .await
        .map_err(|e| not_found(e, "source repository"))?;

    // Queued rather than done inline: fetching an index is a network call to
    // someone else's server, and a request that blocks on one ties up a
    // connection for as long as they feel like taking.
    //
    // `update_sources` refreshes every repository. Keyed on the minute so a
    // double-click queues one job.
    let bucket = chrono::Utc::now().timestamp() / 60;
    let job = state
        .queue
        .enqueue(
            JobKind::UpdateSources,
            serde_json::json!({ "repo_id": id }),
            None,
            Some(&format!("update_sources:manual:{bucket}")),
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

/// `GET /api/v1/source-repos/{id}/available`
#[utoipa::path(
    get,
    path = "/source-repos/{id}/available",
    tag = "sources",
    params(("id" = Uuid, Path, description = "Repository id")),
    responses(
        (status = OK, body = Vec<SourceEntryDto>),
        (status = NOT_FOUND, description = "No such repository"),
    ),
)]
pub async fn available(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<SourceEntryDto>>> {
    state
        .sources
        .get_repo(SourceRepoId(id))
        .await
        .map_err(|e| not_found(e, "source repository"))?;

    let installed = state.sources.list().await?;
    let entries = state.sources.list_entries(SourceRepoId(id)).await?;

    Ok(Json(
        entries
            .into_iter()
            .map(|entry| {
                // Marked here rather than in the `From` impl, which has no way
                // to see the installed list. Without it the UI cannot tell
                // "install" from "upgrade".
                let installed_version = installed
                    .iter()
                    .find(|s| s.repo_id == entry.repo_id && s.external_id == entry.external_id)
                    .map(|s| s.version);
                SourceEntryDto {
                    installed_version,
                    ..SourceEntryDto::from(entry)
                }
            })
            .collect(),
    ))
}

/// `GET /api/v1/sources`
#[utoipa::path(
    get,
    path = "/sources",
    tag = "sources",
    responses((status = OK, body = Vec<InstalledSourceDto>)),
)]
pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<InstalledSourceDto>>> {
    Ok(Json(
        state
            .sources
            .list()
            .await?
            .into_iter()
            .map(InstalledSourceDto::from)
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InstallRequest {
    pub repo_id: Uuid,
    /// The source's own id, as the repository index lists it.
    pub external_id: String,
}

/// `POST /api/v1/sources` — install from a repository entry.
///
/// Synchronous rather than a job. The client is waiting to use the source, the
/// work is one download and one compile, and the capability refusal has to
/// reach the person who pressed install — a failed job they have to go and
/// read reports a host gap as though it were a site problem, which ADR-0004
/// names as the worst outcome.
#[utoipa::path(
    post,
    path = "/sources",
    tag = "sources",
    request_body = InstallRequest,
    responses(
        (status = OK, body = InstalledSourceDto),
        (status = NOT_FOUND, description = "No such repository or entry"),
        (status = NOT_IMPLEMENTED, description = "The source needs a capability this build lacks"),
        (status = BAD_GATEWAY, description = "The package could not be fetched"),
    ),
)]
pub async fn install(
    State(state): State<Arc<AppState>>,
    Json(body): Json<InstallRequest>,
) -> ApiResult<Json<InstalledSourceDto>> {
    let installed = state
        .registry
        .install(SourceRepoId(body.repo_id), &ExternalKey(body.external_id))
        .await
        .map_err(|e| not_found(e, "source"))?;

    // Registered now rather than at the next restart, or the first download
    // from a freshly installed source ignores the limit it declared.
    state
        .limiter
        .register(installed.id, installed.declared_rate_limit)
        .await;

    Ok(Json(installed.into()))
}

/// `DELETE /api/v1/sources/{id}`
///
/// > [!WARNING]
/// > This cascades. Every series from this source and everything hanging off
/// > them is removed from the database. Packaged files in the library are left
/// > alone — they are the user's — but nothing will know how to find them
/// > again.
#[utoipa::path(
    delete,
    path = "/sources/{id}",
    tag = "sources",
    params(("id" = Uuid, Path, description = "Source id")),
    responses((status = NO_CONTENT, description = "Removed, or already absent")),
)]
pub async fn uninstall(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    state.registry.uninstall(SourceId(id)).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/v1/sources/{id}/filters`
#[utoipa::path(
    get,
    path = "/sources/{id}/filters",
    tag = "sources",
    params(("id" = Uuid, Path, description = "Source id")),
    responses(
        (status = OK, description = "The source's filter definitions", body = serde_json::Value),
        (status = NOT_FOUND, description = "No such source"),
    ),
)]
pub async fn filters(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let filters = state
        .catalog
        .filters(SourceId(id))
        .await
        .map_err(|e| not_found(e, "source"))?;
    Ok(Json(filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_http_repository_url_is_accepted() {
        for url in [
            "https://example.test/index.min.json",
            "http://localhost:8080/index.json",
        ] {
            assert!(validate_repo_url(url).is_ok(), "{url} should be accepted");
        }
    }

    /// `file://` would make the registry read the local filesystem, and the
    /// egress policy only sees hosts — it has nothing to say about a URL that
    /// never resolves one.
    #[test]
    fn a_non_http_scheme_is_refused() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.test/index.json",
            "data:application/json,{}",
            "javascript:alert(1)",
        ] {
            let error = validate_repo_url(url).expect_err("{url} must be refused");
            assert_eq!(error.problem().status, 400);
        }
    }

    /// Documented because it looks like a bug and is not: the extra slash
    /// does not make the URL hostless, it makes `index.json` the host. The
    /// fetch then fails at resolution, which is the right place for it.
    #[test]
    fn an_odd_looking_url_that_still_names_a_host_is_accepted() {
        assert!(validate_repo_url("https:///index.json").is_ok());
        assert_eq!(
            url::Url::parse("https:///index.json")
                .expect("parses")
                .host_str(),
            Some("index.json")
        );
    }

    /// And the genuinely hostless case, which `url` refuses before this
    /// function sees it.
    #[test]
    fn an_empty_host_does_not_parse_at_all() {
        assert!(url::Url::parse("https://").is_err());
        assert!(validate_repo_url("https://").is_err());
    }

    #[test]
    fn a_relative_url_is_refused() {
        assert!(validate_repo_url("/index.json").is_err());
        assert!(validate_repo_url("not a url").is_err());
    }

    /// A form needs the pointer to attach the message to an input.
    #[test]
    fn a_rejected_url_names_the_field() {
        let error = validate_repo_url("file:///etc/passwd").expect_err("refused");
        assert_eq!(
            error.problem().errors.first().map(|e| e.field.as_str()),
            Some("/url")
        );
    }
}
