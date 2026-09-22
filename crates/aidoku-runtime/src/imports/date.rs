//! Date parsing for the `std::parse_date` host import.
//!
//! Sources write **Swift `DateFormatter` patterns** — Unicode date field
//! symbols — and use this for chapter publication dates.
//!
//! A host that stubs this to zero dates every chapter to 1970. That is wrong
//! and never an error, so nothing surfaces it: the chapter list looks
//! plausible and sorts wrongly. Refusing an unrecognised pattern is better
//! than guessing at it, which is why an unknown letter yields `None` rather
//! than being passed through as a literal.

use chrono::{NaiveDate, NaiveDateTime};

/// Unicode date field symbols mapped to chrono specifiers.
///
/// Longest match first: `MM` and `M` share a prefix, and matching the short
/// one first would leave a stray `M`.
const SYMBOLS: &[(&str, &str)] = &[
    ("yyyy", "%Y"),
    ("yyy", "%Y"),
    ("yy", "%y"),
    ("y", "%Y"),
    ("MMMM", "%B"),
    ("MMM", "%b"),
    ("MM", "%m"),
    ("M", "%-m"),
    ("dd", "%d"),
    ("d", "%-d"),
    ("HH", "%H"),
    ("H", "%-H"),
    ("hh", "%I"),
    ("h", "%-I"),
    ("mm", "%M"),
    ("m", "%-M"),
    ("ss", "%S"),
    ("s", "%-S"),
    ("EEEE", "%A"),
    ("EEE", "%a"),
    ("EE", "%a"),
    ("E", "%a"),
    ("a", "%p"),
    ("ZZZZ", "%z"),
    ("Z", "%z"),
];

/// Translates a `DateFormatter` pattern into a chrono format string.
///
/// Returns `None` for any alphabetic symbol not in [`SYMBOLS`], so an
/// unsupported pattern fails loudly instead of parsing into a wrong instant.
/// Text inside single quotes is a literal, as in the Unicode spec.
pub fn pattern_to_chrono(pattern: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        // Quoted literals: '' is an escaped quote.
        if let Some(after) = rest.strip_prefix('\'') {
            if let Some(after_escape) = after.strip_prefix('\'') {
                out.push('\'');
                rest = after_escape;
                continue;
            }
            let end = after.find('\'')?;
            push_literal(&mut out, &after[..end]);
            rest = &after[end + 1..];
            continue;
        }
        for (symbol, spec) in SYMBOLS {
            if rest.starts_with(symbol) {
                out.push_str(spec);
                rest = &rest[symbol.len()..];
                continue 'outer;
            }
        }
        let ch = rest.chars().next()?;
        if ch.is_ascii_alphabetic() {
            // An unimplemented field symbol. Refusing beats mis-parsing.
            return None;
        }
        push_literal(&mut out, &ch.to_string());
        rest = &rest[ch.len_utf8()..];
    }
    Some(out)
}

fn push_literal(out: &mut String, s: &str) {
    for ch in s.chars() {
        if ch == '%' {
            out.push_str("%%");
        } else {
            out.push(ch);
        }
    }
}

/// Parses a date string into a Unix timestamp in seconds.
///
/// The guest always passes `UTC`; any other zone is refused rather than
/// silently treated as UTC, which would shift every date by the offset.
pub fn parse(date: &str, pattern: &str, timezone: &str) -> Option<i64> {
    if !timezone.is_empty() && !timezone.eq_ignore_ascii_case("UTC") {
        return None;
    }
    let format = pattern_to_chrono(pattern)?;
    let has_time = ["%H", "%-H", "%I", "%-I", "%M", "%-M"]
        .iter()
        .any(|s| format.contains(s));

    if has_time {
        NaiveDateTime::parse_from_str(date.trim(), &format)
            .ok()
            .map(|dt| dt.and_utc().timestamp())
    } else {
        NaiveDate::parse_from_str(date.trim(), &format)
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc().timestamp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_common_patterns() {
        assert_eq!(
            pattern_to_chrono("MM-dd-yyyy HH:mm").as_deref(),
            Some("%m-%d-%Y %H:%M")
        );
        assert_eq!(pattern_to_chrono("yyyy-MM-dd").as_deref(), Some("%Y-%m-%d"));
        assert_eq!(
            pattern_to_chrono("MMMM d, yyyy").as_deref(),
            Some("%B %-d, %Y")
        );
    }

    #[test]
    fn refuses_unknown_field_symbols() {
        // `Q` (quarter) is a real Unicode symbol this host does not implement.
        // Passing it through as a literal would parse some inputs into the
        // wrong instant rather than failing.
        assert_eq!(pattern_to_chrono("yyyy-Q"), None);
    }

    #[test]
    fn quoted_text_is_literal() {
        assert_eq!(
            pattern_to_chrono("yyyy 'at' HH:mm").as_deref(),
            Some("%Y at %H:%M")
        );
    }

    /// The example from the guest's own `parse_date` documentation.
    #[test]
    fn matches_the_documented_example() {
        assert_eq!(
            parse("07-01-2025 13:00", "MM-dd-yyyy HH:mm", "UTC"),
            Some(1_751_374_800)
        );
    }

    #[test]
    fn date_without_time_is_midnight_utc() {
        assert_eq!(
            parse("2025-01-02", "yyyy-MM-dd", "UTC"),
            Some(1_735_776_000)
        );
    }

    #[test]
    fn unparseable_input_is_none_not_zero() {
        // Zero would be a valid timestamp — 1970 — and the guest cannot tell
        // it from a real date.
        assert_eq!(parse("not a date", "yyyy-MM-dd", "UTC"), None);
        assert_eq!(parse("", "yyyy-MM-dd", "UTC"), None);
    }

    #[test]
    fn a_non_utc_zone_is_refused_rather_than_assumed() {
        assert_eq!(parse("2025-01-02", "yyyy-MM-dd", "America/New_York"), None);
        // Empty means unspecified, which the guest treats as UTC.
        assert!(parse("2025-01-02", "yyyy-MM-dd", "").is_some());
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert_eq!(
            parse("  2025-01-02  ", "yyyy-MM-dd", "UTC"),
            Some(1_735_776_000)
        );
    }
}
