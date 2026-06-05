//! Effect Validation
//!
//! Validates effects: call_service, set, fetch, emit, persist, etc.
//! Ported from `builder/packages/shared/src/orbitals/validation/effect-validator.ts`.

use super::{ErrorCode, PageInitPayload, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, FieldType, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

mod call_service;
mod emit;
mod emit_declaration;
mod fetch;
mod navigate;
mod os_watch;
mod persist;
mod r#ref;
mod set;
mod utils;

pub(crate) use call_service::{validate_call_service, validate_call_service_object};
pub(crate) use emit::{extract_emit_object_ref, validate_emit, validate_emit_config};
pub(crate) use emit_declaration::{
    collect_declared_emits, validate_effect, validate_emit_against_declaration,
    validate_required_emit_success,
};
pub(crate) use fetch::{validate_fetch, validate_fetch_include, validate_fetch_object};
pub(crate) use navigate::{
    extract_static_path_prefix, find_matching_page_path, path_matches_pattern, validate_navigate,
};
pub(crate) use os_watch::validate_os_watch;
pub(crate) use persist::validate_persist;
pub(crate) use r#ref::validate_ref;
pub(crate) use set::{is_path_expression, validate_set, validate_set_object};
pub(crate) use utils::{extract_payload_fields, is_upper_snake_case, is_valid_event_identifier};

/// Known effect operators
pub const EFFECT_OPERATORS: &[&str] = &[
    "set",
    "emit",
    "fetch", // Runtime effect (EffectExecutor handles it)
    "persist",
    "delete", // Note: typically a persist sub-action, kept for compat
    "navigate",
    "render-ui",
    "render_ui",
    "call-service",
    "call_service",
    "notify",
    "log", // Runtime effect (EffectExecutor handles it)
    "spawn",
    "destroy", // Legacy alias — canonical name is "despawn"
    "despawn", // Canonical (GAP-E001): used by TS evaluator + runtime
    "update",  // Note: typically a persist sub-action, kept for compat
    // Conditional and array mutation effects
    "if",            // Conditional effect execution
    "array/append",  // Append item to array field
    "array/prepend", // Prepend item to array field
    "array/remove",  // Remove item from array field
    "increment",     // Increment numeric field
    "decrement",     // Decrement numeric field
    // Functional array operators (registered in almadar-operators/operators.json)
    "array/filter", // Filter elements by predicate
    "array/map",    // Transform elements
    "array/reduce", // Reduce array to single value
    // GAP-V002: Control-flow wrappers that can contain effects
    "do",   // Sequential effect execution
    "let",  // Local variable binding in effects
    "when", // Conditional effect (like if but for effects)
    "fn",   // Lambda wrapping effects
    // OS trigger operators
    "os/watch-files",
    "os/watch-process",
    "os/watch-port",
    "os/watch-http",
    "os/watch-cron",
    "os/watch-signal",
    "os/watch-env",
    "os/debounce",
    // Resource operators (ref/deref/swap!/watch/atomic)
    "ref",    // Reactive entity subscription
    "deref",  // Snapshot read
    "swap!",  // Atomic CAS read-modify-write
    "watch",  // Entity change callback
    "atomic", // Transaction group
    // ML effect operators (Gap 4: forward/train/evaluate/checkpoint)
    "forward",
    "train",
    "evaluate",
    "checkpoint/save",
    "checkpoint/load",
    // Agent operators (Phase 1: agent/* namespace)
    "agent/memorize",
    "agent/forget",
    "agent/pin",
    "agent/reinforce",
    "agent/decay",
    "agent/generate",
    "agent/switch-provider",
    "agent/invoke",
    "agent/compact",
    "agent/fork",
    "agent/label",
    "agent/search-code",
    // Agent pure operators (valid in effects too, return values)
    "agent/recall",
    "agent/memories",
    "agent/memory-strength",
    "agent/is-pinned",
    "agent/provider",
    "agent/model",
    "agent/tools",
    "agent/token-count",
    "agent/context-usage",
    "agent/session-id",
];

/// Validate all effects in a schema
pub fn validate_all_effects(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate effects in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_trait_effects(
                    trait_def,
                    &path,
                    &ctx.entity_names,
                    &ctx.entity_definitions,
                    &ctx.service_names,
                    &ctx.page_init_payloads,
                    &ctx.service_actions,
                );
                ctx.merge(result);
            }
        }
    }
}

/// Validate all effects in a trait definition
pub fn validate_trait_effects(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_actions: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect declared emits for this trait
    let declared_emits = collect_declared_emits(trait_def);

    // Validate effects in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_actions,
                    Some(&declared_emits),
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    // Validate effects in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_actions,
                    Some(&declared_emits),
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    result
}

/// Information about a declared emit
#[derive(Clone)]
pub(crate) struct DeclaredEmit {
    pub(crate) event: String,
    pub(crate) is_external: bool,
    pub(crate) payload_fields: Vec<String>,
    pub(crate) required_fields: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_set_effect() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.title".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_set_effect_payload_not_allowed() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@payload.title".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_set_effect_missing_path() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingSetTargetMissingPath));
    }

    #[test]
    fn test_fetch_effect_invalid_entity() {
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("NonExistent".to_string()),
        ]);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectFetchInvalidEntity));
    }

    #[test]
    fn test_call_service_missing_service() {
        let effect = SExpression::Array(vec![
            SExpression::String("call-service".to_string()),
            SExpression::String("unknownService".to_string()),
            SExpression::String("action".to_string()),
        ]);

        let known_services: HashSet<String> = ["api".to_string()].into_iter().collect();
        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &known_services,
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectCallServiceMissingService));
    }

    #[test]
    fn test_navigate_missing_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate without payload
        let effect = SExpression::Array(vec![
            SExpression::String("navigate".to_string()),
            SExpression::String("/inspection/123".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNavigateMissingPayload));
    }

    #[test]
    fn test_navigate_with_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate with payload
        let effect = SExpression::Array(vec![
            SExpression::String("navigate".to_string()),
            SExpression::String("/inspection/123".to_string()),
            SExpression::Object(
                vec![(
                    "id".to_string(),
                    SExpression::String("@entity.id".to_string()),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &HashMap::new(),
            None,
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("/inspection/123", "/inspection/:id"));
        assert!(path_matches_pattern(
            "/users/42/posts/7",
            "/users/:userId/posts/:postId"
        ));
        assert!(path_matches_pattern("/about", "/about"));
        assert!(!path_matches_pattern("/about", "/contact"));
        assert!(!path_matches_pattern("/inspection", "/inspection/:id"));
    }

    #[test]
    fn test_is_upper_snake_case() {
        assert!(is_upper_snake_case("TASK_CREATED"));
        assert!(is_upper_snake_case("START"));
        assert!(is_upper_snake_case("ITEM_1_SELECTED"));
        assert!(!is_upper_snake_case("taskCreated"));
        assert!(!is_upper_snake_case("task_created"));
        assert!(!is_upper_snake_case("_INVALID"));
        assert!(!is_upper_snake_case("INVALID_"));
    }

    #[test]
    fn test_emit_declared_passes() {
        use orbital_core::schema::{EmitDefinition, EventScope, PayloadField};

        let declared_emits = vec![DeclaredEmit {
            event: "TASK_CREATED".to_string(),
            is_external: true,
            payload_fields: vec!["taskId".to_string(), "title".to_string()],
            required_fields: vec!["taskId".to_string()],
        }];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("TASK_CREATED".to_string()),
            SExpression::Object(
                vec![("taskId".to_string(), SExpression::String("123".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_emit_undeclared_warns() {
        let declared_emits: Vec<DeclaredEmit> = vec![];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("UNKNOWN_EVENT".to_string()),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventUndeclared));
    }

    #[test]
    fn test_emit_missing_required_payload_field() {
        let declared_emits = vec![DeclaredEmit {
            event: "ORDER_CONFIRMED".to_string(),
            is_external: true,
            payload_fields: vec![
                "orderId".to_string(),
                "customerId".to_string(),
                "total".to_string(),
            ],
            required_fields: vec!["orderId".to_string(), "customerId".to_string()],
        }];

        // Only provide orderId, missing customerId
        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("ORDER_CONFIRMED".to_string()),
            SExpression::Object(
                vec![(
                    "orderId".to_string(),
                    SExpression::String("123".to_string()),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalPayloadMismatch));
    }

    #[test]
    fn test_internal_emit_no_payload_required() {
        let declared_emits = vec![DeclaredEmit {
            event: "STATUS_CHANGED".to_string(),
            is_external: false, // Internal event
            payload_fields: vec![],
            required_fields: vec![],
        }];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("STATUS_CHANGED".to_string()),
            // No payload needed for internal events
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_fetch_include_valid_relation() {
        use orbital_core::schema::{FieldDefinition, RelationDefinition};

        // Create an entity with a relation field
        let mut entity_defs: HashMap<String, EntityDefinition> = HashMap::new();
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                collection: None,
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
                    FieldDefinition {
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: Some(true),
                        primary_key: None,
                        unique: None,
                        default: None,
                        values: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                        }),
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                ],
                instances: None,
            },
        );

        let known_entities: HashSet<String> = ["Inspection".to_string()].into_iter().collect();
        // Declare the success event so the `emit.success` requirement
        // (added across emit-type ops in commit 222109fa3) is satisfied.
        let declared_emits = vec![DeclaredEmit {
            event: "InspectionLoaded".to_string(),
            is_external: false,
            payload_fields: vec![],
            required_fields: vec![],
        }];

        // Fetch with valid include + required emit.success
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("Inspection".to_string()),
            SExpression::Object(
                vec![
                    (
                        "include".to_string(),
                        SExpression::Array(vec![SExpression::String("company".to_string())]),
                    ),
                    (
                        "emit".to_string(),
                        SExpression::Object(
                            vec![(
                                "success".to_string(),
                                SExpression::String("InspectionLoaded".to_string()),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &entity_defs,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            Some(&declared_emits),
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_fetch_include_invalid_relation() {
        use orbital_core::schema::{FieldDefinition, RelationDefinition};

        // Create an entity with a relation field
        let mut entity_defs: HashMap<String, EntityDefinition> = HashMap::new();
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                collection: None,
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
                    FieldDefinition {
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: Some(true),
                        primary_key: None,
                        unique: None,
                        default: None,
                        values: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                        }),
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                ],
                instances: None,
            },
        );

        let known_entities: HashSet<String> = ["Inspection".to_string()].into_iter().collect();

        // Fetch with invalid include (nonExistent is not a relation)
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("Inspection".to_string()),
            SExpression::Object(
                vec![(
                    "include".to_string(),
                    SExpression::Array(vec![SExpression::String("nonExistent".to_string())]),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &entity_defs,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectFetchInvalidInclude));
    }
}
