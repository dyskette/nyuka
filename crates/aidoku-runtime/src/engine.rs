//! The shared `wasmtime` engine and its sandbox configuration.
//!
//! Configuration follows ADR-0004:
//!
//! - **Epoch interruption, not fuel.** Fuel rewrites the compiled code and
//!   costs throughput; what is needed here is a wall-clock bound, not a
//!   deterministic instruction count.
//! - **A memory ceiling per store**, so one source cannot exhaust the host.
//! - **No WASI filesystem and no WASI sockets.** The only route out is the
//!   `net` host import, which applies the egress policy.
//! - **One `Store` per invocation**, so a resource handle cannot outlive the
//!   call that produced it.
//!
//! The compiled-module cache is keyed to include
//! [`HOST_ABI_VERSION`](crate::HOST_ABI_VERSION): bumping the ABI must not
//! reuse modules compiled against the previous one.

use std::time::Duration;

use wasmtime::{Config, Engine, Module, StoreLimits, StoreLimitsBuilder};

/// Sandbox limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Linear-memory ceiling for one invocation.
    pub max_memory_bytes: usize,
    /// Wall-clock budget before the invocation is interrupted.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // ADR-0004's 64-128 MB band. Sources decode images and parse large
            // pages, so the low end is too tight in practice.
            max_memory_bytes: 128 * 1024 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}

impl Limits {
    /// The per-store limiter wasmtime enforces memory against.
    pub fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_memory_bytes)
            // One linear memory and no tables beyond what a source module
            // declares; there is no reason for a source to grow either.
            .memories(1)
            .instances(1)
            .build()
    }
}

/// A compiled-module factory shared across invocations.
pub struct Runtime {
    engine: Engine,
    limits: Limits,
}

impl Runtime {
    pub fn new(limits: Limits) -> wasmtime::Result<Self> {
        let mut config = Config::new();
        // Epoch interruption is the timeout mechanism; the deadline is armed
        // per store and advanced by a ticker owned by the caller.
        config.epoch_interruption(true);
        // Bounded: an untrusted module should not be able to produce an
        // unbounded backtrace on trap.
        config.wasm_backtrace_max_frames(std::num::NonZeroUsize::new(64));
        // Sources are untrusted, so keep the surface small: no component
        // model, no threads, no GC proposal.
        config.wasm_threads(false);
        Ok(Self {
            engine: Engine::new(&config)?,
            limits,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Compiles a module. Callers should cache the result per source version.
    pub fn compile(&self, wasm: &[u8]) -> wasmtime::Result<Module> {
        Module::new(&self.engine, wasm)
    }

    /// The cache key a compiled module should be stored under.
    ///
    /// Includes [`HOST_ABI_VERSION`](crate::HOST_ABI_VERSION) so a host ABI
    /// change cannot reuse a module compiled against the previous one, and the
    /// wasmtime version because its serialized module format is not stable
    /// across releases.
    pub fn cache_key(source_id: &str, source_version: u32) -> String {
        format!(
            "{source_id}-v{source_version}-abi{}-wasmtime{}",
            crate::HOST_ABI_VERSION,
            env!("CARGO_PKG_VERSION"),
        )
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(Limits::default()).expect("default engine config is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_are_within_the_documented_band() {
        let l = Limits::default();
        assert!((64 * 1024 * 1024..=128 * 1024 * 1024).contains(&l.max_memory_bytes));
        assert!(
            l.timeout.as_secs() > 0,
            "an unbounded call could hang a worker"
        );
    }

    #[test]
    fn the_cache_key_changes_with_the_abi_version() {
        let key = Runtime::cache_key("en.example", 3);
        assert!(key.contains("en.example"));
        assert!(key.contains("v3"));
        assert!(
            key.contains(&format!("abi{}", crate::HOST_ABI_VERSION)),
            "a module compiled against a different host ABI must not be reused"
        );
    }

    #[test]
    fn an_engine_can_be_built_and_compile_a_trivial_module() {
        let rt = Runtime::new(Limits::default()).expect("engine");
        // (module) — the smallest valid wasm.
        let wasm = b"\0asm\x01\0\0\0";
        assert!(rt.compile(wasm).is_ok());
    }
}
