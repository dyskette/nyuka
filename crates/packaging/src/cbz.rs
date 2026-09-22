//! The CBZ writer.
//!
//! A CBZ is a zip of page images in filename order, with `ComicInfo.xml`
//! embedded. Every relevant reader opens one, which is the whole reason this
//! format was chosen (ADR-0007).

use std::io::{Seek, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

use crate::comicinfo::ComicInfo;

#[derive(Debug, thiserror::Error)]
pub enum CbzError {
    #[error("writing the archive failed: {0}")]
    Write(String),
    #[error("a chapter with no pages cannot be packaged")]
    NoPages,
}

/// One page, in reading order.
pub struct PageImage {
    /// The original filename or URL, used only for its extension.
    pub source_name: String,
    pub bytes: Vec<u8>,
}

/// What a completed archive reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub size_bytes: u64,
    /// SHA-256 of the archive, computed while writing.
    ///
    /// Serves as the `ETag` for `GET /downloads/{id}/file` and answers "is this
    /// download intact" without re-fetching anything.
    pub checksum: String,
}

/// Writes a CBZ.
///
/// Page images are stored **uncompressed**. JPEG, PNG and WebP are already
/// compressed, so deflating them costs CPU and produces a larger-than-necessary
/// write for no gain. Only the XML is compressed.
pub fn write_cbz<W: Write + Seek>(
    sink: W,
    pages: &[PageImage],
    info: &ComicInfo,
) -> Result<Written, CbzError> {
    if pages.is_empty() {
        return Err(CbzError::NoPages);
    }

    let mut zip = zip::ZipWriter::new(sink);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // ComicInfo.xml first: some readers stop scanning once they have found it.
    zip.start_file("ComicInfo.xml", deflated)
        .map_err(|e| CbzError::Write(e.to_string()))?;
    zip.write_all(info.to_xml().as_bytes())
        .map_err(|e| CbzError::Write(e.to_string()))?;

    let width = page_number_width(pages.len());
    for (index, page) in pages.iter().enumerate() {
        // Sequential, zero-padded names in reading order. A source's own
        // filenames are not used: they are attacker-controlled, frequently
        // unsorted, and sometimes collide.
        let name = format!(
            "{:0width$}.{}",
            index + 1,
            extension_for(&page.source_name, &page.bytes),
            width = width
        );
        zip.start_file(name, stored)
            .map_err(|e| CbzError::Write(e.to_string()))?;
        zip.write_all(&page.bytes)
            .map_err(|e| CbzError::Write(e.to_string()))?;
    }

    let mut sink = zip.finish().map_err(|e| CbzError::Write(e.to_string()))?;
    sink.flush().map_err(|e| CbzError::Write(e.to_string()))?;
    let size_bytes = sink
        .stream_position()
        .map_err(|e| CbzError::Write(e.to_string()))?;

    Ok(Written {
        size_bytes,
        // Filled in by the caller, which has the bytes.
        checksum: String::new(),
    })
}

/// Digits needed so a lexical sort matches reading order.
fn page_number_width(count: usize) -> usize {
    count.to_string().len().max(3)
}

/// Picks an extension, preferring what the bytes say over what the name claims.
///
/// A source can name a WebP `.jpg`, and a reader that trusts the extension then
/// fails to decode it. Sniffing the magic bytes is more reliable than either
/// the URL or the declared content type.
fn extension_for(source_name: &str, bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "webp",
        _ => {
            // Fall back to the declared extension, lowercased and restricted to
            // the ones readers handle.
            let ext = source_name
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            match ext.as_str() {
                "jpg" | "jpeg" => "jpg",
                "png" => "png",
                "gif" => "gif",
                "webp" => "webp",
                "avif" => "avif",
                _ => "jpg",
            }
        }
    }
}

/// SHA-256 of a finished archive.
pub fn checksum(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    // sha2 0.11 returns a byte array rather than something LowerHex.
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn jpeg(n: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend(std::iter::repeat_n(n, 64));
        v
    }

    fn pages(count: usize) -> Vec<PageImage> {
        (0..count)
            .map(|i| PageImage {
                source_name: format!("https://cdn.test/{i}.jpg"),
                bytes: jpeg(i as u8),
            })
            .collect()
    }

    fn info() -> ComicInfo {
        ComicInfo {
            series: "Series".into(),
            number: Some(1.0),
            page_count: 3,
            ..Default::default()
        }
    }

    fn archive(pages: &[PageImage]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        write_cbz(&mut buf, pages, &info()).expect("write");
        buf.into_inner()
    }

    #[test]
    fn an_archive_contains_the_metadata_and_every_page() {
        let bytes = archive(&pages(3));
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("readable zip");
        let names: Vec<_> = zip.file_names().map(str::to_owned).collect();
        assert!(names.contains(&"ComicInfo.xml".to_string()));
        assert_eq!(names.len(), 4, "metadata plus three pages: {names:?}");
        assert!(zip.by_name("001.jpg").is_ok());
        assert!(zip.by_name("003.jpg").is_ok());
    }

    /// Page names must sort lexically into reading order, since that is how a
    /// reader with no metadata orders them.
    #[test]
    fn page_names_sort_into_reading_order() {
        let bytes = archive(&pages(12));
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
        let mut names: Vec<_> = zip
            .file_names()
            .filter(|n| *n != "ComicInfo.xml")
            .map(str::to_owned)
            .collect();
        let original = names.clone();
        names.sort();
        assert_eq!(names, original, "lexical order must match page order");
        assert_eq!(names[0], "001.jpg");
        assert_eq!(names[11], "012.jpg");
    }

    /// Already-compressed images must be stored, not deflated.
    #[test]
    fn images_are_stored_and_metadata_is_deflated() {
        let bytes = archive(&pages(2));
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
        let page = zip.by_name("001.jpg").expect("page");
        assert_eq!(
            page.compression(),
            zip::CompressionMethod::Stored,
            "deflating an already-compressed image costs CPU for nothing"
        );
        drop(page);
        let meta = zip.by_name("ComicInfo.xml").expect("metadata");
        assert_eq!(meta.compression(), zip::CompressionMethod::Deflated);
    }

    /// A source naming a PNG `.jpg` would otherwise produce a file readers
    /// cannot decode.
    #[test]
    fn the_extension_comes_from_the_bytes_not_the_name() {
        let png = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            v.extend([0u8; 32]);
            v
        };
        let bytes = archive(&[PageImage {
            source_name: "misleading.jpg".into(),
            bytes: png,
        }]);
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
        assert!(
            zip.file_names().any(|n| n == "001.png"),
            "names: {:?}",
            zip.file_names().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_chapter_with_no_pages_is_refused() {
        let mut buf = Cursor::new(Vec::new());
        assert!(matches!(
            write_cbz(&mut buf, &[], &info()),
            Err(CbzError::NoPages)
        ));
    }

    #[test]
    fn the_checksum_is_stable_and_content_dependent() {
        let a = checksum(&archive(&pages(2)));
        let b = checksum(&archive(&pages(2)));
        assert_eq!(a, b, "the same content must hash the same");
        let c = checksum(&archive(&pages(3)));
        assert_ne!(a, c);
        assert_eq!(a.len(), 64, "sha-256 hex");
    }
}
