//! `ComicInfo.xml` against the real ComicInfo v2.0 schema, and the archive
//! against the layout Komga and Kavita document.
//!
//! ADR-0007 follow-up 4. Interoperability is the *reason* CBZ was chosen — a
//! library other tools can open is the property the whole storage decision
//! exists for — and until this, nothing checked that a generated archive
//! satisfies it. The unit tests assert individual fields and count `<` against
//! `>`, which is not validation.
//!
//! # Element order is not cosmetic
//!
//! ComicInfo v2.0 declares its elements in an `xs:sequence`, so a document
//! whose elements are in a different order is invalid however complete it is.
//! Nothing but a schema validator catches that.
//!
//! Skipped when `xmllint` is absent.

use std::io::Write;

use nyuka_domain::model::ReadingDirection;
use nyuka_packaging::cbz::{PageImage, write_cbz};
use nyuka_packaging::comicinfo::ComicInfo;
use nyuka_packaging::paths::{chapter_stem, series_dir};

/// The schema, vendored.
///
/// From <https://github.com/anansi-project/comicinfo>, `schema/v2.0`. Vendored
/// rather than fetched so this test does not depend on a network or on a
/// third-party repository staying reachable — and so the version it validates
/// against cannot change underneath a passing build.
const SCHEMA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ComicInfo-v2.0.xsd"
);

/// Validates a document, returning `None` when `xmllint` is not installed.
fn validate(xml: &str) -> Option<Result<(), String>> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ComicInfo.xml");
    let mut file = std::fs::File::create(&path).expect("create");
    file.write_all(xml.as_bytes()).expect("write");
    drop(file);

    let output = match std::process::Command::new("xmllint")
        .args(["--noout", "--schema", SCHEMA])
        .arg(&path)
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: xmllint is not installed");
            return None;
        }
        Err(e) => panic!("running xmllint: {e}"),
    };

    Some(if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    })
}

/// Everything ADR-0007 says to populate, with realistic values.
fn full() -> ComicInfo {
    ComicInfo {
        series: "Ashfall Chronicle".into(),
        title: Some("The Last Cartographer".into()),
        number: Some(21.0),
        volume: Some(3.0),
        summary: Some("A glassblower inherits an orchard.".into()),
        genres: vec!["Fantasy".into(), "Drama".into()],
        language_iso: Some("en".into()),
        page_count: 42,
        web: Some("https://example.test/series/ashfall".into()),
        year: Some(2026),
        month: Some(9),
        day: Some(23),
        direction: ReadingDirection::RightToLeft,
        ..Default::default()
    }
}

#[test]
fn a_fully_populated_document_validates() {
    let Some(result) = validate(&full().to_xml()) else {
        return;
    };
    result.expect("a full ComicInfo must satisfy the v2.0 schema");
}

/// The common case: a source that gave almost nothing. Every element in v2.0
/// is `minOccurs="0"`, so an empty document is valid — but only if the writer
/// omits what it does not have rather than emitting a placeholder of the wrong
/// type.
#[test]
fn a_nearly_empty_document_validates() {
    let info = ComicInfo {
        series: "Unknown".into(),
        page_count: 1,
        ..Default::default()
    };
    let Some(result) = validate(&info.to_xml()) else {
        return;
    };
    result.expect("a sparse ComicInfo must still validate");
}

/// A half chapter is ordinary — 10.5 is a real chapter — and `Number` is
/// `xs:string`, so it belongs there verbatim.
#[test]
fn a_fractional_chapter_number_validates() {
    let info = ComicInfo {
        number: Some(10.5),
        ..full()
    };
    let Some(result) = validate(&info.to_xml()) else {
        return;
    };
    result.expect("chapter 10.5 must validate");
}

/// `Volume` is `xs:int` in the schema, not a string. A source that reports a
/// half volume would otherwise emit `3.5` into an integer element and produce
/// a document no strict reader accepts.
#[test]
fn a_fractional_volume_does_not_produce_an_invalid_document() {
    let info = ComicInfo {
        volume: Some(3.5),
        ..full()
    };
    let Some(result) = validate(&info.to_xml()) else {
        return;
    };
    result.expect("a fractional volume must not emit a non-integer into Volume");
}

/// Right-to-left is the case that matters for manga, and `Manga` is an
/// enumeration — a value outside it fails validation.
#[test]
fn every_reading_direction_validates() {
    for direction in [
        ReadingDirection::Unknown,
        ReadingDirection::LeftToRight,
        ReadingDirection::RightToLeft,
    ] {
        let info = ComicInfo {
            direction,
            ..full()
        };
        let Some(result) = validate(&info.to_xml()) else {
            return;
        };
        result.unwrap_or_else(|e| panic!("{direction:?} must validate: {e}"));
    }
}

/// Text a source controls ends up in these elements, so the writer has to
/// escape it. An unescaped `&` or `<` makes the document not even well-formed,
/// and every reader drops the metadata rather than the one field.
#[test]
fn hostile_text_from_a_source_stays_well_formed() {
    let info = ComicInfo {
        series: "Tom & Jerry <script>alert(1)</script>".into(),
        title: Some("\"Quoted\" & 'apostrophed'".into()),
        summary: Some("Line one\nLine two — em dash, ünïcode, 日本語".into()),
        ..full()
    };
    let Some(result) = validate(&info.to_xml()) else {
        return;
    };
    result.expect("a source's text must be escaped, not emitted raw");
}

// ---------------------------------------------------------------------------
// The layout Komga and Kavita parse
// ---------------------------------------------------------------------------

/// ADR-0007 commits to `<Series Title>/<Series Title> v03 c021.cbz`, because
/// both scanners read volume and chapter markers out of the filename.
#[test]
fn the_filename_carries_the_markers_both_scanners_read() {
    let stem = chapter_stem("Ashfall Chronicle", Some(3.0), Some(21.0)).expect("stem");

    assert_eq!(stem, "Ashfall Chronicle v03 c021");
    assert_eq!(
        series_dir("Ashfall Chronicle", None).expect("dir"),
        "Ashfall Chronicle",
        "one directory per series, with no source slug unless a collision forces one"
    );
}

/// Zero padding is what makes a lexical sort match reading order, which is how
/// a scanner with no metadata orders a series.
#[test]
fn chapter_numbers_sort_lexically_into_reading_order() {
    let mut names: Vec<String> = [1.0, 2.0, 10.0, 21.0, 100.0]
        .into_iter()
        .map(|n| chapter_stem("S", None, Some(n)).expect("stem"))
        .collect();
    let expected = names.clone();

    names.sort();
    assert_eq!(names, expected, "c001 must sort before c010, not after");
}

/// A fractional chapter keeps its fraction: 10.5 is a real chapter and must
/// not round into 10, which would collide with the chapter before it.
#[test]
fn a_half_chapter_keeps_its_number() {
    let stem = chapter_stem("S", None, Some(10.5)).expect("stem");
    assert!(stem.contains("10.5"), "got {stem}");
}

/// The archive itself: `ComicInfo.xml` at the root, which is where every
/// reader looks, and pages in filename order beside it.
#[test]
fn the_archive_holds_the_metadata_where_a_reader_looks_for_it() {
    let pages: Vec<PageImage> = (1..=3)
        .map(|n| PageImage {
            source_name: format!("{n}.jpg"),
            bytes: vec![0xFF, 0xD8, 0xFF, 0xE0, n as u8],
        })
        .collect();

    let mut buffer = std::io::Cursor::new(Vec::new());
    write_cbz(&mut buffer, &pages, &full()).expect("archive");

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(buffer.into_inner())).expect("zip");
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).expect("entry").name().to_owned())
        .collect();

    assert!(
        names.contains(&"ComicInfo.xml".to_string()),
        "the metadata must be at the archive root, not in a subdirectory: {names:?}"
    );

    let mut pages: Vec<&String> = names.iter().filter(|n| *n != "ComicInfo.xml").collect();
    let in_order = pages.clone();
    pages.sort();
    assert_eq!(
        pages, in_order,
        "page entries must already be in sorted order"
    );

    // And the embedded document is the one that validates.
    let mut entry = zip.by_name("ComicInfo.xml").expect("ComicInfo.xml");
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut entry, &mut xml).expect("read");
    if let Some(result) = validate(&xml) {
        result.expect("the embedded ComicInfo must validate");
    }
}
