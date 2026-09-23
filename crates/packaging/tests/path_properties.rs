//! Property tests for the path builder (ADR-0007 follow-up 1).
//!
//! Series titles and chapter names come from `.aix` packages this project
//! does not review. ADR-0007 asks for traversal sequences, null bytes,
//! overlong UTF-8, right-to-left overrides and 300-character names, with one
//! assertion: **every result stays under the root**.
//!
//! # These are property tests, not a fuzzer
//!
//! `cargo-fuzz` needs nightly for its sanitizers and this workspace pins a
//! stable toolchain. What is here covers the assertion the ADR names, across
//! generated input and an explicit corpus of the shapes it lists — but it is
//! not coverage-guided, so it explores what the generators reach rather than
//! what the code branches on. TODO.md records the difference.

use std::path::{Component, Path, PathBuf};

use nyuka_packaging::paths::{chapter_stem, resolve_within, sanitize_component, series_dir};
use proptest::prelude::*;

/// The one invariant. Everything below is a way of reaching it.
fn stays_under_root(root: &Path, relative: &str) {
    // A sanitized name must never introduce a separator or a parent link,
    // because the caller joins it onto the root.
    let path = Path::new(relative);
    for component in path.components() {
        assert!(
            !matches!(component, Component::ParentDir | Component::RootDir),
            "{relative:?} contains {component:?}"
        );
    }

    // And the containment check must agree. Refusing is always acceptable;
    // escaping is not.
    if let Ok(resolved) = resolve_within(root, path) {
        assert!(
            resolved.starts_with(root),
            "{relative:?} resolved to {resolved:?}, outside {root:?}"
        );
    }
}

/// Not a real directory: `resolve_within` must reach its decision from the
/// path alone, or a check that passes does so because the filesystem happened
/// to agree.
fn root() -> PathBuf {
    PathBuf::from("/library")
}

/// The shapes ADR-0007 lists, as an explicit corpus. A generator reaches
/// these only by luck; naming them means they are always covered.
fn hostile_corpus() -> Vec<String> {
    let mut corpus: Vec<String> = [
        "../../etc/passwd",
        "..",
        "../",
        "./../../",
        "/etc/passwd",
        "//etc/passwd",
        r"..\..\windows\system32",
        r"C:\Windows",
        "a/../../b",
        // Null and control bytes.
        "na\0me",
        "na\tme",
        "na\nme\r",
        // Right-to-left override, which can make a name display as something
        // other than what it is.
        "fi\u{202E}gnp.exe",
        "\u{202D}\u{202E}\u{200F}",
        // Zero-width and format characters.
        "a\u{200B}b\u{FEFF}c",
        // Windows reserved names.
        "CON",
        "con",
        "PRN",
        "AUX",
        "NUL",
        "COM1",
        "LPT1",
        "CON.txt",
        // Trailing dots and spaces, which Windows strips.
        "name.",
        "name ",
        "name...",
        // Empty and whitespace.
        "",
        " ",
        "\t",
        "...",
        // Combining characters and normalization edge cases.
        "e\u{0301}",
        "\u{FB01}",
        // A path separator in a name the source chose.
        "Series/Season 1",
        "Series\\Season 1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    corpus.push("x".repeat(300));
    corpus.push("あ".repeat(300));
    corpus.push("../".repeat(100));
    corpus
}

#[test]
fn the_named_hostile_shapes_all_stay_under_the_root() {
    let root = root();
    for input in hostile_corpus() {
        if let Ok(name) = sanitize_component(&input) {
            assert!(!name.is_empty(), "{input:?} sanitized to an empty name");
            stays_under_root(&root, &name);
        }

        if let Ok(dir) = series_dir(&input, None) {
            stays_under_root(&root, &dir);
        }

        if let Ok(stem) = chapter_stem(&input, Some(1.0), Some(2.0)) {
            stays_under_root(&root, &format!("{stem}.cbz"));
        }
    }
}

/// A sanitized name must be usable as a single path component. Anything that
/// produced a separator would be joined onto the root as two.
#[test]
fn a_sanitized_name_is_exactly_one_component() {
    for input in hostile_corpus() {
        let Ok(name) = sanitize_component(&input) else {
            continue;
        };
        assert_eq!(
            Path::new(&name).components().count(),
            1,
            "{input:?} sanitized to {name:?}, which is not one component"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// Arbitrary text, including whatever the generator happens to produce.
    #[test]
    fn any_title_stays_under_the_root(title in ".*") {
        let root = root();
        if let Ok(dir) = series_dir(&title, None) {
            stays_under_root(&root, &dir);
        }
    }

    /// Weighted toward the characters that matter, since `.*` rarely
    /// generates a traversal sequence on its own.
    #[test]
    fn hostile_alphabets_stay_under_the_root(
        title in prop::collection::vec(
            prop::sample::select(vec![
                "..", "/", "\\", ".", "\0", "\u{202E}", "\u{200B}", "a", " ", ":",
            ]),
            0..40,
        ).prop_map(|parts| parts.concat())
    ) {
        let root = root();
        if let Ok(name) = sanitize_component(&title) {
            prop_assert!(!name.is_empty());
            stays_under_root(&root, &name);
        }
        if let Ok(dir) = series_dir(&title, None) {
            stays_under_root(&root, &dir);
        }
    }

    /// A disambiguator is source-derived too.
    #[test]
    fn any_disambiguator_stays_under_the_root(title in ".{0,64}", tag in ".{0,64}") {
        let root = root();
        if let Ok(dir) = series_dir(&title, Some(&tag)) {
            stays_under_root(&root, &dir);
        }
    }

    /// Volume and chapter numbers come from a source too, and a `NaN` or an
    /// infinity formatted into a filename is a name nobody expects.
    #[test]
    fn any_chapter_number_produces_one_component(
        title in ".{0,64}",
        volume in prop::option::of(any::<f32>()),
        number in prop::option::of(any::<f32>()),
    ) {
        if let Ok(stem) = chapter_stem(&title, volume, number) {
            prop_assert_eq!(
                Path::new(&stem).components().count(),
                1,
                "stem {:?} is not one component",
                stem
            );
            prop_assert!(!stem.contains('/'));
            prop_assert!(!stem.contains('\\'));
        }
    }

    /// `resolve_within` is the last line of defence, and it is given paths
    /// rather than names.
    #[test]
    fn resolve_within_never_escapes(relative in ".*") {
        let root = root();
        if let Ok(resolved) = resolve_within(&root, Path::new(&relative)) {
            prop_assert!(
                resolved.starts_with(&root),
                "{:?} resolved to {:?}",
                relative,
                resolved
            );
        }
    }

    /// Sanitizing is idempotent: running it twice must not change the answer.
    /// If it did, a name stored once and re-derived later would drift.
    #[test]
    fn sanitizing_is_idempotent(input in ".*") {
        if let Ok(once) = sanitize_component(&input) {
            let twice = sanitize_component(&once).expect("a sanitized name sanitizes");
            prop_assert_eq!(once, twice);
        }
    }
}
