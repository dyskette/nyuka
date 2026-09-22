//! The `domain::ports::LibraryStore` adapter over [`store::LibraryStore`].
//!
//! # Why the port is async and the store is not
//!
//! Zipping a chapter and fsyncing it are CPU and syscall work measured in
//! hundreds of milliseconds for a large chapter. Doing that on a runtime
//! worker thread stalls every other task sharing it, including the SSE streams
//! that are supposed to be reporting the progress of this very download. So
//! the store stays plain blocking code and this adapter moves each call onto
//! `spawn_blocking`.
//!
//! # Naming lives here, not in the store
//!
//! The store takes a [`ChapterSpec`] — a title, a volume, a number. The port
//! takes domain `Manga` and `Chapter`. Translating between them is this
//! adapter's whole job, and keeping it here is what lets `paths.rs` be tested
//! against strings rather than against a database's worth of fixtures.

use std::path::Path;
use std::sync::Arc;

use nyuka_domain::model::{Chapter, DownloadedChapter, Manga, PageRef};
use nyuka_domain::ports::{ChapterRead, LibraryStore as LibraryStorePort};
use nyuka_domain::{DomainError, Result};

use crate::cbz::PageImage;
use crate::store::{ChapterSpec, LibraryStore, StoreError, info_for};

/// Adapts the blocking store to the async port.
#[derive(Clone)]
pub struct LibraryStoreAdapter {
    store: Arc<LibraryStore>,
}

impl LibraryStoreAdapter {
    pub fn new(store: LibraryStore) -> Self {
        Self {
            store: Arc::new(store),
        }
    }

    pub fn store(&self) -> &LibraryStore {
        &self.store
    }
}

/// Maps a store failure onto the domain error the job engine branches on.
///
/// `OutOfSpace` must not collapse into `Storage`: `Storage` is retryable and
/// `InsufficientStorage` is not, and retrying a write that filled the disk
/// just fills it again on every attempt (ADR-0007).
fn map(error: StoreError) -> DomainError {
    match error {
        StoreError::OutOfSpace => DomainError::InsufficientStorage,
        other => DomainError::Storage(other.to_string()),
    }
}

/// Runs blocking store work off the runtime's worker threads.
async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        // A join error means the blocking task panicked. Reporting it as
        // `Internal` keeps it out of the retry path: a panic is a bug, and
        // retrying it produces the same panic.
        .map_err(|e| DomainError::Internal(format!("library task failed: {e}")))?
}

#[async_trait::async_trait]
impl LibraryStorePort for LibraryStoreAdapter {
    async fn write_chapter(
        &self,
        manga: &Manga,
        chapter: &Chapter,
        pages: Vec<(PageRef, Vec<u8>)>,
    ) -> Result<DownloadedChapter> {
        if pages.is_empty() {
            // An empty archive is a valid zip, so nothing downstream would
            // reject it: the row would say "downloaded" and the reader would
            // open a chapter with no pages.
            return Err(DomainError::Source {
                message: format!("chapter {} returned no pages", chapter.id),
                retryable: true,
            });
        }

        // Pages arrive in whatever order the fetches completed; the archive's
        // order is the page index, because a CBZ reader sorts by filename.
        let mut pages = pages;
        pages.sort_by_key(|(page, _)| page.index);

        let images: Vec<PageImage> = pages
            .into_iter()
            .map(|(page, bytes)| PageImage {
                source_name: page.url,
                bytes,
            })
            .collect();

        let chapter_id = chapter.id;
        let store = self.store.clone();
        let series = manga.title.clone();
        let volume = chapter.volume;
        let number = chapter.number;
        let page_count = images.len() as u32;
        let direction = manga.direction;

        let placed = blocking(move || {
            let spec = ChapterSpec {
                series_title: &series,
                // Set only when the title genuinely collides with another
                // series; the caller decides, because only it can see both.
                disambiguator: None,
                volume,
                number,
                info: info_for(&series, number, volume, page_count, direction),
            };
            store.write_chapter(&spec, images).map_err(map)
        })
        .await?;

        Ok(DownloadedChapter {
            chapter_id,
            relative_path: placed.relative_path,
            size_bytes: placed.size_bytes,
            checksum: placed.checksum,
            packaged_at: chrono::Utc::now(),
        })
    }

    async fn open_chapter(&self, path: &str) -> Result<Box<dyn ChapterRead>> {
        let store = self.store.clone();
        let path = path.to_string();
        let handle = blocking(move || {
            let file = store.open_chapter(&path).map_err(map)?;
            let size = file
                .metadata()
                .map_err(|e| DomainError::Storage(e.to_string()))?
                .len();
            Ok(OpenChapter {
                file: Arc::new(file),
                size,
            })
        })
        .await?;
        Ok(Box::new(handle))
    }

    async fn delete_chapter(&self, path: &str) -> Result<()> {
        let store = self.store.clone();
        let path = path.to_string();
        blocking(move || store.delete(&path).map_err(map)).await
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let store = self.store.clone();
        let path = path.to_string();
        blocking(move || store.exists(&path).map_err(map)).await
    }

    async fn free_bytes(&self) -> Result<u64> {
        let root = self.store.root().to_path_buf();
        blocking(move || free_bytes(&root)).await
    }
}

/// A `ChapterRead` handle over an open file.
///
/// Reads go through `read_at`, which does not move the file offset, so one
/// handle serves concurrent range requests without a lock or a seek race.
struct OpenChapter {
    file: Arc<std::fs::File>,
    size: u64,
}

#[async_trait::async_trait]
impl ChapterRead for OpenChapter {
    fn size(&self) -> u64 {
        self.size
    }

    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        // Clamp rather than fail: a caller that asked past the end gets the
        // tail, which is what a `Range` response is supposed to return.
        let remaining = self.size.saturating_sub(offset);
        let len = (len as u64).min(remaining) as usize;
        if len == 0 {
            return Ok(Vec::new());
        }

        let file = self.file.clone();
        blocking(move || {
            use std::os::unix::fs::FileExt;
            let mut buffer = vec![0u8; len];
            let mut filled = 0;
            while filled < len {
                match file.read_at(&mut buffer[filled..], offset + filled as u64) {
                    // Zero means end of file, which the clamp above should have
                    // ruled out but a concurrent truncation has not.
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(DomainError::Storage(e.to_string())),
                }
            }
            buffer.truncate(filled);
            Ok(buffer)
        })
        .await
    }
}

/// Space available to an unprivileged writer at `path`.
///
/// Reports `f_bavail`, not `f_bfree`: the difference is the reserved blocks
/// only root can use, and counting them would let `/readyz` call the library
/// writable right up to the point where writes start failing.
///
/// A failure here means the root is unreadable — usually an unmounted volume —
/// which `reconcile_library` treats as "do not conclude anything is missing".
#[cfg(unix)]
pub fn free_bytes(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statvfs(path)
        .map_err(|e| DomainError::Storage(format!("cannot stat library root: {e}")))?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(not(unix))]
pub fn free_bytes(_path: &Path) -> Result<u64> {
    Err(DomainError::Storage(
        "free space reporting is implemented for Unix only".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyuka_domain::model::{ChapterId, ExternalKey, MangaId, ReadingDirection, SourceId};
    use uuid::Uuid;

    fn jpeg() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend([0u8; 32]);
        v
    }

    fn manga(title: &str) -> Manga {
        Manga {
            id: MangaId(Uuid::new_v4()),
            source_id: SourceId(Uuid::new_v4()),
            external_key: ExternalKey("k".into()),
            title: title.into(),
            authors: vec![],
            description: None,
            genres: vec![],
            cover_url: None,
            language: None,
            direction: ReadingDirection::RightToLeft,
            updated_at: chrono::Utc::now(),
        }
    }

    fn chapter(manga_id: MangaId) -> Chapter {
        Chapter {
            id: ChapterId(Uuid::new_v4()),
            manga_id,
            external_key: ExternalKey("c".into()),
            title: None,
            number: Some(21.0),
            volume: Some(3.0),
            language: None,
            published_at: None,
        }
    }

    fn page(index: u32, name: &str) -> (PageRef, Vec<u8>) {
        (
            PageRef {
                index,
                url: name.into(),
                headers: Default::default(),
            },
            jpeg(),
        )
    }

    fn adapter(root: &Path) -> LibraryStoreAdapter {
        LibraryStoreAdapter::new(LibraryStore::open(root).expect("store opens"))
    }

    #[tokio::test]
    async fn a_written_chapter_reports_a_relative_path_that_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Test Series");
        let c = chapter(m.id);

        let download = store
            .write_chapter(&m, &c, vec![page(0, "0.jpg"), page(1, "1.jpg")])
            .await
            .expect("written");

        assert!(
            !Path::new(&download.relative_path).is_absolute(),
            "the port must never hand out an absolute path"
        );
        assert!(store.exists(&download.relative_path).await.expect("exists"));
        assert!(download.size_bytes > 0);
        assert!(!download.checksum.is_empty());
        assert_eq!(download.chapter_id, c.id);
    }

    /// Pages are fetched concurrently, so they arrive out of order. A CBZ
    /// reader sorts by filename, which means the archive's order is decided
    /// here or not at all.
    #[tokio::test]
    async fn pages_are_archived_in_index_order_regardless_of_arrival_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Ordering");
        let c = chapter(m.id);

        let download = store
            .write_chapter(
                &m,
                &c,
                vec![page(2, "c.jpg"), page(0, "a.jpg"), page(1, "b.jpg")],
            )
            .await
            .expect("written");

        let file = std::fs::File::open(dir.path().join(&download.relative_path)).expect("open");
        let mut zip = zip::ZipArchive::new(file).expect("zip");
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).expect("entry").name().to_string())
            .filter(|n| !n.ends_with(".xml"))
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "archive entry order must already be the reading order"
        );
    }

    #[tokio::test]
    async fn a_chapter_with_no_pages_is_refused_rather_than_written_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Empty");
        let c = chapter(m.id);

        let err = store
            .write_chapter(&m, &c, vec![])
            .await
            .expect_err("an empty chapter is not a download");
        assert!(
            err.is_retryable(),
            "a source that returned nothing this time may return pages next time"
        );
    }

    /// The download endpoint serves `Range` requests off this handle, so a
    /// partial read has to return the right window, not just the right length.
    #[tokio::test]
    async fn a_range_read_returns_the_bytes_at_that_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Ranges");
        let c = chapter(m.id);
        let download = store
            .write_chapter(&m, &c, vec![page(0, "0.jpg"), page(1, "1.jpg")])
            .await
            .expect("written");

        let whole = std::fs::read(dir.path().join(&download.relative_path)).expect("read");
        let handle = store
            .open_chapter(&download.relative_path)
            .await
            .expect("open");

        assert_eq!(handle.size(), whole.len() as u64);
        assert_eq!(handle.read_at(0, 4).await.expect("head"), &whole[..4]);
        assert_eq!(
            handle.read_at(10, 16).await.expect("middle"),
            &whole[10..26]
        );
    }

    #[tokio::test]
    async fn a_range_past_the_end_is_clamped_rather_than_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Clamped");
        let c = chapter(m.id);
        let download = store
            .write_chapter(&m, &c, vec![page(0, "0.jpg")])
            .await
            .expect("written");

        let handle = store
            .open_chapter(&download.relative_path)
            .await
            .expect("open");
        let size = handle.size();

        assert_eq!(
            handle.read_at(size - 3, 9999).await.expect("tail").len(),
            3,
            "an over-long range must return the tail, not an error"
        );
        assert!(
            handle.read_at(size, 16).await.expect("past end").is_empty(),
            "a read starting at the end is empty, not a failure"
        );
        assert!(
            handle
                .read_at(size + 100, 16)
                .await
                .expect("far past end")
                .is_empty()
        );
    }

    /// The handle keeps reading the archive it opened, so a request in flight
    /// is not disturbed by a delete.
    #[tokio::test]
    async fn an_open_handle_survives_the_file_being_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        let m = manga("Deleted");
        let c = chapter(m.id);
        let download = store
            .write_chapter(&m, &c, vec![page(0, "0.jpg")])
            .await
            .expect("written");

        let handle = store
            .open_chapter(&download.relative_path)
            .await
            .expect("open");
        store
            .delete_chapter(&download.relative_path)
            .await
            .expect("delete");

        assert_eq!(handle.read_at(0, 4).await.expect("still readable").len(), 4);
    }

    #[tokio::test]
    async fn opening_a_chapter_that_is_not_there_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        assert!(store.open_chapter("Nothing/here.cbz").await.is_err());
    }

    #[tokio::test]
    async fn deleting_a_chapter_that_is_already_gone_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        store
            .delete_chapter("Nothing/here.cbz")
            .await
            .expect("already absent is the desired state");
    }

    #[tokio::test]
    async fn a_path_escaping_the_root_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        assert!(
            store.exists("../../etc/passwd").await.is_err(),
            "the containment check must survive the adapter"
        );
    }

    #[tokio::test]
    async fn free_bytes_reports_a_real_number_for_a_readable_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = adapter(dir.path());
        assert!(
            store.free_bytes().await.expect("stat") > 0,
            "a writable tempdir must report space available"
        );
    }

    /// `reconcile_library` depends on this failing rather than returning zero:
    /// it is how an unmounted root is told apart from an emptied one.
    #[tokio::test]
    async fn free_bytes_fails_on_a_root_that_is_not_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("library");
        let store = adapter(&root);
        std::fs::remove_dir_all(&root).expect("remove the root out from under it");

        assert!(
            store.free_bytes().await.is_err(),
            "a vanished root must be an error, not zero free bytes"
        );
    }
}
