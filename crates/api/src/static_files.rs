//! The embedded SPA (ADR-0006).
//!
//! `rust-embed` over `web/dist`, served from the same origin as the API.
//!
//! Route precedence is load-bearing: `/api/v1/*` matches first, then exact
//! asset paths, then an HTML fallback for client-side routing. **An unmatched
//! path under `/api/v1/` must return problem+json `404`, never `index.html`**
//! — otherwise a typo in a fetch surfaces as an HTML parse error instead of a
//! clear 404. ADR-0006 requires a test for both directions.
//!
//! Caching: content-hashed assets get
//! `Cache-Control: public, max-age=31536000, immutable`; `index.html` gets
//! `no-cache`. Getting these backwards produces a stale UI that survives a
//! redeploy, so that is also a test.
//!
//! # What "content-hashed" means here, and why it is not assumed
//!
//! `immutable` is a promise that a URL's bytes will never change. It is true
//! for `index-CzojFC5Q.js` and false for `index.html`, and getting it wrong
//! in the second direction bricks a deployment for a year of browser cache.
//! So the decision is made by [`is_content_hashed`] from the filename shape
//! Vite actually emits, rather than by assuming everything under `assets/` is
//! safe — a future config could put an unhashed file there.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// The built frontend.
///
/// `build.rs` creates a placeholder `index.html` when the directory is absent,
/// so the backend builds and tests without a frontend toolchain present. A
/// real deployment embeds a real build.
#[derive(Embed)]
// Relative to the crate manifest, which is what `rust-embed` resolves
// against. `$CARGO_MANIFEST_DIR` would need the `interpolate-folder-path`
// feature for no gain here.
#[folder = "../../web/dist"]
pub struct Assets;

/// How long a content-hashed asset may be cached.
pub const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";

/// `index.html` is revalidated every time, because its URL never changes and
/// its contents change on every deploy.
pub const HTML_CACHE: &str = "no-cache";

/// The length of the hash Vite appends.
const HASH_LEN: usize = 8;

/// Whether a path is safe to serve as `immutable`.
///
/// Two conditions, both required:
///
/// 1. The file is under `assets/`, which is where Vite puts hashed output.
/// 2. The stem ends in `-` followed by exactly [`HASH_LEN`] base64url
///    characters.
///
/// The second cannot be done by splitting on the last hyphen: a base64url
/// hash may itself contain one — `library-Bf_lit-9.js` is a real filename
/// from this project's own build, and a naive `rsplit_once('-')` reads its
/// hash as `9`. That mistake fails safe (the file is merely revalidated), but
/// it would have quietly disabled caching for a share of every deploy's
/// assets, which is the kind of thing nobody notices.
///
/// Requiring both conditions is deliberate belt and braces: a config change
/// could put an unhashed file under `assets/`, and `immutable` on a URL whose
/// contents change is a deployment bricked for a year of browser cache.
pub fn is_content_hashed(path: &str) -> bool {
    if !path.starts_with("assets/") {
        return false;
    }

    let Some(stem) = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
    else {
        return false;
    };

    // `-` plus the hash, so the stem must be longer than that to have a name.
    if stem.len() <= HASH_LEN + 1 {
        return false;
    }

    let (name, suffix) = stem.split_at(stem.len() - HASH_LEN - 1);
    if !name.is_empty() && !suffix.starts_with('-') {
        return false;
    }

    let hash = &suffix[1..];
    hash.len() == HASH_LEN
        && hash
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Serves an embedded file, or the SPA shell.
///
/// Anything that is not an embedded file falls through to `index.html` with
/// `200`, because a client-side route is not a missing resource. That is only
/// safe because this handler is mounted as the router's fallback, *after*
/// `/api/v1`.
pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    // Nothing under the API prefix ever gets the shell, whatever routing did
    // to get here. The nested router has its own fallback, but `/api/v1/`
    // with a trailing slash reduces to an inner path of `/` and reaches this
    // handler instead — and the failure ADR-0006 warns about is precisely an
    // API path answering with HTML. A guard on the thing that must not happen
    // is worth more than a routing arrangement that has to stay correct.
    if is_api_path(uri.path()) {
        return crate::error::Problem::no_such_endpoint()
            .with_instance(uri.path().to_string())
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => respond(path, file, &headers),
        // A deep link such as `/library/123`. The router handles it once the
        // shell has loaded.
        None => match Assets::get("index.html") {
            Some(shell) => respond("index.html", shell, &headers),
            // The binary was built without a frontend. Saying so beats an
            // empty 404 that looks like a routing bug.
            None => (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "No frontend is embedded in this build.",
            )
                .into_response(),
        },
    }
}

fn respond(path: &str, file: rust_embed::EmbeddedFile, headers: &HeaderMap) -> Response {
    // `rust-embed` computes this at build time, so it costs nothing per
    // request and is stable across restarts — which a timestamp-derived tag
    // would not be, and which is what makes revalidation work at all.
    let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()));

    if let Some(candidate) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        && matches_etag(candidate, &etag)
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if is_content_hashed(path) {
        IMMUTABLE_CACHE
    } else {
        HTML_CACHE
    };

    let mut response = Response::new(Body::from(file.data.into_owned()));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, value);
    }
    response
}

fn matches_etag(candidate: &str, etag: &str) -> bool {
    candidate.trim() == "*"
        || candidate
            .split(',')
            .map(str::trim)
            .map(|c| c.strip_prefix("W/").unwrap_or(c))
            .any(|c| c == etag)
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a path belongs to the API rather than the SPA.
///
/// Matched on the prefix with a boundary, so `/api/v1beta/x` is not mistaken
/// for an API path and silently denied the shell.
pub fn is_api_path(path: &str) -> bool {
    path == "/api/v1" || path.starts_with("/api/v1/")
}

/// The problem+json `404` for an unmatched path under `/api/v1`.
///
/// Mounted as the API router's own fallback so it wins before the SPA shell.
/// Without it a mistyped fetch returns `index.html` with `200`, and the
/// client reports an HTML parse error rather than a missing endpoint —
/// ADR-0006 calls this the sharp edge of the whole arrangement.
pub async fn api_not_found(Path(rest): Path<String>) -> Response {
    crate::error::Problem::no_such_endpoint()
        .with_instance(format!("/api/v1/{rest}"))
        .into_response()
}

/// The same, for a path with no capture.
pub async fn api_not_found_bare(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> Response {
    // `OriginalUri`, not `Uri`: inside a nested router the latter has already
    // had `/api/v1` stripped, so `instance` would name a path the client
    // never asked for.
    crate::error::Problem::no_such_endpoint()
        .with_instance(uri.path().to_string())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The filenames Vite actually emitted into `web/dist`, including one
    /// whose hash contains a hyphen — which is what broke the first version
    /// of this check.
    #[test]
    fn vite_hashed_filenames_are_recognised() {
        for path in [
            "assets/index-CzojFC5Q.js",
            "assets/index-BGofoWnB.css",
            "assets/library-Bf_lit-9.js",
        ] {
            assert!(is_content_hashed(path), "{path} is safe to cache forever");
        }
    }

    /// `immutable` on a URL whose contents change is a deployment bricked for
    /// a year of browser cache.
    #[test]
    fn unhashed_filenames_are_not() {
        for path in [
            "index.html",
            "favicon.ico",
            "robots.txt",
            "assets/logo.svg",
            "assets/index.js",
            "",
        ] {
            assert!(
                !is_content_hashed(path),
                "{path} must be revalidated, not cached forever"
            );
        }
    }

    /// A hyphenated name that is not a hash must not be mistaken for one.
    #[test]
    fn a_hyphenated_word_is_not_a_hash() {
        for path in [
            "assets/service-worker.js",
            "assets/index-production.js",
            "assets/a-b.js",
        ] {
            assert!(!is_content_hashed(path), "{path} is not content-hashed");
        }
    }

    /// A hashed name outside `assets/` is still refused: the directory is the
    /// second half of the check, not a shortcut past it.
    #[test]
    fn a_hashed_name_outside_the_assets_directory_is_refused() {
        assert!(!is_content_hashed("index-CzojFC5Q.js"));
        assert!(!is_content_hashed("static/index-CzojFC5Q.js"));
    }

    #[test]
    fn api_paths_are_recognised_with_a_boundary() {
        assert!(is_api_path("/api/v1"));
        assert!(is_api_path("/api/v1/"));
        assert!(is_api_path("/api/v1/manga"));
        assert!(is_api_path("/api/v1/a/b/c"));
    }

    /// A prefix match without a boundary would deny the shell to a future
    /// client route that merely starts the same way.
    #[test]
    fn a_path_that_only_starts_like_the_api_prefix_is_not_one() {
        for path in [
            "/api/v1beta/x",
            "/api/v10/x",
            "/api/v1x",
            "/api",
            "/apix/v1",
        ] {
            assert!(!is_api_path(path), "{path} is not an API path");
        }
    }

    #[test]
    fn the_cache_directives_are_the_right_way_round() {
        assert!(IMMUTABLE_CACHE.contains("immutable"));
        assert!(IMMUTABLE_CACHE.contains("max-age=31536000"));
        assert_eq!(HTML_CACHE, "no-cache");
        assert!(
            !HTML_CACHE.contains("immutable"),
            "index.html's URL never changes and its contents change every deploy"
        );
    }

    #[test]
    fn etag_comparison_handles_lists_and_weak_tags() {
        assert!(matches_etag("\"abc\"", "\"abc\""));
        assert!(matches_etag("W/\"abc\"", "\"abc\""));
        assert!(matches_etag("\"x\", \"abc\"", "\"abc\""));
        assert!(matches_etag("*", "\"abc\""));
        assert!(!matches_etag("\"other\"", "\"abc\""));
    }
}
