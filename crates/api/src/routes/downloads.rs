//! `/api/v1/downloads` — requesting a chapter, and reading the file.
//!
//! # The file endpoint is why sessions are cookies
//!
//! A browser reaches `GET /downloads/{id}/file` by navigation or an anchor
//! download, and neither can set a header. That is one of the three call
//! shapes ADR-0005 chose cookie sessions for.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nyuka_domain::model::{ChapterId, JobKind};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult, Problem};
use crate::routes::library::not_found;
use crate::state::AppState;

/// How much of a file one range request may ask for.
///
/// A `Range` header is attacker-controlled, and the response is buffered in
/// memory before it is sent. Without a ceiling, one request for
/// `bytes=0-999999999999` is an allocation request.
pub const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize, ToSchema)]
pub struct DownloadRequest {
    pub chapter_id: Uuid,
}

/// `POST /api/v1/downloads`
///
/// Returns `202` with a `Location` pointing at the job. The work is a series
/// of requests to a third-party site; doing it inline would hold a connection
/// open for minutes.
///
/// Idempotent on the chapter, so a double-click queues one download. An
/// `Idempotency-Key` header is accepted and narrows that further, which is
/// what makes a retried request from a flaky network safe.
#[utoipa::path(
    operation_id = "requestDownload",
    post,
    path = "/downloads",
    tag = "downloads",
    request_body = DownloadRequest,
    responses(
        (status = ACCEPTED, description = "Queued; `Location` names the job"),
        (status = NOT_FOUND, description = "No such chapter"),
        (status = CONFLICT, description = "Already in the library"),
    ),
)]
pub async fn request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<DownloadRequest>,
) -> ApiResult<Response> {
    let chapter_id = ChapterId(body.chapter_id);

    let chapter = state
        .chapters
        .get(chapter_id)
        .await
        .map_err(|e| not_found(e, "chapter"))?;

    // Checked against the filesystem, not just the row: a row whose file has
    // gone must be downloadable again, or a library restored from an empty
    // volume could never be repaired.
    if let Some(existing) = state.chapters.downloaded(chapter_id).await?
        && state.library.exists(&existing.relative_path).await?
    {
        return Err(ApiError(Box::new(Problem::conflict(
            "This chapter is already in the library.",
        ))));
    }

    // The client's key when it sent one, the chapter otherwise. Keying on the
    // chapter alone is what makes a double-click safe; the header is what
    // makes a retry over a flaky network safe, because the second attempt
    // carries the same key as the first.
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty() && k.len() <= 200)
        .map(|k| format!("download:{}:{k}", chapter.id.0))
        .unwrap_or_else(|| format!("download:{}", chapter.id.0));

    let job = state
        .queue
        .enqueue(
            JobKind::DownloadChapter,
            serde_json::json!({ "chapter_id": chapter.id.0 }),
            None,
            Some(&key),
            // Ahead of a follow sweep's downloads: someone is waiting.
            0,
            5,
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, format!("/api/v1/jobs/{}", job.0))],
    )
        .into_response())
}

/// `GET /api/v1/downloads/{chapter_id}/file`
#[utoipa::path(
    operation_id = "getChapterFile",
    get,
    path = "/downloads/{chapter_id}/file",
    tag = "downloads",
    params(("chapter_id" = Uuid, Path, description = "Chapter id")),
    responses(
        (status = OK, description = "The CBZ", content_type = "application/vnd.comicbook+zip"),
        (status = PARTIAL_CONTENT, description = "The requested range"),
        (status = NOT_MODIFIED, description = "The ETag matched"),
        (status = NOT_FOUND, description = "Not downloaded"),
        (status = RANGE_NOT_SATISFIABLE, description = "The range is outside the file"),
    ),
)]
pub async fn file(
    State(state): State<Arc<AppState>>,
    Path(chapter_id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let download = state
        .chapters
        .downloaded(ChapterId(chapter_id))
        .await?
        .ok_or_else(|| ApiError(Box::new(Problem::not_found("downloaded chapter"))))?;

    // The checksum was computed once during the write. Recomputing it per
    // request would read the whole archive to answer a conditional request
    // whose entire purpose is to avoid reading it.
    let etag = format!("\"{}\"", download.checksum);

    if matches_etag(&headers, &etag) {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }

    let handle = state
        .library
        .open_chapter(&download.relative_path)
        .await
        .map_err(|e| not_found(e, "downloaded chapter"))?;
    let size = handle.size();

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.comicbook+zip"),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    // A packaged chapter never changes: the archive is written once and placed
    // atomically. `immutable` is honest here in a way it rarely is.
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );

    let Some(range) = headers.get(header::RANGE) else {
        // The whole file is read into memory. That is the same trade the
        // packaging side already makes — `write_chapter` takes page bytes in
        // memory — and a reader asking for a whole chapter is asking for
        // exactly what was written. A client wanting to bound it sends a
        // `Range`, which `MAX_RANGE_BYTES` then caps.
        let bytes = handle.read_at(0, size as usize).await?;
        return Ok((StatusCode::OK, response_headers, Body::from(bytes)).into_response());
    };

    let requested = range.to_str().ok().and_then(|r| parse_range(r, size));
    let Some((start, end)) = requested else {
        // RFC 9110: an unsatisfiable range gets 416 and a `Content-Range`
        // naming the real size, so the client can ask again correctly.
        return Ok((
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}"))
                    .unwrap_or(HeaderValue::from_static("bytes */0")),
            )],
        )
            .into_response());
    };

    let length = end - start + 1;
    let bytes = handle.read_at(start, length as usize).await?;

    response_headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
            .unwrap_or(HeaderValue::from_static("bytes */0")),
    );

    Ok((
        StatusCode::PARTIAL_CONTENT,
        response_headers,
        Body::from(bytes),
    )
        .into_response())
}

/// Whether `If-None-Match` names this entity.
///
/// `*` matches any existing entity, per RFC 9110. A weak comparison is used
/// because that is what the specification requires for `If-None-Match`, and
/// the tag is strong anyway.
fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if value.trim() == "*" {
        return true;
    }
    value
        .split(',')
        .map(str::trim)
        .map(|candidate| candidate.strip_prefix("W/").unwrap_or(candidate))
        .any(|candidate| candidate == etag)
}

/// Parses a single-range `Range` header, clamped to what the server will send.
///
/// Returns the inclusive byte positions, or `None` when the range cannot be
/// satisfied. Multi-range requests are deliberately not supported: they need a
/// multipart response, no reader asks for one, and the parsing surface is
/// bigger than the feature.
pub fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }

    let (start_text, end_text) = spec.split_once('-')?;
    let (start, end) = match (start_text.trim(), end_text.trim()) {
        // `bytes=-500`: the final 500 bytes.
        ("", suffix) => {
            let length: u64 = suffix.parse().ok()?;
            if length == 0 {
                return None;
            }
            (size.saturating_sub(length), size - 1)
        }
        // `bytes=500-`: from 500 to the end.
        (start, "") => {
            let start: u64 = start.parse().ok()?;
            (start, size - 1)
        }
        (start, end) => {
            let start: u64 = start.parse().ok()?;
            let end: u64 = end.parse().ok()?;
            (start, end.min(size - 1))
        }
    };

    if start > end || start >= size {
        return None;
    }

    // Clamped rather than refused: a client asking for more than the server
    // will send in one response should get a valid partial response and ask
    // again, not an error. The ceiling exists because the body is buffered,
    // and `Range` is attacker-controlled.
    let end = end.min(start + MAX_RANGE_BYTES - 1);
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u64 = 1000;

    #[test]
    fn a_closed_range_is_parsed_inclusively() {
        assert_eq!(parse_range("bytes=0-99", SIZE), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-199", SIZE), Some((100, 199)));
    }

    #[test]
    fn an_open_ended_range_runs_to_the_last_byte() {
        assert_eq!(parse_range("bytes=900-", SIZE), Some((900, 999)));
    }

    #[test]
    fn a_suffix_range_counts_back_from_the_end() {
        assert_eq!(parse_range("bytes=-100", SIZE), Some((900, 999)));
        assert_eq!(
            parse_range("bytes=-5000", SIZE),
            Some((0, 999)),
            "a suffix longer than the file is the whole file, not an error"
        );
    }

    #[test]
    fn an_end_past_the_file_is_clamped_to_the_last_byte() {
        assert_eq!(parse_range("bytes=900-99999", SIZE), Some((900, 999)));
    }

    /// `Range` is attacker-controlled and the body is buffered, so an
    /// unbounded range is an allocation request.
    #[test]
    fn a_range_larger_than_the_ceiling_is_clamped() {
        let huge = MAX_RANGE_BYTES * 4;
        let (start, end) = parse_range("bytes=0-", huge).expect("satisfiable");
        assert_eq!(start, 0);
        assert_eq!(
            end - start + 1,
            MAX_RANGE_BYTES,
            "one response must never carry more than the ceiling"
        );
    }

    #[test]
    fn an_unsatisfiable_range_is_refused() {
        assert_eq!(parse_range("bytes=1000-1100", SIZE), None, "past the end");
        assert_eq!(parse_range("bytes=500-100", SIZE), None, "reversed");
        assert_eq!(parse_range("bytes=-0", SIZE), None, "empty suffix");
        assert_eq!(parse_range("bytes=0-99", 0), None, "empty file");
    }

    #[test]
    fn a_malformed_range_is_refused_rather_than_guessed() {
        for header in [
            "items=0-99",
            "bytes=abc-def",
            "bytes=",
            "0-99",
            "bytes=0-99, 200-299",
        ] {
            assert_eq!(parse_range(header, SIZE), None, "{header} must be refused");
        }
    }

    fn headers_with(name: header::HeaderName, value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(name, HeaderValue::from_str(value).expect("header"));
        map
    }

    #[test]
    fn a_matching_etag_is_recognised() {
        let headers = headers_with(header::IF_NONE_MATCH, "\"abc123\"");
        assert!(matches_etag(&headers, "\"abc123\""));
    }

    #[test]
    fn a_different_etag_is_not() {
        let headers = headers_with(header::IF_NONE_MATCH, "\"other\"");
        assert!(!matches_etag(&headers, "\"abc123\""));
    }

    /// RFC 9110: `*` matches any existing entity.
    #[test]
    fn a_wildcard_matches() {
        assert!(matches_etag(
            &headers_with(header::IF_NONE_MATCH, "*"),
            "\"abc123\""
        ));
    }

    #[test]
    fn a_list_of_candidates_is_searched() {
        let headers = headers_with(header::IF_NONE_MATCH, "\"one\", \"abc123\", \"three\"");
        assert!(matches_etag(&headers, "\"abc123\""));
    }

    /// `If-None-Match` uses weak comparison, so `W/"x"` matches `"x"`.
    #[test]
    fn a_weak_tag_matches_its_strong_form() {
        let headers = headers_with(header::IF_NONE_MATCH, "W/\"abc123\"");
        assert!(matches_etag(&headers, "\"abc123\""));
    }

    #[test]
    fn no_header_never_matches() {
        assert!(!matches_etag(&HeaderMap::new(), "\"abc123\""));
    }
}
