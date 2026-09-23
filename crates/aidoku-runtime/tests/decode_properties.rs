//! Property tests for the two decode paths that read untrusted bytes
//! (ADR-0004 follow-up).
//!
//! A `.aix` package is third-party code this project does not review, and the
//! postcard values a source returns are whatever its WASM module chose to
//! write into guest memory. Both are parsed before anything validates them.
//!
//! The assertion is narrow and it is the one that matters: **no input makes
//! the host panic**. A panic inside a worker takes down the job; a panic
//! inside a host import unwinds across the WASM boundary, which is worse.
//!
//! # These are property tests, not a fuzzer
//!
//! `cargo-fuzz` needs nightly for its sanitizers and this workspace pins a
//! stable toolchain, so this is not coverage-guided: it explores what the
//! generators reach rather than what the code branches on. TODO.md records
//! the difference rather than letting a green run imply more than it shows.

use nyuka_aidoku_runtime::models::{
    Chapter, FilterValue, Manga, MangaPageResult, Page, PageContent,
};
use nyuka_aidoku_runtime::package;
use proptest::prelude::*;

/// Decoding must return an error, not unwind.
macro_rules! decodes_or_errors {
    ($ty:ty, $bytes:expr) => {{
        let _ = postcard::from_bytes::<$ty>($bytes);
    }};
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Arbitrary bytes against every type the host decodes from guest memory.
    #[test]
    fn arbitrary_bytes_never_panic_the_decoder(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        decodes_or_errors!(Manga, &bytes);
        decodes_or_errors!(Chapter, &bytes);
        decodes_or_errors!(Page, &bytes);
        decodes_or_errors!(PageContent, &bytes);
        decodes_or_errors!(MangaPageResult, &bytes);
        decodes_or_errors!(FilterValue, &bytes);
    }

    /// Postcard prefixes collections with a varint length. A guest that
    /// declares a huge one is the shape most likely to turn a small input
    /// into a large allocation, so it gets its own generator.
    #[test]
    fn a_huge_declared_length_does_not_panic(
        length in prop::collection::vec(any::<u8>(), 1..8),
        rest in prop::collection::vec(any::<u8>(), 0..32),
    ) {
        let mut bytes = length;
        bytes.extend_from_slice(&rest);
        decodes_or_errors!(MangaPageResult, &bytes);
        decodes_or_errors!(Manga, &bytes);
    }

    /// A truncated value is what a guest writing a buffer and returning early
    /// produces, and it is far more likely than random bytes.
    #[test]
    fn truncating_a_valid_value_never_panics(cut in 0usize..256) {
        let valid = postcard::to_allocvec(&MangaPageResult {
            entries: vec![Manga {
                key: "series".into(),
                title: "Title".into(),
                ..Default::default()
            }],
            has_next_page: true,
        })
        .expect("serialize");

        let cut = cut.min(valid.len());
        decodes_or_errors!(MangaPageResult, &valid[..cut]);
    }

    /// A single flipped byte in an otherwise valid value, which is what a
    /// guest with an off-by-one in its own writer produces.
    #[test]
    fn flipping_one_byte_never_panics(index in 0usize..64, xor in 1u8..=255) {
        let mut bytes = postcard::to_allocvec(&Manga {
            key: "series".into(),
            title: "Title".into(),
            ..Default::default()
        })
        .expect("serialize");

        if index < bytes.len() {
            bytes[index] ^= xor;
        }
        decodes_or_errors!(Manga, &bytes);
    }

    /// The package loader parses an archive before anything validates it.
    #[test]
    fn arbitrary_package_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        // `NoImports` stands in for a module whose imports are not inspected:
        // this is about the parse, not the capability check.
        let _ = package::load(&bytes, &NoImports);
    }

    /// A zip header followed by rubbish reaches further into the loader than
    /// random bytes do.
    #[test]
    fn a_plausible_archive_prefix_never_panics(rest in prop::collection::vec(any::<u8>(), 0..512)) {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend_from_slice(&rest);
        let _ = package::load(&bytes, &NoImports);
    }
}

/// A `ModuleImports` that reports nothing.
struct NoImports;

impl package::ModuleImports for NoImports {
    fn imports(&self, _wasm: &[u8]) -> Result<Vec<(String, String)>, String> {
        Ok(Vec::new())
    }
}
