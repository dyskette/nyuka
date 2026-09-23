//! Where installed source packages live on disk.
//!
//! A source is a WASM module compiled at install time and held in memory. Its
//! `.aix` was previously downloaded, compiled, and discarded — so after a
//! restart every installed source was listed by the API and returned 404 from
//! everything that needed the module. The bytes are kept here so a restart can
//! compile them again without reaching the network.
//!
//! # This is under `DATA_DIR`, not the library
//!
//! The library is what an operator backs up and syncs between machines.
//! Third-party executable code has no business travelling with it, and a
//! restore of an older library would otherwise roll back the installed sources
//! along with the comics.

use std::path::{Path, PathBuf};

use crate::store::StoreError;

/// The largest package this store will read back.
///
/// The install path already bounds the download. This bounds what a restart
/// will load, so a file that grew on disk — or was placed there — cannot make
/// the server allocate without limit while booting.
pub const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;

/// The `.aix` files for installed sources, one per source id.
pub struct PackageStore {
    root: PathBuf,
}

impl PackageStore {
    /// Opens the store under a data directory, creating it if absent.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = data_dir.as_ref().join("sources");
        std::fs::create_dir_all(&root).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path for a source's package.
    ///
    /// Named by the source's uuid rather than by its `external_id`. The
    /// external id comes from a third-party package and would need sanitising;
    /// a uuid renders as 36 characters of hex and hyphens and cannot escape
    /// the directory however it was obtained.
    fn path_for(&self, source: uuid::Uuid) -> PathBuf {
        self.root.join(format!("{source}.aix"))
    }

    /// Writes a package, replacing any existing one.
    ///
    /// Through a temporary file and a rename, so a crash or a full disk
    /// mid-write leaves either the previous package or none — never a
    /// truncated one that fails to compile at the next boot.
    ///
    /// The temporary file is in the same directory, because `rename` is only
    /// atomic within a filesystem and `DATA_DIR` may be its own mount.
    pub fn write(&self, source: uuid::Uuid, bytes: &[u8]) -> Result<(), StoreError> {
        let final_path = self.path_for(source);
        let temp_path = self.root.join(format!(".{source}.aix.partial"));

        std::fs::write(&temp_path, bytes).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::rename(&temp_path, &final_path).map_err(|e| {
            // The temporary file would otherwise stay behind on every failure.
            let _ = std::fs::remove_file(&temp_path);
            StoreError::Io(e.to_string())
        })?;
        Ok(())
    }

    /// Reads a package back, or `Ok(None)` when there is none.
    ///
    /// An absent package is not an error: a source installed before this store
    /// existed has a database row and no file, and that has to be reported as
    /// "reinstall this" rather than as a failure to start.
    pub fn read(&self, source: uuid::Uuid) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path_for(source);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };

        if metadata.len() > MAX_PACKAGE_BYTES {
            return Err(StoreError::Io(format!(
                "{} is {} bytes, over the {MAX_PACKAGE_BYTES} limit",
                path.display(),
                metadata.len()
            )));
        }

        std::fs::read(&path)
            .map(Some)
            .map_err(|e| StoreError::Io(e.to_string()))
    }

    /// Removes a package. Succeeds when there was none.
    ///
    /// Uninstall must work after a restart that never loaded the source, and
    /// after a data directory was restored without it.
    pub fn remove(&self, source: uuid::Uuid) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path_for(source)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    /// Deletes leftover partial writes.
    ///
    /// Called at startup, for the same reason the library's staging directory
    /// is cleaned: a crash mid-write leaves one, and nothing else ever removes
    /// it.
    pub fn clean_partials(&self) -> Result<u64, StoreError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };

        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                && name.ends_with(".partial")
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// The port implementation.
///
/// The inherent methods above take a `Uuid` because that is what names a file;
/// the port speaks in `SourceId`, which is what the rest of the system uses.
impl nyuka_domain::ports::PackageStorage for PackageStore {
    fn write(
        &self,
        source: nyuka_domain::model::SourceId,
        bytes: &[u8],
    ) -> nyuka_domain::Result<()> {
        PackageStore::write(self, source.0, bytes)
            .map_err(|e| nyuka_domain::DomainError::Storage(e.to_string()))
    }

    fn read(&self, source: nyuka_domain::model::SourceId) -> nyuka_domain::Result<Option<Vec<u8>>> {
        PackageStore::read(self, source.0)
            .map_err(|e| nyuka_domain::DomainError::Storage(e.to_string()))
    }

    fn remove(&self, source: nyuka_domain::model::SourceId) -> nyuka_domain::Result<()> {
        PackageStore::remove(self, source.0)
            .map_err(|e| nyuka_domain::DomainError::Storage(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PackageStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PackageStore::open(dir.path()).expect("open");
        (dir, store)
    }

    #[test]
    fn a_package_round_trips() {
        let (_dir, store) = store();
        let id = uuid::Uuid::new_v4();

        store.write(id, b"PK\x03\x04payload").expect("write");
        assert_eq!(
            store.read(id).expect("read").as_deref(),
            Some(&b"PK\x03\x04payload"[..])
        );
    }

    /// The case that motivated this store: a source with a row and no file.
    #[test]
    fn an_absent_package_is_not_an_error() {
        let (_dir, store) = store();
        assert!(store.read(uuid::Uuid::new_v4()).expect("read").is_none());
    }

    #[test]
    fn writing_twice_replaces_rather_than_appends() {
        let (_dir, store) = store();
        let id = uuid::Uuid::new_v4();

        store.write(id, b"first").expect("write");
        store.write(id, b"second").expect("write");

        assert_eq!(
            store.read(id).expect("read").as_deref(),
            Some(&b"second"[..])
        );
    }

    /// Uninstall has to work after a restart that never loaded the source.
    #[test]
    fn removing_a_package_that_is_not_there_succeeds() {
        let (_dir, store) = store();
        assert!(store.remove(uuid::Uuid::new_v4()).is_ok());
    }

    #[test]
    fn removing_a_package_makes_it_absent() {
        let (_dir, store) = store();
        let id = uuid::Uuid::new_v4();

        store.write(id, b"x").expect("write");
        store.remove(id).expect("remove");

        assert!(store.read(id).expect("read").is_none());
    }

    /// A crash mid-write leaves a partial file, and nothing else removes it.
    #[test]
    fn startup_clears_leftover_partial_writes() {
        let (_dir, store) = store();
        let id = uuid::Uuid::new_v4();
        store.write(id, b"real").expect("write");
        std::fs::write(store.root().join(".abc.aix.partial"), b"junk").expect("partial");

        assert_eq!(store.clean_partials().expect("clean"), 1);
        assert!(
            store.read(id).expect("read").is_some(),
            "a real package must survive the sweep"
        );
    }

    /// A package larger than the cap is refused rather than read into memory.
    #[test]
    fn an_oversized_package_is_refused() {
        let (_dir, store) = store();
        let id = uuid::Uuid::new_v4();
        let path = store.root().join(format!("{id}.aix"));
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(MAX_PACKAGE_BYTES + 1).expect("grow");

        assert!(store.read(id).is_err());
    }

    /// The store lives under the data directory, never in the library.
    #[test]
    fn packages_live_under_the_data_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PackageStore::open(dir.path()).expect("open");

        assert_eq!(store.root(), dir.path().join("sources"));
    }
}
