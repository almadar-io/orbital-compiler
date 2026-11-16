//! An orbital import carries the struct, dictionary and list aliases its upstream declares, qualified by the
//! upstream behavior (R13: no type importing), and the imported traits read them under that qualified name. A name the
//! importer declares itself stays its own.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::OrbitalSchema;

fn upstream() -> &'static str {
    r#"{
  "name": "up",
  "version": "1.0.0",
  "orbitals": [{
    "name": "DocOrbital",
    "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs", "fields": [{ "name": "id", "type": "string", "required": true }] },
    "traits": [{
      "name": "DocView", "linkedEntity": "Doc", 
      "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "OPEN" }],
        "transitions": [{ "from": "idle", "to": "idle", "event": "OPEN", "effects": [["render-ui", "main", { "type": "typography", "content": "x" }]] }] }
    }],
    "pages": [{ "name": "DocPage", "path": "/docs", "traits": [{ "ref": "DocView" }] }],
    "types": {
      "Point": [{ "name": "x", "type": "number" }],
      "Row": [{ "name": "", "type": "Map<string,string>" }]
    }
  }]
}"#
}

fn consumer(own_types: &str) -> String {
    format!(
        r#"{{
  "name": "consumer", "version": "1.0.0",
  "orbitals": [{{
    "name": "Account",
    "uses": [{{ "from": "./upstream.orb", "as": "Up" }}],
    "entity": "Up.orbitals.DocOrbital.entity",
    "traits": [],
    "reference": {{ "ref": "Up.orbitals.DocOrbital" }}{own_types}
  }}]
}}"#
    )
}

fn qualified<'a>(types: &'a std::collections::BTreeMap<String, Vec<orbital_core_base::schema::PayloadField>>, short: &str) -> Option<&'a String> {
    types.keys().find(|k| k.rsplit_once("::").is_some_and(|(_, name)| name == short))
}

fn inlined(own_types: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream()).unwrap();
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer(own_types)).unwrap();
    let mut schema = parse_file(&path).expect("consumer parses");
    let (mut b, mut c) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(&mut schema, &InlineConfig::default(), Some(&path), &mut b, &mut c)
        .expect("inline succeeds");
    schema
}

#[test]
fn the_importer_knows_the_aliases_its_upstream_declares() {
    let schema = inlined("");
    let types = schema.orbitals[0].types.as_ref().expect("types carried");
    assert!(qualified(types, "Point").is_some() && !types.contains_key("Point"), "{:?}", types.keys());
    let row = qualified(types, "Row").expect("Row arrives qualified");
    assert_eq!(types[row][0].field_type, "Map<string,string>");
}

#[test]
fn a_name_the_importer_declares_wins_over_the_upstream_one() {
    let schema = inlined(r#", "types": { "Point": [{ "name": "y", "type": "string" }] }"#);
    let types = schema.orbitals[0].types.as_ref().unwrap();
    assert_eq!(types["Point"][0].name, "y");
    assert_eq!(types[qualified(types, "Point").expect("the upstream Point stays beside it")][0].name, "x");
    assert!(qualified(types, "Row").is_some(), "control: the other upstream aliases still arrive");
}

#[test]
fn a_trait_mounted_from_an_import_brings_the_aliases_its_file_declares() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream()).unwrap();
    let path = dir.path().join("consumer.orb");
    std::fs::write(
        &path,
        r#"{
  "name": "consumer", "version": "1.0.0",
  "orbitals": [{
    "name": "Account",
    "uses": [{ "from": "./upstream.orb", "as": "Up" }],
    "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs", "fields": [{ "name": "id", "type": "string", "required": true }] },
    "traits": [{ "ref": "Up.traits.DocView", "linkedEntity": "Doc" }],
    "pages": [{ "name": "P", "path": "/p", "traits": [{ "ref": "DocView" }] }]
  }]
}"#,
    )
    .unwrap();
    let mut schema = parse_file(&path).expect("consumer parses");
    let (mut b, mut c) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(&mut schema, &InlineConfig::default(), Some(&path), &mut b, &mut c)
        .expect("inline succeeds");
    let types = schema.orbitals[0].types.as_ref().expect("the trait's file's aliases arrive");
    assert!(types.contains_key("up::Point") && types.contains_key("up::Row"), "{:?}", types.keys().collect::<Vec<_>>());
    assert!(!types.contains_key("Point") && !types.contains_key("Row"), "control: a mounted trait brings its file's types qualified by that file (R13), never bare");
}
