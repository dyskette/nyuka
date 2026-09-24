//! The `html` host import, over a real mutable DOM.
//!
//! These are plain functions over the resource table, deliberately free of
//! `wasmtime`, so the semantics can be tested without a guest. The linker
//! bindings are a thin layer on top.
//!
//! Four behaviours here are load-bearing, and each was a real bug first:
//!
//! 1. **`select` searches descendants, excluding the element itself.** An `<a>`
//!    asked for `a.card` must not match itself.
//! 2. **A miss is [`NO_RESULT`], not [`INVALID_QUERY`].** The latter tells a
//!    source its selector is malformed and it abandons the entry.
//! 3. **`attr` honours Jsoup's `abs:` prefix**, resolving against the document
//!    base. Treating `abs:href` as a literal name silently discarded every
//!    entry on every scraping source.
//! 4. **`text` collapses whitespace and decodes entities; `own_text` excludes
//!    child elements.**
//!
//! [`NO_RESULT`]: crate::error::html::NO_RESULT
//! [`INVALID_QUERY`]: crate::error::html::INVALID_QUERY

use std::rc::Rc;

use dom_query::{Document, NodeId, NodeRef, Selection};

use crate::error::html as err;
use crate::resource::Html;

/// Parses a document, recording the base URI it was loaded with.
///
/// `fragment` selects the fragment parser, which is what `html::parse_fragment`
/// maps to; a source usually reaches this through `net::html` instead, where
/// the base is the URL the response came from.
pub fn parse(html: &str, base: Option<&str>, fragment: bool) -> Html {
    let doc = if fragment {
        Document::fragment(html)
    } else {
        Document::from(html)
    };
    Html {
        doc,
        base: base.filter(|b| !b.is_empty()).map(str::to_owned),
    }
}

fn node<'a>(html: &'a Html, id: NodeId) -> NodeRef<'a> {
    NodeRef {
        id,
        tree: &html.doc.tree,
    }
}

/// Descendants matching `selector`, never the node itself.
pub fn select(html: &Rc<Html>, id: NodeId, selector: &str) -> Result<Vec<NodeId>, i32> {
    // A selector that does not parse is INVALID_QUERY. A selector that parses
    // but matches nothing is an empty list — not an error, because a source
    // told its query was malformed abandons the whole entry.
    if dom_query::Matcher::new(selector).is_err() {
        return Err(err::INVALID_QUERY);
    }
    Ok(Selection::from(node(html, id))
        .try_select(selector)
        .map(|found| {
            found
                .nodes()
                .iter()
                .map(|n| n.id)
                // Descendants only: an <a> asked for `a.card` is not a match
                // for itself.
                .filter(|n| *n != id)
                .collect()
        })
        .unwrap_or_default())
}

/// The first matching descendant.
pub fn select_first(html: &Rc<Html>, id: NodeId, selector: &str) -> Result<NodeId, i32> {
    select(html, id, selector)?
        .into_iter()
        .next()
        .ok_or(err::NO_RESULT)
}

/// An attribute value, honouring Jsoup's `abs:` prefix.
///
/// A missing attribute is [`NO_RESULT`](err::NO_RESULT) rather than an empty
/// string: the guest cannot tell an empty string from an absent value, so
/// returning `""` would report a missing cover as a present, blank one.
pub fn attr(html: &Rc<Html>, id: NodeId, key: &str) -> Result<String, i32> {
    let (absolute, key) = match key.strip_prefix("abs:") {
        Some(rest) => (true, rest),
        None => (false, key),
    };
    let raw = node(html, id)
        .attr(key)
        .map(|v| v.to_string())
        .ok_or(err::NO_RESULT)?;
    if !absolute {
        return Ok(raw);
    }
    resolve(base_uri(html, id).as_deref(), &raw).ok_or(err::NO_RESULT)
}

/// All descendant text, whitespace-collapsed.
pub fn text(html: &Rc<Html>, id: NodeId) -> String {
    collapse(&node(html, id).text())
}

/// Text owned directly by this element, excluding child elements.
pub fn own_text(html: &Rc<Html>, id: NodeId) -> String {
    collapse(&node(html, id).immediate_text())
}

/// Descendant text with original whitespace preserved.
pub fn untrimmed_text(html: &Rc<Html>, id: NodeId) -> String {
    node(html, id).text().to_string()
}

/// Serialized markup including the element itself.
pub fn outer_html(html: &Rc<Html>, id: NodeId) -> String {
    node(html, id).html().to_string()
}

/// Serialized markup of the children only.
pub fn inner_html(html: &Rc<Html>, id: NodeId) -> String {
    node(html, id).inner_html().to_string()
}

/// The document base.
///
/// The URL the document was loaded from takes precedence; a `<base href>` in
/// the markup is the fallback, matching the guest's documented behaviour.
pub fn base_uri(html: &Rc<Html>, id: NodeId) -> Option<String> {
    html.base
        .clone()
        .or_else(|| node(html, id).base_uri().map(|v| v.to_string()))
}

pub fn parent(html: &Rc<Html>, id: NodeId) -> Option<NodeId> {
    node(html, id).parent().map(|n| n.id)
}

pub fn children(html: &Rc<Html>, id: NodeId) -> Vec<NodeId> {
    node(html, id).children().iter().map(|n| n.id).collect()
}

pub fn next_sibling(html: &Rc<Html>, id: NodeId) -> Option<NodeId> {
    node(html, id).next_element_sibling().map(|n| n.id)
}

pub fn prev_sibling(html: &Rc<Html>, id: NodeId) -> Option<NodeId> {
    node(html, id).prev_element_sibling().map(|n| n.id)
}

/// Siblings of a node, excluding itself.
pub fn siblings(html: &Rc<Html>, id: NodeId) -> Vec<NodeId> {
    let Some(parent) = node(html, id).parent() else {
        return Vec::new();
    };
    parent
        .element_children()
        .iter()
        .map(|n| n.id)
        .filter(|n| *n != id)
        .collect()
}

/// The first element child, or the first entry of a selection.
pub fn first_child(html: &Rc<Html>, id: NodeId) -> Option<NodeId> {
    node(html, id).first_element_child().map(|n| n.id)
}

/// The last element child.
pub fn last_child(html: &Rc<Html>, id: NodeId) -> Option<NodeId> {
    node(html, id).element_children().last().map(|n| n.id)
}

/// Escapes text for inclusion in markup.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Decodes entities, by parsing the text as a fragment and reading it back.
pub fn unescape(text: &str) -> String {
    Document::fragment(text).text().to_string()
}

pub fn tag_name(html: &Rc<Html>, id: NodeId) -> Option<String> {
    node(html, id).node_name().map(|v| v.to_string())
}

pub fn element_id(html: &Rc<Html>, id: NodeId) -> Option<String> {
    node(html, id).id_attr().map(|v| v.to_string())
}

pub fn class_name(html: &Rc<Html>, id: NodeId) -> Option<String> {
    node(html, id).class().map(|v| v.to_string())
}

pub fn has_class(html: &Rc<Html>, id: NodeId, class: &str) -> bool {
    node(html, id).has_class(class)
}

pub fn has_attr(html: &Rc<Html>, id: NodeId, name: &str) -> bool {
    node(html, id).has_attr(name)
}

// --- mutation ---------------------------------------------------------------
// `dom_query` mutates through `&self`, so a shared `Rc<Html>` is enough and no
// handle needs to be invalidated.

pub fn set_attr(html: &Rc<Html>, id: NodeId, name: &str, value: &str) {
    node(html, id).set_attr(name, value);
}

pub fn remove_attr(html: &Rc<Html>, id: NodeId, name: &str) {
    node(html, id).remove_attr(name);
}

pub fn set_text(html: &Rc<Html>, id: NodeId, text: &str) {
    node(html, id).set_text(text);
}

pub fn set_html(html: &Rc<Html>, id: NodeId, markup: &str) {
    node(html, id).set_html(markup);
}

pub fn append(html: &Rc<Html>, id: NodeId, markup: &str) {
    node(html, id).append_html(markup);
}

pub fn prepend(html: &Rc<Html>, id: NodeId, markup: &str) {
    node(html, id).prepend_html(markup);
}

pub fn remove(html: &Rc<Html>, id: NodeId) {
    node(html, id).remove_from_parent();
}

pub fn add_class(html: &Rc<Html>, id: NodeId, class: &str) {
    node(html, id).add_class(class);
}

pub fn remove_class(html: &Rc<Html>, id: NodeId, class: &str) {
    node(html, id).remove_class(class);
}

/// The root node a parsed document hands back to the guest.
pub fn root(html: &Rc<Html>) -> NodeId {
    html.doc.root().id
}

// --- helpers ----------------------------------------------------------------

/// Collapses runs of whitespace and trims, matching Jsoup's `text()`.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resolves a possibly-relative URL against a base, for the `abs:` prefix.
fn resolve(base: Option<&str>, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if let Ok(u) = url::Url::parse(value)
        && (u.scheme() == "http" || u.scheme() == "https")
    {
        return Some(u.to_string());
    }
    url::Url::parse(base?)
        .ok()?
        .join(value)
        .ok()
        .map(|u| u.to_string())
}

/// Aidoku's `Kind` discriminants, which the guest reads straight off the
/// return value.
pub mod kind {
    pub const UNKNOWN: i32 = 0;
    pub const NODE: i32 = 1;
    pub const TEXT_NODE: i32 = 2;
    pub const DATA_NODE: i32 = 3;
    pub const COMMENT: i32 = 4;
    pub const ELEMENT: i32 = 5;
    pub const ELEMENT_LIST: i32 = 6;
    pub const DOCUMENT: i32 = 7;
}

/// The raw contents of a text or comment node.
///
/// `text()` aggregates descendants, and a leaf has none, so it answers empty
/// for exactly the nodes this is for.
fn node_contents(node: &NodeRef<'_>) -> Option<String> {
    node.query(|n| match &n.data {
        dom_query::NodeData::Text { contents } | dom_query::NodeData::Comment { contents } => {
            Some(contents.to_string())
        }
        _ => None,
    })
    .flatten()
}

/// Whether a text node holds element data rather than prose.
///
/// Jsoup — which this ABI is shaped after — splits the two: the contents of
/// `<script>` and `<style>` are a `DataNode`, everything else a `TextNode`.
/// `dom_query` has one text variant, so the parent's tag decides.
fn is_data_node(node: &NodeRef<'_>) -> bool {
    if !node.is_text() {
        return false;
    }
    node.parent()
        .and_then(|p| p.node_name().map(|n| n.to_string()))
        .is_some_and(|tag| {
            let tag = tag.to_ascii_lowercase();
            tag == "script" || tag == "style"
        })
}

pub fn node_kind(html: &Rc<Html>, id: NodeId) -> i32 {
    let node = node(html, id);
    if node.is_document() || node.is_fragment() {
        kind::DOCUMENT
    } else if node.is_element() {
        kind::ELEMENT
    } else if node.is_comment() {
        kind::COMMENT
    } else if is_data_node(&node) {
        kind::DATA_NODE
    } else if node.is_text() {
        kind::TEXT_NODE
    } else {
        kind::NODE
    }
}

/// A node's data, following Jsoup: a comment's or data node's own contents,
/// and for an element the data of every data node beneath it, joined.
///
/// Descendants, not children — a `<script>`'s body hangs off the script
/// element, so an element's own children are never data nodes.
///
/// Not [`text`]: a script body is not prose, and collapsing its whitespace or
/// decoding its entities would corrupt the JSON a source reads out of it.
pub fn data(html: &Rc<Html>, id: NodeId) -> Option<String> {
    let node = node(html, id);

    if node.is_comment() || is_data_node(&node) {
        return Some(node_contents(&node).unwrap_or_default());
    }
    if !node.is_element() {
        return None;
    }

    let mut out = String::new();
    for descendant in node.descendants_it() {
        if is_data_node(&descendant) {
            out.push_str(&node_contents(&descendant).unwrap_or_default());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.test/browse?page=1";

    /// The same fixture the conformance guest carries, so the two cannot drift.
    const FIXTURE: &str = r#"<div id="wrap">
  <div class="grid" data-count="3">
    <a class="card" href="/series/1" title="First">
      <img class="cover" src="/c1.jpg" alt="Alpha &amp; Co">
      <h3 class="t">  Alpha   &amp; Co  </h3>
    </a>
    <a class="card" href="/series/2">
      <img class="cover" src="/c2.jpg">
      <h3 class="t">Beta</h3>
    </a>
    <a class="card" href="/series/3">
      <h3 class="t">Gamma<em> (side)</em></h3>
    </a>
  </div>
  <div class="empty"></div>
</div>"#;

    fn doc() -> (Rc<Html>, NodeId) {
        let html = Rc::new(parse(FIXTURE, Some(BASE), true));
        let root = root(&html);
        (html, root)
    }

    /// A document carrying each node kind, which `FIXTURE` has no reason to.
    const KINDS: &str = r#"<div id="host">
  <!-- a comment -->
  plain text
  <script type="application/json">{"pages":[1,2]}</script>
  <style>.a{color:red}</style>
</div>"#;

    fn kinds() -> (Rc<Html>, NodeId) {
        let html = Rc::new(parse(KINDS, Some(BASE), true));
        let root = root(&html);
        (html, root)
    }

    #[test]
    fn a_node_reports_its_kind() {
        let (h, r) = kinds();
        let host = select_first(&h, r, "#host").unwrap();
        assert_eq!(node_kind(&h, host), kind::ELEMENT);

        let script = select_first(&h, r, "script").unwrap();
        let body = children(&h, script);
        // A script's contents are a data node, not prose (Jsoup's split).
        assert_eq!(node_kind(&h, body[0]), kind::DATA_NODE);

        let comment = children(&h, host)
            .into_iter()
            .find(|id| node_kind(&h, *id) == kind::COMMENT)
            .expect("the comment is a child of #host");
        assert_eq!(node_kind(&h, comment), kind::COMMENT);

        // Several text-node children are the whitespace between elements, so
        // this names the one that carries prose.
        let text = children(&h, host)
            .into_iter()
            .find(|id| untrimmed_text(&h, *id).contains("plain text"))
            .expect("the bare text is a child of #host");
        assert_eq!(node_kind(&h, text), kind::TEXT_NODE);
    }

    /// The reason `data` exists: a source reads JSON out of a `<script>`, and
    /// `text` would collapse the whitespace and decode entities inside it.
    #[test]
    fn data_returns_a_script_body_verbatim() {
        let (h, r) = kinds();
        let host = select_first(&h, r, "#host").unwrap();

        assert_eq!(
            data(&h, host).as_deref(),
            Some(r#"{"pages":[1,2]}.a{color:red}"#)
        );
    }

    #[test]
    fn data_on_a_comment_is_its_contents() {
        let (h, r) = kinds();
        let host = select_first(&h, r, "#host").unwrap();
        let comment = children(&h, host)
            .into_iter()
            .find(|id| node_kind(&h, *id) == kind::COMMENT)
            .expect("the comment is a child of #host");

        assert_eq!(data(&h, comment).as_deref(), Some(" a comment "));
    }

    /// An element with no script or style children has no data, which is not
    /// the same as having none to give.
    #[test]
    fn data_on_a_plain_element_is_empty() {
        let (h, r) = doc();
        let card = select_first(&h, r, "a.card").unwrap();
        assert_eq!(data(&h, card).as_deref(), Some(""));
    }

    #[test]
    fn select_finds_descendants() {
        let (h, r) = doc();
        assert_eq!(select(&h, r, "a.card").unwrap().len(), 3);
        assert_eq!(select(&h, r, "a.nonexistent").unwrap().len(), 0);
    }

    #[test]
    fn a_bad_selector_is_not_a_miss() {
        let (h, r) = doc();
        assert_eq!(
            select(&h, r, "a[["),
            Err(err::INVALID_QUERY),
            "an unparseable selector must be reported as such"
        );
        assert_eq!(
            select(&h, r, "a.nothing-here").unwrap().len(),
            0,
            "a selector that parses but matches nothing is an empty list, \
             not an error — reporting it as an error makes a source abandon \
             the entry"
        );
    }

    #[test]
    fn select_excludes_the_element_itself() {
        let (h, r) = doc();
        let first = select_first(&h, r, "a.card").unwrap();
        assert_eq!(
            select(&h, first, "a.card").unwrap().len(),
            0,
            "an <a> asked for a.card must not match itself"
        );
    }

    #[test]
    fn attr_present_and_absent() {
        let (h, r) = doc();
        let first = select_first(&h, r, "a.card").unwrap();
        assert_eq!(attr(&h, first, "href").unwrap(), "/series/1");
        assert_eq!(attr(&h, first, "title").unwrap(), "First");
        assert_eq!(
            attr(&h, first, "data-nope"),
            Err(err::NO_RESULT),
            "absent must not read as an empty string"
        );
    }

    #[test]
    fn abs_prefix_resolves_against_the_base() {
        let (h, r) = doc();
        let first = select_first(&h, r, "a.card").unwrap();
        assert_eq!(
            attr(&h, first, "abs:href").unwrap(),
            "https://example.test/series/1"
        );
        let cover = select_first(&h, first, "img.cover").unwrap();
        assert_eq!(
            attr(&h, cover, "abs:src").unwrap(),
            "https://example.test/c1.jpg"
        );
        // A plain lookup stays unresolved.
        assert_eq!(attr(&h, first, "href").unwrap(), "/series/1");
        assert_eq!(attr(&h, first, "abs:nope"), Err(err::NO_RESULT));
    }

    #[test]
    fn text_decodes_entities_and_collapses_whitespace() {
        let (h, r) = doc();
        let first = select_first(&h, r, "a.card").unwrap();
        let title = select_first(&h, first, "h3.t").unwrap();
        assert_eq!(text(&h, title), "Alpha & Co");
    }

    #[test]
    fn own_text_excludes_child_elements() {
        let (h, r) = doc();
        let third = select(&h, r, "a.card").unwrap()[2];
        let title = select_first(&h, third, "h3.t").unwrap();
        assert_eq!(text(&h, title), "Gamma (side)");
        assert_eq!(own_text(&h, title), "Gamma");
    }

    #[test]
    fn base_uri_prefers_the_load_url() {
        let (h, r) = doc();
        assert_eq!(base_uri(&h, r).as_deref(), Some(BASE));
    }

    #[test]
    fn mutation_round_trips() {
        let (h, r) = doc();
        let first = select_first(&h, r, "a.card").unwrap();
        set_attr(&h, first, "data-added", "yes");
        assert_eq!(attr(&h, first, "data-added").unwrap(), "yes");
        remove_attr(&h, first, "data-added");
        assert_eq!(attr(&h, first, "data-added"), Err(err::NO_RESULT));

        let title = select_first(&h, first, "h3.t").unwrap();
        set_text(&h, title, "Replaced");
        assert_eq!(text(&h, title), "Replaced");

        add_class(&h, first, "marked");
        assert!(has_class(&h, first, "marked"));
        remove_class(&h, first, "marked");
        assert!(!has_class(&h, first, "marked"));
    }

    #[test]
    fn traversal() {
        let (h, r) = doc();
        let cards = select(&h, r, "a.card").unwrap();
        assert_eq!(next_sibling(&h, cards[0]), Some(cards[1]));
        assert_eq!(prev_sibling(&h, cards[1]), Some(cards[0]));
        assert_eq!(siblings(&h, cards[0]).len(), 2);
        assert!(parent(&h, cards[0]).is_some());
        assert_eq!(tag_name(&h, cards[0]).as_deref(), Some("a"));
    }

    #[test]
    fn removal_detaches_the_node() {
        let (h, r) = doc();
        let cards = select(&h, r, "a.card").unwrap();
        remove(&h, cards[0]);
        assert_eq!(select(&h, r, "a.card").unwrap().len(), 2);
    }

    #[test]
    fn absolute_values_pass_through_unchanged() {
        assert_eq!(
            resolve(Some(BASE), "https://cdn.example/x.jpg").as_deref(),
            Some("https://cdn.example/x.jpg")
        );
        // No base and a relative value cannot resolve.
        assert_eq!(resolve(None, "/x.jpg"), None);
    }
}
