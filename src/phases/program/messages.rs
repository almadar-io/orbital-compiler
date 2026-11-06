//! A resolved program's messages: every loaded behavior's `locales/<locale>/<name>.json` (found through
//! `_metadata.messageSources`), merged per declared locale with keys qualified `<name>:<key>` — the shape `i18n/t`
//! reads at runtime. Resolution stores them in the program's `messages`, so the program carries its own text.

use orbital_core_base::messages::NAMESPACE_SEPARATOR;
use orbital_core_base::schema::OrbitalSchema;
use std::collections::BTreeMap;
use std::path::Path;

/// Locale → qualified key → message, for every locale `resolved` declares. An absent catalog contributes nothing
/// (validate reports it); one that cannot be read, or is not a flat object of strings, fails naming the file.
pub fn merged_catalogs(
    resolved: &OrbitalSchema,
    program_dir: &Path,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
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
            let text = match orbital_core_base::read_set::read_to_string(&file) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", file.display())),
            };
            let catalog: BTreeMap<String, String> = serde_json::from_str(&text)
                .map_err(|e| format!("{}: not a flat object of message strings: {e}", file.display()))?;
            for (key, message) in catalog {
                merged.insert(format!("{name}{NAMESPACE_SEPARATOR}{key}"), message);
            }
        }
        out.insert(locale.clone(), merged);
    }
    Ok(out)
}

/// The messages of a program and every lazily-used behavior, merged per locale: what the compiled app's route
/// manifest names its pages with.
pub fn program_messages(
    resolved: &OrbitalSchema,
    lazy: &[super::lazy::ResolvedLazy],
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = resolved.messages.clone().unwrap_or_default();
    for item in lazy {
        for (locale, messages) in program_messages(&item.schema, &item.lazy) {
            out.entry(locale).or_default().extend(messages);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::SchemaMetadata;

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
        )
        .unwrap();
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
    fn every_catalog_read_and_every_absent_one_is_recorded() {
        let dir = tempfile::TempDir::new().unwrap();
        write(dir.path(), "locales/en/site.json", r#"{"home.title":"Home"}"#);
        orbital_core_base::read_set::begin();
        merged_catalogs(&program(Some(vec!["en", "ar"]), &[("site", ".")]), dir.path()).unwrap();
        let reads = orbital_core_base::read_set::end();
        let absent = dir.path().join("locales/ar/site.json");
        assert!(reads.paths().any(|p| p == &dir.path().join("locales/en/site.json")));
        assert!(reads.paths().any(|p| p == &absent), "an absent catalog decided the result too");
        write(dir.path(), "locales/ar/site.json", r#"{"home.title":"x"}"#);
        assert!(!reads.is_current());
    }

    #[test]
    fn control_a_program_without_locales_has_no_catalogs() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "locales/en/site.json",
            r#"{"home.title":"Home"}"#,
        );
        assert!(merged_catalogs(&program(None, &[("site", ".")]), dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_catalog_that_is_not_a_flat_object_of_strings_fails_naming_the_file() {
        let dir = tempfile::TempDir::new().unwrap();
        write(dir.path(), "locales/en/site.json", r#"{"home": {"title": "Home"}}"#);
        let error = merged_catalogs(&program(Some(vec!["en"]), &[("site", ".")]), dir.path()).unwrap_err();
        assert!(error.contains("locales/en/site.json"), "{error}");
        write(dir.path(), "locales/en/site.json", "{ not json");
        assert!(merged_catalogs(&program(Some(vec!["en"]), &[("site", ".")]), dir.path()).is_err(), "control: unparsable text fails too");
    }
}
