//! Host conformance fixture.
//!
//! Every check runs inside the guest against HTML compiled into this module,
//! and each result is returned as one `Manga` entry: `key` is the check name,
//! `title` is `PASS` or `FAIL: <detail>`. The host test asserts that every
//! entry passed, so a host bug is reported by name instead of surfacing as an
//! empty result.
#![no_std]

use aidoku::{
    Manga, MangaPageResult, Result, Source,
    alloc::{String, Vec, format},
    imports::html::Html,
    prelude::*,
};

/// Fixture HTML. Shaped like a real listing page: a wrapper, repeated cards,
/// nested titles and covers, an entry missing its cover, and an entity to
/// decode.
/// The document's base, as `Request::html()` would supply it.
const BASE_URL: &str = "https://example.test/browse?page=1";

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

struct Conformance;

struct Report {
    checks: Vec<Manga>,
}

impl Report {
    fn new() -> Self {
        Self { checks: Vec::new() }
    }

    fn check(&mut self, name: &str, got: Option<String>, want: Option<&str>) {
        let ok = match (got.as_deref(), want) {
            (Some(g), Some(w)) => g == w,
            (None, None) => true,
            _ => false,
        };
        let title = if ok {
            String::from("PASS")
        } else {
            format!("FAIL: want {:?} got {:?}", want, got.as_deref())
        };
        self.checks.push(Manga {
            key: String::from(name),
            title,
            ..Default::default()
        });
    }

    fn check_usize(&mut self, name: &str, got: usize, want: usize) {
        let title = if got == want {
            String::from("PASS")
        } else {
            format!("FAIL: want {want} got {got}")
        };
        self.checks.push(Manga {
            key: String::from(name),
            title,
            ..Default::default()
        });
    }
}

impl Source for Conformance {
    fn new() -> Self {
        Self
    }

    /// Runs the battery. `query`, `page`, and `filters` are ignored: the host
    /// test calls this with an empty query and no filters.
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
                r.check("html::parse_fragment", Some(format!("{e:?}")), Some("ok"));
                return Ok(MangaPageResult {
                    entries: r.checks,
                    has_next_page: false,
                });
            }
        };
        r.check("html::parse_fragment", Some(String::from("ok")), Some("ok"));

        // --- select / size ---------------------------------------------------
        let cards = doc.select("a.card");
        let count = cards.as_ref().map(|c| c.size()).unwrap_or(0);
        r.check_usize("html::select a.card -> size", count, 3);

        // A selector that matches nothing must be a miss, not an error.
        let none = doc.select("a.nonexistent");
        r.check_usize(
            "html::select no match -> size 0",
            none.as_ref().map(|c| c.size()).unwrap_or(0),
            0,
        );

        // --- attr on the first card -----------------------------------------
        if let Some(first) = doc.select_first("a.card") {
            r.check("html::attr href", first.attr("href"), Some("/series/1"));
            r.check("html::attr title", first.attr("title"), Some("First"));
            // A missing attribute must read as absent, not as an empty string.
            r.check("html::attr missing -> None", first.attr("data-nope"), None);

            // --- nested select_first + text ---------------------------------
            r.check(
                "nested select_first h3.t -> text",
                first.select_first("h3.t").and_then(|e| e.text()),
                // Entity decoded, whitespace collapsed.
                Some("Alpha & Co"),
            );
            r.check(
                "nested select_first img.cover -> attr src",
                first.select_first("img.cover").and_then(|e| e.attr("src")),
                Some("/c1.jpg"),
            );

            // select must search DESCENDANTS, so an <a> must not match itself.
            r.check_usize(
                "select excludes self",
                first.select("a.card").as_ref().map(|c| c.size()).unwrap_or(0),
                0,
            );
        } else {
            r.check("html::select_first a.card", None, Some("found"));
        }

        // --- indexing a selection -------------------------------------------
        if let Some(list) = doc.select("a.card") {
            r.check(
                "select->get(1) attr href",
                list.get(1).and_then(|e| e.attr("href")),
                Some("/series/2"),
            );
            r.check(
                "select->get(2) nested text with inline child",
                list.get(2)
                    .and_then(|e| e.select_first("h3.t"))
                    .and_then(|e| e.text()),
                // `text` is the whole subtree, including the <em>.
                Some("Gamma (side)"),
            );
            // own_text excludes child elements.
            r.check(
                "own_text excludes children",
                list.get(2)
                    .and_then(|e| e.select_first("h3.t"))
                    .and_then(|e| e.own_text()),
                Some("Gamma"),
            );
            // A missing cover must be absent rather than empty.
            r.check(
                "missing child -> None",
                list.get(2).and_then(|e| e.select_first("img.cover")).and_then(|e| e.attr("src")),
                None,
            );
            // Out-of-range indexing is a miss.
            r.check(
                "select->get out of range -> None",
                list.get(99).and_then(|e| e.attr("href")),
                None,
            );
        }

        // --- an element with no text ----------------------------------------
        // `None`, not `Some("")`. The guest's read_string does
        // `if string.is_empty() { None }`, so an empty string and an absent
        // value are INDISTINGUISHABLE across this boundary. A host cannot
        // signal "found, but empty" — worth knowing before designing any host
        // behaviour that depends on the difference.
        r.check(
            "empty element text -> None (empty is indistinguishable from absent)",
            doc.select_first("div.empty").and_then(|e| e.text()),
            None,
        );

        // --- attribute on a container ---------------------------------------
        r.check(
            "attr on container",
            doc.select_first("div.grid").and_then(|e| e.attr("data-count")),
            Some("3"),
        );

        // --- base_uri and the `abs:` prefix ---------------------------------
        // Jsoup's convention, and how sources turn relative hrefs and image
        // srcs into absolute URLs. A host that treats `abs:href` as a literal
        // attribute name finds nothing and the source discards the entry.
        r.check(
            "html::base_uri",
            // base_uri is on Element, not Document.
            doc.select_first("div.grid").and_then(|e| e.base_uri()),
            Some(BASE_URL),
        );

        if let Some(first) = doc.select_first("a.card") {
            r.check(
                "attr abs:href resolves against base",
                first.attr("abs:href"),
                Some("https://example.test/series/1"),
            );
            r.check(
                "attr abs: on nested img src",
                first.select_first("img.cover").and_then(|e| e.attr("abs:src")),
                Some("https://example.test/c1.jpg"),
            );
            // A plain lookup must still return the raw, unresolved value.
            r.check(
                "attr href stays relative without abs:",
                first.attr("href"),
                Some("/series/1"),
            );
            // abs: on a missing attribute is still a miss.
            r.check("attr abs: on missing -> None", first.attr("abs:nope"), None);
        }

        if let Some(list) = doc.select("a.card") {
            // Base must propagate through select -> get -> nested select_first.
            r.check(
                "base propagates through select->get",
                list.get(1).and_then(|e| e.attr("abs:href")),
                Some("https://example.test/series/2"),
            );
        }

        Ok(MangaPageResult {
            entries: r.checks,
            has_next_page: false,
        })
    }

    fn get_manga_update(
        &self,
        manga: Manga,
        _needs_details: bool,
        _needs_chapters: bool,
    ) -> Result<Manga> {
        Ok(manga)
    }

    fn get_page_list(
        &self,
        _manga: Manga,
        _chapter: aidoku::Chapter,
    ) -> Result<Vec<aidoku::Page>> {
        Ok(Vec::new())
    }
}

register_source!(Conformance);
