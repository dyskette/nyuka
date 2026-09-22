//! Host-side mirrors of the guest's postcard-encoded models.
//!
//! # Field order is the wire format
//!
//! Postcard is not self-describing: a struct is its fields in declaration
//! order, with no names or tags, and a unit enum variant is its declaration
//! index as a varint. So **reordering a field or a variant here silently
//! changes the meaning of every byte after it**, and adding one in the middle
//! is a wire break with no error at the boundary — the guest and host simply
//! disagree about what they are looking at.
//!
//! These definitions mirror aidoku-rs at the commit recorded in
//! [`ABI_SOURCE_COMMIT`](crate::ABI_SOURCE_COMMIT). The conformance fixture is
//! what proves they still agree; see ADR-0004.
//!
//! Bump [`HOST_ABI_VERSION`](crate::HOST_ABI_VERSION) on any change here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Context passed alongside a page URL to a page processor or image-request
/// modifier. `HashMap<String, String>` in the guest.
pub type PageContext = HashMap<String, String>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MangaStatus {
    #[default]
    Unknown,
    Ongoing,
    Completed,
    Cancelled,
    Hiatus,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentRating {
    #[default]
    Unknown,
    Safe,
    Suggestive,
    Nsfw,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Viewer {
    #[default]
    Unknown,
    LeftToRight,
    RightToLeft,
    Vertical,
    Webtoon,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateStrategy {
    #[default]
    Always,
    Never,
}

/// A series. Field order mirrors the guest exactly.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manga {
    pub key: String,
    pub title: String,
    pub cover: Option<String>,
    pub artists: Option<Vec<String>>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub status: MangaStatus,
    pub content_rating: ContentRating,
    pub viewer: Viewer,
    pub update_strategy: UpdateStrategy,
    pub next_update_time: Option<i64>,
    pub chapters: Option<Vec<Chapter>>,
}

/// A chapter. Field order mirrors the guest exactly.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub key: String,
    pub title: Option<String>,
    pub chapter_number: Option<f32>,
    pub volume_number: Option<f32>,
    pub date_uploaded: Option<i64>,
    pub scanlators: Option<Vec<String>>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub thumbnail: Option<String>,
    pub locked: bool,
}

/// What a page holds.
///
/// Variant order is the wire encoding. `Image` is kept in place even though
/// this host does not implement `canvas` (tier 3, ADR-0004): removing it would
/// shift `Zip` from index 3 to index 2 and silently mis-decode every archive
/// page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PageContent {
    /// An image URL, with context for a page processor or image-request
    /// modifier.
    Url(String, Option<PageContext>),
    /// Markdown text.
    Text(String),
    /// A raw image handed over as a canvas resource.
    ///
    /// The guest encodes an `ImageRef`, which wraps a resource handle. Held as
    /// an opaque `i32` here: this host does not implement `canvas`, so a page
    /// arriving as `Image` is reported as an unsupported capability rather
    /// than rendered. The inner encoding is therefore **unvalidated** — only
    /// its position matters.
    Image(i32),
    /// A zip archive URL and a path to an image inside it.
    Zip(String, String),
}

impl Default for PageContent {
    fn default() -> Self {
        Self::Url(String::new(), None)
    }
}

/// A page. Field order mirrors the guest exactly.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub content: PageContent,
    pub thumbnail: Option<String>,
    pub has_description: bool,
    pub description: Option<String>,
}

/// One page of catalog results.
///
/// The guest derives only `Serialize` for this type, since it only ever sends
/// it. The host only ever receives it, so it needs `Deserialize`; `Serialize`
/// is kept so tests can round-trip.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaPageResult {
    pub entries: Vec<Manga>,
    pub has_next_page: bool,
}

/// A filter value supplied to `get_search_manga_list`.
///
/// Mirrors the guest's `FilterValue`. Variant order is the wire encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterValue {
    Text {
        id: String,
        value: String,
    },
    Sort {
        id: String,
        index: i32,
        ascending: bool,
    },
    Check {
        id: String,
        value: i32,
    },
    Select {
        id: String,
        value: String,
    },
    MultiSelect {
        id: String,
        included: Vec<String>,
        excluded: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A baseline that the mirrors are internally consistent. It cannot prove
    /// agreement with the guest — only the conformance fixture does that — but
    /// it catches a mirror that does not even round-trip against itself.
    #[test]
    fn models_round_trip_through_postcard() {
        let manga = Manga {
            key: "series-1".into(),
            title: "Alpha".into(),
            cover: Some("https://example.test/c1.jpg".into()),
            authors: Some(vec!["Author".into()]),
            status: MangaStatus::Ongoing,
            content_rating: ContentRating::Safe,
            viewer: Viewer::RightToLeft,
            update_strategy: UpdateStrategy::Always,
            chapters: Some(vec![Chapter {
                key: "c1".into(),
                chapter_number: Some(1.5),
                date_uploaded: Some(1_700_000_000),
                locked: false,
                ..Default::default()
            }]),
            ..Default::default()
        };

        let bytes = postcard::to_allocvec(&manga).expect("serialize");
        let back: Manga = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(manga, back);
    }

    #[test]
    fn page_result_round_trips() {
        let result = MangaPageResult {
            entries: vec![Manga {
                key: "k".into(),
                title: "t".into(),
                ..Default::default()
            }],
            has_next_page: true,
        };
        let bytes = postcard::to_allocvec(&result).expect("serialize");
        assert_eq!(
            result,
            postcard::from_bytes::<MangaPageResult>(&bytes).expect("deserialize")
        );
    }

    /// Unit enum variants encode as their declaration index, which is why the
    /// order in this module is load-bearing rather than cosmetic.
    #[test]
    fn unit_enums_encode_as_declaration_index() {
        for (value, want) in [
            (MangaStatus::Unknown, 0u8),
            (MangaStatus::Ongoing, 1),
            (MangaStatus::Completed, 2),
            (MangaStatus::Cancelled, 3),
            (MangaStatus::Hiatus, 4),
        ] {
            assert_eq!(postcard::to_allocvec(&value).unwrap(), vec![want]);
        }
        assert_eq!(
            postcard::to_allocvec(&ContentRating::Nsfw).unwrap(),
            vec![3u8]
        );
        assert_eq!(postcard::to_allocvec(&Viewer::Webtoon).unwrap(), vec![4u8]);
        assert_eq!(
            postcard::to_allocvec(&UpdateStrategy::Never).unwrap(),
            vec![1u8]
        );
    }

    /// `Zip` must stay at index 3. If the unimplemented `Image` variant were
    /// dropped, every archive page would decode as the wrong variant.
    #[test]
    fn page_content_variant_indices_are_stable() {
        let url = postcard::to_allocvec(&PageContent::Url("u".into(), None)).unwrap();
        assert_eq!(url[0], 0);
        let text = postcard::to_allocvec(&PageContent::Text("t".into())).unwrap();
        assert_eq!(text[0], 1);
        let image = postcard::to_allocvec(&PageContent::Image(7)).unwrap();
        assert_eq!(image[0], 2);
        let zip = postcard::to_allocvec(&PageContent::Zip("a".into(), "b".into())).unwrap();
        assert_eq!(zip[0], 3);
    }

    /// An empty filter list is what the host sends when no filters are active,
    /// and getting it wrong is what made the first spike run return -1.
    #[test]
    fn empty_filter_list_is_a_single_zero_byte() {
        let empty: Vec<FilterValue> = Vec::new();
        assert_eq!(postcard::to_allocvec(&empty).unwrap(), vec![0u8]);
    }
}
