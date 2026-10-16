use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{ListenSource, OrbitalSchema, TraitReference};
use std::collections::{HashMap, HashSet};

/// Validate that external emits with an empty payload actually reach a consumer.
///
/// An external emit with no payload only matters when another orbital in the
/// validation scope listens for that event and would receive nothing. An
/// unconsumed external emit (no listens edge, e.g. an over-scoped UI trigger in
/// a single-orbital app) is inert, so it produces no diagnostic.
///
/// Consumer existence is a listens edge WIRED TO THIS EMITTER — matched on the
/// listen's recorded `source` trait, not on the event name alone. The previous
/// name-only lookup was itself the name matching this comment disclaimed: it
/// credited every emitter of a shared name with a consumer that only one of
/// them was connected to. A `source: Any` (or absent) listen still matches by
/// name, because such a listen really does consume from any emitter.
///
/// In single-orbital scope there are no other orbitals, so the check is silent
/// by construction.
pub(crate) fn validate_external_emit_payloads(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::EventScope;

    let mut result = ValidationResult::new();

    // A consumer is one WIRED TO THIS EMITTER, not merely anything listening to
    // the same event name. Keying on the name alone is a string join: in
    // std-subscription-billing 25 traits emit `DISMISS` and 4 are wired, so the
    // other 21 — unrelated alerts that merely share the word — inherited a
    // consumer they were never connected to and were reported as defective.
    // `ListenDefinition.source` already records the emitter, and every listen in
    // the corpus carries it (3,027 of 3,027), so match on it.
    //
    // `Any`/absent keeps the old name-only behaviour: such a listen genuinely
    // does consume from any emitter of that name, so narrowing it would hide a
    // real finding.
    let mut wildcard_listeners: HashMap<String, HashSet<String>> = HashMap::new();
    let mut targeted_listeners: HashMap<(String, String), HashSet<String>> = HashMap::new();
    // Subsets of the two above: listeners that actually EXPECT data, i.e. whose
    // triggered event declares at least one REQUIRED field. A listener
    // expecting nothing cannot "receive nothing".
    let mut expectant_wildcard: HashSet<String> = HashSet::new();
    let mut expectant_targeted: HashSet<(String, String)> = HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                // The internal event this listen routes to, whose declared
                // payload IS the consumer's expectation.
                let triggered = listen.triggers.as_deref().unwrap_or(listen.event.as_str());
                let expects_fields = td
                    .state_machine
                    .events
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .find(|e| e.key == triggered)
                    .and_then(|e| e.payload_schema.as_ref())
                    // REQUIRED fields, not merely declared ones. A consumer
                    // declaring `id : string` (no `!`) has said in the type
                    // system that it tolerates the field's absence — treating
                    // that as an expectation contradicts the declaration.
                    // `std-realtime-chat`'s create modal declares exactly this:
                    // optional `id`/`row` it never needs.
                    .map(|p| p.iter().any(|f| f.required == Some(true)))
                    .unwrap_or(false);
                match &listen.source {
                    Some(ListenSource::Trait { trait_name, .. })
                    | Some(ListenSource::Orbital { trait_name, .. }) => {
                        targeted_listeners
                            .entry((trait_name.clone(), listen.event.clone()))
                            .or_default()
                            .insert(orbital.name.clone());
                        if expects_fields {
                            expectant_targeted.insert((trait_name.clone(), listen.event.clone()));
                        }
                    }
                    Some(ListenSource::Any) | None => {
                        wildcard_listeners
                            .entry(listen.event.clone())
                            .or_default()
                            .insert(orbital.name.clone());
                        if expects_fields {
                            expectant_wildcard.insert(listen.event.clone());
                        }
                    }
                }
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
                    // orbital is wired to THIS emitter for this event.
                    let listens_to_this_emitter = targeted_listeners
                        .get(&(trait_def.name.clone(), emit.event.clone()))
                        .map(|orbitals| orbitals.iter().any(|o| o != &orbital.name))
                        .unwrap_or(false);
                    let listens_to_any_emitter = wildcard_listeners
                        .get(&emit.event)
                        .map(|orbitals| orbitals.iter().any(|o| o != &orbital.name))
                        .unwrap_or(false);
                    let has_consumer = listens_to_this_emitter || listens_to_any_emitter;

                    // …and one that would RECEIVE NOTHING, which is the other
                    // half of this check's own premise and was never
                    // implemented. A consumer only loses something if it
                    // expects fields: if every listener's triggered event
                    // declares an empty payload, an empty emit satisfies all of
                    // them and there is nothing to report.
                    //
                    // The case this exists for is a payload-free cross-orbital
                    // INTENT event — `CREATE_CHANNEL` fired by a "New Channel"
                    // button (whose `ui-button` emit is honestly `-> external
                    // { }`) and consumed by a `mode: create` modal, which has no
                    // record to identify and declares no required field. Demanding
                    // a payload there forced the emitter to declare an `id` it
                    // never sends, which is the lie the sibling
                    // ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH then reports.
                    // The two rules cannot both be satisfied by a correct
                    // program, so this one yields.
                    let consumer_expects_fields = expectant_targeted
                        .contains(&(trait_def.name.clone(), emit.event.clone()))
                        || expectant_wildcard.contains(&emit.event);

                    if !has_payload && has_consumer && consumer_expects_fields {
                        result.add_error(
                            ValidationError::error(
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
            local: false,
            effect_row: None,
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
            definer_knob: None,
            scope_overridden: None,
            event: event.to_string(),
            scope: Some(scope),
            description: Some("desc".to_string()),
            synonyms: None,
            tier: None,
            payload_schema: payload,
            event_id: None,
            payload_entity: None,
        }
    }

    /// `trait_with`, plus the declared payload of the event the listen routes
    /// to — i.e. what the consumer EXPECTS. Without this a fixture models a
    /// listener that wants nothing, which no longer counts as "would receive
    /// nothing" (see the check's `consumer_expects_fields` arm).
    fn consumer_trait_expecting(
        name: &str,
        listens: Vec<ListenDefinition>,
        event: &str,
        field: &str,
    ) -> TraitReference {
        let mut tr = trait_with(name, None, Some(listens));
        if let TraitReference::Inline(def) = &mut tr {
            def.state_machine.events = Some(vec![orbital_core::schema::EventDefinition {
                key: event.to_string(),
                id: None,
                name: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_schema: Some(vec![orbital_core::schema::PayloadField {
                    values: None,
                    name: field.to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                    projected_from: None,
                }]),
                payload_entity: None,
                external: None,
                listened: None,
            }]);
        }
        tr
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
            types: None,
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
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
            reference: None,
            config: None,
        }
    }

    fn schema_with(orbitals: Vec<Orbital>) -> OrbitalSchema {
        OrbitalSchema {
            theme_definitions: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals,
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    os_grants: None,
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
            vec![consumer_trait_expecting(
                "LedgerTrait",
                vec![listen("ORDER_PAID")],
                "ORDER_PAID",
                "orderId",
            )],
        );
        let schema = schema_with(vec![emitter, consumer]);

        let result = validate_external_emit_payloads(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventExternalMissingPayload));
    }

    /// A payload-free cross-orbital INTENT event is legal and must stay silent:
    /// a "New Channel" button honestly emits `-> external { }`, and the
    /// `mode: create` modal that consumes it has no record to identify, so its
    /// triggered event declares no fields. Nothing is lost, so there is nothing
    /// to report — the check's premise is a consumer that would RECEIVE
    /// NOTHING, and a consumer expecting nothing cannot.
    ///
    /// Demanding a payload here forced the emitter to declare an `id` it never
    /// sends, which the sibling cross-emitter-consensus check then reported as a
    /// conflict — two rules no correct program could satisfy at once.
    #[test]
    fn payload_free_intent_event_with_an_expectationless_consumer_is_silent() {
        let emitter = orbital_with(
            "ChatOrbital",
            "ChatMessage",
            vec![trait_with(
                "ChatRoom",
                Some(vec![emit("CREATE_CHANNEL", EventScope::External, None)]),
                None,
            )],
        );
        // Consumer listens, but declares no payload for the triggered event.
        let consumer = orbital_with(
            "ChannelOrbital",
            "Channel",
            vec![trait_with(
                "ChannelCreate",
                None,
                Some(vec![listen_from("CREATE_CHANNEL", "ChatRoom")]),
            )],
        );
        let result = validate_external_emit_payloads(&schema_with(vec![emitter, consumer]));
        assert!(
            result.errors.is_empty() && result.warnings.is_empty(),
            "an expectationless consumer loses nothing: {:?} {:?}",
            result.errors,
            result.warnings
        );
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
            vec![trait_with(
                "LedgerTrait",
                None,
                Some(vec![listen("ORDER_PAID")]),
            )],
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
                        values: None,
                        name: "orderId".to_string(),
                        field_type: "string".to_string(),
                        required: Some(true),
                        description: Some("The order ID".to_string()),
                        properties: None,
                        type_when: None,
                        entity: None,
                        projected_from: None,
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

    fn listen_from(event: &str, source_trait: &str) -> ListenDefinition {
        ListenDefinition {
            source: Some(ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
            ..listen(event)
        }
    }

    /// A listener wired to a DIFFERENT emitter of the same event name is not a
    /// consumer of this one. Two unrelated traits sharing the word `DISMISS`
    /// used to credit each other with each other's consumers — the corpus case
    /// was 25 emitters of `DISMISS` in one schema, 4 wired, 21 reported as
    /// defective purely for sharing the string.
    #[test]
    fn test_targeted_listener_on_another_emitter_is_not_a_consumer() {
        let schema = schema_with(vec![
            orbital_with(
                "AlertOrbital",
                "Alert",
                vec![trait_with(
                    "InlineAlertRender7",
                    Some(vec![emit("DISMISS", EventScope::External, None)]),
                    None,
                )],
            ),
            orbital_with(
                "InvoiceOrbital",
                "Invoice",
                vec![trait_with(
                    "InvoiceConfirmListener",
                    None,
                    Some(vec![listen_from("DISMISS", "InvoiceActionConfirm")]),
                )],
            ),
        ]);

        let result = validate_external_emit_payloads(&schema);
        assert!(
            result.is_valid(),
            "a listener sourced from InvoiceActionConfirm must not credit InlineAlertRender7"
        );
    }

    /// Positive control for the narrowing above: when the listener names THIS
    /// emitter, the payload check must still fire. Without this, the fix could
    /// silence the diagnostic entirely and the corpus would look clean for the
    /// wrong reason.
    #[test]
    fn test_targeted_listener_on_this_emitter_still_warns() {
        let schema = schema_with(vec![
            orbital_with(
                "AlertOrbital",
                "Alert",
                vec![trait_with(
                    "InlineAlertRender7",
                    Some(vec![emit("DISMISS", EventScope::External, None)]),
                    None,
                )],
            ),
            orbital_with(
                "InvoiceOrbital",
                "Invoice",
                vec![consumer_trait_expecting(
                    "InvoiceConfirmListener",
                    vec![listen_from("DISMISS", "InlineAlertRender7")],
                    "DISMISS",
                    "id",
                )],
            ),
        ]);

        let result = validate_external_emit_payloads(&schema);
        assert!(
            !result.is_valid(),
            "a listener wired to THIS emitter must still require a payload"
        );
    }
}
