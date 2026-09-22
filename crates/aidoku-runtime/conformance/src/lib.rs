//! Host conformance fixture.
//!
//! Every check runs inside the guest, against data compiled into this module,
//! and the result is reported back through whatever channel the entry point
//! already has:
//!
//! | Entry point             | Report channel                        |
//! |-------------------------|---------------------------------------|
//! | `get_search_manga_list` | one `Manga` per check: `key` is the check name, `title` is `PASS` or a `FAIL` with both values |
//! | `get_manga_update`      | the returned manga's `tags`           |
//! | `get_page_list`         | one `Page` per check, as `PageContent::Text` |
//!
//! So a host bug is reported by name instead of surfacing as an empty result,
//! and the last two also prove that the host's postcard mirrors agree with the
//! guest's structs — a host that encodes `Manga` wrongly cannot even be called.
//!
//! See ADR-0004.
#![no_std]

use aidoku::{
    Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Source,
    Viewer,
    alloc::{String, Vec, format, string::ToString, vec},
    imports::{
        defaults::{DefaultValue, defaults_get, defaults_set},
        html::Html,
        std::parse_date,
    },
    prelude::*,
};

/// The document's base, as `Request::html()` would supply it.
const BASE_URL: &str = "https://example.test/browse?page=1";

/// Fixture HTML. Shaped like a real listing page: a wrapper, repeated cards,
/// nested titles and covers, an entry missing its cover, and an entity to
/// decode.
const FIXTURE: &str = r#"<div id="wrap">
  <div class="grid" data-count="3">
    <a class="card" href="/series/1" title="First">
      <img class="cover" src="/c1.jpg" alt="Alpha &amp; Co">
      <h3 class="t">  Alpha   &amp; Co  </h3>
      <span class="ch">Ch. 12</span>
    </a>
    <a class="card" href="/series/2">
      <img class="cover" src="/c2.jpg">
      <h3 class="t">Beta</h3>
      <span class="ch">Ch. 3</span>
    </a>
    <a class="card" href="/series/3">
      <h3 class="t">Gamma<em> (side)</em></h3>
    </a>
  </div>
  <div class="empty"></div>
</div>"#;

// What the host is expected to send into get_manga_update and get_page_list.
// The host builds these from its own mirrors, so agreement here is agreement
// about the wire format.
const PROBE_MANGA_KEY: &str = "probe-key";
const PROBE_MANGA_TITLE: &str = "Probe Title";
const PROBE_COVER: &str = "https://example.test/probe.jpg";
const PROBE_CHAPTER_KEY: &str = "probe-chapter";
const PROBE_CHAPTER_TITLE: &str = "Probe Chapter";
const PROBE_NEXT_UPDATE: i64 = 1_700_000_000;

struct Conformance;

/// Collects `name=PASS` / `name=FAIL: …` lines, rendered into whichever shape
/// the calling entry point returns.
struct Report {
    lines: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn record(&mut self, name: &str, ok: bool, detail: String) {
        self.lines.push(if ok {
            format!("{name}=PASS")
        } else {
            format!("{name}=FAIL: {detail}")
        });
    }

    fn str(&mut self, name: &str, got: Option<String>, want: Option<&str>) {
        let ok = match (got.as_deref(), want) {
            (Some(g), Some(w)) => g == w,
            (None, None) => true,
            _ => false,
        };
        self.record(name, ok, format!("want {:?} got {:?}", want, got.as_deref()));
    }

    fn usize(&mut self, name: &str, got: usize, want: usize) {
        self.record(name, got == want, format!("want {want} got {got}"));
    }

    fn i64(&mut self, name: &str, got: Option<i64>, want: Option<i64>) {
        self.record(name, got == want, format!("want {want:?} got {got:?}"));
    }

    fn bool(&mut self, name: &str, got: bool, want: bool) {
        self.record(name, got == want, format!("want {want} got {got}"));
    }

    fn f32(&mut self, name: &str, got: Option<f32>, want: Option<f32>) {
        let ok = match (got, want) {
            (Some(g), Some(w)) => (g - w).abs() < f32::EPSILON,
            (None, None) => true,
            _ => false,
        };
        self.record(name, ok, format!("want {want:?} got {got:?}"));
    }

    /// One `Manga` per check, for `get_search_manga_list`.
    fn into_entries(self) -> Vec<Manga> {
        self.lines
            .into_iter()
            .map(|line| {
                let (name, result) = split_once_eq(&line);
                Manga {
                    key: name,
                    title: result,
                    ..Default::default()
                }
            })
            .collect()
    }

    /// One `Page` per check, for `get_page_list`.
    fn into_pages(self) -> Vec<Page> {
        self.lines
            .into_iter()
            .map(|line| Page {
                content: PageContent::Text(line),
                thumbnail: None,
                has_description: false,
                description: None,
            })
            .collect()
    }
}

fn split_once_eq(line: &str) -> (String, String) {
    match line.find('=') {
        Some(i) => (line[..i].to_string(), line[i + 1..].to_string()),
        None => (line.to_string(), String::new()),
    }
}

impl Source for Conformance {
    fn new() -> Self {
        Self
    }

    /// The `html`, `std` and `defaults` battery.
    fn get_search_manga_list(
        &self,
        _query: Option<String>,
        _page: i32,
        _filters: Vec<aidoku::FilterValue>,
    ) -> Result<MangaPageResult> {
        let mut r = Report::new();

        // Parsed WITH a base URL, because that is how a source receives a page
        // from `Request::html()` and it is what `abs:` resolution needs.
        let doc = match Html::parse_fragment_with_url(FIXTURE, BASE_URL) {
            Ok(d) => d,
            Err(e) => {
                r.str("html::parse_fragment", Some(format!("{e:?}")), Some("ok"));
                return Ok(MangaPageResult {
                    entries: r.into_entries(),
                    has_next_page: false,
                });
            }
        };
        r.str("html::parse_fragment", Some(String::from("ok")), Some("ok"));

        // --- select / size ---------------------------------------------------
        let cards = doc.select("a.card");
        r.usize(
            "html::select a.card -> size",
            cards.as_ref().map(|c| c.size()).unwrap_or(0),
            3,
        );
        // A selector that matches nothing must be a miss, not an error.
        r.usize(
            "html::select no match -> size 0",
            doc.select("a.nonexistent")
                .as_ref()
                .map(|c| c.size())
                .unwrap_or(0),
            0,
        );

        // --- attributes and text on the first card ---------------------------
        if let Some(first) = doc.select_first("a.card") {
            r.str("html::attr href", first.attr("href"), Some("/series/1"));
            r.str("html::attr title", first.attr("title"), Some("First"));
            // A missing attribute must read as absent, not as an empty string.
            r.str("html::attr missing -> None", first.attr("data-nope"), None);
            r.str(
                "nested select_first h3.t -> text",
                first.select_first("h3.t").and_then(|e| e.text()),
                // Entity decoded, whitespace collapsed.
                Some("Alpha & Co"),
            );
            r.str(
                "nested select_first img.cover -> attr src",
                first.select_first("img.cover").and_then(|e| e.attr("src")),
                Some("/c1.jpg"),
            );
            // select must search DESCENDANTS, so an <a> must not match itself.
            r.usize(
                "select excludes self",
                first
                    .select("a.card")
                    .as_ref()
                    .map(|c| c.size())
                    .unwrap_or(0),
                0,
            );
        } else {
            r.str("html::select_first a.card", None, Some("found"));
        }

        // --- indexing a selection -------------------------------------------
        if let Some(list) = doc.select("a.card") {
            r.str(
                "select->get(1) attr href",
                list.get(1).and_then(|e| e.attr("href")),
                Some("/series/2"),
            );
            r.str(
                "select->get(2) nested text with inline child",
                list.get(2)
                    .and_then(|e| e.select_first("h3.t"))
                    .and_then(|e| e.text()),
                // `text` is the whole subtree, including the <em>.
                Some("Gamma (side)"),
            );
            // own_text excludes child elements.
            r.str(
                "own_text excludes children",
                list.get(2)
                    .and_then(|e| e.select_first("h3.t"))
                    .and_then(|e| e.own_text()),
                Some("Gamma"),
            );
            // A missing cover must be absent rather than empty.
            r.str(
                "missing child -> None",
                list.get(2)
                    .and_then(|e| e.select_first("img.cover"))
                    .and_then(|e| e.attr("src")),
                None,
            );
            // Out-of-range indexing is a miss.
            r.str(
                "select->get out of range -> None",
                list.get(99).and_then(|e| e.attr("href")),
                None,
            );
        }

        // --- an element with no text ----------------------------------------
        // `None`, not `Some("")`. The guest's read_string does
        // `if string.is_empty() { None }`, so an empty string and an absent
        // value are INDISTINGUISHABLE across this boundary. A host cannot
        // signal "found, but empty".
        r.str(
            "empty element text -> None (empty is indistinguishable from absent)",
            doc.select_first("div.empty").and_then(|e| e.text()),
            None,
        );

        // --- attribute on a container ---------------------------------------
        r.str(
            "attr on container",
            doc.select_first("div.grid")
                .and_then(|e| e.attr("data-count")),
            Some("3"),
        );

        // --- base_uri and the `abs:` prefix ---------------------------------
        // Jsoup's convention, and how sources turn relative hrefs and image
        // srcs into absolute URLs. A host that treats `abs:href` as a literal
        // attribute name finds nothing and the source discards the entry.
        r.str(
            "html::base_uri",
            // base_uri is on Element, not Document.
            doc.select_first("div.grid").and_then(|e| e.base_uri()),
            Some(BASE_URL),
        );

        if let Some(first) = doc.select_first("a.card") {
            r.str(
                "attr abs:href resolves against base",
                first.attr("abs:href"),
                Some("https://example.test/series/1"),
            );
            r.str(
                "attr abs: on nested img src",
                first
                    .select_first("img.cover")
                    .and_then(|e| e.attr("abs:src")),
                Some("https://example.test/c1.jpg"),
            );
            // A plain lookup must still return the raw, unresolved value.
            r.str(
                "attr href stays relative without abs:",
                first.attr("href"),
                Some("/series/1"),
            );
            // abs: on a missing attribute is still a miss.
            r.str("attr abs: on missing -> None", first.attr("abs:nope"), None);
        }

        if let Some(list) = doc.select("a.card") {
            // Base must propagate through select -> get -> nested select_first.
            r.str(
                "base propagates through select->get",
                list.get(1).and_then(|e| e.attr("abs:href")),
                Some("https://example.test/series/2"),
            );
        }

        // --- std::parse_date -------------------------------------------------
        // Formats are Swift DateFormatter patterns, and real sources use this
        // for chapter publication dates. A host that stubs it returns 0 and
        // every chapter is dated 1970 — wrong, but never an error.
        r.i64(
            "std::parse_date MM-dd-yyyy HH:mm",
            parse_date("07-01-2025 13:00", "MM-dd-yyyy HH:mm"),
            Some(1_751_374_800),
        );
        r.i64(
            "std::parse_date yyyy-MM-dd",
            parse_date("2025-01-02", "yyyy-MM-dd"),
            Some(1_735_776_000),
        );
        r.i64(
            "std::parse_date unparseable -> None",
            parse_date("not a date", "yyyy-MM-dd"),
            None,
        );

        // --- defaults round trip ---------------------------------------------
        // Backed by the per-source key-value namespace on the host.
        defaults_set("conformance.string", DefaultValue::String("hello".into()));
        r.str(
            "defaults set/get string",
            defaults_get::<String>("conformance.string"),
            Some("hello"),
        );
        r.str(
            "defaults get unset -> None",
            defaults_get::<String>("conformance.never-set"),
            None,
        );

        Ok(MangaPageResult {
            entries: r.into_entries(),
            has_next_page: false,
        })
    }

    /// Asserts the `Manga` the host encoded, and reports through `tags`.
    ///
    /// This is the postcard round-trip check: a host whose `Manga` mirror
    /// disagrees with the guest cannot even reach this function.
    fn get_manga_update(
        &self,
        manga: Manga,
        needs_details: bool,
        needs_chapters: bool,
    ) -> Result<Manga> {
        let mut r = Report::new();

        r.str("manga.key", Some(manga.key.clone()), Some(PROBE_MANGA_KEY));
        r.str("manga.title", Some(manga.title.clone()), Some(PROBE_MANGA_TITLE));
        r.str("manga.cover", manga.cover.clone(), Some(PROBE_COVER));
        r.usize(
            "manga.authors len",
            manga.authors.as_ref().map(|a| a.len()).unwrap_or(0),
            2,
        );
        r.str(
            "manga.authors[1]",
            manga.authors.as_ref().and_then(|a| a.get(1).cloned()),
            Some("Second Author"),
        );
        r.str("manga.artists absent", manga.artists.clone().map(|_| String::from("present")), None);
        r.bool("manga.status == Ongoing", manga.status == MangaStatus::Ongoing, true);
        r.bool(
            "manga.content_rating == Safe",
            manga.content_rating == ContentRating::Safe,
            true,
        );
        r.bool(
            "manga.viewer == RightToLeft",
            manga.viewer == Viewer::RightToLeft,
            true,
        );
        r.i64("manga.next_update_time", manga.next_update_time, Some(PROBE_NEXT_UPDATE));

        // Nested Vec<Chapter> is where a field-order mistake shows up first.
        let chapters = manga.chapters.clone().unwrap_or_default();
        r.usize("manga.chapters len", chapters.len(), 1);
        if let Some(c) = chapters.first() {
            r.str("chapter.key", Some(c.key.clone()), Some(PROBE_CHAPTER_KEY));
            r.str("chapter.title", c.title.clone(), Some(PROBE_CHAPTER_TITLE));
            r.f32("chapter.chapter_number", c.chapter_number, Some(1.5));
            r.f32("chapter.volume_number", c.volume_number, Some(2.0));
            r.i64("chapter.date_uploaded", c.date_uploaded, Some(PROBE_NEXT_UPDATE));
            r.bool("chapter.locked", c.locked, true);
            r.str("chapter.language", c.language.clone(), Some("en"));
        }

        // The two flags must arrive distinguishable, not both true.
        r.bool("needs_details", needs_details, true);
        r.bool("needs_chapters", needs_chapters, false);

        Ok(Manga {
            key: manga.key,
            title: manga.title,
            tags: Some(r.lines),
            ..Default::default()
        })
    }

    /// Asserts both structs the host encoded, and reports one `Page` per check.
    fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
        let mut r = Report::new();
        r.str(
            "page_list manga.key",
            Some(manga.key),
            Some(PROBE_MANGA_KEY),
        );
        r.str(
            "page_list chapter.key",
            Some(chapter.key),
            Some(PROBE_CHAPTER_KEY),
        );
        r.f32("page_list chapter.chapter_number", chapter.chapter_number, Some(1.5));
        r.str("page_list chapter.title", chapter.title, Some(PROBE_CHAPTER_TITLE));
        r.bool("page_list chapter.locked", chapter.locked, true);
        r.usize(
            "page_list chapter.scanlators len",
            chapter.scanlators.as_ref().map(|s| s.len()).unwrap_or(0),
            1,
        );
        Ok(r.into_pages())
    }
}

/// Keeps `vec!` referenced so the import is used regardless of feature shape.
#[allow(dead_code)]
fn _unused() -> Vec<String> {
    vec![String::new()]
}

register_source!(Conformance);
