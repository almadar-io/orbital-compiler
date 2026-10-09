//! An imported page remembers which upstream page it is (`sourcePage`): the
//! per-locale imports of one upstream page share it, which is what makes them
//! language alternates of each other.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::schema::{OrbitalSchema, PageDefinition, PageRef};

const UPSTREAM: &str = r#"{
  "name": "source-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SiteOrbital",
      "entity": {
        "name": "Site",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "SiteView",
          "linkedEntity": "Site",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [] } ]
          }
        }
      ],
      "pages": [
        { "id": "page-home", "name": "HomePage", "path": "/", "traits": [ { "ref": "SiteView" } ] },
        { "id": "page-about", "name": "AboutPage", "path": "/about", "traits": [ { "ref": "SiteView" } ] },
        { "name": "NoIdPage", "path": "/no-id", "traits": [ { "ref": "SiteView" } ] },
        { "id": "page-relay", "sourcePage": "page-origin", "name": "RelayPage", "path": "/relay", "traits": [ { "ref": "SiteView" } ] }
      ]
    }
  ]
}"#;

fn import(name: &str, pages: &str) -> String {
    format!(
        r#"{{
      "name": "{name}",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.SiteOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.SiteOrbital", "pages": {pages} }}
    }}"#
    )
}

fn inline(orbitals: &[String]) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let path = dir.path().join("consumer.orb");
    std::fs::write(
        &path,
        format!(
            r#"{{ "name": "source-consumer", "version": "1.0.0", "locales": ["en", "ar"], "orbitals": [ {} ] }}"#,
            orbitals.join(",")
        ),
    )
    .expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    schema
}

fn page<'a>(schema: &'a OrbitalSchema, path: &str) -> &'a PageDefinition {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .find_map(|p| match p {
            PageRef::Inline(p) if p.path == path => Some(p),
            _ => None,
        })
        .unwrap_or_else(|| panic!("page {path}"))
}

fn locales() -> OrbitalSchema {
    inline(&[
        import("SiteEN", "{}"),
        import(
            "SiteAR",
            r#"{ "/": "/ar", "/about": "/ar/about", "/no-id": "/ar/no-id", "/relay": "/ar/relay" }"#,
        ),
    ])
}

#[test]
fn per_locale_imports_of_one_upstream_page_share_its_identity() {
    let s = locales();
    let about = page(&s, "/about").source_page.clone();
    assert!(about.is_some());
    assert_eq!(page(&s, "/ar/about").source_page, about);
    assert_eq!(page(&s, "/ar").source_page, page(&s, "/").source_page);
}

#[test]
fn control_different_upstream_pages_stay_distinct() {
    let s = locales();
    assert_ne!(
        page(&s, "/ar").source_page,
        page(&s, "/ar/about").source_page
    );
}

#[test]
fn edge_an_upstream_page_without_an_authored_id_still_shares_its_minted_one() {
    let s = locales();
    let no_id = page(&s, "/no-id").source_page.clone();
    assert!(no_id.is_some());
    assert_eq!(page(&s, "/ar/no-id").source_page, no_id);
}

#[test]
fn edge_a_page_already_imported_keeps_its_original_identity() {
    let s = locales();
    assert_eq!(
        page(&s, "/relay").source_page.as_deref(),
        Some("page-origin")
    );
    assert_eq!(
        page(&s, "/ar/relay").source_page.as_deref(),
        Some("page-origin")
    );
}

#[test]
fn a_single_page_reference_shares_the_identity_of_the_page_it_names() {
    let page_ref = r#"{
      "name": "AboutAR",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.entity",
      "traits": [ { "ref": "Up.traits.SiteView" } ],
      "pages": [ { "ref": "Up.pages.AboutPage", "path": "/ar/about" } ]
    }"#;
    let s = inline(&[import("SiteEN", "{}"), page_ref.to_string()]);
    let about = page(&s, "/about").source_page.clone();
    assert!(about.is_some());
    assert_eq!(page(&s, "/ar/about").source_page, about);
}
