//! A resolved program's message catalogs: every loaded behavior's
//! `locales/<locale>/<name>.json` (found through `_metadata.messageSources`),
//! merged per declared locale with keys qualified `<name>:<key>` — the shape
//! `i18n/t` reads at runtime.

use orbital_core::messages::NAMESPACE_SEPARATOR;
use orbital_core::schema::OrbitalSchema;
use std::collections::BTreeMap;
use std::path::Path;

/// Locale → qualified key → message, for every locale `resolved` declares.
pub fn merged_catalogs(
    resolved: &OrbitalSchema,
    program_dir: &Path,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let sources = resolved
        .metadata
        .as_ref()
        .and_then(|m| m.message_sources.as_ref());
    let mut out = BTreeMap::new();
    for locale in resolved.locales.iter().flatten() {
        let mut merged = BTreeMap::new();
        for (name, dir) in sources.into_iter().flatten() {
            let file = program_dir
                .join(dir)
                .join("locales")
                .join(locale)
                .join(format!("{name}.json"));
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(&text)
            else {
                continue;
            };
            for (key, value) in map {
                if let serde_json::Value::String(message) = value {
                    merged.insert(format!("{name}{NAMESPACE_SEPARATOR}{key}"), message);
                }
            }
        }
        out.insert(locale.clone(), merged);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::SchemaMetadata;

    fn program(locales: Option<Vec<&str>>, sources: &[(&str, &str)]) -> OrbitalSchema {
        let mut schema: OrbitalSchema =
            serde_json::from_value(serde_json::json!({ "name": "site", "orbitals": [] })).unwrap();
        schema.locales = locales.map(|l| l.into_iter().map(String::from).collect());
        schema.metadata = Some(SchemaMetadata {
            message_sources: Some(
                sources
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..SchemaMetadata::default()
        });
        schema
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn merges_every_behaviors_catalog_with_qualified_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "locales/en/site.json",
            r#"{"home.title":"Home"}"#,
        );
        write(
            dir.path(),
            "blog/locales/en/post.json",
            r#"{"title":"Why"}"#,
        );
        write(
            dir.path(),
            "locales/ar/site.json",
            r#"{"home.title":"الرئيسية"}"#,
        );
        let catalogs = merged_catalogs(
            &program(Some(vec!["en", "ar"]), &[("site", "."), ("post", "blog")]),
            dir.path(),
        );
        assert_eq!(
            catalogs["en"].get("site:home.title").map(String::as_str),
            Some("Home")
        );
        assert_eq!(
            catalogs["en"].get("post:title").map(String::as_str),
            Some("Why")
        );
        assert_eq!(
            catalogs["ar"].get("site:home.title").map(String::as_str),
            Some("الرئيسية")
        );
        assert_eq!(
            catalogs["ar"].get("post:title"),
            None,
            "an absent catalog contributes nothing (validate reports it)"
        );
    }

    #[test]
    fn control_a_program_without_locales_has_no_catalogs() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "locales/en/site.json",
            r#"{"home.title":"Home"}"#,
        );
        assert!(merged_catalogs(&program(None, &[("site", ".")]), dir.path()).is_empty());
    }
}

/// Writes `<orb stem>.<locale>.json` beside `orb_path` for each locale the
/// resolved program declares.
pub fn write_catalogs(
    orb_path: &Path,
    resolved: &OrbitalSchema,
    program_dir: &Path,
) -> std::io::Result<()> {
    let stem = orb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = orb_path.parent().unwrap_or(Path::new("."));
    for (locale, messages) in merged_catalogs(resolved, program_dir) {
        let json = serde_json::to_string_pretty(&messages).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(format!("{stem}.{locale}.json")), json)?;
    }
    Ok(())
}

/// The compiled app's catalogs: the program and every lazily-used behavior
/// merged per locale (the compiled path builds them into one app).
pub fn program_catalogs(
    resolved: &OrbitalSchema,
    lazy: &[super::lazy::ResolvedLazy],
    program_dir: &Path,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = merged_catalogs(resolved, program_dir);
    for item in lazy {
        for (locale, messages) in program_catalogs(&item.schema, &item.lazy, program_dir) {
            out.entry(locale).or_default().extend(messages);
        }
    }
    out
}
