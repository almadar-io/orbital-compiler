//! Integration tests for the `emit:` config validator.
//!
//! Stage 3 of the close-the-circuit plan adds per-operator validation
//! for the `emit:` options key:
//!
//!   - `fetch`, `persist`, `call-service` → `success`, `failure`
//!   - `ref`                              → `onChange`, `failure`
//!   - `set`                              → `success` (sync)
//!   - `os/watch-*`                       → `onMessage`, `failure`
//!
//! Violations:
//!   - Unknown key for operator → `ORB_EFF_EMIT_KEY_INVALID`
//!   - Non-UPPER_SNAKE event    → `ORB_EFF_EMIT_INVALID_NAME`
//!   - Undeclared event         → `ORB_EFF_EMIT_UNDECLARED_EVENT` (warning)

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use std::sync::Arc;
use orbital_core_base::schema::{
    EmitDefinition, EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType, Orbital,
    OrbitalSchema, SExpression, StateDefinition, StateMachine, StringOrArray, TraitDefinition,
    TraitReference, TransitionDefinition,
};

// ============================================================================
// Helpers
// ============================================================================

fn emit_object(entries: &[(&str, &str)]) -> SExpression {
    let mut m: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
    for (k, v) in entries {
        m.insert((*k).into(), SExpression::String(((*v).to_string()).into()));
    }
    SExpression::object(m)
}

fn options_with_emit(emit: SExpression) -> SExpression {
    let mut opts: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
    opts.insert("emit".into(), emit);
    SExpression::object(opts)
}

/// Build an inline orbital containing one trait with the given effects.
fn schema_with_effects(
    effects: Vec<SExpression>,
    declared_emits: Vec<EmitDefinition>,
) -> OrbitalSchema {
    let entity = EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        store: None,
        shared: false,
        identity: false,
        browser: false,
        locale_instances: None,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: "Patient".to_string(),
        collection: Some("patients".to_string()),
        persistence: None,
        fields: vec![FieldDefinition {
            nullable: None,
            additional_properties: None,
            mock: None,
            merged_from: None,
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
            projected_from: None,
            record: None,
            derived: None,
        }],
        instances: None,
        id: None,
        invariants: None,
        indexes: None,
    };

    let trait_def = TraitDefinition {
        dispatch: None,
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "PatientLoader".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        linked_entity: Some("Patient".to_string()),
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
                effects: Some(effects),
                ensures: None,
            }],
        }
        .into(),
        emits: if declared_emits.is_empty() {
            None
        } else {
            Some(declared_emits.into())
        },
        listens: None,
        ticks: None,
        config: None,
        data_entities: None,
        source_declaration: None,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    };

    OrbitalSchema {
        type_contract_version: None,
        resolved: None,        types: None,
        store: None,
        theme_definitions: None,
        themes: None,
        site: None,
        public_dir: None,
        locales: None,
        lazy_pages: None,
        theme: None,
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            spec: false,
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(Arc::new(trait_def))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        }],
        metadata: None,
        ledger: None,
        schema_version: None,
        config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
    }
}

fn declared_emit(event: &str, external: bool) -> EmitDefinition {
    EmitDefinition {
        definer_knob: None,
        scope_overridden: None,
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
        payload_entity: None,
    }
}

// ============================================================================
// Tests — per-operator allowed-key enforcement
// ============================================================================

#[test]
fn fetch_with_success_and_failure_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String(("fetch".to_string()).into()),
        SExpression::String(("Patient".to_string()).into()),
        options_with_emit(emit_object(&[
            ("success", "PATIENT_LOADED"),
            ("failure", "PATIENT_LOAD_FAILED"),
        ])),
    ].into())];
    let schema = schema_with_effects(
        effects,
        vec![
            declared_emit("PATIENT_LOADED", false),
            declared_emit("PATIENT_LOAD_FAILED", false),
        ],
    );
    let result = validate_comprehensive_without_services(&schema);
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
fn set_with_success_is_valid() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String(("set".to_string()).into()),
        SExpression::String(("@entity.status".to_string()).into()),
        SExpression::String(("done".to_string()).into()),
        options_with_emit(emit_object(&[("success", "STATUS_CHANGED")])),
    ].into())];
    let schema = schema_with_effects(effects, vec![declared_emit("STATUS_CHANGED", false)]);
    let result = validate_comprehensive_without_services(&schema);
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
        SExpression::String(("fetch".to_string()).into()),
        SExpression::String(("Patient".to_string()).into()),
        options_with_emit(emit_object(&[("success", "patientLoaded")])),
    ].into())];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive_without_services(&schema);
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
fn os_watch_http_is_retired_and_unknown_while_a_known_os_operator_is_not() {
    let unknown = |op: &str| {
        let effects = vec![SExpression::Array(vec![
            str_expr(op),
            str_expr("wss://push"),
        ].into())];
        let schema = schema_with_effects(effects, vec![]);
        validate_comprehensive_without_services(&schema)
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectUnknownType)
    };
    assert!(unknown("os/watch-http"));
    assert!(!unknown("os/watch-port"));
}

// ============================================================================
// Tests — call-service `key` / `emit.cancelled` / `cancel-call`
// ============================================================================

fn options(entries: Vec<(&str, SExpression)>) -> SExpression {
    SExpression::object(
        entries
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect::<orbital_core_base::schema::SObject>(),
    )
}

fn str_expr(s: &str) -> SExpression {
    SExpression::String(s.to_string().into())
}

fn keyed_call_service(opts: SExpression) -> SExpression {
    SExpression::Array(vec![
        str_expr("call-service"),
        str_expr("svc"),
        str_expr("run"),
        options(vec![]),
        opts,
    ].into())
}

fn cancel_call(key: SExpression) -> SExpression {
    SExpression::Array(vec![str_expr("cancel-call"), key].into())
}

fn has_code(effects: Vec<SExpression>, emits: Vec<EmitDefinition>, code: ErrorCode) -> bool {
    validate_comprehensive_without_services(&schema_with_effects(effects, emits))
        .errors
        .iter()
        .any(|e| e.code == code)
}

fn cancelled_emit_declared() -> Vec<EmitDefinition> {
    vec![
        declared_emit("DONE", false),
        declared_emit("FAILED", false),
        declared_emit("STOPPED", false),
    ]
}

#[test]
fn keyed_call_with_cancelled_and_a_cancel_call_has_no_cancel_errors() {
    let effects = vec![
        keyed_call_service(options(vec![
            ("key", str_expr("@entity.id")),
            (
                "emit",
                emit_object(&[
                    ("success", "DONE"),
                    ("failure", "FAILED"),
                    ("cancelled", "STOPPED"),
                ]),
            ),
        ])),
        cancel_call(str_expr("@entity.id")),
    ];
    let result = validate_comprehensive_without_services(&schema_with_effects(effects, cancelled_emit_declared()));
    assert!(
        !result.errors.iter().any(|e| matches!(
            e.code,
            ErrorCode::CancelCallNoKeyedCall
                | ErrorCode::EffectEmitKeyInvalid
                | ErrorCode::EffectUnknownType
        )),
        "got: {:?}",
        result.errors
    );
}



#[test]
fn cancel_call_with_no_keyed_call_anywhere_is_an_error() {
    let effects = vec![
        keyed_call_service(options(vec![("emit", emit_object(&[("success", "DONE")]))])),
        cancel_call(str_expr("@entity.id")),
    ];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::CancelCallNoKeyedCall
    ));
}

#[test]
fn cancel_call_with_no_call_service_at_all_is_an_error() {
    assert!(has_code(
        vec![cancel_call(str_expr("k"))],
        vec![],
        ErrorCode::CancelCallNoKeyedCall
    ));
}

#[test]
fn cancel_call_is_satisfied_by_a_keyed_call_in_another_trait_of_the_app() {
    let mut schema = schema_with_effects(vec![cancel_call(str_expr("k"))], vec![]);
    let keyed = schema_with_effects(
        vec![keyed_call_service(options(vec![
            ("key", str_expr("k")),
            ("emit", emit_object(&[("success", "DONE")])),
        ]))],
        vec![declared_emit("DONE", false)],
    );
    let mut other = keyed.orbitals.into_iter().next().unwrap();
    other.name = "OtherOrbital".to_string();
    schema.orbitals.push(other);
    let result = validate_comprehensive_without_services(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CancelCallNoKeyedCall),
        "got: {:?}",
        result.errors
    );
}

#[test]
fn cancel_call_is_satisfied_by_a_keyed_substrate_op_and_not_by_an_unkeyed_one() {
    let substrate = |opts: SExpression| SExpression::Array(vec![str_expr("program/eval"), str_expr("@entity.id"), opts].into());
    let keyed = substrate(options(vec![("key", str_expr("k")), ("emit", emit_object(&[("success", "DONE")]))]));
    assert!(
        !has_code(vec![keyed, cancel_call(str_expr("k"))], vec![declared_emit("DONE", false)], ErrorCode::CancelCallNoKeyedCall),
        "a keyed substrate op is a call cancel-call reaches"
    );
    let unkeyed = substrate(options(vec![("emit", emit_object(&[("success", "DONE")]))]));
    assert!(
        has_code(vec![unkeyed, cancel_call(str_expr("k"))], vec![declared_emit("DONE", false)], ErrorCode::CancelCallNoKeyedCall),
        "control: an unkeyed one is not"
    );
    let key_as_argument = substrate(options(vec![("key", str_expr("k"))]));
    assert!(
        has_code(vec![key_as_argument, cancel_call(str_expr("k"))], vec![], ErrorCode::CancelCallNoKeyedCall),
        "edge: a trailing object without emit is an argument, not options"
    );
}

#[test]
fn cancel_call_inside_a_conditional_is_still_found() {
    let nested = SExpression::Array(vec![
        str_expr("if"),
        SExpression::Bool(true),
        cancel_call(str_expr("k")),
    ].into());
    assert!(has_code(
        vec![nested],
        vec![],
        ErrorCode::CancelCallNoKeyedCall
    ));
}


fn os_call(op: &str, emit: &[(&str, &str)]) -> SExpression {
    SExpression::Array(vec![
        SExpression::String(op.to_string().into()),
        SExpression::String(("127.0.0.1".to_string()).into()),
        SExpression::Number(0.0),
        options_with_emit(emit_object(emit)),
    ].into())
}

fn emit_key_invalid(op: &str, emit: &[(&str, &str)]) -> bool {
    let declared = emit.iter().map(|(_, e)| declared_emit(e, false)).collect();
    let schema = schema_with_effects(vec![os_call(op, emit)], declared);
    validate_comprehensive_without_services(&schema)
        .errors
        .iter()
        .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid)
}

