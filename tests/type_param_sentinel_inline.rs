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
    assert_eq!(
        ctx.result.errors.len(),
        2,
        "one per surviving sentinel occurrence (emits + events): {:?}",
        ctx.result.errors
    );
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
    let row = &trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .unwrap()[0];
    assert_eq!(row.field_type, "object");
    let props = row.properties.as_ref().expect("entity fields substituted");
    assert_eq!(props.len(), 3);
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(ctx.result.errors.is_empty(), "{:?}", ctx.result.errors);
    let mut ctx = ValidationContext::new();
    generic_types::validate_generic_types(&schema, &mut ctx);
    assert!(
        !ctx.result
            .warnings
            .iter()
            .any(|w| w.path.contains("RowSaved") && w.message.contains("no declared properties")),
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
        ctx.result
            .errors
            .iter()
            .any(|e| e.code.as_str() == "ORB_T_UNRESOLVED_TYPE_PARAM"),
        "an unresolvable @entity must be swept: {:?}",
        ctx.result.errors
    );
}

// ---------------------------------------------------------------------------
// Sigil type constructors (2026-09-01). Each arm gets a resolution test AND an
// unresolvable test: a check that can only pass proves nothing, and these three
// constructors ship with little or no corpus usage, so their correctness rests
// entirely here.
// ---------------------------------------------------------------------------

fn schema_with_sigils(action_default: Option<&str>, identity_shape: bool) -> OrbitalSchema {
    let mut config = json!({
        "actionPayload": { "type": "@payload(action)" },
        "targetEntity":  { "type": "entity", "default": "Thing" },
        "targetRow":     { "type": "@entity(targetEntity)" },
        "boundRow":      { "type": "@entity" },
        "actor":         { "type": "@user" }
    });
    if let Some(d) = action_default {
        config["action"] = json!({ "type": "event", "default": d });
    }
    let trait_json = json!({
        "name": "ThingSave",
        "linkedEntity": "Thing",
        "config": config,
        "emits": [{
            "event": "PAY_BILL",
            "payloadSchema": [{ "name": "id", "type": "string", "required": true }]
        }],
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
        }
    });
    let mut orbital = json!({
        "name": "ThingOrbital",
        "entity": {
            "name": "Thing",
            "persistence": "runtime",
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "a", "type": "string" }
            ]
        },
        "traits": [trait_json],
        "pages": []
    });
    if identity_shape {
        orbital["expects"] = json!([{
            "kind": "identity",
            "name": "Customer",
            "shape": [
                { "name": "email", "type": "string" },
                { "name": "role", "type": "string" }
            ]
        }]);
    }
    serde_json::from_value(json!({ "name": "App", "orbitals": [orbital] }))
        .expect("schema fixture must deserialize")
}

fn knob_type(schema: &OrbitalSchema, knob: &str) -> String {
    thing_trait(schema).config.as_ref().unwrap()[knob]
        .field_type
        .clone()
}

#[test]
fn sigil_types_resolve_against_the_knob_values() {
    let mut schema = schema_with_sigils(Some("PAY_BILL"), true);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");

    // Never `object`: a ConfigField typed literally "object" is an
    // unconditional ORB_T_CONFIG_OBJECT_ARRAY_FORBIDDEN regardless of props.
    for knob in ["actionPayload", "targetRow", "boundRow", "actor"] {
        let t = knob_type(&schema, knob);
        assert_ne!(t, "object", "{knob} must not resolve to bare object");
        assert!(!t.starts_with('@'), "{knob} left a sentinel: {t}");
    }
    assert_eq!(knob_type(&schema, "actionPayload"), "PayBillPayload");
    assert_eq!(knob_type(&schema, "targetRow"), "ThingRow");
    // Bare `@entity` in CONFIG position — the arity-0 arm, resolved against
    // the trait's own linkedEntity. This is what types the row-shaped knobs
    // (`loadedRow`, `row`, `currentRow`, `focusedRow`) that `json` was
    // standing in for.
    assert_eq!(knob_type(&schema, "boundRow"), "ThingRow");
    assert_eq!(knob_type(&schema, "actor"), "IdentityRow");

    let cfg = thing_trait(&schema).config.as_ref().unwrap();
    assert!(cfg["actionPayload"]
        .properties
        .as_ref()
        .unwrap()
        .contains_key("id"));
    assert!(cfg["targetRow"]
        .properties
        .as_ref()
        .unwrap()
        .contains_key("a"));
    assert!(cfg["actor"]
        .properties
        .as_ref()
        .unwrap()
        .contains_key("email"));

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(ctx.result.errors.is_empty(), "{:?}", ctx.result.errors);
}

#[test]
fn an_event_nothing_declares_resolves_to_an_empty_shape() {
    // "The target declares nothing" is an ANSWER, not a failure — it is what
    // lets a generated atom validate standalone, where the knob holds its own
    // default and no listener has declared that event.
    let mut schema = schema_with_sigils(Some("NOBODY_DECLARES_ME"), true);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    assert_eq!(
        knob_type(&schema, "actionPayload"),
        "NobodyDeclaresMePayload"
    );
    let props = thing_trait(&schema).config.as_ref().unwrap()["actionPayload"]
        .properties
        .as_ref()
        .expect("empty shape is still a shape");
    assert!(props.is_empty());

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "empty is not unresolved: {:?}",
        ctx.result.errors
    );
}

#[test]
fn a_payload_knob_with_no_event_knob_is_swept() {
    // The 15.2% corpus case: a payload knob on a trait with no `action` knob at
    // all — inert by construction, and a real defect rather than a type gap.
    let mut schema = schema_with_sigils(None, true);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result
            .errors
            .iter()
            .any(|e| e.path.contains("actionPayload")
                && e.code.as_str() == "ORB_T_UNRESOLVED_TYPE_PARAM"),
        "unresolvable @payload must be swept, not silently shipped: {:?}",
        ctx.result.errors
    );
}

#[test]
fn bare_at_entity_in_config_without_a_linked_entity_is_swept() {
    // The arity-0 arm's unresolvable half: no `linkedEntity`, so there is no
    // bound row to read. Must keep the sentinel and be swept, never degrade to
    // `object` — a `ConfigField` typed literally "object" is an unconditional
    // ORB_T_CONFIG_OBJECT_ARRAY_FORBIDDEN, so a degrade would trade a silent
    // wrong type for a confusing hard error at an unrelated gate.
    let mut schema = schema_with_sigils(Some("PAY_BILL"), true);
    {
        let TraitReference::Inline(td) = &mut schema.orbitals[0].traits[0] else {
            panic!("fixture trait must be inline");
        };
        td.linked_entity = None;
    }
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");

    assert_eq!(knob_type(&schema, "boundRow"), "@entity");
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.iter().any(
            |e| e.path.contains("boundRow") && e.code.as_str() == "ORB_T_UNRESOLVED_TYPE_PARAM"
        ),
        "unresolvable bare @entity must be swept: {:?}",
        ctx.result.errors
    );
}

#[test]
fn at_entity_naming_an_undeclared_entity_is_swept() {
    // The `@entity <knob>` arm's unresolvable half. `targetEntity` names an
    // entity no orbital declares and no `expects` promises, so there is no
    // shape to read. Every arm ships BOTH halves: a resolution test alone
    // cannot distinguish "resolved" from "degraded silently", and this arm has
    // no corpus site to catch a defect for us.
    let mut schema = schema_with_sigils(Some("PAY_BILL"), true);
    {
        let TraitReference::Inline(td) = &mut schema.orbitals[0].traits[0] else {
            panic!("fixture trait must be inline");
        };
        let cfg = td.config.as_mut().expect("fixture declares config");
        cfg.get_mut("targetEntity")
            .expect("fixture declares targetEntity")
            .default = Some(orbital_core::schema::SExpression::String(
            "Ghost".to_string(),
        ));
    }
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");

    assert_eq!(
        knob_type(&schema, "targetRow"),
        "@entity(targetEntity)",
        "an unresolvable @entity must KEEP its sentinel, never degrade to object"
    );
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result
            .errors
            .iter()
            .any(|e| e.path.contains("targetRow")
                && e.code.as_str() == "ORB_T_UNRESOLVED_TYPE_PARAM"),
        "unresolvable @entity must be swept: {:?}",
        ctx.result.errors
    );
}

#[test]
fn at_user_without_a_declared_identity_shape_is_swept() {
    // A bare `expects identity` is a sanctioned escape hatch — existence only,
    // no shape — so `@user` has nothing to read. That is a loud error naming
    // the fix, not a degrade to object.
    let mut schema = schema_with_sigils(Some("PAY_BILL"), false);
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.iter().any(|e| e.path.contains("actor")),
        "unresolvable @user must be swept: {:?}",
        ctx.result.errors
    );
}

// ---------------------------------------------------------------------
// Stage C: `resolve_sentinel_fields` arm-4 stamp fix (orbital-import
// Stage C program) — one trait exercising all four sentinel-resolution
// arms in `events[].payloadSchema` AND `emits[].payloadSchema`, so the JS
// twin (`resolveTypeParamSentinels`, `packages/almadar-runtime`) can be
// asserted byte-equal against this Rust-produced fixture (the
// `derive_id.json` shared-fixture pattern — write with `UPDATE_FIXTURES=1`,
// else assert byte-identical to the committed file).
// ---------------------------------------------------------------------

/// The RAW (pre-`inline_schema`) input JSON — no ids. `inline_schema`'s
/// id-stamping pass mints fresh ULIDs (timestamp + randomness) on any
/// orbital/entity/trait/event that lacks one, which is non-deterministic
/// run-to-run — so the fixture's `"schema"` field is this raw input (what
/// the JS twin resolves FROM), never the post-inline result (ids would
/// drift on every `UPDATE_FIXTURES=1` regeneration).
fn raw_schema_json() -> serde_json::Value {
    let four_forms = json!([
        // Arm 1: `@entity` self-ref.
        { "name": "selfRow", "type": "@entity" },
        // Arm 1 (array form): `[@entity]`.
        { "name": "selfRows", "type": "[@entity]" },
        // Arm 2: a DECLARED `typeParams` entry of kind `Entity` with no
        // call-site `typeArgs` — defaults to `entity_def` (`ParamSub::Entity`).
        { "name": "typedRow", "type": "$RowType" },
        // Arm 4: a bare `$Name` sentinel with NO declared `typeParams` entry
        // of that name (the "legacy alias sentinel" arm).
        { "name": "legacyRow", "type": "$LegacyAlias" }
    ]);
    let trait_json = json!({
        "name": "ThingSave",
        "linkedEntity": "Thing",
        "typeParams": [ { "name": "RowType", "kind": "Entity" } ],
        "emits": [{
            "event": "RowSaved",
            "payloadSchema": four_forms
        }],
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }],
            "events": [{
                "key": "RowSaved",
                "payloadSchema": four_forms
            }]
        }
    });
    json!({
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
    })
}

fn schema_with_all_four_sentinel_forms() -> OrbitalSchema {
    serde_json::from_value(raw_schema_json()).expect("schema fixture must deserialize")
}

/// Shared-fixture contract (`derive_id.json` pattern, mirrors
/// `orbital.rs::assert_or_write_fixture`): write `{ schema, expected }` to
/// `tests/fixtures/orbital_import_stage_b/sentinel_payload.json` when
/// `UPDATE_FIXTURES=1`, else assert the current resolution is byte-identical
/// to the committed fixture. `schema` is the RAW pre-resolution input
/// (`raw_schema_json`) — deterministic, unlike the post-inline result.
fn assert_or_write_sentinel_fixture(expected: &serde_json::Value) {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/orbital_import_stage_b");
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join("sentinel_payload.json");
    let combined = json!({
        "schema": raw_schema_json(),
        "expected": expected,
    });
    let pretty = serde_json::to_string_pretty(&combined).expect("pretty print fixture");
    if std::env::var("UPDATE_FIXTURES").as_deref() == Ok("1") {
        std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
        return;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing fixture {path:?} — run with UPDATE_FIXTURES=1 to create it, \
             then commit the file"
        )
    });
    assert_eq!(
        existing.trim_end(),
        pretty,
        "sentinel resolution drifted from the committed fixture ({path:?}) — the JS \
         twin asserts the SAME file byte-equal; re-run with UPDATE_FIXTURES=1 only \
         after confirming the drift is intentional"
    );
}

#[test]
fn arm_four_legacy_alias_sentinel_stamps_entity_and_all_four_forms_resolve() {
    let mut schema = schema_with_all_four_sentinel_forms();
    inline_schema(&mut schema, &InlineConfig::default(), None).expect("inline must succeed");
    let trait_def = thing_trait(&schema);

    let emit_payload = trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .expect("emit payloadSchema must be populated");
    let event_payload = trait_def.state_machine.events.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .expect("event payloadSchema must be populated");

    for payload in [emit_payload, event_payload] {
        assert_eq!(payload.len(), 4);
        assert_eq!(payload[0].field_type, "object");
        assert_eq!(payload[0].entity.as_deref(), Some("Thing"), "arm 1 @entity");
        assert_eq!(payload[1].field_type, "[object]");
        assert_eq!(
            payload[1].entity.as_deref(),
            Some("Thing"),
            "arm 1 [@entity]"
        );
        assert_eq!(payload[2].field_type, "object");
        assert_eq!(
            payload[2].entity.as_deref(),
            Some("Thing"),
            "arm 2 declared Entity-kind $param"
        );
        assert_eq!(payload[3].field_type, "object");
        assert_eq!(
            payload[3].entity.as_deref(),
            Some("Thing"),
            "arm 4 legacy alias sentinel — the asymmetry this program fixed"
        );
    }

    // Gate C / sweep stays clean — every sentinel fully resolved.
    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "fully-resolved schema must not trip the sweep: {:?}",
        ctx.result.errors
    );

    let expected = json!({
        "emits": serde_json::to_value(emit_payload).unwrap(),
        "events": serde_json::to_value(event_payload).unwrap(),
    });
    assert_or_write_sentinel_fixture(&expected);
}
