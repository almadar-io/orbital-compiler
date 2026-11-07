//! Phase F integration tests.
//!
//! Covers F.1 (emit-payload missing-required escalation toggle) and F.3
//! (literal-union default check) using inline OrbitalSchema fixtures (no .orb test files,
//! per the Phase F brief).

use orbital_compiler::phases::validation::{
    validate_comprehensive_full, ErrorCode, SchemaLanguage,
};
use std::sync::Arc;
use orbital_core_base::schema::{
    EmitDefinition, EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType,
    ListenDefinition, ListenSource, Orbital, OrbitalSchema, PayloadField, SExpression,
    StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
    TransitionDefinition,
};

// ============================================================================
// Shared helpers
// ============================================================================

fn s(s: &str) -> SExpression {
    SExpression::String(s.to_string().into())
}

fn payload_field(name: &str, ty: &str, required: bool) -> PayloadField {
    PayloadField { nullable: None, additional_properties: None, values: None,
        name: name.to_string(),
        field_type: ty.to_string(),
        required: if required { Some(true) } else { None },
        description: None,
        properties: None,
        type_when: None,
        entity: None,
        projected_from: None,
        items: None,
        relation: None,
    }
}

fn emit_def(event: &str, fields: Vec<PayloadField>) -> EmitDefinition {
    EmitDefinition {
        definer_knob: None,
        scope_overridden: None,
        event: event.to_string(),
        description: None,
        synonyms: None,
        tier: None,
        scope: Some(EventScope::External),
        payload_schema: if fields.is_empty() {
            None
        } else {
            Some(fields)
        },
        event_id: None,
        payload_entity: None,
    }
}

fn task_entity() -> EntityDefinition {
    EntityDefinition {
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
        name: "Task".to_string(),
        collection: Some("tasks".to_string()),
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
    }
}

fn run(schema: &OrbitalSchema) -> orbital_compiler::phases::validation::ValidationResult {
    validate_comprehensive_full(
        schema,
        None,
        SchemaLanguage::English,
        orbital_compiler::embedded::ServiceRegistryData::default(),
        Vec::new(),
    )
}

// ============================================================================
// F.1 — emit-payload missing-required escalation
// ============================================================================

fn schema_for_f1(strict_target_event: &str) -> OrbitalSchema {
    // Trait declares EVT with a required field, but a transition emits EVT
    // without the field.
    let mut payload_obj = orbital_core_base::schema::SObject::new();
    payload_obj.insert("name".into(), s("Ada"));
    let emit = SExpression::Array(vec![
        s("emit"),
        s(strict_target_event),
        SExpression::object(payload_obj),
    ].into());

    let trait_def = TraitDefinition { dropped_listens: None,
        dispatch: None,
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "TaskEmitter".to_string(),
        entity_rebindable: None,
        entity_contract: None,
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
                await_render: None,
            }],
        }.into(),
        emits: Some(vec![emit_def(
            strict_target_event,
            vec![
                payload_field("id", "string", true),
                payload_field("name", "string", false),
            ],
        )].into()),
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
        messages: None,
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
            entity: EntityRef::Inline(task_entity()),
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

#[test]
fn f1_strict_mode_reports_missing_required_in_errors() {
    let schema = schema_for_f1("USER_SAVED");
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::EmitPayloadMissingRequired));
    assert!(
        hit,
        "F.1 strict: expected EmitPayloadMissingRequired in errors. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

// (The former "lenient mode" test was removed with the phase_f_strict
// toggle: there was never a second severity behind it.)

// ============================================================================
// F.3 — literal-union default check
// ============================================================================

