//! CBZ writing, `ComicInfo.xml` generation, and library path layout.
//!
//! Implements `domain::ports::LibraryStore`. See ADR-0007.
//!
//! # Format
//!
//! - One CBZ per chapter: a zip of page images in zero-padded filename order,
//!   with `ComicInfo.xml` embedded.
//! - Page images are **`Stored`, not `Deflate`**. JPEG and WebP do not
//!   compress further; deflating costs CPU for a larger write. Compress only
//!   the XML.
//! - Target **ComicInfo schema v2.0**, which is the released version. v2.1 is
//!   a draft.
//! - `Manga` is written as `YesAndRightToLeft` where the source indicates it,
//!   and `Unknown` rather than guessed.
//!
//! # Layout
//!
//! ```text
//! /library/
//!   <Series Title>/
//!     cover.jpg
//!     <Series Title> v03 c021.cbz
//! ```
//!
//! This shape is what Komga and Kavita parse. Source identity lives in the
//! database and in `ComicInfo.xml`, not in the path; a slug is appended to the
//! directory only to break a real title collision between two sources.
#![forbid(unsafe_code)]

pub mod cbz;
pub mod comicinfo;
pub mod paths;
pub mod store;
