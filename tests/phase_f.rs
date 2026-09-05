//! Phase F integration tests.
//!
//! Covers F.1 (emit-payload missing-required escalation toggle), F.2
//! (cross-emitter declared-payload consensus), and F.3 (literal-union
//! default check) using inline OrbitalSchema fixtures (no .orb test files,
//! per the Phase F brief).

use orbital_compiler::phases::validation::{
    validate_comprehensive_full, ErrorCode, SchemaLanguage,
};
use orbital_core::schema::{
    EmitDefinition, EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType,
    ListenDefinition, ListenSource, Orbital, OrbitalSchema, PayloadField, SExpression,
    StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
    TransitionDefinition,
};
use std::collections::HashMap;

// ============================================================================
// Shared helpers
// ============================================================================

fn s(s: &str) -> SExpression {
    SExpression::String(s.to_string())
}

fn payload_field(name: &str, ty: &str, required: bool) -> PayloadField {
    PayloadField {
        name: name.to_string(),
        field_type: ty.to_string(),
        required: if required { Some(true) } else { None },
        description: None,
        properties: None,
        type_when: None,
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
    }
}

fn task_entity() -> EntityDefinition {
    EntityDefinition {
        shared: false,
        identity: false,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: "Task".to_string(),
        collection: Some("tasks".to_string()),
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
    }
}

fn run(schema: &OrbitalSchema) -> orbital_compiler::phases::validation::ValidationResult {
    validate_comprehensive_full(
        schema,
        None,
        SchemaLanguage::English,
        orbital_compiler::embedded::ServiceRegistryData::default(),
    )
}

// ============================================================================
// F.1 — emit-payload missing-required escalation
// ============================================================================

fn schema_for_f1(strict_target_event: &str) -> OrbitalSchema {
    // Trait declares EVT with a required field, but a transition emits EVT
    // without the field.
    let mut payload_obj = HashMap::new();
    payload_obj.insert("name".to_string(), s("Ada"));
    let emit = SExpression::Array(vec![
        s("emit"),
        s(strict_target_event),
        SExpression::Object(payload_obj),
    ]);

    let trait_def = TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "TaskEmitter".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        linked_entity: Some("Task".to_string()),
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
                effects: Some(vec![emit]),
            }],
        },
        emits: Some(vec![emit_def(
            strict_target_event,
            vec![
                payload_field("id", "string", true),
                payload_field("name", "string", false),
            ],
        )]),
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
        theme: None,
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(task_entity()),
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
// F.2 — cross-emitter declared-payload consensus
// ============================================================================

fn schema_with_two_emitters(
    event: &str,
    a_fields: Vec<PayloadField>,
    b_fields: Vec<PayloadField>,
) -> OrbitalSchema {
    let mk_trait = |name: &str,
                    emits: Option<Vec<EmitDefinition>>,
                    listens: Option<Vec<ListenDefinition>>|
     -> TraitDefinition {
        TraitDefinition {
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            linked_entity: Some("Task".to_string()),
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
                    effects: None,
                }],
            },
            emits,
            listens,
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
    };
    // F4 narrowing precondition: cross-emitter mismatch only fires when at
    // least one cross-orbital listener subscribes to the event. Adding a
    // second orbital with a `Listener` trait reaches every L2.6 narrowing
    // arm for the test case.
    let listener_orbital = Orbital {
        types: None,
        name: "ListenerOrbital".to_string(),
        uses: None,
        expects: None,
        theme: None,
        services: None,
        entity: EntityRef::Inline(task_entity()),
        auxiliary_entities: None,
        traits: vec![TraitReference::Inline(mk_trait(
            "Listener",
            None,
            Some(vec![ListenDefinition {
                event: event.to_string(),
                triggers: Some("INIT".to_string()),
                scope: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
                source: Some(ListenSource::Orbital {
                    orbital: "TestOrbital".to_string(),
                    trait_name: "EmitterA".to_string(),
                    orbital_id: None,
                    trait_id: None,
                }),
                event_id: None,
                triggers_id: None,
            }]),
        ))],
        pages: None,
        emits: None,
        listens: None,
        exposes: None,
        domain_context: None,
        design: None,
        id: None,
    };
    OrbitalSchema {
        theme: None,
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![
            Orbital {
                types: None,
                name: "TestOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(task_entity()),
                auxiliary_entities: None,
                traits: vec![
                    TraitReference::Inline(mk_trait(
                        "EmitterA",
                        Some(vec![emit_def(event, a_fields)]),
                        None,
                    )),
                    TraitReference::Inline(mk_trait(
                        "EmitterB",
                        Some(vec![emit_def(event, b_fields)]),
                        None,
                    )),
                ],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
                id: None,
            },
            listener_orbital,
        ],
        metadata: None,
        ledger: None,
        schema_version: None,
    }
}

#[test]
fn f2_mismatched_emitters_warns() {
    let schema = schema_with_two_emitters(
        "X",
        vec![
            payload_field("a", "string", true),
            payload_field("b", "string", true),
        ],
        vec![payload_field("a", "string", true)],
    );
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        hit,
        "F.2: expected CrossEmitterDeclaredPayloadMismatch when two emitters disagree. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn f2_matching_emitters_pass() {
    let schema = schema_with_two_emitters(
        "X",
        vec![payload_field("a", "string", true)],
        vec![payload_field("a", "string", true)],
    );
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        !hit,
        "F.2: matching emitters should not surface CrossEmitterDeclaredPayloadMismatch. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

// ============================================================================
// F.3 — literal-union default check
// ============================================================================

fn schema_with_default(field: FieldDefinition) -> OrbitalSchema {
    let entity = EntityDefinition {
        shared: false,
        identity: false,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: "Order".to_string(),
        collection: Some("orders".to_string()),
        persistence: None,
        fields: vec![
            FieldDefinition {
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
            },
            field,
        ],
        instances: None,
        id: None,
    };
    OrbitalSchema {
        theme: None,
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity),
            auxiliary_entities: None,
            traits: vec![],
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

#[test]
fn f3_default_outside_values_errors() {
    let field = FieldDefinition {
        name: Some("status".to_string()),
        field_type: FieldType::String,
        required: Some(false),
        primary_key: None,
        unique: None,
        default: Some(s("d")),
        values: Some(vec!["a".to_string(), "b".to_string(), "c".to_string()]),
        relation: None,
        items: None,
        properties: None,
        intrinsic: None,
        description: None,
        synonyms: None,
    };
    let schema = schema_with_default(field);
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::LiteralUnionDefaultMismatch));
    assert!(
        hit,
        "F.3: expected LiteralUnionDefaultMismatch when default isn't in values. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn f3_default_inside_values_passes() {
    let field = FieldDefinition {
        name: Some("status".to_string()),
        field_type: FieldType::String,
        required: Some(false),
        primary_key: None,
        unique: None,
        default: Some(s("b")),
        values: Some(vec!["a".to_string(), "b".to_string(), "c".to_string()]),
        relation: None,
        items: None,
        properties: None,
        intrinsic: None,
        description: None,
        synonyms: None,
    };
    let schema = schema_with_default(field);
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::LiteralUnionDefaultMismatch));
    assert!(
        !hit,
        "F.3: default in values should not flag. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

#[test]
fn f3_no_values_passes() {
    // A field with a default but no values list is fine.
    let field = FieldDefinition {
        name: Some("status".to_string()),
        field_type: FieldType::String,
        required: Some(false),
        primary_key: None,
        unique: None,
        default: Some(s("anything")),
        values: None,
        relation: None,
        items: None,
        properties: None,
        intrinsic: None,
        description: None,
        synonyms: None,
    };
    let schema = schema_with_default(field);
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .any(|e| matches!(e.code, ErrorCode::LiteralUnionDefaultMismatch));
    assert!(!hit);
}
