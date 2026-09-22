//! The `domain::ports::PageFetcher` implementation.
//!
//! This lives in the runtime crate rather than in `jobs` because the egress
//! policy lives here. A page image is an ordinary outbound request to a host
//! the source chose, so it gets the same resolver-level vetting as the
//! source's own requests: no time-of-check/time-of-use window, and redirects
//! re-checked rather than followed on trust (ADR-0004).

use std::sync::Arc;
use std::time::Duration;

use nyuka_domain::model::PageRef;
use nyuka_domain::ports::PageFetcher;
use nyuka_domain::{DomainError, Result};

use crate::imports::net::{VettingResolver, build_async_client};

#[derive(Debug, Clone)]
pub struct FetcherConfig {
    pub user_agent: String,
    pub timeout: Duration,
    /// Refuses a page larger than this before reading it into memory.
    ///
    /// A chapter is held in memory until it is packaged, so an unbounded page
    /// size is an unbounded allocation driven by a remote server's
    /// `Content-Length`.
    pub max_page_bytes: u64,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("nyuka/", env!("CARGO_PKG_VERSION")).to_string(),
            timeout: Duration::from_secs(30),
            max_page_bytes: 32 * 1024 * 1024,
        }
    }
}

pub struct VettedFetcher {
    client: reqwest::Client,
    max_page_bytes: u64,
}

impl VettedFetcher {
    pub fn new(config: FetcherConfig, resolver: Arc<VettingResolver>) -> Result<Self> {
        let client = build_async_client(&config.user_agent, config.timeout, resolver)
            .map_err(|e| DomainError::Internal(format!("cannot build the page client: {e}")))?;
        Ok(Self {
            client,
            max_page_bytes: config.max_page_bytes,
        })
    }
}

/// Whether another attempt could plausibly succeed.
///
/// A blocked host or a 404 will not change on retry; a timeout, a 5xx, or a
/// 429 will. Getting this backwards either burns every attempt on a permanent
/// failure or gives up on a site that was briefly slow.
fn retryable(status: Option<reqwest::StatusCode>, error: Option<&reqwest::Error>) -> bool {
    if let Some(status) = status {
        return status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    }
    match error {
        // A resolver denial arrives as a connect error, and no amount of
        // waiting makes a private address public.
        Some(e) => !e.is_connect() || e.is_timeout(),
        None => false,
    }
}

#[async_trait::async_trait]
impl PageFetcher for VettedFetcher {
    async fn fetch(&self, page: &PageRef) -> Result<Vec<u8>> {
        let mut request = self.client.get(&page.url);
        for (name, value) in &page.headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(|e| DomainError::Source {
            message: format!("page {} could not be fetched: {e}", page.index),
            retryable: retryable(None, Some(&e)),
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(DomainError::Source {
                message: format!("page {} returned {status}", page.index),
                retryable: retryable(Some(status), None),
            });
        }

        // Check the declared length before reading. It is only a hint — a
        // server can lie or omit it — so the read below is bounded again.
        if let Some(len) = response.content_length()
            && len > self.max_page_bytes
        {
            return Err(DomainError::Source {
                message: format!(
                    "page {} declares {len} bytes, over the {} byte limit",
                    page.index, self.max_page_bytes
                ),
                retryable: false,
            });
        }

        let bytes = response.bytes().await.map_err(|e| DomainError::Source {
            message: format!("page {} could not be read: {e}", page.index),
            retryable: true,
        })?;

        if bytes.len() as u64 > self.max_page_bytes {
            return Err(DomainError::Source {
                message: format!(
                    "page {} sent {} bytes, over the {} byte limit",
                    page.index,
                    bytes.len(),
                    self.max_page_bytes
                ),
                retryable: false,
            });
        }
        if bytes.is_empty() {
            return Err(DomainError::Source {
                message: format!("page {} was empty", page.index),
                retryable: true,
            });
        }

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str) -> PageRef {
        PageRef {
            index: 0,
            url: url.into(),
            headers: vec![],
        }
    }

    #[test]
    fn a_server_error_retries_and_a_client_error_does_not() {
        assert!(retryable(Some(reqwest::StatusCode::BAD_GATEWAY), None));
        assert!(retryable(
            Some(reqwest::StatusCode::TOO_MANY_REQUESTS),
            None
        ));
        assert!(
            !retryable(Some(reqwest::StatusCode::NOT_FOUND), None),
            "a missing page will still be missing next time"
        );
        assert!(
            !retryable(Some(reqwest::StatusCode::FORBIDDEN), None),
            "a 403 usually means a missing referer, which a retry will not add"
        );
    }

    /// The fetcher must not be a way around the egress policy: a blocked host
    /// has to fail here exactly as it does for the source's own requests.
    #[tokio::test]
    async fn a_blocked_host_is_refused() {
        let fetcher =
            VettedFetcher::new(FetcherConfig::default(), Arc::new(VettingResolver::new()))
                .expect("fetcher builds");

        let err = fetcher
            .fetch(&page("http://127.0.0.1:1/page.jpg"))
            .await
            .expect_err("loopback must be refused");
        assert!(
            !err.is_retryable(),
            "a private address will not become public on retry"
        );
    }

    #[tokio::test]
    async fn a_url_that_is_not_http_is_refused() {
        let fetcher =
            VettedFetcher::new(FetcherConfig::default(), Arc::new(VettingResolver::new()))
                .expect("fetcher builds");
        assert!(fetcher.fetch(&page("file:///etc/passwd")).await.is_err());
    }
}
