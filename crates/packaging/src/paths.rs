//! Path construction and sanitization.
//!
//! # Every input here is attacker-influenced
//!
//! Series titles, chapter titles and page filenames all originate in
//! third-party WASM (ADR-0004). A source can return any string it likes, so
//! this module is the boundary that decides what becomes a filesystem path.
//!
//! Two controls, deliberately both:
//!
//! 1. **Sanitize each component** — strip separators and control characters,
//!    reject traversal, normalize Unicode, cap length.
//! 2. **Verify containment afterwards** — resolve the finished path and check
//!    it is under the library root.
//!
//! Sanitizing inputs and checking the result are different controls and the
//! second is not redundant: it catches anything the first failed to anticipate,
//! including symlinks, which no amount of string cleaning addresses.
//!
//! ADR-0007 asks for this module to be fuzzed, not only unit-tested.

use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Why a name or path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("name is empty after sanitization")]
    Empty,
    #[error("path escapes the library root")]
    Escapes,
    #[error("path could not be resolved: {0}")]
    Unresolvable(String),
}

/// Names Windows refuses regardless of extension.
///
/// Included even though the service runs on Linux: the library volume is
/// meant to be movable, and a directory that cannot be copied to another
/// machine is a portability bug that only shows up at the worst moment.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Filesystems cap a name at 255 **bytes**, not characters. A 200-character
/// CJK title is 600 bytes and would be rejected by the filesystem rather than
/// by us.
const MAX_COMPONENT_BYTES: usize = 255;

/// Turns arbitrary source text into one safe path component.
///
/// Never returns something containing a separator, so the result cannot become
/// more than one component however it is joined.
pub fn sanitize_component(input: &str) -> Result<String, PathError> {
    // NFC first: two byte sequences that look identical must not become two
    // different directories, and macOS would re-normalize them anyway.
    let normalized: String = input.nfc().collect();

    let mut out = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            // Separators, including the Windows one: a backslash is an
            // ordinary character on Linux but a separator on the machine the
            // volume might be moved to.
            '/' | '\\' => out.push('-'),
            // Reserved on Windows, and several are shell-significant.
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('-'),
            // Control characters, including the NUL that truncates a C string.
            c if c.is_control() => {}
            // Bidirectional overrides can make a filename display as something
            // other than what it is.
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' => {}
            c => out.push(c),
        }
    }

    // Collapse whitespace, then trim. Trailing dots and spaces are silently
    // stripped by Windows, so "name." and "name" would collide there.
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // Also trim the '-' that replaced separators: a leading dash makes a
    // filename look like a flag to every command-line tool an operator might
    // use on the library.
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == '-' || c.is_whitespace());

    // `.` and `..` are traversal; anything reducing to them is refused rather
    // than rewritten, since there is no sensible name to substitute.
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(PathError::Empty);
    }

    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let mut result = if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };

    if result.len() > MAX_COMPONENT_BYTES {
        result = truncate_bytes(&result, MAX_COMPONENT_BYTES);
        // Truncation can leave a trailing dot or space again.
        result = result
            .trim_matches(|c: char| c == '.' || c.is_whitespace())
            .to_string();
        if result.is_empty() {
            return Err(PathError::Empty);
        }
    }
    Ok(result)
}

/// Truncates on a character boundary, never mid-codepoint.
fn truncate_bytes(s: &str, max: usize) -> String {
    let mut end = max.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Builds the directory a series lives in.
///
/// `<Series Title>` — the shape Komga and Kavita parse. Source identity is not
/// in the path: it lives in the database and in `ComicInfo.xml`, and a slug is
/// appended only to break a real collision (ADR-0007).
pub fn series_dir(title: &str, disambiguator: Option<&str>) -> Result<String, PathError> {
    let base = sanitize_component(title)?;
    match disambiguator {
        Some(slug) => {
            let slug = sanitize_component(slug)?;
            sanitize_component(&format!("{base} [{slug}]"))
        }
        None => Ok(base),
    }
}

/// Builds a chapter's filename, without extension.
///
/// `<Series Title> v03 c021` — volume and chapter markers in the form both
/// Komga and Kavita recognise. Zero-padded so a lexical sort matches a
/// numeric one, which is how a reader with no metadata orders them.
pub fn chapter_stem(
    series: &str,
    volume: Option<f32>,
    number: Option<f32>,
) -> Result<String, PathError> {
    let series = sanitize_component(series)?;
    let mut name = series;
    if let Some(v) = volume {
        name.push_str(&format!(" v{}", pad_number(v, 2)));
    }
    if let Some(n) = number {
        name.push_str(&format!(" c{}", pad_number(n, 3)));
    }
    sanitize_component(&name)
}

/// Formats a chapter or volume number, keeping a fractional part when there is
/// one: chapter 10.5 must not become 10.
fn pad_number(value: f32, width: usize) -> String {
    if value.fract().abs() < f32::EPSILON {
        format!("{:0width$}", value as i64, width = width)
    } else {
        let whole = value.trunc() as i64;
        let frac = format!("{value}");
        let tail = frac.split('.').nth(1).unwrap_or("0");
        format!("{:0width$}.{tail}", whole, width = width)
    }
}

/// Resolves a path against the library root and verifies it stays inside.
///
/// This is the second control, and it is the one that catches what
/// sanitization did not anticipate. It resolves symlinks, so a directory
/// pointing outside the library is refused even though every component of the
/// path is a perfectly ordinary name.
pub fn resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, PathError> {
    // An absolute or traversing relative path is refused before touching the
    // filesystem.
    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::Escapes);
            }
        }
    }

    let root = root
        .canonicalize()
        .map_err(|e| PathError::Unresolvable(e.to_string()))?;
    let joined = root.join(relative);

    // The target usually does not exist yet, so canonicalize the deepest
    // existing ancestor and check that.
    let mut probe = joined.as_path();
    let existing = loop {
        if probe.exists() {
            break probe;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return Err(PathError::Escapes),
        }
    };
    let resolved = existing
        .canonicalize()
        .map_err(|e| PathError::Unresolvable(e.to_string()))?;
    if !resolved.starts_with(&root) {
        return Err(PathError::Escapes);
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_titles_survive_unchanged() {
        assert_eq!(sanitize_component("One Piece").unwrap(), "One Piece");
        assert_eq!(sanitize_component("Re:Zero").unwrap(), "Re-Zero");
        assert_eq!(
            sanitize_component("ワンピース").unwrap(),
            "ワンピース",
            "non-latin titles must not be mangled"
        );
    }

    /// The classic attack: a title that walks out of the library.
    #[test]
    fn traversal_cannot_survive_a_component() {
        assert_eq!(sanitize_component(".."), Err(PathError::Empty));
        assert_eq!(sanitize_component("."), Err(PathError::Empty));
        // Separators are replaced, so this stays ONE component and cannot
        // traverse. A `..` surviving as ordinary characters inside a name is
        // harmless — what matters is that no separator remains and that the
        // component is not itself a traversal.
        let s = sanitize_component("../../etc/passwd").unwrap();
        assert!(!s.contains('/') && !s.contains('\\'), "got {s:?}");
        assert!(s != ".." && s != ".", "got {s:?}");
        assert_eq!(Path::new(&s).components().count(), 1, "got {s:?}");
    }

    #[test]
    fn leading_and_trailing_dashes_are_trimmed() {
        // `-rf` as a filename is a hazard for any tool an operator runs over
        // the library.
        assert_eq!(sanitize_component("/rf").unwrap(), "rf");
        assert_eq!(sanitize_component("Series/").unwrap(), "Series");
    }

    #[test]
    fn windows_separators_are_treated_as_separators() {
        let s = sanitize_component(r"..\..\windows\system32").unwrap();
        assert!(!s.contains('\\'), "got {s:?}");
    }

    #[test]
    fn control_characters_and_nul_are_removed() {
        let s = sanitize_component("bad\0name\u{7}here").unwrap();
        assert_eq!(s, "badnamehere");
        assert!(!s.contains('\0'));
    }

    /// A right-to-left override can make `evil\u{202E}gnp.exe` display as
    /// something else entirely.
    #[test]
    fn bidirectional_overrides_are_removed() {
        let s = sanitize_component("report\u{202E}fdp.exe").unwrap();
        assert!(!s.contains('\u{202E}'), "got {s:?}");
    }

    #[test]
    fn windows_reserved_names_are_escaped() {
        assert_eq!(sanitize_component("CON").unwrap(), "_CON");
        assert_eq!(sanitize_component("nul.txt").unwrap(), "_nul.txt");
        assert_eq!(
            sanitize_component("console").unwrap(),
            "console",
            "only exact reserved stems are escaped"
        );
    }

    /// Windows strips trailing dots and spaces, so "name." and "name" would be
    /// the same file there but not here.
    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        assert_eq!(sanitize_component("Series...").unwrap(), "Series");
        assert_eq!(sanitize_component("Series   ").unwrap(), "Series");
    }

    #[test]
    fn long_names_are_truncated_on_a_character_boundary() {
        let long = "あ".repeat(200); // 600 bytes
        let s = sanitize_component(&long).unwrap();
        assert!(s.len() <= MAX_COMPONENT_BYTES, "{} bytes", s.len());
        // Still valid UTF-8 and not cut mid-codepoint.
        assert!(s.chars().all(|c| c == 'あ'));
    }

    #[test]
    fn unicode_is_normalized_so_lookalikes_collide_consistently() {
        // é as one codepoint, and as e + combining accent.
        let composed = sanitize_component("Caf\u{00E9}").unwrap();
        let decomposed = sanitize_component("Cafe\u{0301}").unwrap();
        assert_eq!(
            composed, decomposed,
            "two spellings of the same title must be one directory"
        );
    }

    #[test]
    fn a_title_of_only_punctuation_is_refused_rather_than_guessed() {
        assert_eq!(sanitize_component("..."), Err(PathError::Empty));
        assert_eq!(sanitize_component("   "), Err(PathError::Empty));
        assert_eq!(sanitize_component(""), Err(PathError::Empty));
    }

    #[test]
    fn chapter_names_follow_the_reader_convention() {
        assert_eq!(
            chapter_stem("Series", Some(3.0), Some(21.0)).unwrap(),
            "Series v03 c021"
        );
        assert_eq!(
            chapter_stem("Series", None, Some(5.0)).unwrap(),
            "Series c005"
        );
        assert_eq!(chapter_stem("Series", None, None).unwrap(), "Series");
    }

    /// Chapter 10.5 must not become chapter 10.
    #[test]
    fn fractional_chapter_numbers_are_preserved() {
        assert_eq!(
            chapter_stem("Series", None, Some(10.5)).unwrap(),
            "Series c010.5"
        );
    }

    #[test]
    fn a_disambiguator_is_appended_only_when_given() {
        assert_eq!(series_dir("Series", None).unwrap(), "Series");
        assert_eq!(
            series_dir("Series", Some("en.asurascans")).unwrap(),
            "Series [en.asurascans]"
        );
    }

    #[test]
    fn containment_rejects_traversal_and_absolute_paths() {
        let root = tempfile::tempdir().expect("tempdir");
        let root = root.path();
        assert!(resolve_within(root, Path::new("Series/file.cbz")).is_ok());
        assert_eq!(
            resolve_within(root, Path::new("../escape")),
            Err(PathError::Escapes)
        );
        assert_eq!(
            resolve_within(root, Path::new("/etc/passwd")),
            Err(PathError::Escapes)
        );
        assert_eq!(
            resolve_within(root, Path::new("a/../../b")),
            Err(PathError::Escapes)
        );
    }

    /// The reason containment is a separate control: no amount of string
    /// cleaning detects a symlink, because every component is an ordinary name.
    #[cfg(unix)]
    #[test]
    fn containment_rejects_a_symlink_out_of_the_library() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");

        assert_eq!(
            resolve_within(root.path(), Path::new("escape/stolen.cbz")),
            Err(PathError::Escapes),
            "a symlinked directory must not be writable through"
        );
    }
}
