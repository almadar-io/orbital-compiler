//! Integration tests for the V2 Phase 0 emit-literal payload validator.
//!
//! Covers the three error classes from
//! `phases/validation/emit_payload.rs`:
//!
//!   - `EmitPayloadMissingRequired`: declared field omitted from the emit literal.
//!   - `EmitPayloadExtraField`: emit literal carries a field not on the payload schema.
//!   - `EmitPayloadFieldTypeMismatch`: literal primitive type disagrees with declared `field_type`.
//!
//! All classes are warn-only in V2 Phase 0; tests verify warnings surface on
//! the result but do not block validation.
use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core_base::schema::{
    EmitDefinition, EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType, Orbital,
    OrbitalSchema, PayloadField, SExpression, StateDefinition, StateMachine, StringOrArray,
    TraitDefinition, TraitReference, TransitionDefinition,
};

// ============================================================================
// Helpers
// ============================================================================

fn string_sexpr(s: &str) -> SExpression {
    SExpression::String(s.to_string())
}

fn number_sexpr(n: f64) -> SExpression {
    SExpression::Number(n)
}

fn emit_effect(event: &str, literal: Option<SExpression>) -> SExpression {
    let mut arr = vec![string_sexpr("emit"), string_sexpr(event)];
    if let Some(lit) = literal {
        arr.push(lit);
    }
    SExpression::Array(arr)
}

fn payload_field(name: &str, ty: &str, required: bool) -> PayloadField {
    PayloadField { values: None,
        name: name.to_string(),
        field_type: ty.to_string(),
        required: if required { Some(true) } else { None },
        description: None,
        properties: None,
        type_when: None,
        entity: None,
        projected_from: None,
        items: None,
    }
}

fn declared_emit_with_payload(event: &str, fields: Vec<PayloadField>) -> EmitDefinition {
    EmitDefinition {
        definer_knob: None,
        scope_overridden: None,
        event: event.to_string(),
        description: None,
        synonyms: None,
        tier: None,
        scope: Some(EventScope::Internal),
        payload_schema: Some(fields),
        event_id: None,
        payload_entity: None,
    }
}

fn schema_with_emit(emit: SExpression, declared: EmitDefinition) -> OrbitalSchema {
    let entity = EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        store: None,
        shared: false,
        identity: false,
        local: false,
        seed_mock: false,
        locale_instances: None,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: "Task".to_string(),
        collection: Some("tasks".to_string()),
        persistence: None,
        fields: vec![FieldDefinition {
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
        }],
        instances: None,
        id: None,
        invariants: None,
    };
    let trait_def = TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "TaskEmitter".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        local: false,
        linked_entity: Some("Task".to_string()),
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
                effects: Some(vec![emit]),
                ensures: None,
            }],
        },
        emits: Some(vec![declared]),
        listens: None,
        ticks: None,
        config: None,
        data_entities: None,
        scope: None,
        source_declaration: None,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    };
    OrbitalSchema {
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

// ============================================================================
// Tests
// ============================================================================

#[test]
fn missing_required_field_emits_warning() {
    // USER_SAVED declares { id: string! (required), name: string }.
    // Emit omits `id`.
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("name".to_string(), string_sexpr("Ada"));
    let emit = emit_effect("USER_SAVED", Some(SExpression::Object(literal)));
    let declared = declared_emit_with_payload(
        "USER_SAVED",
        vec![
            payload_field("id", "string", true),
            payload_field("name", "string", false),
        ],
    );
    let schema = schema_with_emit(emit, declared);
    let result = validate_comprehensive_without_services(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::EmitPayloadMissingRequired));
    assert!(
        hit,
        "expected EmitPayloadMissingRequired warning; got errors={:?} warnings={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>(),
        result.warnings.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn extra_field_emits_warning() {
    // USER_SAVED declares { id: string }. Emit provides `nickname` too.
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("id".to_string(), string_sexpr("u-1"));
    literal.insert("nickname".to_string(), string_sexpr("ada"));
    let emit = emit_effect("USER_SAVED", Some(SExpression::Object(literal)));
    let declared =
        declared_emit_with_payload("USER_SAVED", vec![payload_field("id", "string", false)]);
    let schema = schema_with_emit(emit, declared);
    let result = validate_comprehensive_without_services(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::EmitPayloadExtraField));
    assert!(
        hit,
        "expected EmitPayloadExtraField warning; got errors={:?} warnings={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>(),
        result.warnings.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn primitive_type_mismatch_emits_warning() {
    // USER_SAVED declares { count: number }. Emit provides a string literal.
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("count".to_string(), string_sexpr("seventeen"));
    let emit = emit_effect("USER_SAVED", Some(SExpression::Object(literal)));
    let declared =
        declared_emit_with_payload("USER_SAVED", vec![payload_field("count", "number", false)]);
    let schema = schema_with_emit(emit, declared);
    let result = validate_comprehensive_without_services(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::EmitPayloadFieldTypeMismatch));
    assert!(
        hit,
        "expected EmitPayloadFieldTypeMismatch warning; got errors={:?} warnings={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>(),
        result.warnings.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn valid_emit_passes_without_warnings() {
    // USER_SAVED declares { id: string!, name: string }. Emit satisfies both.
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("id".to_string(), string_sexpr("u-1"));
    literal.insert("name".to_string(), string_sexpr("Ada"));
    let emit = emit_effect("USER_SAVED", Some(SExpression::Object(literal)));
    let declared = declared_emit_with_payload(
        "USER_SAVED",
        vec![
            payload_field("id", "string", true),
            payload_field("name", "string", false),
        ],
    );
    let schema = schema_with_emit(emit, declared);
    let result = validate_comprehensive_without_services(&schema);
    let any_p07_hit = result.errors.iter().chain(result.warnings.iter()).any(|e| {
        matches!(
            e.code,
            ErrorCode::EmitPayloadMissingRequired
                | ErrorCode::EmitPayloadExtraField
                | ErrorCode::EmitPayloadFieldTypeMismatch
        )
    });
    assert!(
        !any_p07_hit,
        "valid emit should not surface P0.7 warnings; got {:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn number_literal_matches_number_declared_type() {
    // Guard against false positives on numeric literals.
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("count".to_string(), number_sexpr(42.0));
    let emit = emit_effect("COUNT_UPDATED", Some(SExpression::Object(literal)));
    let declared = declared_emit_with_payload(
        "COUNT_UPDATED",
        vec![payload_field("count", "number", true)],
    );
    let schema = schema_with_emit(emit, declared);
    let result = validate_comprehensive_without_services(&schema);
    let any_p07_hit = result.errors.iter().chain(result.warnings.iter()).any(|e| {
        matches!(
            e.code,
            ErrorCode::EmitPayloadMissingRequired
                | ErrorCode::EmitPayloadExtraField
                | ErrorCode::EmitPayloadFieldTypeMismatch
        )
    });
    assert!(!any_p07_hit);
}

fn list_field_emit(value: SExpression) -> bool {
    let mut literal = orbital_core_base::schema::SObject::new();
    literal.insert("icons".to_string(), value);
    let emit = emit_effect("USER_SAVED", Some(SExpression::Object(literal)));
    let declared =
        declared_emit_with_payload("USER_SAVED", vec![payload_field("icons", "[string]", true)]);
    let result = validate_comprehensive_without_services(&emit_schema(emit, declared));
    result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::EmitPayloadFieldTypeMismatch))
}

fn emit_schema(emit: SExpression, declared: EmitDefinition) -> OrbitalSchema {
    schema_with_emit(emit, declared)
}

#[test]
fn an_empty_list_takes_the_declared_list_type() {
    assert!(!list_field_emit(SExpression::Array(vec![])));
}

#[test]
fn control_a_string_in_a_list_field_is_a_mismatch() {
    assert!(list_field_emit(string_sexpr("github")));
}
