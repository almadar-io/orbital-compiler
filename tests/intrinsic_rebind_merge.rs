//! Field-level rebindability end-to-end: intrinsic machinery travels with a
//! trait through a 3-level rebind chain (base → variation → consumer); a DATA
//! field absent on a rebind target with no remap is rejected; a `fields:` remap
//! satisfies a renamed DATA read.
//!
//! Mirrors `relation_rebind_merge.rs` / `config_override_round_trip.rs` style.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::error::ErrorCode;
use orbital_compiler::phases::validation::validate_comprehensive;
use orbital_core::schema::{EntityRef, OrbitalSchema};
use std::path::Path;

/// True when the comprehensive validation result contains a
/// `ORB_T_REBIND_MISSING_FIELDS` error.
fn has_rebind_missing_fields(schema: &OrbitalSchema) -> bool {
    validate_comprehensive(schema)
        .errors
        .iter()
        .any(|e| !e.is_warning() && e.code == ErrorCode::RebindMissingEntityFields)
}

// ---------------------------------------------------------------------------
// Base widget atom: a self-contained widget whose `viewState` is INTRINSIC
// machinery (computed internally, read via @entity, not consumer-supplied) and
// `payload` is a DATA field the consumer must supply.
// ---------------------------------------------------------------------------
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

// Variation atom: imports the base, rebinds the widget onto its OWN entity
// `VariationItem` (which declares `payload` as DATA). The intrinsic `viewState`
// must be carried here by the merge.
const VARIATION: &str = r#"{
  "name": "test-widget-variation",
  "version": "1.0.0",
  "orbitals": [{
    "name": "VariationOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "VariationItem",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "payload", "type": "string" }
      ]
    },
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "VariationReview", "linkedEntity": "VariationItem" }
    ]
  }]
}"#;

// Consumer (organism): imports the variation, rebinds onto `DomainRecord`
// (declares `payload`). The intrinsic `viewState` must travel ALL THREE levels.
const CONSUMER_OK: &str = r#"{
  "name": "test-widget-consumer",
  "version": "1.0.0",
  "orbitals": [{
    "name": "ConsumerOrbital",
    "uses": [ { "from": "./variation.orb", "as": "Var" } ],
    "entity": {
      "name": "DomainRecord",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "payload", "type": "string" }
      ]
    },
    "traits": [
      { "ref": "Var.traits.VariationReview", "name": "ConsumerReview", "linkedEntity": "DomainRecord" }
    ]
  }]
}"#;

// Consumer that DOES NOT supply the DATA `payload` and provides no remap.
const CONSUMER_MISSING_DATA: &str = r#"{
  "name": "test-widget-consumer-missing",
  "version": "1.0.0",
  "orbitals": [{
    "name": "ConsumerMissingOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "BareRecord",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true }
      ]
    },
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "BareReview", "linkedEntity": "BareRecord" }
    ]
  }]
}"#;

// Consumer that supplies the DATA field under a DIFFERENT name + a `fields:`
// remap (payload -> body). The remap rewrites the trait's @entity.payload read
// and the contract requirement so validation passes against `body`.
const CONSUMER_REMAP: &str = r#"{
  "name": "test-widget-consumer-remap",
  "version": "1.0.0",
  "orbitals": [{
    "name": "ConsumerRemapOrbital",
    "uses": [ { "from": "./base.orb", "as": "Base" } ],
    "entity": {
      "name": "RemapRecord",
      "persistence": "persistent",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "body", "type": "string" }
      ]
    },
    "traits": [
      { "ref": "Base.traits.WidgetReview", "name": "RemapReview", "linkedEntity": "RemapRecord", "fields": { "payload": "body" } }
    ]
  }]
}"#;

fn write_chain(dir: &Path) {
    std::fs::write(dir.join("base.orb"), BASE).expect("write base");
    std::fs::write(dir.join("variation.orb"), VARIATION).expect("write variation");
}

fn inline(path: &Path) -> OrbitalSchema {
    let mut schema = parse_file(path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(path)).expect("inline");
    schema
}

fn primary_field_names(schema: &OrbitalSchema) -> Vec<String> {
    match &schema.orbitals[0].entity {
        EntityRef::Inline(e) => e.fields.iter().filter_map(|f| f.name.clone()).collect(),
        _ => panic!("expected inline entity"),
    }
}

/// (a) Intrinsic `viewState` carried through the 3-level chain onto DomainRecord.
#[test]
fn intrinsic_field_carried_through_three_level_rebind() {
    let dir = tempfile::TempDir::new().expect("temp");
    write_chain(dir.path());
    let consumer = dir.path().join("consumer.orb");
    std::fs::write(&consumer, CONSUMER_OK).expect("write consumer");

    let schema = inline(&consumer);
    let fields = primary_field_names(&schema);
    assert!(
        fields.contains(&"viewState".to_string()),
        "intrinsic `viewState` must travel base→variation→consumer onto DomainRecord, got {fields:?}"
    );
    assert!(
        !has_rebind_missing_fields(&schema),
        "a satisfied 3-level rebind must not raise ORB_T_REBIND_MISSING_FIELDS"
    );
}

/// (b) DATA field `payload` missing on the target with no remap → rejected.
#[test]
fn missing_data_field_with_no_remap_is_rejected() {
    let dir = tempfile::TempDir::new().expect("temp");
    write_chain(dir.path());
    let consumer = dir.path().join("consumer_missing.orb");
    std::fs::write(&consumer, CONSUMER_MISSING_DATA).expect("write");

    let schema = inline(&consumer);
    // The required DATA `payload` must NOT have been auto-merged onto BareRecord.
    let fields = primary_field_names(&schema);
    assert!(
        !fields.contains(&"payload".to_string()),
        "a required DATA field must not be silently merged onto the target, got {fields:?}"
    );
    assert!(
        has_rebind_missing_fields(&schema),
        "a DATA field absent on the target with no remap must raise ORB_T_REBIND_MISSING_FIELDS"
    );
}

/// (c) `fields:` remap (payload -> body) satisfies the renamed DATA field.
#[test]
fn fields_remap_satisfies_renamed_data_field() {
    let dir = tempfile::TempDir::new().expect("temp");
    write_chain(dir.path());
    let consumer = dir.path().join("consumer_remap.orb");
    std::fs::write(&consumer, CONSUMER_REMAP).expect("write");

    let schema = inline(&consumer);

    // The trait's @entity.payload read must be rewritten to @entity.body.
    let body_referenced = match &schema.orbitals[0].traits[0] {
        orbital_core::schema::TraitReference::Inline(td) => {
            let s = serde_json::to_string(&td.state_machine).unwrap();
            s.contains("@entity.body") && !s.contains("@entity.payload")
        }
        _ => false,
    };
    assert!(
        body_referenced,
        "`fields` remap must rewrite @entity.payload reads to @entity.body"
    );

    assert!(
        !has_rebind_missing_fields(&schema),
        "a `fields`-remapped DATA field (payload→body) must satisfy the rebind"
    );
}
