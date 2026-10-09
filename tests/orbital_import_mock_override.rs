//! An import's `mock { field: "a, b" }` gives the materialized primary entity
//! its own sample data (G-ORB-114), so a shared atom (std-identity) seeds the
//! consumer's vocabulary — e.g. personas whose role a policy actually admits.
//! Keys name fields after `fields {}` renames and may name an `extend {}` field.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::orbital_ref_config::validate_orbital_ref_diagnostics;
use orbital_core::schema::{EntityRef, OrbitalSchema};

const UPSTREAM: &str = r#"{
  "name": "test-mock-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "IdentityOrbital",
      "entity": {
        "name": "Person",
        "persistence": "persistent",
        "identity": true,
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "name", "type": "string", "mock": "Maya Chen, Diego Alvarez" },
          { "name": "role", "type": "string", "default": "member" }
        ]
      },
      "traits": [
        {
          "name": "MyAccount",
          "linkedEntity": "Person",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.kind" }] ] } ]
          }
        }
      ],
      "pages": [ { "name": "AccountPage", "path": "/account", "traits": [ { "ref": "MyAccount" } ] } ]
    }
  ]
}"#;

fn consumer(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-mock-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Account",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.IdentityOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.IdentityOrbital", "entity": "Customer"{reference_extra} }}
    }}
  ]
}}"#
    )
}

fn inline(reference_extra: &str) -> (OrbitalSchema, Vec<String>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(reference_extra)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let (mut trait_ref, mut orbital_ref, mut uses_config) = (Vec::new(), Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut trait_ref,
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let codes = validate_orbital_ref_diagnostics(&orbital_ref, &schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    (schema, codes)
}

fn mock_of(schema: &OrbitalSchema, field: &str) -> Option<String> {
    let EntityRef::Inline(def) = &schema.orbitals[0].entity else {
        panic!("materialized entity");
    };
    def.fields
        .iter()
        .find(|f| f.name.as_deref() == Some(field))
        .unwrap_or_else(|| panic!("field {field}"))
        .mock
        .clone()
}

#[test]
fn an_import_mock_replaces_the_upstream_samples() {
    let (schema, codes) = inline(r#", "mock": { "role": "customer, customer, store-manager", "name": "Ana Ruiz, Sam Okafor" }"#);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(mock_of(&schema, "role").as_deref(), Some("customer, customer, store-manager"));
    assert_eq!(mock_of(&schema, "name").as_deref(), Some("Ana Ruiz, Sam Okafor"));
}

#[test]
fn control_without_mock_the_upstream_samples_stay() {
    let (schema, codes) = inline("");
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(mock_of(&schema, "name").as_deref(), Some("Maya Chen, Diego Alvarez"));
    assert_eq!(mock_of(&schema, "role"), None);
}

#[test]
fn a_mock_key_names_the_field_after_its_rename_and_may_name_an_extend_field() {
    let (schema, codes) = inline(
        r#", "fields": { "role": "kind" }, "extend": [ { "name": "tier", "type": "string" } ], "mock": { "kind": "customer, staff", "tier": "gold, silver" }"#,
    );
    assert!(!codes.iter().any(|c| c == "ORB_O_MOCK_UNKNOWN_FIELD"), "{codes:?}");
    assert_eq!(mock_of(&schema, "kind").as_deref(), Some("customer, staff"));
    assert_eq!(mock_of(&schema, "tier").as_deref(), Some("gold, silver"));
}

#[test]
fn a_mock_key_naming_no_field_is_diagnosed_including_a_pre_rename_name() {
    let (_, codes) = inline(r#", "mock": { "nope": "x" }"#);
    assert!(codes.iter().any(|c| c == "ORB_O_MOCK_UNKNOWN_FIELD"), "{codes:?}");
    let (_, codes) = inline(r#", "fields": { "role": "kind" }, "mock": { "role": "x" }"#);
    assert!(codes.iter().any(|c| c == "ORB_O_MOCK_UNKNOWN_FIELD"), "{codes:?}");
}
