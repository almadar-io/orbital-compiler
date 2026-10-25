//! Reference-pair collection for trait References: each trait Reference is
//! paired with its deferred upstream declaration when the session asks.

use orbital_compiler::phases::inline::{inline_schema, InlineSession};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::InlineConfig;

const K_ATOM: &str = r#"{
  "name": "k-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "KOrbital",
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "n", "type": "number" }
        ]
      },
      "traits": [
        {
          "name": "Widget",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": { "k": { "type": "number", "default": 0 } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD" }, { "key": "LEAF_PING" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "LOAD", "effects": [ ["set", "@entity.n", "@config.k"] ] },
              { "from": "idle", "to": "idle", "event": "LEAF_PING", "effects": [] }
            ]
          }
        }
      ]
    }
  ]
}"#;

const K_MIDDLE: &str = r#"{
  "name": "k-middle",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MidOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Inner" } ],
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [ { "ref": "Inner.traits.Widget", "name": "Mid", "linkedEntity": "Widget" } ]
    }
  ]
}"#;

fn k_consumer(from: &str, reference: &str) -> String {
    format!(
        r#"{{
  "name": "k-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Composer",
      "uses": [ {{ "from": "{from}", "as": "Up" }} ],
      "entity": {{
        "name": "Rec",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [ {{ "ref": "{reference}", "name": "T", "linkedEntity": "Rec", "config": {{ "k": {{ "type": "number", "default": 1 }} }} }} ]
    }}
  ]
}}"#
    )
}

fn references_of(path: &std::path::Path) -> Vec<serde_json::Value> {
    let session = InlineSession::default();
    session.collect_references();
    let config = InlineConfig { session: Some(session.clone()), ..InlineConfig::default() };
    let mut schema = parse_file(path).expect("must parse");
    inline_schema(&mut schema, &config, Some(path)).expect("inline must succeed");
    serde_json::to_value(session.take_references())
        .expect("serializes")
        .as_array()
        .cloned()
        .expect("a list")
}

fn k_dir(consumer: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), K_ATOM).expect("write atom");
    std::fs::write(dir.path().join("middle.orb"), K_MIDDLE).expect("write middle");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer).expect("write consumer");
    (dir, path)
}

#[test]
fn a_trait_reference_is_paired_with_its_deferred_upstream_declaration() {
    let (_dir, path) = k_dir(&k_consumer("./atom.orb", "Up.traits.Widget"));
    let references = references_of(&path);
    assert_eq!(references.len(), 1, "{references:#?}");
    let pair = &references[0];
    assert_eq!(pair["kind"], "trait");
    assert_eq!(pair["alias"], "Up");
    assert_eq!(pair["name"], "Widget");
    assert_eq!(pair["path"], "orbitals[name=Composer].traits[0]");
    assert_eq!(pair["callSite"]["ref"], "Up.traits.Widget");
    assert_eq!(pair["callSite"]["config"]["k"]["default"].as_f64(), Some(1.0));
    assert_eq!(pair["declaration"]["name"], "Widget");
    assert!(
        pair["declaration"].to_string().contains("@config.k"),
        "the declaration keeps `@config.k` deferred: {}",
        pair["declaration"]
    );
}

#[test]
fn control_a_file_with_no_uses_collects_no_references() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("atom.orb");
    std::fs::write(&path, K_ATOM).expect("write atom");
    assert!(references_of(&path).is_empty());
}

#[test]
fn control_collection_is_off_until_the_session_asks() {
    let (_dir, path) = k_dir(&k_consumer("./atom.orb", "Up.traits.Widget"));
    let session = InlineSession::default();
    let config = InlineConfig { session: Some(session.clone()), ..InlineConfig::default() };
    let mut schema = parse_file(&path).expect("must parse");
    inline_schema(&mut schema, &config, Some(&path)).expect("inline must succeed");
    assert!(session.take_references().is_empty());
}

#[test]
fn a_ref_trait_inside_the_import_is_expanded_in_the_declaration() {
    let (_dir, path) = k_dir(&k_consumer("./middle.orb", "Up.traits.Mid"));
    let references = references_of(&path);
    assert_eq!(references.len(), 1, "only the compiled file's own reference is answered, not the import's: {references:#?}");
    let consumer = &references[0];
    assert_eq!(consumer["alias"], "Up");
    assert_eq!(consumer["name"], "Mid");
    assert_eq!(consumer["declaration"]["name"], "Mid");
    let declaration = consumer["declaration"].to_string();
    assert!(declaration.contains("LEAF_PING"), "the leaf's body is expanded: {declaration}");
    assert!(declaration.contains("@config.k"), "still deferred: {declaration}");
}
