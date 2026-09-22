//! The per-invocation resource table.
//!
//! The Aidoku ABI is handle-based: a host function returns an `Rid`, the guest
//! passes it back, and `std::destroy` releases it. This table is what those
//! handles index.
//!
//! One table per `Store`, and therefore per invocation (ADR-0004). A handle
//! from one invocation must never resolve in another — which the per-`Store`
//! lifetime gives for free, and which is why handles are never reused across
//! stores.

use std::collections::HashMap;
use std::rc::Rc;

use dom_query::{Document, NodeId};

/// A parsed document plus the base URI it was loaded with.
///
/// The base travels with every node derived from the document, because
/// `attr("abs:href")` resolves against it and must keep working through
/// `select` → `get` → nested `select_first`. That path is how a source reaches
/// the `<img>` inside a card, and losing the base there is what made every
/// HTML-scraping source return zero entries.
pub struct Html {
    pub doc: Document,
    /// Normally the URL of the request the document came from.
    pub base: Option<String>,
}

/// An in-flight or completed HTTP request.
#[derive(Debug, Default, Clone)]
pub struct Request {
    pub method: i32,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub response: Option<Response>,
}

#[derive(Debug, Default, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The URL the response actually came from, after redirects. This, not the
    /// requested URL, is the document base.
    pub final_url: Option<String>,
}

/// Anything the guest can hold a handle to.
pub enum Resource {
    /// Bytes the guest reads back with `std::buffer_len` + `std::read_buffer`.
    Buffer(Vec<u8>),
    Request(Request),
    /// One element or document node.
    Node {
        html: Rc<Html>,
        id: NodeId,
    },
    /// The result of a `select`, addressed by index with `html::get`.
    NodeList {
        html: Rc<Html>,
        ids: Rc<Vec<NodeId>>,
    },
}

impl Resource {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Buffer(_) => "buffer",
            Self::Request(_) => "request",
            Self::Node { .. } => "node",
            Self::NodeList { .. } => "node-list",
        }
    }
}

/// Handles, and the objects behind them.
#[derive(Default)]
pub struct Table {
    entries: HashMap<i32, Resource>,
    next: i32,
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a resource and returns its handle.
    ///
    /// Handles start at 1: zero is used as a null pointer elsewhere in the ABI,
    /// so keeping it out of the handle space stops the two being confused.
    pub fn insert(&mut self, resource: Resource) -> i32 {
        self.next += 1;
        let id = self.next;
        self.entries.insert(id, resource);
        id
    }

    pub fn get(&self, id: i32) -> Option<&Resource> {
        self.entries.get(&id)
    }

    pub fn get_mut(&mut self, id: i32) -> Option<&mut Resource> {
        self.entries.get_mut(&id)
    }

    /// Releases a handle. Unknown handles are ignored: `std::destroy` returns
    /// nothing, and a guest double-destroying is not worth trapping over.
    pub fn remove(&mut self, id: i32) {
        self.entries.remove(&id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borrows a buffer, if that is what the handle refers to.
    pub fn buffer(&self, id: i32) -> Option<&[u8]> {
        match self.entries.get(&id) {
            Some(Resource::Buffer(b)) => Some(b),
            _ => None,
        }
    }

    /// Borrows a node, if that is what the handle refers to.
    pub fn node(&self, id: i32) -> Option<(&Rc<Html>, NodeId)> {
        match self.entries.get(&id) {
            Some(Resource::Node { html, id }) => Some((html, *id)),
            _ => None,
        }
    }

    pub fn request_mut(&mut self, id: i32) -> Option<&mut Request> {
        match self.entries.get_mut(&id) {
            Some(Resource::Request(r)) => Some(r),
            _ => None,
        }
    }

    pub fn request(&self, id: i32) -> Option<&Request> {
        match self.entries.get(&id) {
            Some(Resource::Request(r)) => Some(r),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_start_at_one_and_are_unique() {
        let mut t = Table::new();
        let a = t.insert(Resource::Buffer(vec![1]));
        let b = t.insert(Resource::Buffer(vec![2]));
        assert_eq!(
            a, 1,
            "zero is reserved as a null pointer elsewhere in the ABI"
        );
        assert_ne!(a, b);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn handles_are_not_reused_after_removal() {
        let mut t = Table::new();
        let a = t.insert(Resource::Buffer(vec![1]));
        t.remove(a);
        let b = t.insert(Resource::Buffer(vec![2]));
        assert_ne!(
            a, b,
            "a reused handle would let a stale guest reference resolve"
        );
        assert!(t.get(a).is_none());
    }

    #[test]
    fn destroying_an_unknown_handle_is_not_an_error() {
        let mut t = Table::new();
        t.remove(9999);
        assert!(t.is_empty());
    }

    #[test]
    fn typed_accessors_reject_the_wrong_kind() {
        let mut t = Table::new();
        let buf = t.insert(Resource::Buffer(vec![7]));
        let req = t.insert(Resource::Request(Request::default()));
        assert_eq!(t.buffer(buf), Some(&[7u8][..]));
        assert!(t.buffer(req).is_none());
        assert!(t.request(buf).is_none());
        assert!(t.request(req).is_some());
    }
}
