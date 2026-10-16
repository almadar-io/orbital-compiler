//! `notify` is `.lolo`-only sugar for `render-ui toast` (owner ruling
//! 2026-09-11), desugared away at lowering — the `.orb` IR never carries a
//! `notify` effect kind. A `notify` tuple that still reaches L2 (a
//! hand-authored `.orb`, or a registry not yet re-emitted) must fail
//! validation with `ORB_EFF_NOTIFY_RETIRED`, never panic the effect parser.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core::schema::{
    EntityDefinition, EntityRef, FieldDefinition, FieldType, Orbital, OrbitalSchema, SExpression,
    StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
    TransitionDefinition,
};

fn schema_with_effects(effects: Vec<SExpression>) -> OrbitalSchema {
    let entity = EntityDefinition {
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
        name: "Toast".to_string(),
        collection: Some("toasts".to_string()),
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
    };

    let trait_def = TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "Notifier".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        local: false,
        category: None,
        capabilities: vec![],
        linked_entity: Some("Toast".to_string()),
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
    };

    OrbitalSchema {
        theme_definitions: None,
        site: None,
        public_dir: None,
        locales: None,
        lazy_pages: None,
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
            traits: vec![TraitReference::Inline(trait_def)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
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
    os_grants: None,
    }
}

fn notify_tuple(level: &str, message: &str) -> SExpression {
    SExpression::Array(vec![
        SExpression::String("notify".to_string()),
        SExpression::String(level.to_string()),
        SExpression::String(message.to_string()),
    ])
}

#[test]
fn a_stale_notify_tuple_fails_validation_with_the_retirement_code() {
    let schema = schema_with_effects(vec![notify_tuple("error", "Saving failed")]);
    let result = validate_comprehensive_without_services(&schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNotifyRetired),
        "expected ORB_EFF_NOTIFY_RETIRED, got: {:?}",
        result.errors
    );
}

#[test]
fn the_retirement_code_carries_the_orb_string() {
    assert_eq!(
        ErrorCode::EffectNotifyRetired.as_str(),
        "ORB_EFF_NOTIFY_RETIRED"
    );
    assert_eq!(ErrorCode::EffectNotifyRetired.category(), "effect");
}

#[test]
fn a_render_ui_toast_effect_carries_no_retirement_error() {
    let toast = SExpression::Array(vec![
        SExpression::String("render-ui".to_string()),
        SExpression::String("toast".to_string()),
        SExpression::Object(
            [
                (
                    "type".to_string(),
                    SExpression::String("alert".to_string()),
                ),
                (
                    "variant".to_string(),
                    SExpression::String("error".to_string()),
                ),
                (
                    "message".to_string(),
                    SExpression::String("Saving failed".to_string()),
                ),
                ("dismissible".to_string(), SExpression::Bool(true)),
            ]
            .into_iter()
            .collect(),
        ),
    ]);
    let schema = schema_with_effects(vec![toast]);
    let result = validate_comprehensive_without_services(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNotifyRetired),
        "a render-ui toast must never trip the notify-retired check: {:?}",
        result.errors
    );
}
