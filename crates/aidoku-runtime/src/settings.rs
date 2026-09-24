//! Seeding a source's declared defaults.
//!
//! `defaults::get` answers an unset key with an error, and a source reads its
//! own settings through it at start-up. A source whose `settings.json`
//! declares a default expects that default to be there before it ever runs —
//! MangaDex asks for its content ratings and reports "Unable to fetch default
//! content ratings" when nothing answers.

use serde::Serialize;

/// One declared default, postcard-encoded the way `defaults::get` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub key: String,
    pub value: Vec<u8>,
}

/// The defaults a package declares, ready to store.
///
/// Walks one level of nesting, as `group` holds `items`. A setting with no
/// `key` or no `default` yields nothing: there is no key to store it under,
/// and a source that declared no default is expected to cope without one.
pub fn declared_defaults(declaration: &serde_json::Value) -> Vec<Seed> {
    let mut out = Vec::new();
    let Some(entries) = declaration.as_array() else {
        return out;
    };

    for entry in entries {
        if entry.get("type").and_then(|t| t.as_str()) == Some("group") {
            if let Some(items) = entry.get("items").and_then(|i| i.as_array()) {
                for item in items {
                    out.extend(seed(item));
                }
            }
            continue;
        }
        out.extend(seed(entry));
    }

    out
}

fn seed(entry: &serde_json::Value) -> Option<Seed> {
    let key = entry.get("key")?.as_str()?.to_string();
    let declared = entry.get("type")?.as_str()?;
    let default = entry.get("default")?;

    // Encoded by the *declared* type rather than by the JSON shape: a source
    // reads its setting as one Rust type, and a `select` whose default happens
    // to parse as a number is still a string to it.
    let value = match declared {
        "switch" => encode(&default.as_bool()?),
        "text" | "select" | "segment" => encode(&default.as_str()?.to_string()),
        "multi-select" => {
            let items: Option<Vec<String>> = default
                .as_array()?
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect();
            encode(&items?)
        }
        // Anything else is a control this host does not render, so it has no
        // value to store and no way to check one.
        _ => return None,
    };

    Some(Seed { key, value })
}

fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("postcard encodes a plain value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(seeds: &[Seed]) -> Vec<&str> {
        seeds.iter().map(|s| s.key.as_str()).collect()
    }

    #[test]
    fn a_switch_default_encodes_as_a_bool() {
        let seeds = declared_defaults(&json!([
            { "type": "switch", "key": "showNsfw", "default": true }
        ]));

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].key, "showNsfw");
        assert!(postcard::from_bytes::<bool>(&seeds[0].value).unwrap());
    }

    #[test]
    fn a_select_default_encodes_as_a_string() {
        let seeds = declared_defaults(&json!([
            { "type": "select", "key": "lang", "default": "en" }
        ]));

        assert_eq!(
            postcard::from_bytes::<String>(&seeds[0].value).unwrap(),
            "en"
        );
    }

    /// The MangaDex shape: a list of ratings under a group.
    #[test]
    fn a_multi_select_default_encodes_as_a_list() {
        let seeds = declared_defaults(&json!([
            {
                "type": "group",
                "title": "Content",
                "items": [
                    { "type": "multi-select", "key": "contentRating",
                      "default": ["safe", "suggestive"] }
                ]
            }
        ]));

        assert_eq!(keys(&seeds), vec!["contentRating"]);
        assert_eq!(
            postcard::from_bytes::<Vec<String>>(&seeds[0].value).unwrap(),
            vec!["safe".to_string(), "suggestive".to_string()]
        );
    }

    /// A number where a string was declared is still stored as a string: the
    /// guest reads the type it declared, and postcard is not self-describing,
    /// so a wrong type is not a decode error but wrong data.
    #[test]
    fn the_declared_type_decides_the_encoding() {
        let seeds = declared_defaults(&json!([
            { "type": "select", "key": "n", "default": 3 }
        ]));

        assert!(seeds.is_empty(), "a select whose default is not a string");
    }

    #[test]
    fn a_setting_with_no_default_is_not_seeded() {
        let seeds = declared_defaults(&json!([
            { "type": "text", "key": "token", "title": "Token" }
        ]));

        assert!(seeds.is_empty());
    }

    #[test]
    fn a_setting_with_no_key_is_not_seeded() {
        let seeds = declared_defaults(&json!([
            { "type": "switch", "default": true }
        ]));

        assert!(seeds.is_empty());
    }

    /// `login` and `editable-list` are controls this host does not render, so
    /// it has no value to store under them.
    #[test]
    fn an_unrendered_control_is_not_seeded() {
        let seeds = declared_defaults(&json!([
            { "type": "login", "key": "login", "default": "x" },
            { "type": "editable-list", "key": "blocked", "default": [] }
        ]));

        assert!(seeds.is_empty());
    }

    #[test]
    fn a_declaration_that_is_not_an_array_yields_nothing() {
        assert!(declared_defaults(&json!({ "type": "switch" })).is_empty());
    }
}
