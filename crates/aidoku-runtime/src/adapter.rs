//! The `domain` provider ports, implemented over the WASM host.
//!
//! This is where the runtime stops being standalone. Everything above it sees
//! only `domain` traits and `domain` types; nothing of `wasmtime`, `postcard`,
//! or this crate's wire mirrors escapes.
//!
//! # Every call runs on `spawn_blocking`
//!
//! A source invocation is synchronous and CPU-bound, and an [`Invocation`]
//! holds `Rc` values so it is not `Send` — it cannot cross an `await` even if
//! we wanted it to. Both facts point the same way: the whole invocation
//! happens inside one blocking task, and only owned, `Send` values cross into
//! it (ADR-0004, ADR-0003).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use nyuka_domain::model::{
    Capability, ContentRating, Cursor, ExternalKey, InstalledSource, MangaStatus, Page,
    PageContent, ReadingDirection, SourceChapter, SourceId, SourceManga, SourcePage,
};
use nyuka_domain::ports::{SourceCatalog, SourceItem};
use nyuka_domain::{DomainError, Result};

use crate::engine::Runtime;
use crate::models;
use crate::package::{self, Package, capability_name};
use crate::source::{Invocation, RunError, invoke};
use crate::state::Defaults;

/// A source that has been installed and compiled.
pub struct LoadedSource {
    pub id: SourceId,
    pub manifest: package::Manifest,
    pub module: wasmtime::Module,
    pub required: Vec<Capability>,
    pub filters: Option<serde_json::Value>,
}

/// Holds compiled modules for the installed sources.
///
/// Compilation is expensive, so modules are cached here and re-instantiated
/// per call. The cache key includes the host ABI version, so a bump cannot
/// reuse a module compiled against the previous one.
pub struct SourceRuntime {
    runtime: Arc<Runtime>,
    sources: RwLock<HashMap<SourceId, Arc<LoadedSource>>>,
    defaults: Arc<dyn Defaults>,
}

impl SourceRuntime {
    pub fn new(runtime: Arc<Runtime>, defaults: Arc<dyn Defaults>) -> Self {
        Self {
            runtime,
            sources: RwLock::new(HashMap::new()),
            defaults,
        }
    }

    /// Drops a compiled module.
    ///
    /// Returning `Ok` when the source was not loaded is deliberate: uninstall
    /// must succeed after a restart, when nothing has been compiled yet but
    /// the database row is still there.
    pub fn remove(&self, id: SourceId) -> Result<()> {
        self.sources
            .write()
            .map_err(|_| DomainError::Internal("source registry poisoned".into()))?
            .remove(&id);
        Ok(())
    }

    /// Compiles and registers a package, refusing it if this host cannot run
    /// it.
    pub fn install(
        &self,
        id: SourceId,
        repo_id: nyuka_domain::model::SourceRepoId,
        bytes: &[u8],
    ) -> Result<InstalledSource> {
        let pkg: Package = package::load(bytes, &package::WasmtimeImports(&self.runtime))
            .map_err(map_load_error)?;
        let module = self
            .runtime
            .compile(&pkg.wasm)
            .map_err(|e| DomainError::Source {
                message: format!("compiling source module: {e}"),
                retryable: false,
            })?;

        let installed = InstalledSource {
            id,
            repo_id,
            external_id: nyuka_domain::model::ExternalKey(pkg.manifest.info.id.clone()),
            name: pkg.manifest.info.name.clone(),
            version: pkg.manifest.info.version,
            languages: pkg.manifest.info.languages.clone(),
            required_capabilities: pkg.required.clone(),
            // A rate limit is not in the manifest — a source declares it at
            // runtime through `net::set_rate_limit`, so it cannot be known
            // until the module first runs. The caller persists whatever the
            // module later declares; `None` here means "not yet declared",
            // not "unlimited". `SourceLimiter` applies the operator's cap
            // until one arrives.
            declared_rate_limit: None,
        };

        let loaded = Arc::new(LoadedSource {
            id,
            manifest: pkg.manifest,
            module,
            required: pkg.required,
            filters: pkg.filters,
        });
        self.sources
            .write()
            .map_err(|_| DomainError::Internal("source registry poisoned".into()))?
            .insert(id, loaded);
        Ok(installed)
    }

    fn get(&self, id: SourceId) -> Result<Arc<LoadedSource>> {
        self.sources
            .read()
            .map_err(|_| DomainError::Internal("source registry poisoned".into()))?
            .get(&id)
            .cloned()
            .ok_or(DomainError::NotFound)
    }

    /// Runs one invocation on a blocking thread.
    ///
    /// The closure receives a fresh [`Invocation`]; nothing from it escapes,
    /// which is what keeps the non-`Send` store inside the blocking task.
    async fn run<T, F>(&self, source: SourceId, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Invocation) -> std::result::Result<T, RunError> + Send + 'static,
    {
        let loaded = self.get(source)?;
        let runtime = self.runtime.clone();
        let defaults = self.defaults.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let mut invocation = invoke(&runtime, &loaded.module, defaults)?;
            f(&mut invocation)
        })
        .await
        .map_err(|e| DomainError::Internal(format!("source task failed: {e}")))?;

        joined.map_err(map_run_error)
    }
}

#[async_trait]
impl SourceCatalog for SourceRuntime {
    async fn list(
        &self,
        source: SourceId,
        query: Option<&str>,
        _filters: &serde_json::Value,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SourceManga>> {
        // Sources page by number, so the cursor carries one. A cursor that is
        // not a page number is a client error rather than a source failure.
        let page = match cursor {
            Some(Cursor(c)) => c
                .parse::<i32>()
                .map_err(|_| DomainError::Invalid(format!("cursor {c:?} is not a page number")))?,
            None => 1,
        };
        let query = query.map(str::to_owned);

        let result = self
            .run(source, move |inv| inv.search(query.as_deref(), page, &[]))
            .await?;

        Ok(Page {
            items: result.entries.into_iter().map(map_manga).collect(),
            next: result.has_next_page.then(|| Cursor((page + 1).to_string())),
        })
    }

    async fn filters(&self, source: SourceId) -> Result<serde_json::Value> {
        Ok(self
            .get(source)?
            .filters
            .clone()
            .unwrap_or(serde_json::Value::Array(Vec::new())))
    }
}

#[async_trait]
impl SourceItem for SourceRuntime {
    async fn details(&self, source: SourceId, key: &ExternalKey) -> Result<SourceManga> {
        let probe = probe_manga(key);
        let updated = self
            .run(source, move |inv| inv.manga_update(&probe, true, false))
            .await?;
        Ok(map_manga(updated))
    }

    async fn chapters(&self, source: SourceId, key: &ExternalKey) -> Result<Vec<SourceChapter>> {
        let probe = probe_manga(key);
        let updated = self
            .run(source, move |inv| inv.manga_update(&probe, false, true))
            .await?;
        Ok(updated
            .chapters
            .unwrap_or_default()
            .into_iter()
            .map(map_chapter)
            .collect())
    }

    async fn pages(
        &self,
        source: SourceId,
        manga: &ExternalKey,
        chapter: &ExternalKey,
    ) -> Result<Vec<SourcePage>> {
        let probe = probe_manga(manga);
        let chapter = models::Chapter {
            key: chapter.0.clone(),
            ..Default::default()
        };
        let pages = self
            .run(source, move |inv| inv.page_list(&probe, &chapter))
            .await?;
        Ok(pages
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| map_page(i as u32, p))
            .collect())
    }
}

/// The minimum a source needs to identify a series it is asked about.
fn probe_manga(key: &ExternalKey) -> models::Manga {
    models::Manga {
        key: key.0.clone(),
        ..Default::default()
    }
}

// --- wire types to domain types ---------------------------------------------
// These mappings are the boundary. Nothing above this module sees
// `crate::models`.

fn map_manga(m: models::Manga) -> SourceManga {
    SourceManga {
        key: ExternalKey(m.key),
        title: m.title,
        cover: m.cover,
        authors: m.authors.unwrap_or_default(),
        artists: m.artists.unwrap_or_default(),
        description: m.description,
        url: m.url,
        tags: m.tags.unwrap_or_default(),
        status: map_status(m.status),
        content_rating: map_rating(m.content_rating),
        direction: map_direction(m.viewer),
        chapters: m
            .chapters
            .map(|cs| cs.into_iter().map(map_chapter).collect()),
    }
}

fn map_chapter(c: models::Chapter) -> SourceChapter {
    SourceChapter {
        key: ExternalKey(c.key),
        title: c.title,
        number: c.chapter_number,
        volume: c.volume_number,
        published_at: c
            .date_uploaded
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
        scanlators: c.scanlators.unwrap_or_default(),
        url: c.url,
        language: c.language,
        locked: c.locked,
    }
}

/// Maps a page, dropping ones this host cannot serve.
///
/// A `PageContent::Image` arrives as a canvas resource, and this build does not
/// implement `canvas` — but a source needing it is refused at install time, so
/// reaching one here would mean the capability check was bypassed.
fn map_page(index: u32, p: models::Page) -> Option<SourcePage> {
    let content = match p.content {
        models::PageContent::Url(url, _context) => PageContent::Url {
            url,
            headers: Vec::new(),
        },
        models::PageContent::Text(text) => PageContent::Text(text),
        models::PageContent::Zip(archive, path) => PageContent::Zip { archive, path },
        models::PageContent::Image(_) => return None,
    };
    Some(SourcePage {
        index,
        content,
        description: p.description,
    })
}

fn map_status(s: models::MangaStatus) -> MangaStatus {
    match s {
        models::MangaStatus::Unknown => MangaStatus::Unknown,
        models::MangaStatus::Ongoing => MangaStatus::Ongoing,
        models::MangaStatus::Completed => MangaStatus::Completed,
        models::MangaStatus::Cancelled => MangaStatus::Cancelled,
        models::MangaStatus::Hiatus => MangaStatus::Hiatus,
    }
}

fn map_rating(r: models::ContentRating) -> ContentRating {
    match r {
        models::ContentRating::Unknown => ContentRating::Unknown,
        models::ContentRating::Safe => ContentRating::Safe,
        models::ContentRating::Suggestive => ContentRating::Suggestive,
        models::ContentRating::Nsfw => ContentRating::Nsfw,
    }
}

/// `Viewer` carries reading direction plus layout; the domain only needs the
/// direction, which is what reaches `ComicInfo.xml` (ADR-0007).
fn map_direction(v: models::Viewer) -> ReadingDirection {
    match v {
        models::Viewer::RightToLeft => ReadingDirection::RightToLeft,
        models::Viewer::LeftToRight | models::Viewer::Vertical | models::Viewer::Webtoon => {
            ReadingDirection::LeftToRight
        }
        models::Viewer::Unknown => ReadingDirection::Unknown,
    }
}

// --- errors -----------------------------------------------------------------

/// Maps a load failure, keeping the capability names so the API can report a
/// host gap rather than a generic error (ADR-0004).
fn map_load_error(e: package::LoadError) -> DomainError {
    match e {
        package::LoadError::UnsupportedCapabilities { missing, .. } => {
            DomainError::UnsupportedCapability(
                missing
                    .iter()
                    .map(capability_name)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }
        other => DomainError::Invalid(other.to_string()),
    }
}

/// Maps a run failure, deciding whether the job engine should retry.
///
/// A network failure inside the source is worth retrying; a source that
/// returned malformed data, or is missing an export, will fail the same way
/// every time and must not be retried (ADR-0003).
fn map_run_error(e: RunError) -> DomainError {
    match e {
        RunError::Guest(code) => DomainError::Source {
            message: format!("source returned error code {code}"),
            // -3 is the guest's request error: transient.
            retryable: code == crate::error::aidoku::REQUEST,
        },
        RunError::MissingExport(name) => DomainError::Source {
            message: format!("source does not implement {name}"),
            retryable: false,
        },
        // Both mean the source produced something this host cannot read, and
        // it will produce the same thing next time.
        RunError::Malformed(detail) => DomainError::Source {
            message: format!("source returned malformed data: {detail}"),
            retryable: false,
        },
        RunError::Decode { entry, source } => DomainError::Source {
            message: format!("source returned undecodable {entry}: {source}"),
            retryable: false,
        },
        RunError::Wasm(e) => DomainError::Source {
            message: format!("source trapped: {e}"),
            retryable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_collapses_to_a_reading_direction() {
        assert_eq!(
            map_direction(models::Viewer::RightToLeft),
            ReadingDirection::RightToLeft
        );
        // Vertical and webtoon are layouts, not directions; both read
        // left-to-right for ComicInfo purposes.
        assert_eq!(
            map_direction(models::Viewer::Webtoon),
            ReadingDirection::LeftToRight
        );
        assert_eq!(
            map_direction(models::Viewer::Unknown),
            ReadingDirection::Unknown
        );
    }

    #[test]
    fn absent_collections_become_empty_rather_than_optional() {
        let mapped = map_manga(models::Manga {
            key: "k".into(),
            title: "t".into(),
            ..Default::default()
        });
        assert_eq!(mapped.key, ExternalKey("k".into()));
        assert!(mapped.authors.is_empty());
        assert!(mapped.tags.is_empty());
        assert!(
            mapped.chapters.is_none(),
            "no chapters is distinct from none returned"
        );
    }

    #[test]
    fn a_chapter_timestamp_becomes_a_datetime() {
        let mapped = map_chapter(models::Chapter {
            key: "c".into(),
            date_uploaded: Some(1_700_000_000),
            ..Default::default()
        });
        assert_eq!(
            mapped.published_at.map(|d| d.timestamp()),
            Some(1_700_000_000)
        );
    }

    /// The job engine's backoff decision depends on this, so it is worth
    /// pinning rather than leaving to inspection.
    #[test]
    fn only_request_failures_are_retryable() {
        let transient = map_run_error(RunError::Guest(crate::error::aidoku::REQUEST));
        assert!(transient.is_retryable(), "a network blip should be retried");

        let permanent = map_run_error(RunError::Guest(crate::error::aidoku::DESERIALIZE));
        assert!(
            !permanent.is_retryable(),
            "malformed data fails the same way every time"
        );

        let missing = map_run_error(RunError::MissingExport("get_page_list".into()));
        assert!(!missing.is_retryable());
    }

    /// An unsupported capability must reach the API with its name intact.
    #[test]
    fn a_capability_refusal_keeps_the_capability_name() {
        let err = map_load_error(package::LoadError::UnsupportedCapabilities {
            id: "en.example".into(),
            missing: vec![Capability::Canvas, Capability::JsWebView],
        });
        match err {
            DomainError::UnsupportedCapability(names) => {
                assert!(names.contains("canvas"), "{names}");
                assert!(names.contains("web view"), "{names}");
            }
            other => panic!("expected UnsupportedCapability, got {other:?}"),
        }
    }

    /// A canvas page cannot be served, and dropping it silently is correct
    /// only because such a source is refused at install.
    #[test]
    fn an_image_page_is_dropped() {
        assert!(
            map_page(
                0,
                models::Page {
                    content: models::PageContent::Image(7),
                    ..Default::default()
                }
            )
            .is_none()
        );
        assert!(
            map_page(
                0,
                models::Page {
                    content: models::PageContent::Url("https://x/1.jpg".into(), None),
                    ..Default::default()
                }
            )
            .is_some()
        );
    }
}
