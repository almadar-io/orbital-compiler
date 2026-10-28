//! Trait Validation
//!
//! Validates the linked entity of trait references and inline trait definitions.
//! Ported from `builder/packages/shared/src/orbitals/validation/trait-validator.ts`.

use super::{trait_path, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, TraitDefinition, TraitReference};
use std::collections::HashSet;

/// Validate all traits in a schema
pub fn validate_all_traits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let path = trait_path(i, &orbital.name, j, trait_ref);
            let result = validate_trait_reference(trait_ref, &path, &ctx.entity_names);
            ctx.merge(result);
        }
    }
}

/// Validate a trait reference (inline or external)
pub fn validate_trait_reference(
    trait_ref: &TraitReference,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match trait_ref {
        TraitReference::Inline(trait_def) => {
            result.merge(validate_trait_definition(trait_def, path, known_entities));
        }
        TraitReference::Reference { linked_entity, .. } => {
            if let Some(linked) = linked_entity {
                if !known_entities.contains(linked) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EntityInvalidRelation,
                            format!("{}.linkedEntity", path),
                            format!("Linked entity '{}' does not exist", linked),
                        )
                        .with_suggestion("Ensure the linked entity is defined in the schema"),
                    );
                }
            }
        }
    }

    result
}

/// Validate a full trait definition
pub fn validate_trait_definition(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let Some(linked) = &trait_def.linked_entity {
        if !known_entities.contains(linked) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidRelation,
                    format!("{}.linkedEntity", path),
                    format!("Linked entity '{}' does not exist", linked),
                )
                .with_suggestion("Ensure the linked entity is defined in the schema"),
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
    };

    fn create_valid_state_machine() -> StateMachine {
        StateMachine {
            states: vec![
                StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                    holds: None,
                },
                StateDefinition {
                    name: "active".to_string(),
                    is_initial: None,
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                    holds: None,
                },
            ],
            events: Some(vec![
                EventDefinition {
                    id: None,
                    key: "INIT".to_string(),
                    name: Some("Initialize".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    input: None,
                    listened: None,
                    inbound: None,
                },
                EventDefinition {
                    id: None,
                    key: "START".to_string(),
                    name: Some("Start".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    input: None,
                    listened: None,
                    inbound: None,
                },
            ]),
            transitions: vec![
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![orbital_core_base::schema::SExpression::Array(vec![
                        orbital_core_base::schema::SExpression::String(("render-ui".to_string()).into()),
                        orbital_core_base::schema::SExpression::String(("main".to_string()).into()),
                        orbital_core_base::schema::SExpression::object({
                            let mut m = orbital_core_base::schema::SObject::new();
                            m.insert(
                                "type".to_string(),
                                orbital_core_base::schema::SExpression::String(("box".to_string()).into()),
                            );
                            m
                        }),
                    ])]),
                    ensures: None,
                },
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "active".to_string(),
                    event: "START".to_string(),
                    guard: None,
                    effects: None,
                    ensures: None,
                },
            ],
        }
    }

    fn create_valid_trait() -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "TestTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: create_valid_state_machine(),
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

    #[test]
    fn test_valid_trait() {
        let trait_def = create_valid_trait();
        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        // May have warnings but no errors
        assert!(
            result.errors.iter().all(|e| e.is_warning()),
            "Unexpected errors: {:?}",
            result.errors
        );
    }
}
