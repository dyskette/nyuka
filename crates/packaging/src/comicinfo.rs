//! `ComicInfo.xml` generation.
//!
//! This is the interoperability contract, not an extra. Komga, Kavita,
//! YACReader and the offline readers all read this file, and embedding it is
//! the reliable alternative to having them guess series and chapter numbers
//! from filenames (ADR-0007).
//!
//! Targets **schema v2.0**, which is the released version. v2.1 is a draft, so
//! its fields are not emitted.

use nyuka_domain::model::ReadingDirection;

/// The fields a chapter archive carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComicInfo {
    pub series: String,
    pub title: Option<String>,
    pub number: Option<f32>,
    pub volume: Option<f32>,
    pub summary: Option<String>,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub genres: Vec<String>,
    pub language_iso: Option<String>,
    pub page_count: u32,
    pub web: Option<String>,
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub direction: ReadingDirection,
}

/// The `Manga` element's enum.
///
/// This is how reading direction reaches every other reader, so `Unknown` is
/// written rather than guessed: claiming left-to-right for a series that is
/// right-to-left makes it unreadable in the wrong order, which is worse than
/// letting the reader decide.
fn manga_value(direction: ReadingDirection) -> &'static str {
    match direction {
        ReadingDirection::RightToLeft => "YesAndRightToLeft",
        ReadingDirection::LeftToRight => "Yes",
        ReadingDirection::Unknown => "Unknown",
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most control characters outright; a reader that
            // hits one usually rejects the whole file rather than the field.
            c if c.is_control() && c != '\n' && c != '\r' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn element(out: &mut String, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    out.push_str(&format!("  <{name}>{}</{name}>\n", escape(value)));
}

impl ComicInfo {
    /// Serializes to XML.
    ///
    /// Element order follows the v2.0 schema. It is a sequence rather than a
    /// set, so a reader validating against the XSD rejects a file whose
    /// elements are shuffled.
    pub fn to_xml(&self) -> String {
        let mut out = String::from(r#"<?xml version="1.0" encoding="utf-8"?>"#);
        out.push('\n');
        out.push_str(
            r#"<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">"#,
        );
        out.push('\n');

        element(&mut out, "Title", self.title.as_deref().unwrap_or(""));
        element(&mut out, "Series", &self.series);
        if let Some(n) = self.number {
            element(&mut out, "Number", &format_number(n));
        }
        if let Some(v) = self.volume {
            element(&mut out, "Volume", &format_number(v));
        }
        element(&mut out, "Summary", self.summary.as_deref().unwrap_or(""));
        if let Some(y) = self.year {
            element(&mut out, "Year", &y.to_string());
        }
        if let Some(m) = self.month {
            element(&mut out, "Month", &m.to_string());
        }
        if let Some(d) = self.day {
            element(&mut out, "Day", &d.to_string());
        }
        element(&mut out, "Writer", &self.writers.join(", "));
        element(&mut out, "Penciller", &self.pencillers.join(", "));
        element(&mut out, "Genre", &self.genres.join(", "));
        element(&mut out, "Web", self.web.as_deref().unwrap_or(""));
        element(&mut out, "PageCount", &self.page_count.to_string());
        element(
            &mut out,
            "LanguageISO",
            self.language_iso.as_deref().unwrap_or(""),
        );
        element(&mut out, "Manga", manga_value(self.direction));

        out.push_str("</ComicInfo>\n");
        out
    }
}

/// Whole numbers without a decimal point: readers parse `Number` loosely, and
/// `1` sorts better than `1.0` in the ones that treat it as a string.
fn format_number(value: f32) -> String {
    if value.fract().abs() < f32::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ComicInfo {
        ComicInfo {
            series: "Series".into(),
            title: Some("The Beginning".into()),
            number: Some(21.0),
            volume: Some(3.0),
            page_count: 18,
            language_iso: Some("en".into()),
            direction: ReadingDirection::RightToLeft,
            genres: vec!["Action".into(), "Adventure".into()],
            web: Some("https://example.test/series/1".into()),
            ..Default::default()
        }
    }

    #[test]
    fn emits_the_fields_readers_use() {
        let xml = sample().to_xml();
        assert!(xml.contains("<Series>Series</Series>"));
        assert!(xml.contains("<Number>21</Number>"));
        assert!(xml.contains("<Volume>3</Volume>"));
        assert!(xml.contains("<PageCount>18</PageCount>"));
        assert!(xml.contains("<LanguageISO>en</LanguageISO>"));
    }

    /// Reading direction is the field this format exists to carry for manga.
    #[test]
    fn reading_direction_maps_to_the_manga_enum() {
        assert!(
            sample()
                .to_xml()
                .contains("<Manga>YesAndRightToLeft</Manga>")
        );
        let ltr = ComicInfo {
            direction: ReadingDirection::LeftToRight,
            ..sample()
        };
        assert!(ltr.to_xml().contains("<Manga>Yes</Manga>"));
        let unknown = ComicInfo {
            direction: ReadingDirection::Unknown,
            ..sample()
        };
        assert!(
            unknown.to_xml().contains("<Manga>Unknown</Manga>"),
            "unknown must be written, not guessed as one direction"
        );
    }

    /// A title containing markup must not produce a file every reader rejects.
    #[test]
    fn markup_in_a_title_is_escaped() {
        let info = ComicInfo {
            series: r#"Tom & Jerry <"best"> 'ever'"#.into(),
            ..sample()
        };
        let xml = info.to_xml();
        assert!(xml.contains("Tom &amp; Jerry &lt;&quot;best&quot;&gt; &apos;ever&apos;"));
        // No raw angle bracket survived inside the element text.
        assert!(!xml.contains(r#"<"best">"#));
    }

    #[test]
    fn control_characters_are_dropped_rather_than_emitted() {
        let info = ComicInfo {
            series: "Bad\u{0}Title\u{7}".into(),
            ..sample()
        };
        assert!(info.to_xml().contains("<Series>BadTitle</Series>"));
    }

    #[test]
    fn absent_fields_are_omitted_rather_than_empty() {
        let minimal = ComicInfo {
            series: "Series".into(),
            page_count: 1,
            ..Default::default()
        };
        let xml = minimal.to_xml();
        assert!(
            !xml.contains("<Summary>"),
            "an empty element is not the same as an absent one"
        );
        assert!(!xml.contains("<Writer>"));
        assert!(xml.contains("<Series>Series</Series>"));
    }

    #[test]
    fn fractional_chapter_numbers_survive() {
        let info = ComicInfo {
            number: Some(10.5),
            ..sample()
        };
        assert!(info.to_xml().contains("<Number>10.5</Number>"));
    }

    #[test]
    fn the_document_is_well_formed() {
        let xml = sample().to_xml();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
        assert!(xml.trim_end().ends_with("</ComicInfo>"));
        // Balanced: every opening tag has a closing one.
        let opens = xml.matches('<').count();
        let closes = xml.matches('>').count();
        assert_eq!(opens, closes);
    }
}
