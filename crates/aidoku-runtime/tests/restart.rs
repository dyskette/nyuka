//! An installed source must survive a restart.
//!
//! Before packages were saved, a source was downloaded, compiled into an
//! in-memory map, and its bytes discarded. Nothing repopulated that map at
//! startup, so after a restart every installed source was listed by the API
//! and returned "not found" from anything that needed its module — browse,
//! catalog, download and follow checks alike. This is the regression test for
//! that.
//!
//! Uses a real package from `target/aix`, fetched by `cargo xtask
//! fetch-sources`. It is a build artifact and is not committed, so this test
//! skips when there is none.

use std::sync::Arc;

use nyuka_aidoku_runtime::adapter::SourceRuntime;
use nyuka_aidoku_runtime::engine::{Limits, Runtime};
use nyuka_aidoku_runtime::state::MemoryDefaults;
use nyuka_domain::model::{SourceId, SourceRepoId};
use nyuka_domain::ports::PackageStorage;
use nyuka_packaging::packages::PackageStore;

/// Any real package will do: this exercises saving and reloading, not the
/// behaviour of a particular source.
fn a_real_package() -> Option<Vec<u8>> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/aix");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "aix"))
        .collect();
    if entries.is_empty() {
        eprintln!(
            "skipping: no packages in {} — run `cargo xtask fetch-sources`",
            dir.display()
        );
        return None;
    }
    // Sorted so a failure names the same package every time.
    entries.sort();
    std::fs::read(entries.first()?).ok()
}

fn fresh_runtime() -> Arc<SourceRuntime> {
    let engine = Arc::new(Runtime::new(Limits::default()).expect("engine"));
    Arc::new(SourceRuntime::new(
        engine,
        Arc::new(MemoryDefaults::default()),
    ))
}

#[test]
fn a_saved_package_loads_into_a_new_runtime() {
    let Some(bytes) = a_real_package() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let store = PackageStore::open(dir.path()).expect("store");

    let id = SourceId(uuid::Uuid::new_v4());
    let repo = SourceRepoId(uuid::Uuid::new_v4());

    // Install.
    let first = fresh_runtime();
    let prepared = first.prepare(&bytes).expect("prepare");
    let installed = first.register(id, repo, prepared).expect("register");
    PackageStorage::write(&store, id, &bytes).expect("save the package");
    assert!(first.is_loaded(id));

    // Restart: a new runtime with an empty map, exactly as a new process has.
    let second = fresh_runtime();
    assert!(
        !second.is_loaded(id),
        "a fresh runtime starts with nothing loaded — which is the whole problem"
    );

    let saved = PackageStorage::read(&store, id)
        .expect("read")
        .expect("the package must still be there");
    let prepared = second.prepare(&saved).expect("prepare after restart");
    let reloaded = second.register(id, repo, prepared).expect("register");

    assert!(second.is_loaded(id), "the source must be usable again");
    assert_eq!(
        reloaded.external_id, installed.external_id,
        "and it must be the same source, under the same id"
    );
    assert_eq!(reloaded.id, id);
}

/// The compounding bug: `install` registered the module under a freshly
/// generated id while `upsert` returned the existing row's id, so reinstalling
/// left the new module unreachable under the id everything else used.
///
/// Registration now takes the id as an argument, and this pins that
/// registering under a chosen id makes it reachable under exactly that one.
#[test]
fn registering_under_a_chosen_id_is_reachable_under_that_id() {
    let Some(bytes) = a_real_package() else {
        return;
    };
    let runtime = fresh_runtime();
    let repo = SourceRepoId(uuid::Uuid::new_v4());

    let canonical = SourceId(uuid::Uuid::new_v4());
    let throwaway = SourceId(uuid::Uuid::new_v4());

    let prepared = runtime.prepare(&bytes).expect("prepare");
    runtime
        .register(canonical, repo, prepared)
        .expect("register");

    assert!(runtime.is_loaded(canonical));
    assert!(
        !runtime.is_loaded(throwaway),
        "no other id may resolve to it"
    );
}

/// Reinstalling replaces the module rather than leaving the old one serving.
#[test]
fn reinstalling_replaces_the_loaded_module() {
    let Some(bytes) = a_real_package() else {
        return;
    };
    let runtime = fresh_runtime();
    let id = SourceId(uuid::Uuid::new_v4());
    let repo = SourceRepoId(uuid::Uuid::new_v4());

    for _ in 0..2 {
        let prepared = runtime.prepare(&bytes).expect("prepare");
        runtime.register(id, repo, prepared).expect("register");
    }

    assert!(runtime.is_loaded(id));
}

/// A source whose package is gone must not be silently absent: the error has
/// to say what is wrong, because "no source matches that identifier" sends an
/// operator looking for a bad id when the fix is to reinstall.
#[test]
fn an_unloaded_source_says_so() {
    let runtime = fresh_runtime();
    let id = SourceId(uuid::Uuid::new_v4());

    let error = runtime
        .settings_declaration(id)
        .expect_err("an unloaded source has no declaration");

    let message = error.to_string();
    assert!(
        message.contains("not loaded") && message.contains("reinstall"),
        "the error must name the problem and the fix, got: {message}"
    );
}

// ---------------------------------------------------------------------------
// The id a source is registered under
// ---------------------------------------------------------------------------

/// A source repository whose `upsert` returns an id of its own choosing.
///
/// That is what the real one does on a reinstall: it keys on
/// `(repo_id, external_id)` and returns the *existing* row's id, not the one
/// it was handed. Everything else is unused by this path.
struct ReinstallingSources {
    existing: SourceId,
}

#[async_trait::async_trait]
impl nyuka_domain::ports::SourceRepository for ReinstallingSources {
    async fn get(
        &self,
        _id: SourceId,
    ) -> nyuka_domain::Result<nyuka_domain::model::InstalledSource> {
        Err(nyuka_domain::DomainError::NotFound)
    }
    async fn list(&self) -> nyuka_domain::Result<Vec<nyuka_domain::model::InstalledSource>> {
        Ok(vec![])
    }
    async fn upsert(
        &self,
        _source: &nyuka_domain::model::InstalledSource,
    ) -> nyuka_domain::Result<SourceId> {
        Ok(self.existing)
    }
    async fn remove(&self, _id: SourceId) -> nyuka_domain::Result<()> {
        Ok(())
    }
    async fn list_repos(&self) -> nyuka_domain::Result<Vec<nyuka_domain::model::SourceRepo>> {
        Ok(vec![])
    }
    async fn list_entries(
        &self,
        _repo: SourceRepoId,
    ) -> nyuka_domain::Result<Vec<nyuka_domain::model::SourceEntry>> {
        Ok(vec![])
    }
    async fn get_entry(
        &self,
        _repo: SourceRepoId,
        _key: &nyuka_domain::model::ExternalKey,
    ) -> nyuka_domain::Result<nyuka_domain::model::SourceEntry> {
        Err(nyuka_domain::DomainError::NotFound)
    }
    async fn upsert_repo(
        &self,
        _repo: &nyuka_domain::model::SourceRepo,
    ) -> nyuka_domain::Result<SourceRepoId> {
        Err(nyuka_domain::DomainError::NotFound)
    }
    async fn remove_repo(&self, _id: SourceRepoId) -> nyuka_domain::Result<()> {
        Ok(())
    }
    async fn replace_entries(
        &self,
        _repo: SourceRepoId,
        _entries: &[nyuka_domain::model::SourceEntry],
    ) -> nyuka_domain::Result<u64> {
        Ok(0)
    }
    async fn get_repo(
        &self,
        _id: SourceRepoId,
    ) -> nyuka_domain::Result<nyuka_domain::model::SourceRepo> {
        Err(nyuka_domain::DomainError::NotFound)
    }
    async fn mark_repo_refreshed(&self, _id: SourceRepoId) -> nyuka_domain::Result<()> {
        Ok(())
    }
    async fn kv_get(&self, _source: SourceId, _key: &str) -> nyuka_domain::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn kv_set(
        &self,
        _source: SourceId,
        _key: &str,
        _value: Vec<u8>,
    ) -> nyuka_domain::Result<()> {
        Ok(())
    }
    async fn kv_list(&self, _source: SourceId) -> nyuka_domain::Result<Vec<(String, Vec<u8>)>> {
        Ok(vec![])
    }
}

/// The bug: `install` generated an id, registered the module under it, and
/// then took the database's id for everything else. On a reinstall those
/// differ, so the compiled module sat under a key nothing ever looked up and
/// the source was listed by the API while every call needing it failed.
#[tokio::test]
async fn install_registers_under_the_id_the_database_returned() {
    let Some(bytes) = a_real_package() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let existing = SourceId(uuid::Uuid::new_v4());
    let repo = SourceRepoId(uuid::Uuid::new_v4());

    let runtime = fresh_runtime();
    let registry = nyuka_aidoku_runtime::registry::AidokuRegistry::new(
        runtime.clone(),
        Arc::new(ReinstallingSources { existing }),
        Arc::new(nyuka_aidoku_runtime::imports::net::VettingResolver::new()),
        Arc::new(PackageStore::open(dir.path()).expect("store")),
        nyuka_aidoku_runtime::registry::RegistryConfig::default(),
    )
    .expect("registry");

    let installed = registry.install_bytes(repo, &bytes).await.expect("install");

    assert_eq!(
        installed.id, existing,
        "the reported id is the database's, which is what every caller uses"
    );
    assert!(
        runtime.is_loaded(existing),
        "and the module must be loaded under exactly that id"
    );
}

/// The package has to be on disk by the time install returns, or a source
/// that works now stops working at the next restart.
#[tokio::test]
async fn install_saves_the_package_for_the_next_restart() {
    let Some(bytes) = a_real_package() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let existing = SourceId(uuid::Uuid::new_v4());
    let store = Arc::new(PackageStore::open(dir.path()).expect("store"));

    let registry = nyuka_aidoku_runtime::registry::AidokuRegistry::new(
        fresh_runtime(),
        Arc::new(ReinstallingSources { existing }),
        Arc::new(nyuka_aidoku_runtime::imports::net::VettingResolver::new()),
        store.clone(),
        nyuka_aidoku_runtime::registry::RegistryConfig::default(),
    )
    .expect("registry");

    registry
        .install_bytes(SourceRepoId(uuid::Uuid::new_v4()), &bytes)
        .await
        .expect("install");

    assert_eq!(
        PackageStorage::read(store.as_ref(), existing)
            .expect("read")
            .as_deref(),
        Some(&bytes[..]),
        "saved under the database's id, not the generated one"
    );
}
