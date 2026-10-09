//! Page modifiers (`access`, `indexing`, `title`, `description`) survive
//! imports: a whole-orbital `pages {}` remap keeps them, a page reference
//! inherits what it does not override, and no import may weaken an upstream
//! `access: authenticated` to `public`.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig, InlineError};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::{compile_schema, CompilerOptions};
use orbital_core::schema::{
    OrbitalSchema, PageAccess, PageDefinition, PageIndexing, PageMeta, PageRef, SiteMeta,
};

const UPSTREAM: &str = r#"{
  "name": "modifiers-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AccountOrbital",
      "entity": {
        "name": "Account",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "AccountView",
          "linkedEntity": "Account",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [] } ]
          }
        }
      ],
      "pages": [
        {
          "name": "AccountPage", "path": "/account", "traits": [ { "ref": "AccountView" } ],
          "access": "authenticated", "indexing": "noindex", "title": ["i18n/t", "up:meta.account"]
        },
        {
          "name": "AboutPage", "path": "/about", "traits": [ { "ref": "AccountView" } ],
          "access": "public", "indexing": "index", "title": "About", "description": "Who we are"
        }
      ]
    }
  ]
}"#;

fn whole_import(pages: &str) -> String {
    format!(
        r#"{{
      "name": "AccountImport",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.AccountOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.AccountOrbital", "pages": {pages} }}
    }}"#
    )
}

fn page_import(page_ref: &str) -> String {
    format!(
        r#"{{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.entity",
      "traits": [ {{ "ref": "Up.traits.AccountView" }} ],
      "pages": [ {page_ref} ]
    }}"#
    )
}

fn consumer(orbital: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let path = dir.path().join("consumer.orb");
    std::fs::write(
        &path,
        format!(r#"{{ "name": "modifiers-consumer", "version": "1.0.0", "site": {{ "origin": "https://example.org", "signIn": "/login" }}, "orbitals": [ {orbital} ] }}"#),
    )
    .expect("write consumer");
    (dir, path)
}

fn inline(orbital: &str) -> Result<OrbitalSchema, InlineError> {
    let (_dir, path) = consumer(orbital);
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path))?;
    Ok(schema)
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

#[test]
fn a_pages_remap_keeps_every_modifier() {
    let schema = inline(&whole_import(
        r#"{ "/account": "/me", "/about": "/about-us" }"#,
    ))
    .expect("inline");
    let me = page(&schema, "/me");
    assert_eq!(me.access, Some(PageAccess::Authenticated));
    assert_eq!(me.indexing, Some(PageIndexing::Noindex));
    assert_eq!(me.title, Some(PageMeta::message("up:meta.account")));
    let about = page(&schema, "/about-us");
    assert_eq!(
        about.description,
        Some(PageMeta::Literal("Who we are".into()))
    );
}

#[test]
fn control_an_unremapped_import_keeps_them_too() {
    let schema = inline(&whole_import("{}")).expect("inline");
    assert_eq!(
        page(&schema, "/account").access,
        Some(PageAccess::Authenticated)
    );
}

#[test]
fn a_page_reference_inherits_what_it_does_not_override() {
    let schema = inline(&page_import(
        r#"{ "ref": "Up.pages.AboutPage", "path": "/team", "indexing": "noindex" }"#,
    ))
    .expect("inline");
    let team = page(&schema, "/team");
    assert_eq!(team.indexing, Some(PageIndexing::Noindex));
    assert_eq!(team.access, Some(PageAccess::Public));
    assert_eq!(team.title, Some(PageMeta::Literal("About".into())));
}

#[test]
fn a_page_reference_may_strengthen_access() {
    let schema = inline(&page_import(
        r#"{ "ref": "Up.pages.AboutPage", "path": "/staff", "access": "authenticated" }"#,
    ))
    .expect("inline");
    assert_eq!(
        page(&schema, "/staff").access,
        Some(PageAccess::Authenticated)
    );
}

#[test]
fn a_page_reference_cannot_weaken_access() {
    let err = inline(&page_import(
        r#"{ "ref": "Up.pages.AccountPage", "path": "/open", "access": "public" }"#,
    ))
    .expect_err("weakening must fail");
    assert_eq!(err.structured_code(), "ORB_O_PAGE_ACCESS_WEAKENED");
    assert_eq!(err.structured_path(), "/open");
}

#[test]
fn the_oir_carries_page_modifiers_and_site() {
    let mut schema: OrbitalSchema = serde_json::from_str(UPSTREAM).expect("schema");
    schema.site = Some(SiteMeta {
        origin: Some("https://example.org".into()),
        sign_in: Some("/login".into()),
        ..SiteMeta::default()
    });
    let module = compile_schema(schema, &CompilerOptions::default()).expect("compiles");
    let account = module
        .pages
        .iter()
        .find(|p| p.path == "/account")
        .expect("account page");
    assert_eq!(account.access, Some(PageAccess::Authenticated));
    assert_eq!(account.title, Some(PageMeta::message("up:meta.account")));
    let site = module.site.expect("site");
    assert_eq!(site.origin.as_deref(), Some("https://example.org"));
    assert_eq!(site.sign_in.as_deref(), Some("/login"));
}

#[test]
fn control_a_page_without_modifiers_has_none_in_the_oir() {
    let schema: OrbitalSchema = serde_json::from_str(
        &UPSTREAM
            .replace(r#""access": "authenticated", "indexing": "noindex", "title": ["i18n/t", "up:meta.account"]"#, r#""label": "Account""#),
    )
    .expect("schema");
    let module = compile_schema(schema, &CompilerOptions::default()).expect("compiles");
    let account = module
        .pages
        .iter()
        .find(|p| p.path == "/account")
        .expect("account page");
    assert_eq!((account.access, account.indexing), (None, None));
    assert!(account.title.is_none() && module.site.is_none());
}

#[test]
fn an_unsupported_metadata_expression_is_rejected_by_the_schema_type() {
    let bad = UPSTREAM.replace(
        r#""title": "About""#,
        r#""title": ["str/concat", "a", "b"]"#,
    );
    assert!(serde_json::from_str::<OrbitalSchema>(&bad).is_err());
}

fn whole_import_with(pages: &str, modifiers: &str) -> String {
    format!(
        r#"{{
      "name": "AccountImport",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.AccountOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.AccountOrbital", "pages": {pages}, "pageModifiers": {modifiers} }}
    }}"#
    )
}

#[test]
fn a_remap_entry_declares_modifiers_over_the_upstream_page() {
    let schema = inline(&whole_import_with(
        r#"{ "/about": "/play/about" }"#,
        r#"{ "/about": { "indexing": "noindex", "title": "Play", "translationOf": "about" } }"#,
    ))
    .expect("inline");
    let about = page(&schema, "/play/about");
    assert_eq!(about.indexing, Some(PageIndexing::Noindex));
    assert_eq!(about.title, Some(PageMeta::Literal("Play".into())));
    assert_eq!(about.translation_of.as_deref(), Some("about"));
    assert_eq!(
        about.access,
        Some(PageAccess::Public),
        "absent modifiers inherit"
    );
    assert_eq!(
        about.description,
        Some(PageMeta::Literal("Who we are".into()))
    );
}

#[test]
fn control_modifiers_for_one_entry_leave_the_other_pages_alone() {
    let schema = inline(&whole_import_with(
        r#"{ "/about": "/play/about" }"#,
        r#"{ "/about": { "indexing": "noindex" } }"#,
    ))
    .expect("inline");
    assert_eq!(
        page(&schema, "/account").indexing,
        Some(PageIndexing::Noindex)
    );
    assert_eq!(
        page(&schema, "/account").access,
        Some(PageAccess::Authenticated)
    );
}

#[test]
fn a_remap_entry_cannot_weaken_access_either() {
    let err = inline(&whole_import_with(
        r#"{ "/account": "/me" }"#,
        r#"{ "/account": { "access": "public" } }"#,
    ))
    .expect_err("weakened");
    assert_eq!(err.structured_code(), "ORB_O_PAGE_ACCESS_WEAKENED");
}

#[test]
fn a_page_reference_declares_its_translation_group() {
    let schema = inline(&page_import(
        r#"{ "ref": "Up.pages.AboutPage", "path": "/ar/about", "translationOf": "about" }"#,
    ))
    .expect("inline");
    assert_eq!(
        page(&schema, "/ar/about").translation_of.as_deref(),
        Some("about")
    );
}

#[test]
fn an_import_declaring_index_still_meets_the_indexed_page_rules() {
    use orbital_compiler::phases::validation::page::validate_page_modifiers;
    let schema = inline(&whole_import_with(
        r#"{ "/account": "/me" }"#,
        r#"{ "/account": { "indexing": "index" } }"#,
    ))
    .expect("inline");
    let codes: Vec<&str> = validate_page_modifiers(&schema)
        .errors
        .iter()
        .map(|e| e.code.as_str())
        .collect();
    assert!(codes.contains(&"ORB_P_INDEX_REQUIRES_PUBLIC"), "{codes:?}");
}

#[test]
fn control_an_import_meeting_the_rules_passes_them() {
    use orbital_compiler::phases::validation::page::validate_page_modifiers;
    let schema = inline(&whole_import_with(
        r#"{}"#,
        r#"{ "/about": { "indexing": "index" } }"#,
    ))
    .expect("inline");
    assert!(validate_page_modifiers(&schema).errors.is_empty());
}
