//! Gate A of the type-integrity campaign: `node`-typed entity fields get the
//! full render-ui check family over their `set` literals — closing the §19
//! blind spot (`C-RUI-CALLBACK-GATE-JSON-BLIND` in `Almadar_Compiler_Gaps.md`)
//! where a canvas inside an entity-held view fragment escaped
//! `ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH` entirely.
//!
//! Covered:
//! - a literal pattern tree with an invalid pattern type → blocking error
//! - the §19 case: a canvas callback prop inside a node-field literal whose
//!   listen contract is flat (missing the wrapped `payload` field) → blocking
//! - the classify idiom: a `let`-bound literal reaches the same checks
//! - a computed value into a node field → nothing constructed (no
//!   statically-checkable consequence; `ORB_RUI_NODE_FIELD_UNCHECKED_CONTENT`
//!   was removed — a no-consequence finding, never blocking)
//! - a pattern literal into a NON-node field → blocking
//!   `ORB_RUI_RENDERABLE_IN_GENERIC_FIELD` (anti-loophole)

use orbital_compiler::phases::validation::{render_ui, ValidationContext};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn run(schema: Value) -> (Vec<String>, Vec<String>) {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    let mut ctx = ValidationContext::new();
    for orbital in &schema.orbitals {
        if let orbital_core_base::schema::EntityRef::Inline(e) = &orbital.entity {
            ctx.entity_definitions.insert(e.name.clone(), e.clone());
        }
    }
    render_ui::validate_all_render_ui(&schema, &mut ctx);
    (
        ctx.result
            .errors
            .iter()
            .map(|e| e.code.as_str().to_string())
            .collect(),
        ctx.result
            .warnings
            .iter()
            .map(|e| e.code.as_str().to_string())
            .collect(),
    )
}

/// One orbital, a `NodeScene` entity with a `[node]` `view` field and a
/// shapeless-free `data` field, one trait bound to it whose INIT transition
/// carries the provided effects.
fn schema_with_effects(view_field: Value, effects: Value) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "NodeOrbital",
            "entity": {
                "name": "NodeScene",
                "persistence": "runtime",
                "fields": [
                    { "name": "id", "type": "string" },
                    view_field,
                    { "name": "rows", "type": "array", "items": { "type": "object",
                        "properties": { "label": { "type": "string" } } } }
                ]
            },
            "traits": [{
                "name": "NodeSim",
                "linkedEntity": "NodeScene",
                "stateMachine": {
                    "initial": "idle",
                    "states": [{ "name": "idle" }],
                    "transitions": [{
                        "from": "idle", "to": "idle", "event": "INIT",
                        "effects": effects
                    }]
                }
            }],
            "pages": []
        }]
    })
}

fn node_array_field() -> Value {
    json!({ "name": "view", "type": "array", "items": { "type": "node" } })
}

fn node_scalar_field() -> Value {
    json!({ "name": "view", "type": "node" })
}

#[test]
fn valid_literal_into_node_field_passes() {
    let (errors, warnings) = run(schema_with_effects(
        node_scalar_field(),
        json!([["set", "@entity.view",
            { "type": "typography", "variant": "body", "content": "hi" }]]),
    ));
    assert!(errors.is_empty(), "expected clean, got {errors:?}");
    assert!(
        !warnings.iter().any(|w| w.contains("NODE_FIELD")),
        "expected no node warnings, got {warnings:?}"
    );
}

#[test]
fn invalid_pattern_type_in_node_literal_is_an_error() {
    let (errors, _) = run(schema_with_effects(
        node_scalar_field(),
        json!([["set", "@entity.view",
            { "type": "definitely-not-a-pattern", "content": "x" }]]),
    ));
    assert!(
        !errors.is_empty(),
        "an invalid pattern type inside a node literal must be a blocking error"
    );
}

#[test]
fn flat_callback_contract_in_node_literal_is_caught_the_hotspot_bug() {
    // The §19 reproduction: learning-canvas's onShapeClick wraps its args as
    // `{ payload }`; a trait that declares TAP with flat fields must be
    // rejected even when the canvas lives in an entity-held view fragment.
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "NodeOrbital",
            "entity": {
                "name": "NodeScene",
                "persistence": "runtime",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "view", "type": "array", "items": { "type": "node" } }
                ]
            },
            "traits": [{
                "name": "NodeSim",
                "linkedEntity": "NodeScene",
                "emits": [{
                    "event": "TAP",
                    "payloadSchema": [
                        { "name": "id", "type": "string" },
                        { "name": "index", "type": "number" }
                    ]
                }],
                "stateMachine": {
                    "initial": "idle",
                    "states": [{ "name": "idle" }],
                    "transitions": [{
                        "from": "idle", "to": "idle", "event": "INIT",
                        "effects": [["set", "@entity.view", [
                            { "type": "learning-canvas", "width": 600,
                              "shapes": [], "onShapeClick": "TAP" }
                        ]]]
                    }]
                }
            }],
            "pages": []
        }]
    });
    let (errors, warnings) = run(schema);
    let all: Vec<&String> = errors.iter().chain(warnings.iter()).collect();
    assert!(
        all.iter().any(|c| c.contains("EVENT_BINDING_PAYLOAD_MISMATCH")),
        "the flat TAP contract must trip the payload-mismatch gate inside the node literal; got errors {errors:?} warnings {warnings:?}"
    );
}

#[test]
fn let_bound_literal_is_resolved_and_checked() {
    let (errors, _) = run(schema_with_effects(
        node_array_field(),
        json!([["let", [["itemView", [
            { "type": "definitely-not-a-pattern", "content": "x" }
        ]]],
            ["do", ["set", "@entity.view", "@itemView"]]
        ]]),
    ));
    assert!(
        !errors.is_empty(),
        "a let-bound literal must reach the same pattern checks (classify idiom)"
    );
}

#[test]
fn computed_value_into_node_field_is_silent() {
    // A computed value has no statically-checkable consequence (its shape
    // isn't known here) — nothing is constructed for it at all.
    let (errors, warnings) = run(schema_with_effects(
        node_array_field(),
        json!([["set", "@entity.view", ["array/concat", ["@a"], ["@b"]]]]),
    ));
    assert!(
        errors.is_empty(),
        "computed node content must NOT block, got {errors:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.contains("NODE_FIELD")),
        "expected no finding for computed node content, got {warnings:?}"
    );
}

#[test]
fn pattern_literal_into_non_node_field_is_the_anti_loophole() {
    let (errors, _warnings) = run(schema_with_effects(
        node_scalar_field(),
        json!([["set", "@entity.rows", [
            { "type": "card", "variant": "bordered", "children": [] }
        ]]]),
    ));
    assert!(
        errors
            .iter()
            .any(|e| e == "ORB_RUI_RENDERABLE_IN_GENERIC_FIELD"),
        "expected the anti-loophole error, got {errors:?}"
    );
}

/// `Map string [node]` — an art catalog keyed by kind. The map is renderable
/// content addressed by key, so it must NOT trip the anti-loophole arm, and its
/// leaves must still reach the pattern checks.
#[test]
fn map_of_node_is_renderable_and_its_leaves_are_checked() {
    let map_field = json!({ "name": "view", "type": "object",
                "items": { "type": "array", "items": { "type": "node" } } });

    let (errors, _warnings) = run(schema_with_effects(
        map_field.clone(),
        json!([["set", "@entity.view", {
            "knight": [{ "type": "typography", "variant": "body", "content": "hi" }]
        }]]),
    ));
    assert!(
        errors.is_empty(),
        "a Map-of-node field is node-typed, not a generic loophole, got {errors:?}"
    );

    // The leaves are really checked — a bogus pattern inside a map value errors
    // exactly as it would inside a plain `[node]`.
    let (bad, _) = run(schema_with_effects(
        map_field,
        json!([["set", "@entity.view", {
            "knight": [{ "type": "definitely-not-a-pattern", "content": "x" }]
        }]]),
    ));
    assert!(
        !bad.is_empty(),
        "an invalid pattern inside a map value must still error"
    );
}

#[test]
fn canvas_shape_data_does_not_false_positive() {
    // Canvas shapes share the `grid` name with a real pattern; the detector's
    // second condition (a registered prop of that pattern) must keep shape
    // data out of the anti-loophole arm.
    let (errors, _warnings) = run(schema_with_effects(
        node_scalar_field(),
        json!([["set", "@entity.rows", [
            { "type": "circle", "x": 10, "y": 20, "radius": 5 },
            { "type": "grid", "xStep": 1, "yStep": 1 }
        ]]]),
    ));
    assert!(
        errors.is_empty(),
        "canvas shape data must not trip the anti-loophole arm, got {errors:?}"
    );
}

// ============================================================================
// Gate B: structural callback-payload matching (typed `payload`, no `object`)
// ============================================================================

fn schema_with_tap_payload(payload_field: Value) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "NodeOrbital",
            "entity": {
                "name": "NodeScene",
                "persistence": "runtime",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "view", "type": "array", "items": { "type": "node" } }
                ]
            },
            "traits": [{
                "name": "NodeSim",
                "linkedEntity": "NodeScene",
                "emits": [{ "event": "TAP", "payloadSchema": [payload_field] }],
                "stateMachine": {
                    "initial": "idle",
                    "states": [{ "name": "idle" }],
                    "transitions": [{
                        "from": "idle", "to": "idle", "event": "INIT",
                        "effects": [["set", "@entity.view", [
                            { "type": "learning-canvas", "width": 600,
                              "shapes": [], "onShapeClick": "TAP" }
                        ]]]
                    }]
                }
            }],
            "pages": []
        }]
    })
}

#[test]
fn shapeless_payload_object_is_rejected_with_a_derived_hint() {
    let (errors, _) = run(schema_with_tap_payload(
        json!({ "name": "payload", "type": "object" }),
    ));
    assert!(
        errors
            .iter()
            .any(|c| c == "ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH"),
        "a shapeless `payload : object` against a schema-carrying callback must block; got {errors:?}"
    );
}

#[test]
fn structurally_typed_payload_passes() {
    let (errors, _) = run(schema_with_tap_payload(json!({
        "name": "payload", "type": "object",
        "properties": [
            { "name": "id", "type": "string" },
            { "name": "type", "type": "string" },
            { "name": "index", "type": "number" }
        ]
    })));
    assert!(
        !errors
            .iter()
            .any(|c| c == "ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH"),
        "a structurally matching payload shape must pass; got {errors:?}"
    );
}

#[test]
fn mistyped_nested_payload_field_is_rejected() {
    let (errors, _) = run(schema_with_tap_payload(json!({
        "name": "payload", "type": "object",
        "properties": [
            { "name": "index", "type": "string" }
        ]
    })));
    assert!(
        errors
            .iter()
            .any(|c| c == "ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH"),
        "a mistyped nested field ('index' as string) must block; got {errors:?}"
    );
}
