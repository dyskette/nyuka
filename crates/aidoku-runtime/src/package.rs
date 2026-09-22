//! `.aix` loading, and the install-time capability check.
//!
//! An `.aix` is a zip:
//!
//! ```text
//! Payload/source.json     manifest
//! Payload/main.wasm       the module
//! Payload/filters.json    filter schema
//! Payload/settings.json   optional
//! Payload/icon.png
//! ```
//!
//! # Refuse at install, not at download
//!
//! This host implements tier 1 (`net`, `html`, `defaults`, `std`, `env`) and
//! optionally tier 2 (`js` contexts). It does **not** implement `js::webview_*`
//! or `canvas`, which roughly 16% of the community catalog needs.
//!
//! A source requiring one of those must be refused **when it is installed**,
//! with the missing capability named. ADR-0004 calls a source that installs
//! and then fails mid-download the worst outcome, because the failure reads as
//! a site problem rather than a host gap — and the operator has no way to tell
//! the difference.

use std::io::Read;

use nyuka_domain::model::Capability;
use serde::Deserialize;

/// What an `.aix` could not be loaded for.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a valid .aix archive: {0}")]
    Archive(String),
    #[error("missing {0} in the archive")]
    Missing(&'static str),
    #[error("manifest is not valid: {0}")]
    Manifest(String),
    #[error("module could not be read: {0}")]
    Module(String),
    /// The package needs host capabilities this build does not provide.
    #[error("source '{id}' needs unimplemented capabilities: {}", format_caps(.missing))]
    UnsupportedCapabilities {
        id: String,
        missing: Vec<Capability>,
    },
}

fn format_caps(caps: &[Capability]) -> String {
    caps.iter()
        .map(capability_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A human-readable name, for the problem+json the API returns.
pub fn capability_name(cap: &Capability) -> &'static str {
    match cap {
        Capability::Net => "net",
        Capability::Html => "html",
        Capability::Defaults => "defaults",
        Capability::Std => "std",
        Capability::JsContext => "js (script evaluation)",
        Capability::JsWebView => "js (embedded web view)",
        Capability::Canvas => "canvas (image processing)",
    }
}

/// `Payload/source.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub info: Info,
    #[serde(default)]
    pub listings: Vec<Listing>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default, rename = "contentRating")]
    pub content_rating: u8,
    #[serde(default, rename = "minAppVersion")]
    pub min_app_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Listing {
    pub id: String,
    #[serde(default)]
    pub kind: i32,
}

/// A loaded, accepted package.
#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub wasm: Vec<u8>,
    pub filters: Option<serde_json::Value>,
    pub settings: Option<serde_json::Value>,
    pub required: Vec<Capability>,
}

/// The capabilities this build provides.
pub fn supported_capabilities() -> Vec<Capability> {
    let mut caps = vec![
        Capability::Net,
        Capability::Html,
        Capability::Defaults,
        Capability::Std,
    ];
    if cfg!(feature = "js") {
        caps.push(Capability::JsContext);
    }
    caps
}

/// Maps a wasm import to the capability it needs.
///
/// `js` splits by function prefix: evaluating a script is tier 2, while the
/// web-view functions need a real browser engine and are not implemented.
pub fn capability_for_import(module: &str, name: &str) -> Option<Capability> {
    Some(match module {
        "net" => Capability::Net,
        "html" => Capability::Html,
        "defaults" => Capability::Defaults,
        // `env` holds the functions whose guest-side extern block names no
        // import module; they are part of the std surface.
        "std" | "env" => Capability::Std,
        "canvas" => Capability::Canvas,
        "js" if name.starts_with("webview_") => Capability::JsWebView,
        "js" => Capability::JsContext,
        _ => return None,
    })
}

/// The capabilities a module's imports require, deduplicated.
///
/// Deliberately not routed through the enum's discriminants: casting to `u8`
/// and back would silently change meaning if `Capability`'s variants were ever
/// reordered, and nothing would fail at that point.
pub fn required_capabilities<'a>(
    imports: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<Capability> {
    let mut out: Vec<Capability> = Vec::new();
    for cap in imports
        .into_iter()
        .filter_map(|(m, n)| capability_for_import(m, n))
    {
        if !out.contains(&cap) {
            out.push(cap);
        }
    }
    out
}

/// Loads a package and refuses it if this host cannot run it.
pub fn load(bytes: &[u8], module_imports: &dyn ModuleImports) -> Result<Package, LoadError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| LoadError::Archive(e.to_string()))?;

    let manifest: Manifest = {
        let entry = zip
            .by_name("Payload/source.json")
            .map_err(|_| LoadError::Missing("Payload/source.json"))?;
        serde_json::from_reader(entry).map_err(|e| LoadError::Manifest(e.to_string()))?
    };

    let wasm = {
        let mut entry = zip
            .by_name("Payload/main.wasm")
            .map_err(|_| LoadError::Missing("Payload/main.wasm"))?;
        let mut v = Vec::new();
        entry
            .read_to_end(&mut v)
            .map_err(|e| LoadError::Module(e.to_string()))?;
        v
    };

    let filters = read_json(&mut zip, "Payload/filters.json");
    let settings = read_json(&mut zip, "Payload/settings.json");

    let imports = module_imports.imports(&wasm).map_err(LoadError::Module)?;
    let required = required_capabilities(imports.iter().map(|(m, n)| (m.as_str(), n.as_str())));

    let supported = supported_capabilities();
    let missing: Vec<_> = required
        .iter()
        .copied()
        .filter(|c| !supported.contains(c))
        .collect();
    if !missing.is_empty() {
        return Err(LoadError::UnsupportedCapabilities {
            id: manifest.info.id,
            missing,
        });
    }

    Ok(Package {
        manifest,
        wasm,
        filters,
        settings,
        required,
    })
}

fn read_json<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<serde_json::Value> {
    let entry = zip.by_name(name).ok()?;
    serde_json::from_reader(entry).ok()
}

/// Reads a module's imports. Abstracted so the capability check can be tested
/// without compiling wasm.
pub trait ModuleImports {
    fn imports(&self, wasm: &[u8]) -> Result<Vec<(String, String)>, String>;
}

/// Reads imports by compiling with wasmtime.
pub struct WasmtimeImports<'a>(pub &'a crate::engine::Runtime);

impl ModuleImports for WasmtimeImports<'_> {
    fn imports(&self, wasm: &[u8]) -> Result<Vec<(String, String)>, String> {
        let module = self.0.compile(wasm).map_err(|e| e.to_string())?;
        Ok(module
            .imports()
            .map(|i| (i.module().to_string(), i.name().to_string()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<(String, String)>);

    impl ModuleImports for Fixed {
        fn imports(&self, _wasm: &[u8]) -> Result<Vec<(String, String)>, String> {
            Ok(self.0.clone())
        }
    }

    fn aix(manifest: &str, imports: Vec<(&str, &str)>) -> (Vec<u8>, Fixed) {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            use std::io::Write;
            w.start_file("Payload/source.json", opts).unwrap();
            w.write_all(manifest.as_bytes()).unwrap();
            w.start_file("Payload/main.wasm", opts).unwrap();
            w.write_all(b"\0asm\x01\0\0\0").unwrap();
            w.finish().unwrap();
        }
        let fixed = Fixed(
            imports
                .into_iter()
                .map(|(m, n)| (m.to_string(), n.to_string()))
                .collect(),
        );
        (buf, fixed)
    }

    const MANIFEST: &str = r#"{"info":{"id":"en.example","name":"Example","version":4,
        "url":"https://example.test","contentRating":0,"languages":["en"],
        "minAppVersion":"0.7.1"}}"#;

    #[test]
    fn imports_map_to_capabilities() {
        assert_eq!(capability_for_import("net", "send"), Some(Capability::Net));
        assert_eq!(
            capability_for_import("html", "select"),
            Some(Capability::Html)
        );
        assert_eq!(
            capability_for_import("defaults", "get"),
            Some(Capability::Defaults)
        );
        assert_eq!(
            capability_for_import("std", "destroy"),
            Some(Capability::Std)
        );
        // `env` is part of the std surface, not an unknown module.
        assert_eq!(capability_for_import("env", "print"), Some(Capability::Std));
        assert_eq!(
            capability_for_import("canvas", "new_context"),
            Some(Capability::Canvas)
        );
        assert_eq!(capability_for_import("elsewhere", "x"), None);
    }

    /// The `js` module straddles two tiers, and getting the split wrong would
    /// either refuse installable sources or admit unrunnable ones.
    #[test]
    fn js_splits_by_function_prefix() {
        assert_eq!(
            capability_for_import("js", "context_eval"),
            Some(Capability::JsContext)
        );
        assert_eq!(
            capability_for_import("js", "webview_load"),
            Some(Capability::JsWebView)
        );
    }

    #[test]
    fn a_tier_one_source_loads() {
        let (bytes, imports) = aix(
            MANIFEST,
            vec![("net", "send"), ("html", "select"), ("std", "destroy")],
        );
        let pkg = load(&bytes, &imports).expect("tier-1 source loads");
        assert_eq!(pkg.manifest.info.id, "en.example");
        assert_eq!(pkg.manifest.info.version, 4);
        assert!(pkg.required.contains(&Capability::Net));
        assert!(pkg.required.contains(&Capability::Html));
    }

    #[test]
    fn a_canvas_source_is_refused_with_the_capability_named() {
        let (bytes, imports) = aix(MANIFEST, vec![("net", "send"), ("canvas", "new_context")]);
        let err = load(&bytes, &imports).expect_err("canvas is not implemented");
        match err {
            LoadError::UnsupportedCapabilities {
                ref id,
                ref missing,
            } => {
                assert_eq!(id, "en.example");
                assert_eq!(missing, &[Capability::Canvas]);
                // The message must name the capability, so the API can report
                // a host gap rather than a generic failure.
                let text = err.to_string();
                assert!(text.contains("canvas"), "message was: {text}");
            }
            other => panic!("expected a capability refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_webview_source_is_refused() {
        let (bytes, imports) = aix(MANIFEST, vec![("js", "webview_load")]);
        let err = load(&bytes, &imports).expect_err("webview is not implemented");
        assert!(err.to_string().contains("web view"), "{err}");
    }

    #[test]
    fn several_missing_capabilities_are_all_reported() {
        let (bytes, imports) = aix(
            MANIFEST,
            vec![("canvas", "new_context"), ("js", "webview_load")],
        );
        let err = load(&bytes, &imports).expect_err("both are missing");
        let text = err.to_string();
        assert!(text.contains("canvas"), "{text}");
        assert!(text.contains("web view"), "{text}");
    }

    #[test]
    fn a_missing_manifest_is_reported_as_such() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            use std::io::Write;
            w.start_file("Payload/main.wasm", opts).unwrap();
            w.write_all(b"\0asm\x01\0\0\0").unwrap();
            w.finish().unwrap();
        }
        let err = load(&buf, &Fixed(vec![])).expect_err("no manifest");
        assert!(matches!(err, LoadError::Missing("Payload/source.json")));
    }

    #[test]
    fn a_non_archive_is_rejected_rather_than_panicking() {
        let err = load(b"not a zip at all", &Fixed(vec![])).expect_err("garbage");
        assert!(matches!(err, LoadError::Archive(_)));
    }

    #[test]
    fn tier_two_is_gated_on_the_feature() {
        let supported = supported_capabilities();
        assert_eq!(
            supported.contains(&Capability::JsContext),
            cfg!(feature = "js")
        );
        // Tier 3 is never supported by this build.
        assert!(!supported.contains(&Capability::JsWebView));
        assert!(!supported.contains(&Capability::Canvas));
    }
}
