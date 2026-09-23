//! The `domain::ports::SourceRegistry` implementation.
//!
//! Refreshing an index is plain HTTP and JSON; installing needs the runtime,
//! because refusing a package this host cannot run means reading its imports
//! out of the compiled module. Both halves live here so the capability check
//! and the download that feeds it cannot drift apart.
//!
//! # The index format
//!
//! Verified against the live Aidoku-Community index rather than assumed:
//!
//! ```json
//! { "name": "...", "sources": [ {
//!     "id": "en.guya", "name": "Guya", "version": 2,
//!     "iconURL": "icons/en.guya-v2.png",
//!     "downloadURL": "sources/en.guya-v2.aix",
//!     "languages": ["en"], "contentRating": 0,
//!     "baseURL": "https://guya.cubari.moe",
//!     "minAppVersion": "0.7.1"
//! } ] }
//! ```
//!
//! `iconURL` and `downloadURL` are **relative to the index URL**, so they are
//! resolved here and stored absolute. Leaving them relative would mean every
//! later reader had to know which index a row came from.
//!
//! # Downloads go through the egress policy
//!
//! An index URL and a `.aix` URL are operator-supplied, which makes them
//! exactly the kind of input SSRF protection exists for: a repository pointing
//! at `http://169.254.169.254/` must be refused the same way a source's own
//! request would be (ADR-0004).

use std::sync::Arc;
use std::time::Duration;

use nyuka_domain::model::{
    Capability, ContentRating, ExternalKey, InstalledSource, SourceEntry, SourceId, SourceRepoId,
};
use nyuka_domain::ports::{SourceRegistry, SourceRepository};
use nyuka_domain::{DomainError, Result};
use serde::Deserialize;

use crate::adapter::SourceRuntime;
use crate::imports::net::{VettingResolver, build_async_client};

/// The index document, as published.
#[derive(Debug, Deserialize)]
struct Index {
    #[serde(default)]
    sources: Vec<IndexSource>,
}

#[derive(Debug, Deserialize)]
struct IndexSource {
    id: String,
    name: String,
    version: u32,
    #[serde(rename = "iconURL")]
    icon_url: Option<String>,
    #[serde(rename = "downloadURL")]
    download_url: String,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default, rename = "contentRating")]
    content_rating: i16,
    #[serde(rename = "baseURL")]
    base_url: Option<String>,
    #[serde(rename = "minAppVersion")]
    min_app_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub user_agent: String,
    pub timeout: Duration,
    /// Refuses an index larger than this. The document is parsed into memory,
    /// so its size is decided by a remote server unless something bounds it.
    pub max_index_bytes: usize,
    /// Refuses a package larger than this, for the same reason.
    pub max_package_bytes: usize,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("nyuka/", env!("CARGO_PKG_VERSION")).to_string(),
            timeout: Duration::from_secs(60),
            max_index_bytes: 8 * 1024 * 1024,
            max_package_bytes: 32 * 1024 * 1024,
        }
    }
}

pub struct AidokuRegistry {
    runtime: Arc<SourceRuntime>,
    sources: Arc<dyn SourceRepository>,
    client: reqwest::Client,
    config: RegistryConfig,
    /// Computed once. The port hands out a slice, and the package module
    /// builds a fresh `Vec` per call.
    capabilities: Vec<Capability>,
}

impl AidokuRegistry {
    pub fn new(
        runtime: Arc<SourceRuntime>,
        sources: Arc<dyn SourceRepository>,
        resolver: Arc<VettingResolver>,
        config: RegistryConfig,
    ) -> Result<Self> {
        let client = build_async_client(&config.user_agent, config.timeout, resolver)
            .map_err(|e| DomainError::Internal(format!("cannot build the registry client: {e}")))?;
        Ok(Self {
            runtime,
            sources,
            client,
            config,
            capabilities: crate::package::supported_capabilities(),
        })
    }

    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| DomainError::Source {
                message: format!("{url} could not be fetched: {e}"),
                // A resolver denial arrives as a connect error, and a private
                // address does not become public on retry.
                retryable: !e.is_connect() || e.is_timeout(),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(DomainError::Source {
                message: format!("{url} returned {status}"),
                retryable: status.is_server_error(),
            });
        }

        if let Some(len) = response.content_length()
            && len > limit as u64
        {
            return Err(DomainError::Source {
                message: format!("{url} declares {len} bytes, over the {limit} byte limit"),
                retryable: false,
            });
        }

        let bytes = response.bytes().await.map_err(|e| DomainError::Source {
            message: format!("{url} could not be read: {e}"),
            retryable: true,
        })?;

        // Checked again after reading: `Content-Length` is a hint a server can
        // omit or lie about.
        if bytes.len() > limit {
            return Err(DomainError::Source {
                message: format!(
                    "{url} sent {} bytes, over the {limit} byte limit",
                    bytes.len()
                ),
                retryable: false,
            });
        }
        Ok(bytes.to_vec())
    }
}

/// Resolves an index-relative URL against the index's own URL.
///
/// An absolute entry is left alone: some repositories host packages on a
/// different origin from the index, and rewriting those would break them.
pub fn resolve_url(index_url: &str, entry: &str) -> Result<String> {
    let base = url::Url::parse(index_url)
        .map_err(|e| DomainError::Invalid(format!("repository url is not a url: {e}")))?;
    base.join(entry)
        .map(String::from)
        .map_err(|e| DomainError::Source {
            message: format!("entry url {entry} could not be resolved against the index: {e}"),
            retryable: false,
        })
}

/// Maps an index entry onto the stored shape.
fn to_entry(repo: SourceRepoId, index_url: &str, source: IndexSource) -> Result<SourceEntry> {
    Ok(SourceEntry {
        repo_id: repo,
        external_id: ExternalKey(source.id),
        name: source.name,
        version: source.version,
        icon_url: source
            .icon_url
            .map(|u| resolve_url(index_url, &u))
            .transpose()?,
        download_url: resolve_url(index_url, &source.download_url)?,
        languages: source.languages,
        content_rating: ContentRating::from_i16(source.content_rating),
        base_url: source.base_url,
        min_app_version: source.min_app_version,
    })
}

#[async_trait::async_trait]
impl SourceRegistry for AidokuRegistry {
    async fn refresh_repo(&self, repo: SourceRepoId) -> Result<()> {
        let record = self.sources.get_repo(repo).await?;
        let body = self.get(&record.url, self.config.max_index_bytes).await?;

        let index: Index = serde_json::from_slice(&body).map_err(|e| DomainError::Source {
            message: format!("{} is not a source index: {e}", record.url),
            // A document that will not parse now will not parse on retry.
            retryable: false,
        })?;

        let entries = index
            .sources
            .into_iter()
            .map(|s| to_entry(repo, &record.url, s))
            .collect::<Result<Vec<_>>>()?;

        let count = self.sources.replace_entries(repo, &entries).await?;
        tracing::info!(repo.id = %repo, url = %record.url, entries = count, "refreshed a source index");
        Ok(())
    }

    async fn install(&self, repo: SourceRepoId, entry: &ExternalKey) -> Result<InstalledSource> {
        let listed = self.sources.get_entry(repo, entry).await?;
        let bytes = self
            .get(&listed.download_url, self.config.max_package_bytes)
            .await?;

        // A source id is assigned here, but `upsert` keys on
        // `(repo_id, external_id)`: reinstalling keeps the existing row's id,
        // so the library pointing at this source survives the upgrade.
        let id = SourceId(uuid::Uuid::new_v4());

        // This is where an unsupported capability is refused. Installing a
        // source that then fails mid-download is the worst outcome, because it
        // looks like a site problem rather than a host gap (ADR-0004).
        let installed = self.runtime.install(id, repo, &bytes)?;

        let stored = self.sources.upsert(&installed).await?;
        tracing::info!(
            source.id = %stored,
            // Reaching through the newtype on purpose: `ExternalKey` has no
            // `Display` so that interpolating one anywhere is a deliberate
            // act, not something that happens by reflex (see its doc).
            source.external_id = %installed.external_id.0,
            version = installed.version,
            "installed a source"
        );
        Ok(InstalledSource {
            id: stored,
            ..installed
        })
    }

    async fn uninstall(&self, source: SourceId) -> Result<()> {
        self.runtime.remove(source)?;
        self.sources.remove(source).await
    }

    fn supported_capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    async fn settings_declaration(&self, source: SourceId) -> Result<serde_json::Value> {
        self.runtime.settings_declaration(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "https://example.test/repo/index.min.json";

    #[test]
    fn a_relative_entry_resolves_against_the_index() {
        assert_eq!(
            resolve_url(INDEX, "sources/en.guya-v2.aix").expect("resolve"),
            "https://example.test/repo/sources/en.guya-v2.aix"
        );
    }

    /// Some repositories host packages on a different origin from the index.
    #[test]
    fn an_absolute_entry_is_left_alone() {
        assert_eq!(
            resolve_url(INDEX, "https://cdn.example.test/en.guya-v2.aix").expect("resolve"),
            "https://cdn.example.test/en.guya-v2.aix"
        );
    }

    #[test]
    fn a_root_relative_entry_resolves_against_the_origin() {
        assert_eq!(
            resolve_url(INDEX, "/pkgs/en.guya-v2.aix").expect("resolve"),
            "https://example.test/pkgs/en.guya-v2.aix"
        );
    }

    #[test]
    fn a_repository_url_that_is_not_a_url_is_rejected() {
        assert!(resolve_url("not a url", "a.aix").is_err());
    }

    /// The real index shape, so a field rename upstream fails here rather than
    /// producing an empty catalog nobody notices.
    #[test]
    fn the_published_index_shape_parses() {
        let json = r#"{
            "name": "Community",
            "sources": [{
                "id": "en.guya", "name": "Guya", "version": 2,
                "iconURL": "icons/en.guya-v2.png",
                "downloadURL": "sources/en.guya-v2.aix",
                "languages": ["en"], "contentRating": 0,
                "baseURL": "https://guya.cubari.moe",
                "minAppVersion": "0.7.1"
            }]
        }"#;
        let index: Index = serde_json::from_str(json).expect("parse");
        let repo = SourceRepoId(uuid::Uuid::new_v4());
        let entry =
            to_entry(repo, INDEX, index.sources.into_iter().next().expect("one")).expect("map");

        assert_eq!(entry.external_id.0, "en.guya");
        assert_eq!(entry.version, 2);
        assert_eq!(
            entry.download_url,
            "https://example.test/repo/sources/en.guya-v2.aix"
        );
        assert_eq!(
            entry.icon_url.as_deref(),
            Some("https://example.test/repo/icons/en.guya-v2.png")
        );
        assert_eq!(entry.content_rating, ContentRating::Unknown);
        assert_eq!(entry.languages, vec!["en".to_string()]);
    }

    /// The optional fields really are optional.
    #[test]
    fn an_entry_without_the_optional_fields_parses() {
        let json = r#"{"sources":[{"id":"x","name":"X","version":1,
            "downloadURL":"x.aix"}]}"#;
        let index: Index = serde_json::from_str(json).expect("parse");
        let entry = to_entry(
            SourceRepoId(uuid::Uuid::new_v4()),
            INDEX,
            index.sources.into_iter().next().expect("one"),
        )
        .expect("map");
        assert!(entry.icon_url.is_none());
        assert!(entry.base_url.is_none());
        assert!(entry.min_app_version.is_none());
        assert!(entry.languages.is_empty());
    }

    /// An index with no `sources` key is an empty catalog, not a crash.
    #[test]
    fn an_index_without_sources_is_empty() {
        let index: Index = serde_json::from_str(r#"{"name":"Empty"}"#).expect("parse");
        assert!(index.sources.is_empty());
    }
}
