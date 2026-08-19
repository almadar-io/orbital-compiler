use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{OrbitalSchema, TraitReference};
use std::collections::{HashMap, HashSet};

/// Validate that external emits with an empty payload actually reach a consumer.
///
/// An external emit with no payload only matters when another orbital in the
/// validation scope listens for that event and would receive nothing. An
/// unconsumed external emit (no listens edge, e.g. an over-scoped UI trigger in
/// a single-orbital app) is inert, so it produces no diagnostic. Consumer
/// existence is an exact event-name listens edge in the IR — no name matching,
/// no heuristics. In single-orbital scope there are no other orbitals, so the
/// check is silent by construction.
pub(crate) fn validate_external_emit_payloads(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::EventScope;

    let mut result = ValidationResult::new();

    // event name -> set of orbital names that listen for it (bus consumers).
    let mut listened_by: HashMap<String, HashSet<String>> = HashMap::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                listened_by
                    .entry(listen.event.clone())
                    .or_default()
                    .insert(orbital.name.clone());
            }
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let Some(emits) = &trait_def.emits else {
                continue;
            };
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

                    // A consumer that would receive nothing = some *other*
                    // orbital listens for this exact event name.
                    let has_consumer = listened_by
                        .get(&emit.event)
                        .map(|orbitals| orbitals.iter().any(|o| o != &orbital.name))
                        .unwrap_or(false);

                    if !has_payload && has_consumer {
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
        EmitDefinition, EntityDefinition, EntityRef, EventScope, ListenDefinition, Orbital,
        PayloadField, StateMachine, TraitDefinition, TraitReference,
    };

    fn empty_state_machine() -> StateMachine {
        StateMachine {
            states: vec![],
            events: None,
            transitions: vec![],
        }
    }

    fn trait_with(
        name: &str,
        emits: Option<Vec<EmitDefinition>>,
        listens: Option<Vec<ListenDefinition>>,
    ) -> TraitReference {
        TraitReference::Inline(TraitDefinition {
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits,
            listens,
            state_machine: empty_state_machine(),
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
        })
    }

    fn emit(event: &str, scope: EventScope, payload: Option<Vec<PayloadField>>) -> EmitDefinition {
        EmitDefinition {
            event: event.to_string(),
            scope: Some(scope),
            description: Some("desc".to_string()),
            synonyms: None,
            tier: None,
            payload_schema: payload,
            event_id: None,
        }
    }

    fn listen(event: &str) -> ListenDefinition {
        ListenDefinition {
            event: event.to_string(),
            event_id: None,
            triggers: None,
            triggers_id: None,
            scope: None,
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            source: None,
        }
    }

    fn orbital_with(name: &str, entity: &str, traits: Vec<TraitReference>) -> Orbital {
        Orbital {
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: entity.to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits,
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    fn schema_with(orbitals: Vec<Orbital>) -> OrbitalSchema {
        OrbitalSchema {
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals,
            metadata: None,
            ledger: None,
            schema_version: None,
        }
    }

    // External empty-payload emit WITH a listener in another orbital → warns.
    #[test]
    fn test_external_emit_missing_payload_with_consumer_warns() {
        let emitter = orbital_with(
            "OrderOrbital",
            "Order",
            vec![trait_with(
                "OrderTrait",
                Some(vec![emit("ORDER_PAID", EventScope::External, None)]),
                None,
            )],
        );
        let consumer = orbital_with(
            "LedgerOrbital",
            "Ledger",
            vec![trait_with("LedgerTrait", None, Some(vec![listen("ORDER_PAID")]))],
        );
        let schema = schema_with(vec![emitter, consumer]);

        let result = validate_external_emit_payloads(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventExternalMissingPayload));
    }

    // External empty-payload emit with NO listener anywhere → inert, silent.
    #[test]
    fn test_external_emit_missing_payload_no_consumer_silent() {
        let schema = schema_with(vec![orbital_with(
            "QuizOrbital",
            "Quiz",
            vec![trait_with(
                "QuizTrait",
                Some(vec![emit("NEXT", EventScope::External, None)]),
                None,
            )],
        )]);

        let result = validate_external_emit_payloads(&schema);
        assert!(result.is_valid());
        assert!(result.errors.is_empty());
    }

    // Only the emitting orbital listens (self-listen) → not a cross-orbital
    // consumer, stays silent.
    #[test]
    fn test_external_emit_self_listen_silent() {
        let schema = schema_with(vec![orbital_with(
            "QuizOrbital",
            "Quiz",
            vec![
                trait_with(
                    "QuizTrait",
                    Some(vec![emit("NEXT", EventScope::External, None)]),
                    None,
                ),
                trait_with("QuizProgress", None, Some(vec![listen("NEXT")])),
            ],
        )]);

        let result = validate_external_emit_payloads(&schema);
        assert!(result.is_valid());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_external_emit_with_payload_passes() {
        let consumer = orbital_with(
            "LedgerOrbital",
            "Ledger",
            vec![trait_with("LedgerTrait", None, Some(vec![listen("ORDER_PAID")]))],
        );
        let emitter = orbital_with(
            "OrderOrbital",
            "Order",
            vec![trait_with(
                "OrderTrait",
                Some(vec![emit(
                    "ORDER_PAID",
                    EventScope::External,
                    Some(vec![PayloadField {
                        name: "orderId".to_string(),
                        field_type: "string".to_string(),
                        required: Some(true),
                        description: Some("The order ID".to_string()),
                        properties: None,
                    }]),
                )]),
                None,
            )],
        );
        let schema = schema_with(vec![emitter, consumer]);

        let result = validate_external_emit_payloads(&schema);
        assert!(result.is_valid());
    }

    #[test]
    fn test_internal_emit_no_payload_ok() {
        let schema = schema_with(vec![orbital_with(
            "TaskOrbital",
            "Task",
            vec![trait_with(
                "OrderTrait",
                Some(vec![emit("STATUS_CHANGED", EventScope::Internal, None)]),
                None,
            )],
        )]);

        let result = validate_external_emit_payloads(&schema);
        assert!(result.is_valid());
    }
}
