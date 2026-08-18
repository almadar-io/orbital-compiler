//! Inline-phase test for increment 3-I's `(e : Entity)` type parameter
//! (`docs/Almadar_LOLO.md` ~line 315/1387).
//!
//! `orbital-lolo` lowers a payload field referencing an event alias's own
//! unbound type parameter to the sentinel string `"$e"` (see
//! `orbital-lolo/tests/type_param_sentinel.rs`). The inline phase must
//! substitute that sentinel with the carrying trait's FINAL `linkedEntity`'s
//! field shape — or, when the trait has no linked entity to substitute,
//! leave it in place for the defensive sweep
//! (`ORB_T_UNRESOLVED_TYPE_PARAM`) to catch at validation time.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::validation::{generic_types, type_param_sweep, ValidationContext};
use orbital_core::schema::{EntityRef, OrbitalSchema, TraitReference};
use serde_json::json;

fn schema_with_trait(linked_entity: Option<&str>) -> OrbitalSchema {
    let trait_json = json!({
        "name": "ThingSave",
        "linkedEntity": linked_entity,
        "emits": [{
            "event": "RowSaved",
            "payloadSchema": [{ "name": "row", "type": "$e" }]
        }],
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }],
            "events": [{
                "key": "RowSaved",
                "payloadSchema": [{ "name": "row", "type": "$e" }]
            }]
        }
    });
    let schema_json = json!({
        "name": "App",
        "orbitals": [{
            "name": "ThingOrbital",
            "entity": {
                "name": "Thing",
                "persistence": "runtime",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "a", "type": "string" },
                    { "name": "b", "type": "number" }
                ]
            },
            "traits": [trait_json],
            "pages": []
        }]
    });
    serde_json::from_value(schema_json).expect("schema fixture must deserialize")
}

fn thing_trait(schema: &OrbitalSchema) -> &orbital_core::schema::TraitDefinition {
    let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait definition");
    };
    trait_def
}

#[test]
fn linked_entity_resolves_sentinel_to_object_with_properties() {
    let mut schema = schema_with_trait(Some("Thing"));
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");

    let trait_def = thing_trait(&schema);

    // Entity is still an inline definition post-inline (no imports here).
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("expected an inline entity definition");
    };
    assert_eq!(entity.name, "Thing");

    let emit_payload = trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .expect("emit payloadSchema must be populated");
    assert_eq!(emit_payload.len(), 1);
    assert_eq!(emit_payload[0].name, "row");
    assert_eq!(emit_payload[0].field_type, "object");
    let props = emit_payload[0]
        .properties
        .as_ref()
        .expect("substituted sentinel must carry the entity's fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id", "a", "b"]);
    assert_eq!(props[1].field_type, "string");
    assert_eq!(props[2].field_type, "number");

    // stateMachine.events[] carries the same substitution.
    let event_payload = trait_def.state_machine.events.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .expect("event payloadSchema must be populated");
    assert_eq!(event_payload[0].field_type, "object");
    assert!(event_payload[0].properties.is_some());

    // Gate C / sweep: a fully-substituted schema reports neither the
    // generic-type warning nor the unresolved-type-param error.
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

#[test]
fn no_linked_entity_leaves_sentinel_and_sweep_flags_it() {
    let mut schema = schema_with_trait(None);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");

    let trait_def = thing_trait(&schema);
    let emit_payload = trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .unwrap();
    assert_eq!(
        emit_payload[0].field_type, "$e",
        "no linkedEntity to substitute against — sentinel must survive inline"
    );
    assert!(emit_payload[0].properties.is_none());

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert_eq!(ctx.result.errors.len(), 2, "one per surviving sentinel occurrence (emits + events): {:?}", ctx.result.errors);
    for err in &ctx.result.errors {
        assert_eq!(err.code.as_str(), "ORB_T_UNRESOLVED_TYPE_PARAM");
    }

    // Gate C interaction (item 7): the surviving "$e" sentinel itself must
    // never trip the shapeless-`object`/`json`/`any` deprecation warning —
    // "$e" matches none of those base-type strings.
    let mut gate_c_ctx = ValidationContext::new();
    generic_types::validate_generic_types(&schema, &mut gate_c_ctx);
    assert!(
        gate_c_ctx.result.errors.is_empty(),
        "the unresolved sentinel must not also trip Gate C: {:?}",
        gate_c_ctx.result.errors
    );
}

// ---------------------------------------------------------------------------
// Syntax revision 2026-08-18: the `@entity` bound-row self-reference is the
// param-less sibling of the Entity-kind default — substituted identically at
// inline, swept identically when unresolvable.
// ---------------------------------------------------------------------------

fn schema_with_at_entity(linked_entity: Option<&str>) -> OrbitalSchema {
    let mut schema = schema_with_trait(linked_entity);
    let TraitReference::Inline(trait_def) = &mut schema.orbitals[0].traits[0] else {
        panic!("inline trait expected");
    };
    for emit in trait_def.emits.as_mut().unwrap() {
        for f in emit.payload_schema.as_mut().unwrap() {
            f.field_type = "@entity".to_string();
        }
    }
    for ev in trait_def.state_machine.events.as_mut().unwrap() {
        for f in ev.payload_schema.as_mut().unwrap() {
            f.field_type = "@entity".to_string();
        }
    }
    schema
}

#[test]
fn at_entity_self_reference_resolves_like_the_entity_default() {
    let mut schema = schema_with_at_entity(Some("Thing"));
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    let trait_def = thing_trait(&schema);
    let row = &trait_def.emits.as_ref().unwrap()[0].payload_schema.as_ref().unwrap()[0];
    assert_eq!(row.field_type, "object");
    let props = row.properties.as_ref().expect("entity fields substituted");
    assert_eq!(props.len(), 3);
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(ctx.result.errors.is_empty(), "{:?}", ctx.result.errors);
    let mut ctx = ValidationContext::new();
    generic_types::validate_generic_types(&schema, &mut ctx);
    assert!(
        !ctx.result.warnings.iter().any(|w| w.path.contains("RowSaved") && w.message.contains("no declared properties")),
        "a substituted @entity row must not be a shapeless-object warning: {:?}",
        ctx.result.warnings
    );
}

#[test]
fn at_entity_without_linked_entity_is_swept() {
    let mut schema = schema_with_at_entity(None);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.iter().any(|e| e.code.as_str() == "ORB_T_UNRESOLVED_TYPE_PARAM"),
        "an unresolvable @entity must be swept: {:?}",
        ctx.result.errors
    );
}
