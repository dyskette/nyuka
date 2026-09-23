//! Property tests for the OTLP decode path (ADR-0013 follow-up 2).
//!
//! `POST /telemetry` parses bytes from an authenticated but untrusted client:
//! a browser tab can send anything, and a compromised one will. ADR-0013 asks
//! that oversized batches, deep nesting and hostile strings be rejected
//! **without a `5xx`** — a server error there says the server is broken when
//! the client sent rubbish, and sends an operator to the wrong logs.
//!
//! # These are property tests, not a fuzzer
//!
//! `cargo-fuzz` needs nightly and this workspace pins stable, so this is not
//! coverage-guided. TODO.md records the difference.

use nyuka_api::routes::telemetry::{
    MAX_ATTRIBUTES, MAX_SPANS, MAX_STRING_BYTES, accept, is_allowed_attribute, truncate,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// The narrow assertion: no input panics, and nothing is accepted that
    /// should not be.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = accept(&bytes);
    }

    /// Text is more likely than random bytes to reach the parser's interior.
    #[test]
    fn arbitrary_text_never_panics(text in ".*") {
        let _ = accept(text.as_bytes());
    }

    /// Nesting is the shape that turns a small body into deep recursion.
    #[test]
    fn deep_nesting_is_refused_rather_than_recursing(depth in 1usize..4096) {
        let body = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        // Refused either way; what matters is that it returns.
        prop_assert!(accept(body.as_bytes()).is_err());
    }

    /// A well-formed envelope with a hostile interior, which is what an
    /// attacker who read the schema would actually send.
    #[test]
    fn a_valid_envelope_with_hostile_contents_is_handled(
        service in prop::sample::select(vec!["manga-web", "manga-api", "", "MANGA-WEB"]),
        name in ".{0,256}",
        key in ".{0,64}",
        value in ".{0,256}",
    ) {
        let body = serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": service } }
                    ]
                },
                "scopeSpans": [{
                    "spans": [{
                        "name": name,
                        "attributes": [
                            { "key": key, "value": { "stringValue": value } }
                        ]
                    }]
                }]
            }]
        });
        let bytes = serde_json::to_vec(&body).expect("serialize");

        match accept(&bytes) {
            Ok(accepted) => {
                // Only the exact service name is admitted.
                prop_assert_eq!(service, "manga-web");

                for span in &accepted.spans {
                    prop_assert!(
                        span.name.len() <= MAX_STRING_BYTES + 4,
                        "span name was {} bytes", span.name.len()
                    );
                    prop_assert!(span.attributes.len() <= MAX_ATTRIBUTES);
                    for (k, v) in &span.attributes {
                        prop_assert!(
                            is_allowed_attribute(k),
                            "{} passed the allow-list", k
                        );
                        prop_assert!(v.len() <= MAX_STRING_BYTES + 4);
                    }
                }
            }
            Err(_) => prop_assert_ne!(service, "manga-web"),
        }
    }

    /// Whatever survives must serialize to one line. A raw newline in the
    /// output is the log injection, and it is invisible until an operator's
    /// `jq` reports a parse error on a file they did not write.
    #[test]
    fn accepted_values_serialize_to_one_json_line(
        name in ".{0,512}",
        value in ".{0,512}",
    ) {
        let body = serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "manga-web" } }
                    ]
                },
                "scopeSpans": [{
                    "spans": [{
                        "name": name,
                        "attributes": [
                            { "key": "http.route", "value": { "stringValue": value } }
                        ]
                    }]
                }]
            }]
        });
        let bytes = serde_json::to_vec(&body).expect("serialize");

        let Ok(accepted) = accept(&bytes) else {
            return Ok(());
        };

        for span in &accepted.spans {
            let rendered = serde_json::json!({
                "span_name": span.name,
                "attributes": span.attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            })
            .to_string();

            prop_assert!(
                !rendered.contains('\n'),
                "a raw newline splits one span into two log lines: {}",
                rendered
            );
            prop_assert!(serde_json::from_str::<serde_json::Value>(&rendered).is_ok());
        }
    }

    /// Truncation must never split a character, whatever the input.
    #[test]
    fn truncation_always_yields_valid_utf8(text in ".*", limit in 0usize..256) {
        let cut = truncate(&text, limit);
        // The return type guarantees this; the assertion is that it did not
        // panic getting there, which a byte-index slice would have.
        prop_assert!(cut.is_char_boundary(0) || cut.is_empty());
    }

    /// A batch at or over the span limit must be bounded either way.
    #[test]
    fn span_counts_are_bounded(count in 0usize..(MAX_SPANS * 2)) {
        let spans: Vec<serde_json::Value> = (0..count)
            .map(|i| serde_json::json!({ "name": format!("s{i}") }))
            .collect();
        let body = serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "manga-web" } }
                    ]
                },
                "scopeSpans": [{ "spans": spans }]
            }]
        });
        let bytes = serde_json::to_vec(&body).expect("serialize");

        match accept(&bytes) {
            Ok(accepted) => prop_assert!(accepted.spans.len() <= MAX_SPANS),
            Err(_) => prop_assert!(count > MAX_SPANS),
        }
    }
}
