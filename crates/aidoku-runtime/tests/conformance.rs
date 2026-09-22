//! Runs the conformance fixture against the real host.
//!
//! The fixture is a guest compiled to wasm that carries its own HTML and
//! asserts what the host returns, reporting each check by name. It is what
//! makes the `html` module developable at all: against a live site, "the host
//! is wrong" and "the site changed" are indistinguishable (ADR-0004).
//!
//! Build it with:
//!
//! ```sh
//! crates/aidoku-runtime/conformance/package.sh
//! ```
//!
//! The `.aix` is a build artifact and is not committed, so this test skips
//! when it is absent. CI builds it explicitly and then asserts it exists, so
//! the skip cannot hide a regression there.

use std::path::PathBuf;
use std::sync::Arc;

use nyuka_aidoku_runtime::engine::{Limits, Runtime};
use nyuka_aidoku_runtime::models::{
    Chapter, ContentRating, Manga, MangaStatus, PageContent, Viewer,
};
use nyuka_aidoku_runtime::source::{Invocation, invoke};
use nyuka_aidoku_runtime::state::MemoryDefaults;

const PROBE_MANGA_KEY: &str = "probe-key";
const PROBE_CHAPTER_KEY: &str = "probe-chapter";
const PROBE_NEXT_UPDATE: i64 = 1_700_000_000;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/nyuka-conformance.aix")
}

/// Extracts `Payload/main.wasm` from the packaged fixture.
fn fixture_wasm() -> Option<Vec<u8>> {
    let path = fixture_path();
    if !path.exists() {
        eprintln!(
            "skipping: {} not built — run crates/aidoku-runtime/conformance/package.sh",
            path.display()
        );
        return None;
    }
    let file = std::fs::File::open(&path).expect("opening the fixture");
    let mut zip = zip::ZipArchive::new(file).expect("fixture is a zip");
    let mut entry = zip
        .by_name("Payload/main.wasm")
        .expect("fixture contains Payload/main.wasm");
    let mut wasm = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut wasm).expect("reading main.wasm");
    Some(wasm)
}

fn start() -> Option<Invocation> {
    let wasm = fixture_wasm()?;
    let runtime = Runtime::new(Limits::default()).expect("engine");
    let module = runtime.compile(&wasm).expect("fixture compiles");
    Some(
        invoke(&runtime, &module, Arc::new(MemoryDefaults::default()))
            .expect("fixture instantiates"),
    )
}

/// Collects failures rather than stopping at the first, so one run reports
/// everything that is wrong.
fn assert_all_passed(label: &str, checks: impl IntoIterator<Item = (String, String)>) {
    let mut failures = Vec::new();
    let mut count = 0usize;
    for (name, result) in checks {
        count += 1;
        if result != "PASS" {
            failures.push(format!("  {name}: {result}"));
        }
    }
    assert!(count > 0, "{label} produced no checks at all");
    assert!(
        failures.is_empty(),
        "{} of {count} {label} checks failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{label}: {count} checks passed");
}

/// The `html`, `std` and `defaults` battery.
///
/// Each entry's key is the check name and its title is `PASS` or a `FAIL` with
/// both values, so a host bug is named rather than surfacing as an empty
/// result.
#[test]
fn html_std_and_defaults_conform() {
    let Some(mut source) = start() else { return };
    let result = source.search(None, 1, &[]).expect("search runs");
    assert_all_passed(
        "get_search_manga_list",
        result.entries.into_iter().map(|m| (m.key, m.title)),
    );
}

/// The postcard round trip.
///
/// The host encodes a `Manga` from its own mirrors and the guest asserts every
/// field arrived. A host whose mirrors disagree with the guest cannot reach
/// this function at all, so this is what keeps `models.rs` honest.
#[test]
fn manga_and_chapter_survive_the_round_trip() {
    let Some(mut source) = start() else { return };
    let probe = probe_manga();

    // needs_details = true, needs_chapters = false, so the flags must arrive
    // distinguishable rather than both set.
    let updated = source
        .manga_update(&probe, true, false)
        .expect("manga_update runs");

    let tags = updated.tags.unwrap_or_default();
    assert_all_passed("get_manga_update", tags.into_iter().map(split_check));
}

#[test]
fn page_list_receives_both_structs() {
    let Some(mut source) = start() else { return };
    let probe = probe_manga();
    let chapter = Chapter {
        key: PROBE_CHAPTER_KEY.into(),
        title: Some("Probe Chapter".into()),
        chapter_number: Some(1.5),
        scanlators: Some(vec!["Scans".into()]),
        locked: true,
        ..Default::default()
    };

    let pages = source.page_list(&probe, &chapter).expect("page_list runs");
    assert_all_passed(
        "get_page_list",
        pages.into_iter().filter_map(|p| match p.content {
            PageContent::Text(line) => Some(split_check(line)),
            _ => None,
        }),
    );
}

fn probe_manga() -> Manga {
    Manga {
        key: PROBE_MANGA_KEY.into(),
        title: "Probe Title".into(),
        cover: Some("https://example.test/probe.jpg".into()),
        artists: None,
        authors: Some(vec!["First Author".into(), "Second Author".into()]),
        status: MangaStatus::Ongoing,
        content_rating: ContentRating::Safe,
        viewer: Viewer::RightToLeft,
        next_update_time: Some(PROBE_NEXT_UPDATE),
        chapters: Some(vec![Chapter {
            key: PROBE_CHAPTER_KEY.into(),
            title: Some("Probe Chapter".into()),
            chapter_number: Some(1.5),
            volume_number: Some(2.0),
            date_uploaded: Some(PROBE_NEXT_UPDATE),
            language: Some("en".into()),
            locked: true,
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// Splits `name=PASS` or `name=FAIL: detail`.
///
/// Splitting on the first `=` is wrong: check names contain them too, as in
/// `manga.status == Ongoing=PASS`. Anchor on the result marker instead.
fn split_check(line: String) -> (String, String) {
    if let Some(name) = line.strip_suffix("=PASS") {
        return (name.to_string(), "PASS".to_string());
    }
    if let Some(i) = line.find("=FAIL") {
        return (line[..i].to_string(), line[i + 1..].to_string());
    }
    (line, String::from("FAIL: unparseable check line"))
}

#[cfg(test)]
mod harness_tests {
    use super::split_check;

    #[test]
    fn a_check_name_containing_equals_is_split_correctly() {
        assert_eq!(
            split_check("manga.status == Ongoing=PASS".into()),
            ("manga.status == Ongoing".into(), "PASS".into())
        );
        assert_eq!(
            split_check("chapter.key=FAIL: want \"a=b\" got \"c\"".into()),
            ("chapter.key".into(), "FAIL: want \"a=b\" got \"c\"".into())
        );
    }
}
