//! Per-invocation host state.
//!
//! One of these lives in each `wasmtime::Store`, so its lifetime is exactly one
//! source call (ADR-0004). That is what makes resource handles safe without any
//! generation counter: a handle cannot outlive the table it came from.

use std::collections::HashMap;
use std::sync::Arc;

use crate::imports::net::{RateLimit, VettingResolver};
use crate::resource::Table;

/// Where a source's key-value namespace is read and written.
///
/// Backed by the `source_kv` table in the real adapter; the in-memory
/// implementation is what tests and the conformance fixture use.
pub trait Defaults: Send + Sync {
    /// Postcard-encoded value, or `None` if unset.
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: Vec<u8>);
    fn remove(&self, key: &str);
}

/// A `Defaults` that keeps everything in memory.
#[derive(Default)]
pub struct MemoryDefaults {
    inner: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

impl Defaults for MemoryDefaults {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lock().ok()?.get(key).cloned()
    }

    fn set(&self, key: &str, value: Vec<u8>) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(key.to_string(), value);
        }
    }

    fn remove(&self, key: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(key);
        }
    }
}

/// What the host imports operate on.
pub struct HostState {
    pub table: Table,
    pub defaults: Arc<dyn Defaults>,
    pub client: reqwest::blocking::Client,
    /// The source's declared budget, captured from `net::set_rate_limit`
    /// during `start`. The job engine takes the stricter of this and the
    /// configured cap (ADR-0003).
    pub declared_rate_limit: Option<RateLimit>,
    /// Payloads handed to `env::send_partial_result`, which sources use to
    /// stream progress. These map onto `job.progress` SSE events.
    pub partial_results: Vec<Vec<u8>>,
    /// Diagnostics the guest wrote through `print`.
    pub logs: Vec<String>,
}

impl HostState {
    pub fn new(defaults: Arc<dyn Defaults>, client: reqwest::blocking::Client) -> Self {
        Self {
            table: Table::new(),
            defaults,
            client,
            declared_rate_limit: None,
            partial_results: Vec::new(),
            logs: Vec::new(),
        }
    }

    /// A state with an in-memory key-value store and a vetted HTTP client.
    pub fn with_defaults(user_agent: &str, timeout: std::time::Duration) -> reqwest::Result<Self> {
        let client = crate::imports::net::build_client(
            user_agent,
            timeout,
            Arc::new(VettingResolver::new()),
        )?;
        Ok(Self::new(Arc::new(MemoryDefaults::default()), client))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_defaults_round_trip() {
        let d = MemoryDefaults::default();
        assert_eq!(d.get("k"), None, "unset must be a miss, not empty bytes");
        d.set("k", vec![1, 2, 3]);
        assert_eq!(d.get("k"), Some(vec![1, 2, 3]));
        d.remove("k");
        assert_eq!(d.get("k"), None);
    }
}
