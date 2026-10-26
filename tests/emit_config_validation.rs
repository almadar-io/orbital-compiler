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
        m.insert((*k).to_string(), SExpression::String((*v).to_string()));
    }
    SExpression::Object(m)
}

fn options_with_emit(emit: SExpression) -> SExpression {
    let mut opts: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
    opts.insert("emit".to_string(), emit);
    SExpression::Object(opts)
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
        }],
        instances: None,
        id: None,
        invariants: None,
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
        types: None,
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
            traits: vec![TraitReference::Inline(trait_def)],
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
fn fetch_with_on_message_is_rejected() {
    let effects = vec![SExpression::Array(vec![
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("on_message", "STREAMING")])),
    ])];
    let schema = schema_with_effects(effects, vec![]);
    let result = validate_comprehensive_without_services(&schema);
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
    let result = validate_comprehensive_without_services(&schema);
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
        SExpression::String("fetch".to_string()),
        SExpression::String("Patient".to_string()),
        options_with_emit(emit_object(&[("success", "patientLoaded")])),
    ])];
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
    let result = validate_comprehensive_without_services(&schema);
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
    let result = validate_comprehensive_without_services(&schema);

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
        options_with_emit(emit_object(&[("onMessage", "PUSH_RECEIVED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("PUSH_RECEIVED", false)]);
    let result = validate_comprehensive_without_services(&schema);
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
        options_with_emit(emit_object(&[("onChange", "PATIENT_UPDATED")])),
    ])];
    let schema = schema_with_effects(effects, vec![declared_emit("PATIENT_UPDATED", false)]);
    let result = validate_comprehensive_without_services(&schema);
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
    let result = validate_comprehensive_without_services(&schema);
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
fn snake_case_stream_keys_are_not_a_second_spelling() {
    // One spelling per key, the runtime's route name: `on_message`/`on_change` are unknown keys.
    for (op, key) in [("os/watch-http", "on_message"), ("ref", "on_change")] {
        let effects = vec![SExpression::Array(vec![
            SExpression::String(op.to_string()),
            SExpression::String(if op == "ref" { "Patient" } else { "wss://push" }.to_string()),
            options_with_emit(emit_object(&[(key, "PUSH_RECEIVED")])),
        ])];
        let schema = schema_with_effects(effects, vec![declared_emit("PUSH_RECEIVED", false)]);
        let result = validate_comprehensive_without_services(&schema);
        assert!(
            result.errors.iter().any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
            "{op} emit.{key} must be rejected, got: {:?}",
            result.errors
        );
    }
}

// ============================================================================
// Tests — call-service `key` / `emit.cancelled` / `cancel-call`
// ============================================================================

fn options(entries: Vec<(&str, SExpression)>) -> SExpression {
    SExpression::Object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<orbital_core_base::schema::SObject>(),
    )
}

fn str_expr(s: &str) -> SExpression {
    SExpression::String(s.to_string())
}

fn keyed_call_service(opts: SExpression) -> SExpression {
    SExpression::Array(vec![
        str_expr("call-service"),
        str_expr("svc"),
        str_expr("run"),
        options(vec![]),
        opts,
    ])
}

fn cancel_call(key: SExpression) -> SExpression {
    SExpression::Array(vec![str_expr("cancel-call"), key])
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
            ErrorCode::CallServiceCancelledWithoutKey
                | ErrorCode::CancelCallNoKeyedCall
                | ErrorCode::CallServiceKeyType
                | ErrorCode::EffectEmitKeyInvalid
                | ErrorCode::EffectUnknownType
        )),
        "got: {:?}",
        result.errors
    );
}

#[test]
fn cancelled_without_key_is_an_error() {
    let effects = vec![keyed_call_service(options(vec![(
        "emit",
        emit_object(&[("success", "DONE"), ("cancelled", "STOPPED")]),
    )]))];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::CallServiceCancelledWithoutKey
    ));
}

#[test]
fn a_null_key_does_not_count_as_a_key() {
    let effects = vec![keyed_call_service(options(vec![
        ("key", SExpression::Null),
        ("emit", emit_object(&[("cancelled", "STOPPED")])),
    ]))];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::CallServiceCancelledWithoutKey
    ));
}

#[test]
fn cancelled_with_a_key_is_not_reported() {
    let effects = vec![keyed_call_service(options(vec![
        ("key", str_expr("batch")),
        ("emit", emit_object(&[("cancelled", "STOPPED")])),
    ]))];
    assert!(!has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::CallServiceCancelledWithoutKey
    ));
}

#[test]
fn an_undeclared_cancelled_event_is_validated_like_success_and_failure() {
    let effects = vec![keyed_call_service(options(vec![
        ("key", str_expr("batch")),
        ("emit", emit_object(&[("cancelled", "NOWHERE")])),
    ]))];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::EffectEmitUndeclaredEvent
    ));
}

#[test]
fn a_malformed_cancelled_event_name_is_rejected() {
    let effects = vec![keyed_call_service(options(vec![
        ("key", str_expr("batch")),
        ("emit", emit_object(&[("cancelled", "not valid!")])),
    ]))];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::EffectEmitInvalidName
    ));
}

#[test]
fn cancelled_is_not_an_emit_key_of_fetch() {
    let effects = vec![SExpression::Array(vec![
        str_expr("fetch"),
        str_expr("Patient"),
        options_with_emit(emit_object(&[("cancelled", "STOPPED")])),
    ])];
    assert!(has_code(
        effects,
        cancelled_emit_declared(),
        ErrorCode::EffectEmitKeyInvalid
    ));
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
    let substrate = |opts: SExpression| SExpression::Array(vec![str_expr("program/eval"), str_expr("@entity.id"), opts]);
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
    ]);
    assert!(has_code(
        vec![nested],
        vec![],
        ErrorCode::CancelCallNoKeyedCall
    ));
}

#[test]
fn non_string_key_literals_are_an_error() {
    for bad in [
        SExpression::Number(7.0),
        SExpression::Bool(true),
        options(vec![("a", str_expr("b"))]),
        SExpression::Array(vec![SExpression::Number(1.0)]),
    ] {
        let effects = vec![keyed_call_service(options(vec![
            ("key", bad.clone()),
            ("emit", emit_object(&[("success", "DONE")])),
        ]))];
        assert!(
            has_code(
                effects,
                cancelled_emit_declared(),
                ErrorCode::CallServiceKeyType
            ),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn string_literal_and_expression_keys_are_accepted() {
    for good in [
        str_expr("batch-1"),
        str_expr("@entity.id"),
        SExpression::Array(vec![
            str_expr("str/concat"),
            str_expr("a"),
            str_expr("@entity.id"),
        ]),
    ] {
        let effects = vec![keyed_call_service(options(vec![
            ("key", good.clone()),
            ("emit", emit_object(&[("success", "DONE")])),
        ]))];
        assert!(
            !has_code(
                effects,
                cancelled_emit_declared(),
                ErrorCode::CallServiceKeyType
            ),
            "{good:?} must be accepted"
        );
    }
}

#[test]
fn key_only_options_are_read_as_options_not_params() {
    let effects = vec![keyed_call_service(options(vec![(
        "key",
        SExpression::Number(1.0),
    )]))];
    assert!(has_code(effects, vec![], ErrorCode::CallServiceKeyType));
}

fn os_call(op: &str, emit: &[(&str, &str)]) -> SExpression {
    SExpression::Array(vec![
        SExpression::String(op.to_string()),
        SExpression::String("127.0.0.1".to_string()),
        SExpression::Number(0.0),
        options_with_emit(emit_object(emit)),
    ])
}

fn emit_key_invalid(op: &str, emit: &[(&str, &str)]) -> bool {
    let declared = emit.iter().map(|(_, e)| declared_emit(e, false)).collect();
    let schema = schema_with_effects(vec![os_call(op, emit)], declared);
    validate_comprehensive_without_services(&schema)
        .errors
        .iter()
        .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid)
}

#[test]
fn stream_os_ops_declare_the_on_message_key() {
    for op in ["os/tcp-listen"] {
        assert!(!emit_key_invalid(op, &[("success", "UP"), ("onMessage", "ARRIVED")]), "{op} emit.onMessage must be accepted");
        assert!(emit_key_invalid(op, &[("on_message", "ARRIVED")]), "control: {op} has no on_message key");
    }
    assert!(
        emit_key_invalid("os/tcp-connect", &[("onMessage", "ARRIVED")]),
        "control: a single-answer os op has no onMessage route"
    );
}
