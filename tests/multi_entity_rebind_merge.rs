//! §52 (C-RESOLVE-ATOM-MERGE-FIRST-ENTITY): composed-atom field merges honor
//! the trait ref's `linkedEntity` — a rebind onto an author-declared auxiliary
//! entity lands the imported fields on THAT entity, never on the orbital's
//! first-declared primary; the source side resolves the trait's OWN bound
//! entity (primary or aux) rather than the owning orbital's slot 0.

use orbital_compiler::phases::enrich::{enrich_schema, EnrichmentConfig};
use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{EntityRef, OrbitalSchema};
use std::path::Path;

const BASE: &str = r#"{
  "name": "test-widget-base",
  "version": "1.0.0",
  "orbitals": [{
    "name": "WidgetBaseOrbital",
    "entity": {
      "name": "WidgetView",
      "persistence": "runtime",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "viewState", "type": "string", "intrinsic": true },
        { "name": "payload", "type": "string" }
      ]
    },
    "traits": [{
      "name": "WidgetReview",
      "entityRebindable": true,
      "entityContract": { "requires": ["payload"], "provides": ["viewState"] },
      "linkedEntity": "WidgetView",
      "category": "interaction",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [],
        "transitions": [{
          "from": "idle", "to": "idle", "event": "RENDER",
          "effects": [
            ["set", "@entity.viewState", "computed"],
            ["render-ui", "main", { "type": "typography", "content": "@entity.payload", "state": "@entity.viewState" }]
          ]
        }]
      }
    }]
  }]
}"#;

// Source atom whose trait binds its AUX entity (`AtomSide`), not the primary.
const MULTI_SOURCE: &str = r#"{
  "name": "test-multi-source",
  "version": "1.0.0",
  "orbitals": [{
    "name": "MultiSourceOrbital",
    "entity": {
      "name": "AtomMain",
      "persistence": "runtime",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "mainOnly", "type": "string" }
      ]
    },
    "auxiliaryEntities": [{
      "name": "AtomSide",
      "persistence": "runtime",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "sideState", "type": "string", "intrinsic": true },
        { "name": "sideData", "type": "string" }
      ]
    }],
    "traits": [{
      "name": "SideReview",
      "entityRebindable": true,
      "entityContract": { "requires": ["sideData"], "provides": ["sideState"] },
      "linkedEntity": "AtomSide",
      "category": "interaction",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [],
        "transitions": [{
          "from": "idle", "to": "idle", "event": "RENDER",
          "effects": [ ["set", "@entity.sideState", "computed"] ]
        }]
      }
    }]
  }]
}"#;

// Consumer: primary `Note` + author-declared aux `NoteTagRef`; the widget is
// rebound onto the AUX entity.
const CONSUMER_AUX_REBIND: &str = r#"{
  "name": "test-aux-rebind-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NoteOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "Note",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "title", "type": "string" }
      ]
    },
    "auxiliaryEntities": [{
      "name": "NoteTagRef",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "payload", "type": "string" }
      ]
    }],
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "TagReview", "linkedEntity": "NoteTagRef" }
    ]
  }]
}"#;

const CONSUMER_PRIMARY_REBIND: &str = r#"{
  "name": "test-primary-rebind-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NoteOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "Note",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "payload", "type": "string" }
      ]
    },
    "auxiliaryEntities": [{
      "name": "NoteTagRef",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true }
      ]
    }],
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "NoteReview", "linkedEntity": "Note" }
    ]
  }]
}"#;

const CONSUMER_UNKNOWN_TARGET: &str = r#"{
  "name": "test-unknown-target-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NoteOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "Note",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "payload", "type": "string" }
      ]
    },
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "GhostReview", "linkedEntity": "Ghost" }
    ]
  }]
}"#;

const CONSUMER_MULTI_SOURCE: &str = r#"{
  "name": "test-multi-source-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "RecOrbital",
    "uses": [ { "from": "./multi_source.orb", "as": "Src" } ],
    "entity": {
      "name": "Rec",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "sideData", "type": "string" }
      ]
    },
    "traits": [
      { "ref": "Src.traits.SideReview", "name": "RecReview", "linkedEntity": "Rec" }
    ]
  }]
}"#;

const CONSUMER_NO_REBIND: &str = r#"{
  "name": "test-no-rebind-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "HostOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "HostRecord",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true }
      ]
    },
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "HostReview" }
    ]
  }]
}"#;

fn inline(path: &Path) -> OrbitalSchema {
    let mut schema = parse_file(path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(path)).expect("inline");
    schema
}

fn field_names(entity: &EntityRef) -> Vec<String> {
    match entity {
        EntityRef::Inline(e) => e.fields.iter().filter_map(|f| f.name.clone()).collect(),
        _ => panic!("expected inline entity"),
    }
}

fn aux_entity<'a>(schema: &'a OrbitalSchema, name: &str) -> &'a EntityRef {
    schema.orbitals[0]
        .auxiliary_entities
        .iter()
        .flatten()
        .find(|r| matches!(r, EntityRef::Inline(e) if e.name == name))
        .unwrap_or_else(|| panic!("aux entity {name} not found"))
}

#[test]
fn rebind_to_aux_entity_merges_onto_aux() {
    let dir = tempfile::TempDir::new().expect("temp");
    std::fs::write(dir.path().join("base.orb"), BASE).expect("write base");
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_AUX_REBIND).expect("write");

    let schema = inline(&consumer);
    let aux_fields = field_names(aux_entity(&schema, "NoteTagRef"));
    assert!(
        aux_fields.contains(&"viewState".to_string()),
        "intrinsic must land on the rebind-target AUX entity, got {aux_fields:?}"
    );
    let primary_fields = field_names(&schema.orbitals[0].entity);
    assert_eq!(
        primary_fields,
        vec!["id".to_string(), "title".to_string()],
        "primary must stay byte-unchanged — exactly one destination per merge"
    );
}

#[test]
fn rebind_to_primary_in_multi_entity_orbital_unchanged() {
    let dir = tempfile::TempDir::new().expect("temp");
    std::fs::write(dir.path().join("base.orb"), BASE).expect("write base");
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_PRIMARY_REBIND).expect("write");

    let schema = inline(&consumer);
    let primary_fields = field_names(&schema.orbitals[0].entity);
    assert!(
        primary_fields.contains(&"viewState".to_string()),
        "primary rebind keeps merging onto the primary, got {primary_fields:?}"
    );
    let aux_fields = field_names(aux_entity(&schema, "NoteTagRef"));
    assert_eq!(
        aux_fields,
        vec!["id".to_string()],
        "unrelated aux entity must not receive merged fields"
    );
}

#[test]
fn rebind_to_unknown_target_merges_nowhere_not_the_primary() {
    // §52 fallback retired: "Ghost" names no entity anywhere in the schema
    // (not NoteOrbital's own primary/aux, not `base.orb`'s), so the merge has
    // no landing site and the retired primary-entity fallback no longer
    // applies — `viewState` must not appear on the primary (or anywhere
    // else). `ORB_T_REBIND_TARGET_UNRESOLVED` is the validator's job, tested
    // in `orbital-compiler`'s `trait_validator` unit tests, not here.
    let dir = tempfile::TempDir::new().expect("temp");
    std::fs::write(dir.path().join("base.orb"), BASE).expect("write base");
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_UNKNOWN_TARGET).expect("write");

    let schema = inline(&consumer);
    let primary_fields = field_names(&schema.orbitals[0].entity);
    assert!(
        !primary_fields.contains(&"viewState".to_string()),
        "an unresolvable rebind target must not fall back onto the primary, got {primary_fields:?}"
    );
    for aux in schema.orbitals[0].auxiliary_entities.iter().flatten() {
        if let EntityRef::Inline(def) = aux {
            assert_ne!(
                def.name, "Ghost",
                "an unresolvable target name must never itself materialize as an entity"
            );
        }
    }
}

#[test]
fn source_trait_bound_to_source_aux_merges_aux_fields() {
    let dir = tempfile::TempDir::new().expect("temp");
    std::fs::write(dir.path().join("multi_source.orb"), MULTI_SOURCE).expect("write source");
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_MULTI_SOURCE).expect("write");

    let schema = inline(&consumer);
    let primary_fields = field_names(&schema.orbitals[0].entity);
    assert!(
        primary_fields.contains(&"sideState".to_string()),
        "the source trait's OWN bound entity (the source aux) supplies the merged fields, got {primary_fields:?}"
    );
    assert!(
        !primary_fields.contains(&"mainOnly".to_string()),
        "the source orbital's slot-0 primary must NOT be the merge source, got {primary_fields:?}"
    );
}

#[test]
fn no_rebind_import_registers_aux_once_and_merges_nowhere() {
    let dir = tempfile::TempDir::new().expect("temp");
    std::fs::write(dir.path().join("base.orb"), BASE).expect("write base");
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_NO_REBIND).expect("write");

    let schema = inline(&consumer);
    let registrations = schema.orbitals[0]
        .auxiliary_entities
        .iter()
        .flatten()
        .filter(|r| matches!(r, EntityRef::Inline(e) if e.name == "WidgetView"))
        .count();
    assert_eq!(registrations, 1, "imported entity registered exactly once");
    let primary_fields = field_names(&schema.orbitals[0].entity);
    assert_eq!(
        primary_fields,
        vec!["id".to_string()],
        "no-rebind import with a different entity name merges nothing onto the host"
    );
}

// aux_entity_is_legal_relation_target_and_its_fields_are_walked moved to the std lint golden
// lint-relation-targets `an-auxiliary-entity-is-a-legal-target-and-its-own-relations-are-walked`.

#[test]
fn enrich_stamps_no_default_collection_on_an_aux_entity() {
    let src = r#"{
      "name": "test-aux-enrich",
      "version": "1.0.0",
      "orbitals": [{
        "name": "PostOrbital",
        "entity": {
          "name": "Post",
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true } ]
        },
        "auxiliaryEntities": [{
          "name": "TagRef",
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true } ]
        }],
        "traits": []
      }]
    }"#;
    let mut schema: OrbitalSchema = serde_json::from_str(src).expect("parse schema");
    enrich_schema(&mut schema, &EnrichmentConfig::all());
    let aux = aux_entity(&schema, "TagRef");
    match aux {
        EntityRef::Inline(e) => assert!(
            e.collection.is_none(),
            "a collection is declared or the entity name; enrich derives none"
        ),
        _ => panic!("expected inline aux"),
    }
}
