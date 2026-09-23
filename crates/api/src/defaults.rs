//! The `defaults` host import, backed by the `source_kv` table.
//!
//! # Why this bridges async to blocking
//!
//! The `Defaults` trait is synchronous because it is called from inside a WASM
//! invocation, and the guest's calling convention is synchronous — a source
//! calls `defaults::get` and expects a value, not a future. The storage behind
//! it is async.
//!
//! > [!IMPORTANT]
//! > `Handle::block_on` panics if called from a thread that is currently
//! > driving the async runtime. This is sound only because every WASM
//! > invocation runs on `spawn_blocking`, where blocking is the whole point.
//! > Calling a source directly from an async task would panic here rather than
//! > deadlock, which is the better of the two failures but still a failure.
//!
//! The alternative — snapshotting a source's settings before the invocation
//! and flushing afterwards — was rejected because two concurrent invocations
//! of the same source would then overwrite each other's writes with a stale
//! snapshot, silently.

use std::sync::Arc;

use nyuka_aidoku_runtime::state::Defaults;
use nyuka_domain::model::SourceId;
use nyuka_persistence::repository::Repositories;

pub struct RepositoryDefaults {
    repositories: Arc<Repositories>,
    /// Which source's namespace this instance addresses.
    ///
    /// `None` until an invocation binds one. A `get` before that returns
    /// nothing rather than reading another source's settings.
    source: Option<SourceId>,
}

impl RepositoryDefaults {
    pub fn new(repositories: Arc<Repositories>) -> Self {
        Self {
            repositories,
            source: None,
        }
    }

    /// Binds this to one source's namespace.
    pub fn for_source(&self, source: SourceId) -> Self {
        Self {
            repositories: self.repositories.clone(),
            source: Some(source),
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> Option<F::Output> {
        tokio::runtime::Handle::try_current()
            .ok()
            .map(|handle| handle.block_on(future))
    }
}

impl Defaults for RepositoryDefaults {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let source = self.source?;
        Self::block_on(self.repositories.kv_get(source, key))?
            .ok()
            .flatten()
    }

    fn set(&self, key: &str, value: Vec<u8>) {
        let Some(source) = self.source else { return };
        // A failed write is logged rather than surfaced: the guest's `set` has
        // no error channel, so the alternative is pretending it succeeded
        // without saying so anywhere.
        if let Some(Err(e)) = Self::block_on(self.repositories.kv_set(source, key, value)) {
            tracing::warn!(source.id = %source, key, error = %e, "storing a source setting failed");
        }
    }

    fn remove(&self, key: &str) {
        // Modelled as an empty value rather than a delete: the guest cannot
        // tell "absent" from "empty" through this ABI, and a real delete would
        // need a second repository method for no observable difference.
        self.set(key, Vec::new());
    }
}
