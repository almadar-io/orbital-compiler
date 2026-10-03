//! `uses lazy`: an `orbital X = Alias.orbitals.Y` reference through a lazy
//! alias is split out of the importer and resolved as its own schema. The
//! importer keeps only `lazyPages`; nothing of the lazy behavior is inlined.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::lazy::split_lazy_and_resolve;
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::{CompilerError, CompilerOptions};
use orbital_core::schema::{LazyPage, OrbitalSchema, PageRef};
use std::path::PathBuf;

const POST: &str = r#"{
  "name": "post",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Post",
      "entity": { "name": "PostState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "PostView",
          "linkedEntity": "PostState",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Why we started" }] ] }
            ]
          }
        }
      ],
      "pages": [ { "name": "PostPage", "path": "/post", "traits": [ { "ref": "PostView" } ] } ]
    }
  ]
}"#;

fn home(uses: &str, trait_refs: &str) -> String {
    format!(
        r#"{{
      "name": "Home",
      "uses": [ {uses} ],
      "entity": {{ "name": "HomeState", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [
        {{
          "name": "HomeView",
          "linkedEntity": "HomeState",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "typography", "content": "Home" }}] ] }} ]
          }}
        }}{trait_refs}
      ],
      "pages": [ {{ "name": "HomePage", "path": "/", "traits": [ {{ "ref": "HomeView" }} ] }} ]
    }}"#
    )
}

fn blog(uses: &str, pages: &str) -> String {
    format!(
        r#"{{
      "name": "BlogWhy",
      "uses": [ {uses} ],
      "entity": "Why.orbitals.Post.entity",
      "traits": [],
      "reference": {{ "ref": "Why.orbitals.Post"{pages} }}
    }}"#
    )
}

const LAZY_USE: &str = r#"{ "from": "./post.orb", "as": "Why", "lazy": true }"#;
const EAGER_USE: &str = r#"{ "from": "./post.orb", "as": "Why" }"#;
const REMAP: &str = r#", "pages": { "/post": "/blog/why" }"#;

struct Site {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn site(orbitals: &[String], with_post: bool) -> Site {
    let dir = tempfile::TempDir::new().expect("temp dir");
    if with_post {
        std::fs::write(dir.path().join("post.orb"), POST).expect("write post");
    }
    let path = dir.path().join("site.orb");
    let body = format!(r#"{{ "name": "site", "version": "1.0.0", "orbitals": [ {} ] }}"#, orbitals.join(","));
    std::fs::write(&path, body).expect("write site");
    Site { _dir: dir, path }
}

fn split(site: &Site) -> Result<(OrbitalSchema, Vec<orbital_compiler::phases::lazy::ResolvedLazy>), CompilerError> {
    let mut schema = parse_file(&site.path).expect("site parses");
    let lazy = split_lazy_and_resolve(&mut schema, &CompilerOptions::default(), Some(&site.path))?;
    Ok((schema, lazy))
}

fn inline_code(err: CompilerError) -> &'static str {
    match err {
        CompilerError::Inline(e) => e.structured_code(),
        other => panic!("expected an inline error, got {other:?}"),
    }
}

#[test]
fn a_lazy_reference_is_split_out_and_its_pages_stay_on_the_importer() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let (schema, lazy) = split(&s).expect("split");
    assert_eq!(schema.orbitals.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), vec!["Home"]);
    assert_eq!(
        schema.lazy_pages,
        Some(vec![LazyPage { path: "/blog/why".into(), orbital: "BlogWhy".into(), orb_ref: "lazy/BlogWhy.orb".into() }])
    );
    assert_eq!(lazy.len(), 1);
    assert_eq!(lazy[0].orb_ref, "lazy/BlogWhy.orb");
    let pages: Vec<&str> = lazy[0]
        .schema
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .filter_map(|p| match p {
            PageRef::Inline(def) => Some(def.path.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(pages, vec!["/blog/why"], "the remap applies inside the split schema");
}

#[test]
fn the_importer_still_resolves_after_the_split() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let (mut schema, _) = split(&s).expect("split");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).expect("importer inlines");
    let json = serde_json::to_string(&schema).unwrap();
    assert!(!json.contains("Why we started"), "nothing of the lazy behavior is inlined into the importer");
}

#[test]
fn control_an_eager_reference_is_inlined_and_nothing_is_split() {
    let s = site(&[home(EAGER_USE, ""), blog(EAGER_USE, REMAP)], true);
    let (mut schema, lazy) = split(&s).expect("split");
    assert!(lazy.is_empty());
    assert_eq!(schema.lazy_pages, None);
    inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).expect("inline");
    assert!(serde_json::to_string(&schema).unwrap().contains("Why we started"));
}

#[test]
fn composing_a_trait_from_a_lazy_import_into_an_eager_orbital_is_rejected() {
    let eager_trait = r#", { "ref": "Why.traits.PostView" }"#;
    let s = site(&[home(LAZY_USE, eager_trait), blog(LAZY_USE, REMAP)], true);
    let err = split(&s).map(|_| ()).and_then(|_| {
        let mut schema = parse_file(&s.path).unwrap();
        let _ = split_lazy_and_resolve(&mut schema, &CompilerOptions::default(), Some(&s.path))?;
        inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).map_err(CompilerError::from)
    });
    assert_eq!(inline_code(err.expect_err("eager use of a lazy alias")), "ORB_LAZY_EAGER_REF");
}

#[test]
fn inlining_a_lazy_reference_without_the_split_is_refused() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let mut schema = parse_file(&s.path).unwrap();
    let err = inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).expect_err("unsplit");
    assert_eq!(err.structured_code(), "ORB_LAZY_NOT_SPLIT");
}

#[test]
fn edge_a_lazy_behavior_with_no_pages_is_rejected() {
    let no_pages = POST.replace(r#""pages": [ { "name": "PostPage", "path": "/post", "traits": [ { "ref": "PostView" } ] } ]"#, r#""pages": []"#);
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, "")], false);
    std::fs::write(s.path.parent().unwrap().join("post.orb"), no_pages).unwrap();
    assert_eq!(inline_code(split(&s).expect_err("no pages")), "ORB_LAZY_NO_PAGES");
}

#[test]
fn edge_a_missing_lazy_file_is_reported() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], false);
    let err = split(&s).expect_err("missing post");
    assert!(err.to_string().contains("post"), "{err}");
}

#[test]
fn the_compiled_path_compiles_the_lazy_pages_into_the_program() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let oir = orbital_compiler::phases::compile_file(&s.path, &CompilerOptions::default()).expect("compile");
    let mut paths: Vec<&str> = oir.pages.iter().map(|p| p.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/blog/why"], "each page is its own route chunk in the compiled app");
}

#[test]
fn control_the_compiled_path_is_unchanged_without_lazy_uses() {
    let s = site(&[home(EAGER_USE, ""), blog(EAGER_USE, REMAP)], true);
    let oir = orbital_compiler::phases::compile_file(&s.path, &CompilerOptions::default()).expect("compile");
    assert_eq!(oir.pages.len(), 2);
}

#[test]
fn the_split_moves_the_lazy_orbitals_ledger_row_to_its_own_schema() {
    use orbital_core::identity::{IdKind, IdentityLedger, LedgerEntry, LedgerOwner};
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let mut schema = parse_file(&s.path).unwrap();
    let row = |id: &str, kind: IdKind, name: &str| LedgerEntry {
        id: id.into(),
        kind,
        baked_name: name.into(),
        cur_name: name.into(),
        renames: vec![],
        owner: LedgerOwner::Workspace,
        parent: None,
    };
    schema.orbitals[0].id = Some("orb_HOME".into());
    schema.orbitals[1].id = Some("orb_BLOG".into());
    schema.ledger = Some(IdentityLedger {
        schema_version: 1,
        entries: [row("orb_HOME", IdKind::Orbital, "Home"), row("orb_BLOG", IdKind::Orbital, "BlogWhy")]
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect(),
    });
    let artifacts = orbital_compiler::phases::lazy::split_lazy(&mut schema);
    let ids = |l: &Option<IdentityLedger>| {
        let mut v: Vec<String> = l.as_ref().unwrap().entries.keys().cloned().collect();
        v.sort();
        v
    };
    assert_eq!(ids(&schema.ledger), vec!["orb_HOME"], "the importer no longer carries the split orbital");
    assert_eq!(ids(&artifacts[0].schema.ledger), vec!["orb_BLOG"], "the split schema carries only its own orbital");
}

#[test]
fn a_schema_compiled_from_elsewhere_resolves_imports_against_its_source() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let elsewhere = tempfile::TempDir::new().unwrap();
    let lowered = elsewhere.path().join("lowered.orb");
    std::fs::copy(&s.path, &lowered).unwrap();
    let oir = orbital_compiler::phases::compile_file_with_source(&lowered, &s.path, &CompilerOptions::default())
        .expect("imports resolve beside the source, not beside the lowered copy");
    assert_eq!(oir.pages.len(), 2);
}

#[test]
fn control_compile_file_resolves_against_its_own_path() {
    let s = site(&[home(LAZY_USE, ""), blog(LAZY_USE, REMAP)], true);
    let elsewhere = tempfile::TempDir::new().unwrap();
    let lowered = elsewhere.path().join("lowered.orb");
    std::fs::copy(&s.path, &lowered).unwrap();
    assert!(orbital_compiler::phases::compile_file(&lowered, &CompilerOptions::default()).is_err());
}

#[test]
fn a_program_with_locales_records_where_each_loaded_behavior_lives() {
    let s = site(&[home(EAGER_USE, ""), blog(EAGER_USE, REMAP)], true);
    let mut schema = parse_file(&s.path).unwrap();
    schema.locales = Some(vec!["en".into(), "ar".into()]);
    inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).expect("inline");
    let sources = schema.metadata.and_then(|m| m.message_sources).expect("message sources");
    assert_eq!(sources.get("site").map(String::as_str), Some("."), "the program itself");
    assert_eq!(sources.get("post").map(String::as_str), Some("."), "an imported behavior, relative to the program");
}

#[test]
fn control_a_program_without_locales_records_no_message_sources() {
    let s = site(&[home(EAGER_USE, ""), blog(EAGER_USE, REMAP)], true);
    let mut schema = parse_file(&s.path).unwrap();
    inline_schema(&mut schema, &InlineConfig::default(), Some(&s.path)).expect("inline");
    assert!(schema.metadata.and_then(|m| m.message_sources).is_none());
}
