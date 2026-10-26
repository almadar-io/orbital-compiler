//! Phase F integration tests.
//!
//! Covers F.1 (emit-payload missing-required escalation toggle), F.2
//! (cross-emitter declared-payload consensus), and F.3 (literal-union
//! default check) using inline OrbitalSchema fixtures (no .orb test files,
//! per the Phase F brief).

use orbital_compiler::phases::validation::{
    validate_comprehensive_full, ErrorCode, SchemaLanguage,
};
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
    SExpression::String(s.to_string())
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
    payload_obj.insert("name".to_string(), s("Ada"));
    let emit = SExpression::Array(vec![
        s("emit"),
        s(strict_target_event),
        SExpression::Object(payload_obj),
    ]);

    let trait_def = TraitDefinition {
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
            entity: EntityRef::Inline(task_entity()),
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
    schema_with_two_emitters_and_listener_source(
        event,
        a_fields,
        b_fields,
        ListenSource::Orbital {
            orbital: "TestOrbital".to_string(),
            trait_name: "EmitterA".to_string(),
            orbital_id: None,
            trait_id: None,
        },
    )
}

/// Same two-conflicting-emitters shape as [`schema_with_two_emitters`], with
/// the `ListenerOrbital.Listener` trait's subscription source parameterized
/// — lets the reachability tests below (gap #82 G6) exercise a `Trait`-kind
/// pin and an `Any` wildcard against the identical emitter group, without
/// three copies of this fixture.
fn schema_with_two_emitters_and_listener_source(
    event: &str,
    a_fields: Vec<PayloadField>,
    b_fields: Vec<PayloadField>,
    listener_source: ListenSource,
) -> OrbitalSchema {
    let mk_trait = |name: &str,
                    emits: Option<Vec<EmitDefinition>>,
                    listens: Option<Vec<ListenDefinition>>|
     -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
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
                    effects: None,
                    ensures: None,
                }],
            },
            emits,
            listens,
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
        }
    };
    // F4 narrowing precondition: cross-emitter mismatch only fires when at
    // least one cross-orbital listener subscribes to the event. Adding a
    // second orbital with a `Listener` trait reaches every L2.6 narrowing
    // arm for the test case.
    let listener_orbital = Orbital {
        spec: false,
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
                source: Some(listener_source.clone()),
                event_id: None,
                triggers_id: None,
            }]),
        ))],
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
        orbitals: vec![
            Orbital {
                spec: false,
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
                os_grants: None,
                design: None,
                id: None,
                reference: None,
                config: None,
            },
            listener_orbital,
        ],
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

// ----------------------------------------------------------------------
// Gap #82 G6 — reachability precision (`is_cross_orbital_reachable`).
// ----------------------------------------------------------------------

/// (a) A `Trait`-pinned listener names a trait that is NOT one of the
/// group's actual emitters (it lives in the listener's own orbital, which
/// is neither `EmitterA` nor `EmitterB`'s home). Before the fix, an absent
/// same-orbital match fell through to "assume reachable from anywhere in
/// the schema" — the exact false positive gap #82 G6 named
/// (`AssetLibraryOrbital.DocumentReviewDecision` pinned to
/// `DocumentApprovalInbox`, a trait with no `emits[]`, was credited with
/// `ApprovalRequestOrbital`'s unrelated `APPROVE` conflict). The runtime's
/// `buildSourceMatcher` (`packages/almadar-runtime/src/identity/
/// routing.ts`) never routes a `Trait`-pinned listen across orbitals at
/// all, so this must never fire regardless of what the named trait does.
#[test]
fn f2_trait_pinned_listener_in_other_orbital_not_reachable() {
    let schema = schema_with_two_emitters_and_listener_source(
        "X",
        vec![
            payload_field("a", "string", true),
            payload_field("b", "string", true),
        ],
        vec![payload_field("a", "string", true)],
        ListenSource::Trait {
            trait_name: "ConfigDrivenHandler".to_string(),
            trait_id: None,
        },
    );
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        !hit,
        "G6(a): a Trait-pinned listener naming a trait that isn't one of the \
         conflicting emitters must not make them reachable. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

/// (b) The narrowing predicate must not go blind: an UNPINNED (`*`)
/// listener genuinely can receive either emitter's shape through the same
/// subscription, so a real conflict must still fire.
#[test]
fn f2_wildcard_listener_still_reachable_across_orbitals() {
    let schema = schema_with_two_emitters_and_listener_source(
        "X",
        vec![
            payload_field("a", "string", true),
            payload_field("b", "string", true),
        ],
        vec![payload_field("a", "string", true)],
        ListenSource::Any,
    );
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        hit,
        "G6(b): a wildcard listener must still see a genuine cross-orbital \
         payload conflict. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

// ----------------------------------------------------------------------
// Gap #82 G6 — render-emitter payload derivation (`render_emitter_payload`).
// ----------------------------------------------------------------------

/// A render-ui `["render-ui", "main", { type, action, actionPayload }]`
/// effect — the shape a materialized `ButtonRender`-style embed carries
/// (`packages/almadar-std/behaviors/lolo/ui/core/atoms/ui-button.lolo`:
/// `emits { @config.action -> external {} }` pairs with `actionPayload :
/// @payload @config.action`, registry `payloadFor: "action"`).
fn button_action_payload_effect(event: &str, payload_key: &str, entity_field: &str) -> SExpression {
    let mut node = orbital_core_base::schema::SObject::new();
    node.insert("type".to_string(), s("button"));
    node.insert("action".to_string(), s(event));
    let mut payload = orbital_core_base::schema::SObject::new();
    payload.insert(
        payload_key.to_string(),
        s(&format!("@entity.{}", entity_field)),
    );
    node.insert("actionPayload".to_string(), SExpression::Object(payload));
    SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(node)])
}

/// A trait whose only production of `event` is the rendered button above —
/// its OWN `emits[]` entry is left empty, exactly like `ui-button`'s atom
/// declaration (the event name is only known at the call site).
fn render_button_trait(
    name: &str,
    event: &str,
    payload_key: &str,
    entity_field: &str,
) -> TraitDefinition {
    TraitDefinition {
        dispatch: None,
        effect_row: None,
        type_params: None,
        type_args: None,
        name: name.to_string(),
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
                effects: Some(vec![button_action_payload_effect(
                    event,
                    payload_key,
                    entity_field,
                )]),
                ensures: None,
            }],
        },
        emits: Some(vec![emit_def(event, vec![])]),
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
    }
}

/// A plain declaring trait (no render, an ordinary `emits[]` entry) plus
/// the render-only button trait above, both in `TestOrbital`, reached by a
/// wildcard listener in a second orbital (same F.2.6 narrowing precondition
/// `schema_with_two_emitters` uses).
fn schema_with_render_action_payload(
    event: &str,
    declared_fields: Vec<PayloadField>,
) -> OrbitalSchema {
    let mk_plain = |name: &str, fields: Vec<PayloadField>| -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
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
                    effects: None,
                    ensures: None,
                }],
            },
            emits: Some(vec![emit_def(event, fields)]),
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
        }
    };
    let mk_listener_trait = |listens: Vec<ListenDefinition>| -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "Listener".to_string(),
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
                    effects: None,
                    ensures: None,
                }],
            },
            emits: None,
            listens: Some(listens),
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
        }
    };

    let listener_orbital = Orbital {
        spec: false,
        types: None,
        name: "ListenerOrbital".to_string(),
        uses: None,
        expects: None,
        theme: None,
        services: None,
        entity: EntityRef::Inline(task_entity()),
        auxiliary_entities: None,
        traits: vec![TraitReference::Inline(mk_listener_trait(vec![
            ListenDefinition {
                event: event.to_string(),
                triggers: Some("INIT".to_string()),
                scope: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
                source: Some(ListenSource::Any),
                event_id: None,
                triggers_id: None,
            },
        ]))],
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
        orbitals: vec![
            Orbital {
                spec: false,
                types: None,
                name: "TestOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(task_entity()),
                auxiliary_entities: None,
                traits: vec![
                    TraitReference::Inline(mk_plain("DeclaredEmitter", declared_fields)),
                    TraitReference::Inline(render_button_trait("RenderButton", event, "id", "id")),
                ],
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
            },
            listener_orbital,
        ],
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

/// (c) `RenderButton`'s own `emits[]` entry for `APPROVE` is empty — like
/// `ui-button`'s atom declaration — but its render-ui `actionPayload : {
/// id: @entity.id }` sends `{ id }`, typed `string!` off `Task.id`. That
/// matches `DeclaredEmitter`'s honestly-declared `APPROVE { id: string! }`
/// exactly, so no mismatch should surface. Before the fix, the empty
/// declared schema was compared literally and this fired
/// `CrossEmitterDeclaredPayloadMismatch` (the exact false positive latent
/// in `std-approval-request`, per gap #82 G6).
#[test]
fn f2_render_action_payload_matches_declared_no_mismatch() {
    let schema =
        schema_with_render_action_payload("APPROVE", vec![payload_field("id", "string", true)]);
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        !hit,
        "G6(c): a button's actionPayload-derived {{id}} must be recognised as \
         matching the declared APPROVE {{id: string!}} shape. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

/// Derivation must not blind the check to a REAL divergence: `Task` has no
/// `title` field, so `RenderButton`'s `actionPayload: { id: @entity.id }`
/// still resolves to `{ id: string! }` while `DeclaredEmitter` now declares
/// an unrelated `{ label: string! }` — genuinely different shapes, and the
/// mismatch must still fire.
#[test]
fn f2_render_action_payload_mismatch_still_detected() {
    let schema =
        schema_with_render_action_payload("APPROVE", vec![payload_field("label", "string", true)]);
    let result = run(&schema);
    let hit = result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .any(|e| matches!(e.code, ErrorCode::CrossEmitterDeclaredPayloadMismatch));
    assert!(
        hit,
        "G6: a genuine divergence between a render-derived payload and the \
         declared one must still be reported. errors={:?}",
        result.errors.iter().map(|e| &e.code).collect::<Vec<_>>()
    );
}

// ============================================================================
// F.3 — literal-union default check
// ============================================================================

fn schema_with_default(field: FieldDefinition) -> OrbitalSchema {
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
        name: "Order".to_string(),
        collection: Some("orders".to_string()),
        persistence: None,
        fields: vec![
            FieldDefinition {
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
            },
            field,
        ],
        instances: None,
        id: None,
        invariants: None,
        indexes: None,
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
            traits: vec![],
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

