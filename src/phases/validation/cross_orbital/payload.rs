use super::collect_all_traits;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::OrbitalSchema;

/// Validate that external emits have payload definitions
pub(crate) fn validate_external_emit_payloads(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::EventScope;

    let mut result = ValidationResult::new();

    let all_traits = collect_all_traits(schema);
    for trait_def in &all_traits {
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                // Check if scope is external
                let is_external = emit
                    .scope
                    .as_ref()
                    .map(|s| *s == EventScope::External)
                    .unwrap_or(false);

                if is_external {
                    // External emits MUST have payload with at least one field
                    let has_payload = emit
                        .payload_schema
                        .as_ref()
                        .map(|p| !p.is_empty())
                        .unwrap_or(false);

                    if !has_payload {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::TraitEventExternalMissingPayload,
                                format!("traits.{}.emits.{}", trait_def.name, emit.event),
                                format!(
                                    "External event '{}' in trait '{}' should have a payload definition with typed fields",
                                    emit.event, trait_def.name
                                ),
                            )
                            .with_suggestion(
                                "Add a payload array with fields: { \"name\": \"fieldName\", \"type\": \"string\", \"required\": true, \"description\": \"...\" }"
                            )
                            .with_expected_shape(r#"{
  "event": "EVENT_NAME",
  "scope": "external",
  "description": "What this event means",
  "payload": [
    { "name": "entityId", "type": "string", "required": true, "description": "ID of the entity" }
  ]
}"#),
                        );
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EmitDefinition, EntityDefinition, EntityRef, EventScope, Orbital, PayloadField,
        StateMachine, TraitDefinition, TraitReference,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
                uses: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
            }],
            metadata: None,
        }
    }

    #[test]
    fn test_external_emit_missing_payload() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit but no payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None, // Missing payload!
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventExternalMissingPayload));
    }

    #[test]
    fn test_external_emit_with_payload_passes() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit WITH payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: Some(vec![PayloadField {
                    name: "orderId".to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: Some("The order ID".to_string()),
                    properties: None,
                }]),
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
    }

    #[test]
    fn test_internal_emit_no_payload_ok() {
        let mut schema = create_minimal_schema();

        // Internal emits don't require payloads - add as inline trait
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "STATUS_CHANGED".to_string(),
                scope: Some(EventScope::Internal),
                description: Some("Internal status tracking".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None,
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
    }
}
