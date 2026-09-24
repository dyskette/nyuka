//! Running a source: instantiate, call an entry point, decode the result.
//!
//! # The call ABI
//!
//! Arguments are handles to host buffers. `read_string` arguments are raw
//! UTF-8; everything else is postcard-encoded, and a decode failure on the
//! guest side surfaces as `-1` with no further detail.
//!
//! A successful return is a **pointer into guest memory**, not a handle:
//!
//! ```text
//! [0..4]  total length, including this 8-byte header (i32 LE)
//! [4..8]  capacity (i32 LE)
//! [8..]   postcard payload
//! ```
//!
//! and it is released with the guest's `free_result`. A negative return is an
//! error code.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use wasmtime::{Extern, Instance, Linker, Module, Store, Val};

use crate::engine::Runtime;
use crate::imports::bindings;
use crate::imports::net::RateLimit;
use crate::models::{Chapter, FilterValue, Manga, MangaPageResult, Page};
use crate::resource::Resource;
use crate::state::{Defaults, HostState};

/// What went wrong running a source.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Guest(i32),
    /// The source failed with something it wrote for a person to read.
    #[error("{0}")]
    Message(String),
    #[error("source has no export named {0}")]
    MissingExport(String),
    #[error("source result was malformed: {0}")]
    Malformed(String),
    #[error("decoding {entry}: {source}")]
    Decode {
        entry: &'static str,
        #[source]
        source: postcard::Error,
    },
    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),
}

/// What a guest writes where a result block's length goes when it is instead
/// returning an `AidokuError::Message`.
const MESSAGE_SENTINEL: i32 = -1;

/// One instantiated source, ready for a single call.
///
/// Held per invocation rather than reused: the store carries the resource
/// table, and a handle must not outlive the call that produced it.
pub struct Invocation {
    store: Store<HostState>,
    instance: Instance,
}

impl Invocation {
    /// Instantiates a module and runs its `start` function.
    pub fn new(
        runtime: &Runtime,
        module: &Module,
        defaults: Arc<dyn Defaults>,
        client: reqwest::blocking::Client,
    ) -> Result<Self, RunError> {
        let mut store = Store::new(runtime.engine(), HostState::new(defaults, client));
        // Arm the wall-clock bound. The caller advances the engine's epoch.
        store.set_epoch_deadline(1);

        let mut linker: Linker<HostState> = Linker::new(runtime.engine());
        bindings::register(&mut linker)?;
        let instance = linker.instantiate(&mut store, module)?;

        let mut me = Self { store, instance };
        // `start` is where a source registers itself and declares its rate
        // limit, so it must run before any entry point.
        if let Some(start) = me.instance.get_func(&mut me.store, "start") {
            let results = start.ty(&me.store).results().len();
            let mut out = vec![Val::I32(0); results];
            start.call(&mut me.store, &[], &mut out)?;
        }
        Ok(me)
    }

    /// The rate limit the source declared during `start`, if any.
    pub fn declared_rate_limit(&self) -> Option<RateLimit> {
        self.store.data().declared_rate_limit
    }

    /// Diagnostics the source wrote through `print`.
    pub fn logs(&self) -> &[String] {
        &self.store.data().logs
    }

    /// Payloads the source streamed through `send_partial_result`.
    pub fn partial_results(&self) -> &[Vec<u8>] {
        &self.store.data().partial_results
    }

    fn put_bytes(&mut self, bytes: Vec<u8>) -> i32 {
        self.store.data_mut().table.insert(Resource::Buffer(bytes))
    }

    fn put_encoded<T: Serialize>(&mut self, value: &T) -> Result<i32, RunError> {
        let bytes = postcard::to_allocvec(value).map_err(|source| RunError::Decode {
            entry: "argument",
            source,
        })?;
        Ok(self.put_bytes(bytes))
    }

    /// Calls an export and decodes its postcard result.
    fn call<T: DeserializeOwned>(
        &mut self,
        name: &'static str,
        args: &[Val],
    ) -> Result<T, RunError> {
        let func = self
            .instance
            .get_func(&mut self.store, name)
            .ok_or_else(|| RunError::MissingExport(name.to_string()))?;
        let mut out = [Val::I32(0)];
        func.call(&mut self.store, args, &mut out)?;
        let ret = out[0].unwrap_i32();
        if ret < 0 {
            return Err(RunError::Guest(ret));
        }
        if let Some(message) = self.read_error_message(ret)? {
            return Err(RunError::Message(message));
        }
        let payload = self.read_result(ret)?;
        let decoded = postcard::from_bytes::<T>(&payload).map_err(|source| RunError::Decode {
            entry: name,
            source,
        })?;
        // Hand the allocation back, or a long-lived store would leak one
        // result per call.
        if let Ok(free) = self
            .instance
            .get_typed_func::<i32, ()>(&mut self.store, "free_result")
        {
            free.call(&mut self.store, ret)?;
        }
        Ok(decoded)
    }

    /// A source's own error message, if that is what it returned.
    ///
    /// An `AidokuError::Message` comes back as a *non-negative* pointer with
    /// `-1` where a result block carries its length, and its own layout after
    /// that: `[-1, capacity, length, bytes]`. Read as a result block it looks
    /// like a length of -1, which is how a source explaining itself in words
    /// surfaced as "malformed data" and its explanation was discarded.
    fn read_error_message(&mut self, ptr: i32) -> Result<Option<String>, RunError> {
        let memory = self.memory()?;

        let mut header = [0u8; 12];
        memory
            .read(&mut self.store, ptr as usize, &mut header)
            .map_err(|e| RunError::Malformed(e.to_string()))?;

        let sentinel = i32::from_le_bytes(header[0..4].try_into().expect("12 bytes read"));
        if sentinel != MESSAGE_SENTINEL {
            return Ok(None);
        }

        // The length counts the twelve-byte header with it.
        let total = i32::from_le_bytes(header[8..12].try_into().expect("12 bytes read"));
        let Ok(len) = usize::try_from(total - 12) else {
            return Err(RunError::Malformed(format!(
                "error message length {total} is shorter than its own header"
            )));
        };

        let mut bytes = vec![0u8; len];
        memory
            .read(&mut self.store, (ptr + 12) as usize, &mut bytes)
            .map_err(|e| RunError::Malformed(e.to_string()))?;

        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    fn memory(&mut self) -> Result<wasmtime::Memory, RunError> {
        match self.instance.get_export(&mut self.store, "memory") {
            Some(Extern::Memory(memory)) => Ok(memory),
            _ => Err(RunError::Malformed("source exports no memory".into())),
        }
    }

    /// Reads the `[len, capacity, payload]` block a successful call returns.
    fn read_result(&mut self, ptr: i32) -> Result<Vec<u8>, RunError> {
        let memory = self.memory()?;
        let mut header = [0u8; 8];
        memory
            .read(&mut self.store, ptr as usize, &mut header)
            .map_err(|e| RunError::Malformed(e.to_string()))?;
        let len = i32::from_le_bytes(header[0..4].try_into().expect("8 bytes read"));
        if len < 8 {
            return Err(RunError::Malformed(format!(
                "result length {len} is shorter than its own header"
            )));
        }
        let mut payload = vec![0u8; (len - 8) as usize];
        memory
            .read(&mut self.store, (ptr + 8) as usize, &mut payload)
            .map_err(|e| RunError::Malformed(e.to_string()))?;
        Ok(payload)
    }

    /// `get_search_manga_list(query, page, filters)`.
    ///
    /// The query is raw UTF-8; the filters are postcard. Passing the query
    /// where the filters belong is what makes a source answer `-1`.
    pub fn search(
        &mut self,
        query: Option<&str>,
        page: i32,
        filters: &[FilterValue],
    ) -> Result<MangaPageResult, RunError> {
        let query_rid = self.put_bytes(query.unwrap_or_default().as_bytes().to_vec());
        let filters_rid = self.put_encoded(&filters.to_vec())?;
        self.call(
            "get_search_manga_list",
            &[Val::I32(query_rid), Val::I32(page), Val::I32(filters_rid)],
        )
    }

    /// `get_manga_update(manga, needs_details, needs_chapters)`.
    pub fn manga_update(
        &mut self,
        manga: &Manga,
        needs_details: bool,
        needs_chapters: bool,
    ) -> Result<Manga, RunError> {
        let manga_rid = self.put_encoded(manga)?;
        self.call(
            "get_manga_update",
            &[
                Val::I32(manga_rid),
                Val::I32(needs_details as i32),
                Val::I32(needs_chapters as i32),
            ],
        )
    }

    /// `get_page_list(manga, chapter)`.
    pub fn page_list(&mut self, manga: &Manga, chapter: &Chapter) -> Result<Vec<Page>, RunError> {
        let manga_rid = self.put_encoded(manga)?;
        let chapter_rid = self.put_encoded(chapter)?;
        self.call(
            "get_page_list",
            &[Val::I32(manga_rid), Val::I32(chapter_rid)],
        )
    }
}

/// Convenience for the common case: default limits, in-memory defaults, and a
/// vetted client.
pub fn invoke(
    runtime: &Runtime,
    module: &Module,
    defaults: Arc<dyn Defaults>,
) -> Result<Invocation, RunError> {
    let client = crate::imports::net::build_client(
        concat!("nyuka/", env!("CARGO_PKG_VERSION")),
        Duration::from_secs(30),
        Arc::new(crate::imports::net::VettingResolver::new()),
    )
    .map_err(|e| RunError::Malformed(e.to_string()))?;
    Invocation::new(runtime, module, defaults, client)
}
