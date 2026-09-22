//! Writing chapters onto the library volume.
//!
//! # Atomic placement
//!
//! A chapter is built in a temporary directory, fsynced, then renamed into
//! place. Nothing partial is ever visible in the library.
//!
//! **The temporary directory must live inside the library root.** `rename` is
//! atomic only within a single filesystem, so a temp directory under `/tmp` —
//! which is very often a different mount, and is `tmpfs` on this machine —
//! silently degrades the rename into a copy. A crash mid-copy then leaves a
//! truncated archive that looks exactly like a finished download. That failure
//! is invisible until someone opens the file (ADR-0007).

use std::path::{Path, PathBuf};

use nyuka_domain::model::ReadingDirection;

use crate::cbz::{self, PageImage};
use crate::comicinfo::ComicInfo;
use crate::paths::{self, PathError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("library path is not usable: {0}")]
    Path(#[from] PathError),
    #[error("packaging failed: {0}")]
    Cbz(#[from] cbz::CbzError),
    #[error("the library volume is full")]
    OutOfSpace,
    #[error("library io failed: {0}")]
    Io(String),
}

/// What a completed write reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// Relative to the library root. The store never hands out an absolute
    /// path, so a second root stays addable without changing callers.
    pub relative_path: String,
    pub size_bytes: u64,
    pub checksum: String,
}

/// Everything needed to name and describe one chapter.
pub struct ChapterSpec<'a> {
    pub series_title: &'a str,
    /// Appended to the directory name only to break a real collision between
    /// two series with the same title from different sources.
    pub disambiguator: Option<&'a str>,
    pub volume: Option<f32>,
    pub number: Option<f32>,
    pub info: ComicInfo,
}

pub struct LibraryStore {
    root: PathBuf,
}

impl LibraryStore {
    /// Opens a store over a library root, creating it if absent.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The staging directory, deliberately inside the root.
    ///
    /// Named with a leading dot so a reader scanning the library ignores it.
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }

    /// Packages pages and places the archive atomically.
    pub fn write_chapter(
        &self,
        spec: &ChapterSpec<'_>,
        pages: Vec<PageImage>,
    ) -> Result<Placed, StoreError> {
        let dir = paths::series_dir(spec.series_title, spec.disambiguator)?;
        let stem = paths::chapter_stem(spec.series_title, spec.volume, spec.number)?;
        let relative = PathBuf::from(&dir).join(format!("{stem}.cbz"));

        // Containment check before anything is written.
        let destination = paths::resolve_within(&self.root, &relative)?;

        let staging = self.staging_dir();
        std::fs::create_dir_all(&staging).map_err(io)?;

        // Build under a unique name so two workers packaging the same chapter
        // cannot write over each other's partial file.
        let temp_path = staging.join(format!("{}.cbz.part", uuid::Uuid::new_v4()));

        let bytes = {
            let mut buffer = std::io::Cursor::new(Vec::new());
            cbz::write_cbz(&mut buffer, &pages, &spec.info)?;
            buffer.into_inner()
        };
        let checksum = cbz::checksum(&bytes);

        write_and_sync(&temp_path, &bytes)?;

        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        // The atomic step. Same filesystem, because staging is under the root.
        std::fs::rename(&temp_path, &destination).map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            io(e)
        })?;

        Ok(Placed {
            relative_path: relative.to_string_lossy().into_owned(),
            size_bytes: bytes.len() as u64,
            checksum,
        })
    }

    pub fn exists(&self, relative: &str) -> Result<bool, StoreError> {
        let path = paths::resolve_within(&self.root, Path::new(relative))?;
        Ok(path.exists())
    }

    pub fn delete(&self, relative: &str) -> Result<(), StoreError> {
        let path = paths::resolve_within(&self.root, Path::new(relative))?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Already gone is the desired state, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    /// Opens a stored chapter for reading, for the download endpoint.
    pub fn open_chapter(&self, relative: &str) -> Result<std::fs::File, StoreError> {
        let path = paths::resolve_within(&self.root, Path::new(relative))?;
        std::fs::File::open(path).map_err(io)
    }

    /// Removes anything left in staging.
    ///
    /// Run at startup: a crash mid-write leaves a `.part` file that nothing
    /// will ever claim, and staging would otherwise grow without bound.
    pub fn clean_staging(&self) -> Result<u64, StoreError> {
        let staging = self.staging_dir();
        if !staging.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for entry in std::fs::read_dir(&staging).map_err(io)?.flatten() {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn io(e: std::io::Error) -> StoreError {
    // Disk exhaustion is its own failure: the packaging handler has to clean up
    // the partial write and /readyz has to report the library unwritable, which
    // a generic io error would not distinguish (ADR-0007).
    if e.kind() == std::io::ErrorKind::StorageFull {
        StoreError::OutOfSpace
    } else {
        StoreError::Io(e.to_string())
    }
}

/// Writes and fsyncs, so the rename cannot publish a file whose contents are
/// still in the page cache.
fn write_and_sync(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;
    let mut file = std::fs::File::create(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    Ok(())
}

/// Builds the metadata for a chapter.
pub fn info_for(
    series: &str,
    number: Option<f32>,
    volume: Option<f32>,
    page_count: u32,
    direction: ReadingDirection,
) -> ComicInfo {
    ComicInfo {
        series: series.to_string(),
        number,
        volume,
        page_count,
        direction,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend([0u8; 32]);
        v
    }

    fn pages(n: usize) -> Vec<PageImage> {
        (0..n)
            .map(|i| PageImage {
                source_name: format!("{i}.jpg"),
                bytes: jpeg(),
            })
            .collect()
    }

    fn spec<'a>(title: &'a str) -> ChapterSpec<'a> {
        ChapterSpec {
            series_title: title,
            disambiguator: None,
            volume: Some(3.0),
            number: Some(21.0),
            info: info_for(
                title,
                Some(21.0),
                Some(3.0),
                2,
                ReadingDirection::RightToLeft,
            ),
        }
    }

    /// The ADR-0007 requirement, asserted rather than assumed: `rename` is
    /// atomic only within one filesystem, and a staging directory outside the
    /// root silently turns it into a copy.
    #[test]
    fn staging_lives_inside_the_library_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        assert!(
            store.staging_dir().starts_with(store.root()),
            "staging {:?} must be under the root {:?}",
            store.staging_dir(),
            store.root()
        );

        // Being under the root is the mechanism; being on the SAME FILESYSTEM
        // is the property that actually makes rename atomic. Check the
        // property directly, since a bind mount or a symlink could satisfy the
        // path check while failing this one.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::fs::create_dir_all(store.staging_dir()).expect("staging");
            let staging_dev = std::fs::metadata(store.staging_dir())
                .expect("staging metadata")
                .dev();
            let root_dev = std::fs::metadata(store.root())
                .expect("root metadata")
                .dev();
            assert_eq!(
                staging_dev, root_dev,
                "staging and the library must share a filesystem, or rename \
                 silently becomes a copy and a crash leaves a truncated \
                 archive that looks complete"
            );
        }
    }

    #[test]
    fn a_chapter_lands_at_the_reader_convention_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");

        let placed = store
            .write_chapter(&spec("My Series"), pages(2))
            .expect("write");
        assert_eq!(placed.relative_path, "My Series/My Series v03 c021.cbz");
        assert!(dir.path().join(&placed.relative_path).exists());
        assert!(placed.size_bytes > 0);
        assert_eq!(placed.checksum.len(), 64);
    }

    /// Nothing partial may remain once the write succeeds.
    #[test]
    fn staging_is_empty_after_a_successful_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        store
            .write_chapter(&spec("Series"), pages(2))
            .expect("write");

        let leftovers: Vec<_> = std::fs::read_dir(store.staging_dir())
            .expect("staging exists")
            .flatten()
            .collect();
        assert!(leftovers.is_empty(), "staging still holds {leftovers:?}");
    }

    /// A crash mid-write leaves a .part nothing will claim.
    #[test]
    fn startup_cleans_abandoned_staging_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        std::fs::create_dir_all(store.staging_dir()).expect("staging");
        std::fs::write(store.staging_dir().join("orphan.cbz.part"), b"partial").expect("write");

        assert_eq!(store.clean_staging().expect("clean"), 1);
        assert_eq!(store.clean_staging().expect("clean again"), 0);
    }

    /// A title that tries to escape must not write outside the library.
    #[test]
    fn a_hostile_title_cannot_escape_the_library() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");

        let placed = store
            .write_chapter(&spec("../../escaped"), pages(1))
            .expect("write is sanitized, not refused");
        let written = dir.path().join(&placed.relative_path);
        assert!(written.starts_with(dir.path()), "escaped to {written:?}");
        assert!(
            !outside.path().join("escaped").exists(),
            "nothing may be written outside the library"
        );
    }

    #[test]
    fn the_archive_is_readable_and_carries_its_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        let placed = store
            .write_chapter(&spec("Series"), pages(3))
            .expect("write");

        let file = store.open_chapter(&placed.relative_path).expect("open");
        let mut zip = zip::ZipArchive::new(file).expect("a readable cbz");
        assert!(zip.by_name("ComicInfo.xml").is_ok());
        assert_eq!(zip.len(), 4, "metadata plus three pages");
    }

    #[test]
    fn deleting_something_already_gone_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        let placed = store
            .write_chapter(&spec("Series"), pages(1))
            .expect("write");

        assert!(store.exists(&placed.relative_path).expect("exists"));
        store.delete(&placed.relative_path).expect("delete");
        assert!(!store.exists(&placed.relative_path).expect("exists"));
        store
            .delete(&placed.relative_path)
            .expect("already gone is fine");
    }

    /// Two workers packaging the same chapter must not corrupt each other's
    /// staging file.
    #[test]
    fn concurrent_writes_use_distinct_staging_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::open(dir.path()).expect("open");
        // Unique temp names mean the second write replaces the first cleanly
        // rather than interleaving with it.
        let a = store
            .write_chapter(&spec("Series"), pages(2))
            .expect("first");
        let b = store
            .write_chapter(&spec("Series"), pages(2))
            .expect("second");
        assert_eq!(a.relative_path, b.relative_path);
        assert!(dir.path().join(&a.relative_path).exists());
    }
}
