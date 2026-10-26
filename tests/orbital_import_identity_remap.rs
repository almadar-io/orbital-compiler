//! An import's `entities {}` may retarget the identity its upstream EXPECTS
//! (`expects entity Person [identity]`) onto the consumer's own identity roster, so an
//! atom whose records are owned by "the person signed in" composes under any
//! noun (Customer, User, Member). Relations to the expected name follow the
//! remap, including relations on aux entities an import auto-clones, and the
//! inherited expectation takes the consumer's name.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::orbital_ref_config::validate_orbital_ref_diagnostics;
use orbital_compiler::phases::validation::{entity_access, ValidationContext};
use orbital_core_base::schema::{EntityRef, ExpectDeclaration, OrbitalSchema};

const OWNER_POLICY: &str = r#"["=", ["object/get", "@entity", "ownerId"], "@user.id"]"#;

fn upstream() -> String {
    format!(
        r#"{{
  "name": "test-identity-remap-upstream",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "RecordsOrbital",
      "expects": [ {{ "kind": "identity", "name": "Person", "shape": [ {{ "name": "id", "type": "string", "required": true }} ] }} ],
      "entity": {{
        "name": "Run",
        "persistence": "persistent",
        "collection": "runs",
        "read_policy": {OWNER_POLICY},
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "ownerId", "type": "relation", "required": true, "relation": {{ "entity": "Person", "cardinality": "one" }} }}
        ]
      }},
      "auxiliaryEntities": [
        {{
          "name": "Note",
          "persistence": "persistent",
          "collection": "notes",
          "read_policy": {OWNER_POLICY},
          "fields": [
            {{ "name": "id", "type": "string", "required": true }},
            {{ "name": "ownerId", "type": "relation", "required": true, "relation": {{ "entity": "Person", "cardinality": "one" }} }}
          ]
        }}
      ],
      "traits": [
        {{
          "name": "RunList",
          "linkedEntity": "Run",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["fetch", "Run"], ["fetch", "Note"], ["render-ui", "main", {{ "type": "stack" }}] ] }} ]
          }}
        }}
      ],
      "pages": [ {{ "name": "RunsPage", "path": "/runs", "traits": [ {{ "ref": "RunList" }} ] }} ]
    }},
    {{
      "name": "UseOrbital",
      "entity": {{ "name": "Session", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [
        {{
          "name": "SessionView",
          "linkedEntity": "Session",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["fetch", "Run"], ["fetch", "Note"], ["render-ui", "main", {{ "type": "stack" }}] ] }} ]
          }}
        }}
      ],
      "pages": [ {{ "name": "UsePage", "path": "/use", "traits": [ {{ "ref": "SessionView" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn consumer(entities: &str, use_entities: &str) -> String {
    format!(
        r#"{{
  "name": "test-identity-remap-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "AccountOrbital",
      "entity": {{
        "name": "Customer",
        "persistence": "persistent",
        "identity": true,
        "fields": [ {{ "name": "id", "type": "string", "required": true }}, {{ "name": "name", "type": "string" }} ]
      }},
      "traits": [
        {{
          "name": "AccountView",
          "linkedEntity": "Customer",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "stack" }}] ] }} ]
          }}
        }}
      ],
      "pages": [ {{ "name": "AccountPage", "path": "/account", "traits": [ {{ "ref": "AccountView" }} ] }} ]
    }},
    {{
      "name": "Records",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.RecordsOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.RecordsOrbital", "entity": "Run"{entities} }}
    }},
    {{
      "name": "Use",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.UseOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.UseOrbital", "entities": {{ "Run": "Run"{use_entities} }} }}
    }}
  ]
}}"#
    )
}

fn inline(entities: &str, use_entities: &str) -> (OrbitalSchema, Vec<String>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream()).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(entities, use_entities)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let mut orbital_ref = Vec::new();
    let mut uses_config = Vec::new();
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let mut codes: Vec<String> = validate_orbital_ref_diagnostics(&orbital_ref, &schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    let mut ctx = ValidationContext::new();
    entity_access::validate_entity_access(&schema, &mut ctx);
    codes.extend(ctx.result.errors.iter().map(|e| e.code.as_str().to_string()));
    (schema, codes)
}

fn relation_targets(schema: &OrbitalSchema, orbital: &str) -> Vec<(String, String)> {
    let records = schema.orbitals.iter().find(|o| o.name == orbital).expect("orbital");
    let primary = match &records.entity {
        EntityRef::Inline(def) => vec![def.clone()],
        _ => vec![],
    };
    let aux = records
        .auxiliary_entities
        .iter()
        .flatten()
        .filter_map(|e| match e {
            EntityRef::Inline(def) => Some(def.clone()),
            _ => None,
        });
    let mut out: Vec<(String, String)> = primary
        .into_iter()
        .chain(aux)
        .flat_map(|def| {
            def.fields
                .iter()
                .filter_map(|f| f.relation.as_ref().map(|r| (def.name.clone(), r.entity.clone())))
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort();
    out
}

fn inherited_identity_names(schema: &OrbitalSchema) -> Vec<Option<String>> {
    let records = schema.orbitals.iter().find(|o| o.name == "Records").expect("Records orbital");
    records
        .expects
        .iter()
        .flatten()
        .filter_map(|e| match e {
            ExpectDeclaration::Identity { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

const REMAP: &str = r#", "entities": { "Person": "Customer" }"#;
const USE_REMAP: &str = r#", "Person": "Customer""#;

#[test]
fn an_expected_identity_retargets_onto_the_consumer_identity() {
    let (schema, errors) = inline(REMAP, USE_REMAP);
    for orbital in ["Records", "Use"] {
        let targets = relation_targets(&schema, orbital);
        assert!(!targets.is_empty(), "{orbital} carries the owner relations");
        for (entity, target) in targets {
            assert_eq!(target, "Customer", "{entity}.ownerId still targets {target}");
        }
    }
    assert_eq!(inherited_identity_names(&schema), vec![Some("Customer".to_string())]);
    assert!(!errors.iter().any(|c| c == "ORB_O_ENTITY_UNKNOWN_KEY"), "{errors:?}");
}

#[test]
fn control_without_the_remap_the_relations_keep_the_expected_name() {
    let (schema, _) = inline("", "");
    for orbital in ["Records", "Use"] {
        assert!(relation_targets(&schema, orbital).iter().all(|(_, t)| t == "Person"));
    }
    assert_eq!(inherited_identity_names(&schema), vec![Some("Person".to_string())]);
}

#[test]
fn control_a_key_naming_neither_an_entity_nor_an_expected_identity_is_still_unknown() {
    let (_, errors) = inline(r#", "entities": { "Nobody": "Customer" }"#, "");
    assert!(errors.iter().any(|c| c == "ORB_O_ENTITY_UNKNOWN_KEY"), "{errors:?}");
}

/// The consumer's identity comes from an import of an import (std-identity
/// re-exported by a profile atom) declared AFTER the import that targets it.
const IDENTITY_ATOM: &str = r#"{
  "name": "test-identity-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "IdentityOrbital",
      "id": "orb_01J00000000000000000000001",
      "entity": { "name": "Person", "id": "ent_01J00000000000000000000001", "persistence": "persistent", "identity": true,
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "role", "type": "string" } ] },
      "traits": [
        { "name": "MyAccount", "linkedEntity": "Person", "category": "interaction",
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "stack" }] ] } ] } }
      ],
      "pages": [ { "name": "AccountPage", "path": "/account", "traits": [ { "ref": "MyAccount" } ] } ]
    }
  ]
}"#;

const PROFILE_ATOM: &str = r#"{
  "name": "test-profile-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ProfileOrbital",
      "id": "orb_01J00000000000000000000002",
      "uses": [ { "from": "./identity.orb", "as": "Identity" } ],
      "entity": "Identity.orbitals.IdentityOrbital.entity",
      "traits": [],
      "reference": { "ref": "Identity.orbitals.IdentityOrbital", "entity": "User" }
    }
  ]
}"#;

const NESTED_CONSUMER: &str = r#"{
  "name": "test-identity-remap-nested",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Records",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.RecordsOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.RecordsOrbital", "entity": "Run", "entities": { "Person": "User" } }
    },
    {
      "name": "ProfilePage",
      "uses": [ { "from": "./profile.orb", "as": "Profile" } ],
      "entity": "Profile.orbitals.ProfileOrbital.entity",
      "traits": [],
      "reference": { "ref": "Profile.orbitals.ProfileOrbital", "entity": "User" }
    }
  ]
}"#;

#[test]
fn an_identity_from_a_later_import_of_an_import_is_a_valid_remap_target() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream()).expect("write upstream");
    std::fs::write(dir.path().join("identity.orb"), IDENTITY_ATOM).expect("write identity");
    std::fs::write(dir.path().join("profile.orb"), PROFILE_ATOM).expect("write profile");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, NESTED_CONSUMER).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let (mut orbital_ref, mut uses_config) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let codes: Vec<String> = validate_orbital_ref_diagnostics(&orbital_ref, &schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    assert!(!codes.iter().any(|c| c == "ORB_O_ENTITY_TARGET_UNKNOWN"), "{codes:?}");
    let user_id = schema
        .orbitals
        .iter()
        .find(|o| o.name == "ProfilePage")
        .and_then(|o| match &o.entity {
            EntityRef::Inline(def) => def.id.clone(),
            _ => None,
        })
        .expect("ProfilePage materializes User with an id");
    let records = schema.orbitals.iter().find(|o| o.name == "Records").expect("Records");
    let EntityRef::Inline(run) = &records.entity else { panic!("Run materialized") };
    let owner = run.fields.iter().find(|f| f.name.as_deref() == Some("ownerId")).and_then(|f| f.relation.as_ref()).expect("ownerId relation");
    assert_eq!(owner.entity, "User");
    assert_eq!(owner.entity_id.as_deref(), Some(user_id.as_str()), "the relation points at the materialized identity's id");
}
