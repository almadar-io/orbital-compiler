//! Integration tests for the `@trait.X` binding validator.
//!
//! Exercises the validation pass added for Stage 1.2 of the `@trait.X`
//! binding plan (see `docs/Almadar_Std_Gaps.md` §3.8). The binding is
//! client-only (resolved by `<TraitFrame>` in the interpreted runtime,
//! emitted as a direct view-component reference in the compiled path),
//! so the compiler's job is strictly detection of invalid usage:
//!
//! - Unknown trait reference → `ORB_BINDING_TRAIT_UNKNOWN`
//! - Self-reference            → `ORB_BINDING_TRAIT_SELF_REFERENCE`
//! - Bare `@trait`             → `ORB_BINDING_TRAIT_MISSING_NAME`
//! - Extra segment (`@trait.X.main`) → `ORB_BINDING_TRAIT_INVALID_FORMAT`
//! - Non-PascalCase trait name → `ORB_BINDING_TRAIT_INVALID_FORMAT`
//! - Cycle in the reference graph → `ORB_BINDING_TRAIT_CYCLE`
//!
//! The binding is intentionally single-segment — the `.slot` suffix was
//! dropped to keep the compiled-path semantic aligned without resorting
//! to page-level hook hoisting / slot-specific pattern extraction.

use orbital_compiler::phases::validation::binding::{
    detect_trait_binding_cycles, validate_trait_binding_usage,
};
use orbital_compiler::phases::validation::ErrorCode;
use orbital_core_base::schema::{
    SExpression, StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
    TransitionDefinition,
};
use std::collections::HashSet;

// ============================================================================
// Helpers
// ============================================================================

/// Build a trait whose render-ui effect's pattern tree has a single
/// `children` array containing the provided binding string.
fn trait_with_trait_binding_in_children(trait_name: &str, binding: &str) -> TraitDefinition {
    let mut pattern_obj = serde_json::Map::new();
    pattern_obj.insert(
        "type".to_string(),
        serde_json::Value::String("stack".to_string()),
    );
    pattern_obj.insert(
        "children".to_string(),
        serde_json::Value::Array(vec![serde_json::Value::String(binding.to_string())]),
    );

    let render_ui = SExpression::Array(vec![
        SExpression::String("render-ui".to_string()),
        SExpression::String("main".to_string()),
        // Pattern config — reuse the same value shape the lowerer
        // produces (strings in `children` survive as SExpression::String).
        object_to_sexpr(serde_json::Value::Object(pattern_obj)),
    ]);

    TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: trait_name.to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        local: false,
        linked_entity: Some("Item".to_string()),
        description: None,
        state_machine: StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
                holds: None,
            }],
            events: Some(vec![]),
            transitions: vec![TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(vec![render_ui]),
                ensures: None,
            }],
        },
        emits: None,
        listens: None,
        ticks: None,
        config: None,
        data_entities: None,
        scope: None,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    }
}

fn object_to_sexpr(v: serde_json::Value) -> SExpression {
    match v {
        serde_json::Value::Null => SExpression::Null,
        serde_json::Value::Bool(b) => SExpression::Bool(b),
        serde_json::Value::Number(n) => SExpression::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => SExpression::String(s),
        serde_json::Value::Array(items) => {
            SExpression::Array(items.into_iter().map(object_to_sexpr).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = orbital_core_base::schema::SObject::new();
            for (k, v) in map {
                out.insert(k, object_to_sexpr(v));
            }
            SExpression::Object(out)
        }
    }
}

fn known_traits(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

// ============================================================================
// Tests — single-binding validator
// ============================================================================

#[test]
fn valid_trait_binding_emits_no_error() {
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.ItemBrowse");
    let known = known_traits(&["ItemBrowse", "ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result.errors.is_empty(),
        "expected no errors for valid binding, got: {:?}",
        result.errors
    );
    assert!(
        result.warnings.is_empty(),
        "expected no warnings: {:?}",
        result.warnings
    );
}

#[test]
fn unknown_trait_binding_emits_error() {
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.DoesNotExist");
    let known = known_traits(&["ItemBrowse", "ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitUnknown),
        "expected BindingTraitUnknown, got: {:?}",
        result.errors
    );
}

#[test]
fn self_reference_emits_error() {
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.ItemLayout");
    let known = known_traits(&["ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitSelfReference),
        "expected BindingTraitSelfReference, got: {:?}",
        result.errors
    );
}

#[test]
fn bare_at_trait_emits_missing_name() {
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait");
    let known = known_traits(&["ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    // Bare `@trait` has length 6 so the collector's `s.len() >= 6`
    // guard fires. Validator must flag it.
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitMissingName),
        "expected BindingTraitMissingName, got: {:?}",
        result.errors
    );
}

#[test]
fn extra_segment_emits_invalid_format() {
    // `@trait.X.main` was the former cross-slot pull syntax. Rejected
    // outright now to keep interpreted + compiled paths aligned.
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.ItemBrowse.main");
    let known = known_traits(&["ItemBrowse", "ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitInvalidFormat),
        "expected BindingTraitInvalidFormat for `.slot` suffix, got: {:?}",
        result.errors
    );
}

#[test]
fn non_pascal_trait_name_emits_invalid_format() {
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.itemBrowse");
    let known = known_traits(&["ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitInvalidFormat),
        "expected BindingTraitInvalidFormat for non-PascalCase, got: {:?}",
        result.errors
    );
}

// ============================================================================
// Tests — cycle detection
// ============================================================================

#[test]
fn two_way_cycle_is_detected() {
    // A → B → A. Cycle detector must flag one (and only one) error.
    let a = trait_with_trait_binding_in_children("A", "@trait.B");
    let b = trait_with_trait_binding_in_children("B", "@trait.A");
    let traits = vec![TraitReference::Inline(a), TraitReference::Inline(b)];
    let result = detect_trait_binding_cycles(&traits, "orbitals[0]");
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitCycle),
        "expected BindingTraitCycle, got: {:?}",
        result.errors
    );
}

#[test]
fn acyclic_reference_graph_produces_no_cycle_error() {
    // A → B, B → nothing. No cycle.
    let a = trait_with_trait_binding_in_children("A", "@trait.B");
    let b = trait_with_trait_binding_in_children("B", "some-non-binding");
    let traits = vec![TraitReference::Inline(a), TraitReference::Inline(b)];
    let result = detect_trait_binding_cycles(&traits, "orbitals[0]");
    assert!(
        result.errors.is_empty(),
        "expected no cycle error, got: {:?}",
        result.errors
    );
}

#[test]
fn three_way_cycle_is_detected() {
    // A → B → C → A.
    let a = trait_with_trait_binding_in_children("A", "@trait.B");
    let b = trait_with_trait_binding_in_children("B", "@trait.C");
    let c = trait_with_trait_binding_in_children("C", "@trait.A");
    let traits = vec![
        TraitReference::Inline(a),
        TraitReference::Inline(b),
        TraitReference::Inline(c),
    ];
    let result = detect_trait_binding_cycles(&traits, "orbitals[0]");
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitCycle),
        "expected BindingTraitCycle, got: {:?}",
        result.errors
    );
}

// ============================================================================
// Tests — render-only position restriction (ORB_BINDING_TRAIT_OUTSIDE_RENDER_UI)
// ============================================================================

/// Build a trait whose transition carries a `@trait.X` binding in a non-
/// render position: `effect_kind` selects where ("guard" | "fetch" | "set").
fn trait_with_binding_outside_render(effect_kind: &str, binding: &str) -> TraitDefinition {
    let binding_sexpr = SExpression::String(binding.to_string());
    let (guard, effects) = match effect_kind {
        "guard" => (
            Some(binding_sexpr.clone()),
            Some(vec![SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("Item".to_string()),
            ])]),
        ),
        // A `fetch` effect with the @trait binding buried in its options map.
        "fetch" => (
            None,
            Some(vec![SExpression::Array(vec![
                SExpression::String("fetch".to_string()),
                SExpression::String("Item".to_string()),
                SExpression::Object({
                    let mut m = orbital_core_base::schema::SObject::new();
                    m.insert("emit".to_string(), binding_sexpr.clone());
                    m
                }),
            ])]),
        ),
        // A `set` effect targeting @trait.X as the field path — nonsense.
        "set" => (
            None,
            Some(vec![SExpression::Array(vec![
                SExpression::String("set".to_string()),
                binding_sexpr.clone(),
                SExpression::String("?payload.x".to_string()),
            ])]),
        ),
        _ => panic!("unknown effect_kind: {effect_kind}"),
    };
    TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "BadTrait".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        local: false,
        linked_entity: Some("Item".to_string()),
        description: None,
        state_machine: StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
                holds: None,
            }],
            events: Some(vec![]),
            transitions: vec![TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard,
                effects,
                ensures: None,
            }],
        },
        emits: None,
        listens: None,
        ticks: None,
        config: None,
        data_entities: None,
        scope: None,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    }
}

#[test]
fn trait_binding_in_guard_is_rejected() {
    let trait_def = trait_with_binding_outside_render("guard", "@trait.ItemBrowse");
    let known = known_traits(&["ItemBrowse"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitOutsideRenderUi),
        "expected BindingTraitOutsideRenderUi for @trait in a guard, got: {:?}",
        result.errors
    );
}

#[test]
fn trait_binding_in_fetch_effect_is_rejected() {
    let trait_def = trait_with_binding_outside_render("fetch", "@trait.ItemBrowse");
    let known = known_traits(&["ItemBrowse"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitOutsideRenderUi),
        "expected BindingTraitOutsideRenderUi for @trait in a fetch effect, got: {:?}",
        result.errors
    );
}

#[test]
fn trait_binding_in_set_effect_is_rejected() {
    let trait_def = trait_with_binding_outside_render("set", "@trait.ItemBrowse");
    let known = known_traits(&["ItemBrowse"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitOutsideRenderUi),
        "expected BindingTraitOutsideRenderUi for @trait in a set effect, got: {:?}",
        result.errors
    );
}

#[test]
fn trait_binding_in_render_ui_is_accepted() {
    // Sanity: the same binding that's rejected in guards/fetch/set is fine
    // when it lives in a render-ui children array.
    let trait_def = trait_with_trait_binding_in_children("ItemLayout", "@trait.ItemBrowse");
    let known = known_traits(&["ItemBrowse", "ItemLayout"]);
    let result = validate_trait_binding_usage(&trait_def, "orbitals[0].traits[0]", &known);
    assert!(
        result.errors.is_empty(),
        "expected no errors for @trait in render-ui, got: {:?}",
        result.errors
    );
}
