//! One program resolve shares its loaded files and each imported behavior's
//! inlined result across every orbital and every lazy behavior. Sharing must
//! change nothing: each piece resolves exactly as it would alone.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::lazy::{resolve_program_with_path, split_lazy};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::{resolve_schema_with_path, CompilerOptions};
use orbital_core_base::schema::{Orbital, OrbitalSchema};
use std::path::{Path, PathBuf};

const LEAF: &str = r#"{
  "name": "leaf",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Leaf",
      "entity": { "name": "LeafState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "LeafView", "linkedEntity": "LeafState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Leaf" }] ] } ] } }
      ],
      "pages": [ { "name": "LeafPage", "path": "/leaf", "traits": [ { "ref": "LeafView" } ] } ]
    }
  ]
}"#;

/// `post` imports `leaf` with a call-site config knob `leaf` never declares,
/// so every inline of `post` records one `uses` config site.
const POST: &str = r#"{
  "name": "post",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Post",
      "uses": [ { "from": "./leaf.orb", "as": "L", "config": { "bogus": { "type": "number", "default": 1 } } } ],
      "entity": { "name": "PostState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "PostView", "linkedEntity": "PostState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Why we started" }] ] } ] } }
      ],
      "pages": [ { "name": "PostPage", "path": "/post", "traits": [ { "ref": "PostView" } ] } ]
    }
  ]
}"#;

const OTHER: &str = r#"{
  "name": "other",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Other",
      "entity": { "name": "OtherState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "OtherView", "linkedEntity": "OtherState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Other" }] ] } ] } }
      ],
      "pages": [ { "name": "OtherPage", "path": "/other", "traits": [ { "ref": "OtherView" } ] } ]
    }
  ]
}"#;

fn reference(
    name: &str,
    from: &str,
    alias: &str,
    upstream: &str,
    page_from: &str,
    page_to: &str,
    lazy: bool,
) -> String {
    format!(
        r#"{{
      "name": "{name}",
      "uses": [ {{ "from": "{from}", "as": "{alias}"{lazy} }} ],
      "entity": "{alias}.orbitals.{upstream}.entity",
      "traits": [],
      "reference": {{ "ref": "{alias}.orbitals.{upstream}", "pages": {{ "{page_from}": "{page_to}" }} }}
    }}"#,
        lazy = if lazy { r#", "lazy": true"# } else { "" }
    )
}

struct Program {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn program(orbitals: &[String], locales: bool) -> Program {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (file, body) in [("leaf.orb", LEAF), ("post.orb", POST), ("other.orb", OTHER)] {
        std::fs::write(dir.path().join(file), body).expect("write behavior");
    }
    let path = dir.path().join("site.orb");
    let locales = if locales {
        r#", "locales": ["en"]"#
    } else {
        ""
    };
    let body = format!(
        r#"{{ "name": "site", "version": "1.0.0"{locales}, "orbitals": [ {} ] }}"#,
        orbitals.join(",")
    );
    std::fs::write(&path, body).expect("write site");
    Program { _dir: dir, path }
}

fn resolved(path: &Path) -> OrbitalSchema {
    resolve_schema_with_path(
        parse_file(path).expect("parses"),
        &CompilerOptions::default(),
        Some(path),
    )
    .expect("resolves")
}

/// Fixtures carry no identity ledger, so each resolve mints fresh ids; compare everything else.
fn without_ids(value: serde_json::Value) -> serde_json::Value {
    fn is_identity_key(k: &str) -> bool {
        k == "id"
            || k == "sourcePage"
            || k == "ledger"
            || k == "schemaVersion"
            || k.ends_with("Id")
            || k.ends_with("Ids")
    }
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(k, _)| !is_identity_key(k))
                .map(|(k, v)| (k, without_ids(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(without_ids).collect())
        }
        other => other,
    }
}

/// The orbital as JSON, its imports written relative to the program's directory: resolution anchors a relative import to
/// the file it names (G-CROSS-358), and two programs in different directories name the same sibling.
fn orbital_json(schema: &OrbitalSchema, name: &str, program: &Path) -> serde_json::Value {
    let orbital: &Orbital = schema
        .orbitals
        .iter()
        .find(|o| o.name == name)
        .expect("orbital present");
    let dir = format!("{}/", std::path::absolute(program.parent().expect("a program directory")).expect("absolute").display());
    let mut json = without_ids(serde_json::to_value(orbital).expect("serializes"));
    for decl in json["uses"].as_array_mut().into_iter().flatten() {
        if let Some(from) = decl["from"].as_str().and_then(|f| f.strip_prefix(&dir)).map(|rest| format!("./{rest}")) {
            decl["from"] = serde_json::Value::String(from);
        }
    }
    json
}

fn blog(name: &str, to: &str) -> String {
    reference(name, "./post.orb", "Why", "Post", "/post", to, false)
}

#[test]
fn an_orbital_that_reuses_an_import_resolves_as_it_would_alone() {
    let both = program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false);
    let only_b = program(&[blog("BlogB", "/b")], false);
    assert_eq!(
        orbital_json(&resolved(&both.path), "BlogB", &both.path),
        orbital_json(&resolved(&only_b.path), "BlogB", &only_b.path)
    );
}

#[test]
fn control_the_first_user_of_an_import_is_unchanged_too() {
    let both = program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false);
    let only_a = program(&[blog("BlogA", "/a")], false);
    assert_eq!(
        orbital_json(&resolved(&both.path), "BlogA", &both.path),
        orbital_json(&resolved(&only_a.path), "BlogA", &only_a.path)
    );
}

fn uses_config_site_count(p: &Program) -> usize {
    let mut schema = parse_file(&p.path).expect("parses");
    let (mut orbital_diags, mut uses_diags) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&p.path),
        &mut orbital_diags,
        &mut uses_diags,
    )
    .expect("inlines");
    uses_diags.iter().filter(|d| d.alias == "L").count()
}

#[test]
fn every_use_of_an_import_still_records_its_uses_config_sites() {
    let one = uses_config_site_count(&program(&[blog("BlogA", "/a")], false));
    assert!(
        one >= 1,
        "post's own `uses` config call site is recorded"
    );
    let two =
        uses_config_site_count(&program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false));
    assert_eq!(two, 2 * one);
}

fn lazy_program() -> Program {
    program(
        &[
            reference("BlogA", "./post.orb", "Why", "Post", "/post", "/a", true),
            reference("BlogB", "./post.orb", "Why", "Post", "/post", "/b", true),
            reference(
                "Elsewhere",
                "./other.orb",
                "O",
                "Other",
                "/other",
                "/elsewhere",
                true,
            ),
        ],
        true,
    )
}

#[test]
fn each_lazy_behavior_resolves_as_it_would_alone() {
    let p = lazy_program();
    let (_, lazy) = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .expect("resolves");
    let mut split_source = parse_file(&p.path).unwrap();
    for artifact in split_lazy(&mut split_source) {
        let alone = resolve_schema_with_path(
            artifact.schema.clone(),
            &CompilerOptions::default(),
            Some(&p.path),
        )
        .expect("alone");
        let shared = lazy
            .iter()
            .find(|l| l.orbital == artifact.orbital)
            .expect("resolved lazily");
        assert_eq!(
            without_ids(serde_json::to_value(&shared.schema).unwrap()),
            without_ids(serde_json::to_value(&alone).unwrap()),
            "{}",
            artifact.orbital
        );
    }
}

#[test]
fn a_lazy_behavior_lists_only_the_catalogs_it_loaded() {
    let p = lazy_program();
    let (_, lazy) = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .expect("resolves");
    let sources = |name: &str| {
        let item = lazy
            .iter()
            .find(|l| l.orbital == name)
            .expect("lazy behavior");
        item.schema
            .metadata
            .as_ref()
            .and_then(|m| m.message_sources.clone())
            .expect("message sources")
    };
    let b = sources("BlogB");
    assert!(
        b.contains_key("post") && b.contains_key("leaf"),
        "the second user of post still lists post and leaf: {b:?}"
    );
    let elsewhere = sources("Elsewhere");
    assert!(elsewhere.contains_key("other"), "{elsewhere:?}");
    assert!(
        !elsewhere.contains_key("post") && !elsewhere.contains_key("leaf"),
        "no catalogs from other lazy behaviors: {elsewhere:?}"
    );
}
