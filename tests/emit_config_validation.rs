//! Integration tests for the `emit:` config validator.
//!
//! Stage 3 of the close-the-circuit plan adds per-operator validation
//! for the `emit:` options key:
//!
//!   - `fetch`, `persist`, `call-service` → `success`, `failure`
//!   - `ref`                              → `on_change`, `failure`
//!   - `set`                              → `success` (sync)
//!   - `os/watch-*`                       → `on_message`, `failure`
//!
//! Violations:
//!   - Unknown key for operator → `ORB_EFF_EMIT_KEY_INVALID`
//!   - Non-UPPER_SNAKE event    → `ORB_EFF_EMIT_INVALID_NAME`
//!   - Undeclared event         → `ORB_EFF_EMIT_UNDECLARED_EVENT` (warning)

use orbital_compiler::phases::validation::{validate_comprehensive, ErrorCode};
use orbital_core::schema::{
    EmitDefinition, EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType, Orbital,
    OrbitalSchema, SExpression, StateDefinition, StateMachine, StringOrArray, TraitDefinition,
    TraitReference, TransitionDefinition,
};
use std::collections::HashMap;

// ============================================================================
// Helpers
// ============================================================================

fn emit_object(entries: &[(&str, &str)]) -> SExpression {
    let mut m: HashMap<String, SExpression> = HashMap::new();
    for (k, v) in entries {
        m.insert((*k).to_string(), SExpression::String((*v).to_string()));
    }
    SExpression::Object(m)
}

fn options_with_emit(emit: SExpression) -> SExpression {
    let mut opts: HashMap<String, SExpression> = HashMap::new();
    opts.insert("emit".to_string(), emit);
    SExpression::Object(opts)
}

/// Build an inline orbital containing one trait with the given effects.
fn schema_with_effects(
    effects: Vec<SExpression>,
    declared_emits: Vec<EmitDefinition>,
) -> OrbitalSchema {
    let entity = EntityDefinition {
        shared: false,
        identity: false,
        name: "Patient".to_string(),
        collection: Some("patients".to_string()),
        persistence: None,
        fields: vec![FieldDefinition {
            name: Some("id".to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: Some(true),
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }],
        instances: None,
        id: None,
    };

    let trait_def = TraitDefinition {
        name: "PatientLoader".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        linked_entity: Some("Patient".to_string()),
        description: None,
        state_machine: StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }],
            events: Some(vec![]),
            transitions: vec![TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(effects),
            }],
        },
        emits: if declared_emits.is_empty() {
            None
        } else {
            Some(declared_emits)
        },
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
    };

    OrbitalSchema {
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            name: "TestOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(trait_def)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }],
        metadata: None,
        ledger: None,
        schema_version: None,
    }
}

fn declared_emit(event: &str, external: bool) -> EmitDefinition {
    EmitDefinition {
        event: event.to_string(),
        description: None,
        synonyms: None,
        tier: None,
        scope: Some(if external {
            EventScope::External
        } else {
            EventScope::Internal
        }),
        payload_schema: None,
        event_id: None,
    }
}

// ============================================================================
// Tests — per-operator allowed-key enforcement
// ============================================================================

#[test]
fn fetch_with_success_and_failure_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[
            ("success", "PATIENT_LOADED"),
            ("failure", "PATIENT_LOAD_FAILED"),
        ])),
    ])];
    let schema = schema_with_effects(
        effects,
        vec![
            declared_emit("PATIENT_LOADED", false),
            declared_emit("PATIENT_LOAD_FAILED", false),
        ],
    );
    let result = validate_comprehensive(&schema);
    assert!(
        !result.errors.iter().any(|e| matches!(
            e.code,
            ErrorCode::EffectEmitKeyInvalid
                | ErrorCode::EffectEmitInvalidName
                | ErrorCode::EffectEmitUndeclaredEvent
        )),
        "expected no emit errors, got: {:?}",
        result.errors
    );
}

#[test]
fn fetch_with_on_message_is_rejected() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("on_message", "STREAMING")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "expected EffectEmitKeyInvalid, got: {:?}",
        result.errors
    );
}

#[test]
fn set_with_failure_is_rejected() {
    // set is synchronous — only `success` fires. `failure` is meaningless.
    let effects = vec![SExpression::Array(vec![
        SExpression::String("set".to_string()),
        SExpression::String("@entity.status".to_string()),
        SExpression::String("done".to_string()),
        options_with_emit(emit_object(&[("failure", "WRITE_FAILED")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "expected EffectEmitKeyInvalid on set.emit.failure, got: {:?}",
        result.errors
    );
}

#[test]
fn set_with_success_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("set".to_string()),
        SExpression::String("@entity.status".to_string()),
        SExpression::String("done".to_string()),
        options_with_emit(emit_object(&[("success", "STATUS_CHANGED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("STATUS_CHANGED", false)]);
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "unexpected EffectEmitKeyInvalid: {:?}",
        result.errors
    );
}

#[test]
fn mixed_case_event_name_accepted_post_phase_2_5() {
    // Post-Phase 2.5: event names are nominal identifiers (declared via
    // `type X = Event<T>`). MixedCase is now accepted alongside UPPER_SNAKE.
    // The camelCase / MixedCase form no longer trips EffectEmitInvalidName.
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("success", "patientLoaded")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitInvalidName),
        "did not expect EffectEmitInvalidName for MixedCase identifier; got: {:?}",
        result.errors
    );
}

#[test]
fn malformed_event_identifier_still_rejected() {
    // Identifiers starting with a digit or containing hyphens remain invalid
    // — the post-Phase-2.5 check only relaxes the UPPER_SNAKE requirement,
    // not the identifier-well-formedness rule.
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("success", "1badName")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitInvalidName),
        "expected EffectEmitInvalidName for '1badName', got: {:?}",
        result.errors
    );
}

#[test]
fn undeclared_event_is_warning() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("success", "PATIENT_LOADED")])),
    ])];
    // No declared emit for PATIENT_LOADED
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);

    // The validator promotes every diagnostic to an Error severity by
    // design ("zero non-error output" — see `ValidationError::warning`).
    // `EffectEmitUndeclaredEvent` consequently lands in `errors`, not
    // `warnings`. The test name + docstring predate that design change;
    // kept for historical grep, behaviour is now "emit-undeclared is a
    // blocking error".
    let undeclared: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::EffectEmitUndeclaredEvent)
        .collect();
    assert!(
        !undeclared.is_empty(),
        "expected EffectEmitUndeclaredEvent error, got errors: {:?}, warnings: {:?}",
        result.errors,
        result.warnings
    );
}

#[test]
fn os_watch_http_on_message_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("os/watch-http".to_string()),
        SExpression::String("wss://push".to_string()),
        options_with_emit(emit_object(&[("on_message", "PUSH_RECEIVED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("PUSH_RECEIVED", false)]);
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "unexpected EffectEmitKeyInvalid, got: {:?}",
        result.errors
    );
}

#[test]
fn ref_with_on_change_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("ref".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("on_change", "PATIENT_UPDATED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("PATIENT_UPDATED", false)]);
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "unexpected EffectEmitKeyInvalid, got: {:?}",
        result.errors
    );
}

#[test]
fn ref_with_success_is_rejected() {
    // Ref is reactive — `success` makes no sense, only `on_change` + `failure`.
    let effects = vec![SExpression::Array(vec![
        SExpression::String("ref".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("success", "LOADED")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive(&schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "expected EffectEmitKeyInvalid on ref.emit.success, got: {:?}",
        result.errors
    );
}

#[test]
fn camel_case_on_message_alias_accepted() {
    // Resolver accepts both `on_message` and `onMessage`; the validator
    // should see them as the same canonical key, not report `onMessage`
    // as unknown.
    let effects = vec![SExpression::Array(vec![
        SExpression::String("os/watch-cron".to_string()),
        SExpression::String("*/5 * * * *".to_string()),
        options_with_emit(emit_object(&[("onMessage", "CRON_TICKED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("CRON_TICKED", false)]);
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
        "camelCase onMessage should be accepted, got: {:?}",
        result.errors
    );
}
