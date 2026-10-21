//! ORB_LISTEN_ROUTE_STARVES_REQUIRED_FIELD (error).
//!
//! A `listens { Src.EVENT -> TRIGGER }` route delivers whatever the source
//! trait's declared emit contract produces (pass-through), or whatever an
//! explicit `with { ... }` mapping constructs instead of it (a mapping
//! REPLACES the delivered payload — it does not merge with the source's
//! fields, the same "replace, don't merge" semantics `rebind`'s listens
//! rewrite already relies on). Either way, if the LISTENING trait's TRIGGER
//! declares a required field the route can never actually supply, the
//! consumer's transition reads it as `undefined` at runtime — a live wire
//! whose payload silently arrives empty.
//!
//! Declared-contract comparison only: payload schemas + `payload_mapping`
//! keys + the listen's recorded `source`. No inference from effect bodies,
//! no name-matching across traits beyond that recorded source — the same
//! (orbital, trait) resolution `listens.rs`'s integrity check already uses
//! for `ListenSource::Trait`/`ListenSource::Orbital`.

use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{
    ListenSource, OrbitalSchema, PayloadField, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Required LEAF field paths in a payload contract (dotted, e.g.
/// `data.content`). An entity-typed field (`PayloadField.entity`) is one
/// leaf even though it flattens into `properties` — its value is a single
/// entity instance, not a struct the route assembles field-by-field.
///
/// Each leaf carries its OPTIONAL ancestor paths: a required field inside an
/// optional container is owed only when the route delivers that container.
fn required_leaf_paths(
    fields: &[PayloadField],
    prefix: &str,
    optional_ancestors: &[String],
    out: &mut Vec<(String, Vec<String>)>,
) {
    for field in fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        let descends =
            field.entity.is_none() && field.properties.as_ref().is_some_and(|p| !p.is_empty());
        if descends {
            let mut ancestors = optional_ancestors.to_vec();
            if field.required != Some(true) {
                ancestors.push(path.clone());
            }
            required_leaf_paths(field.properties.as_ref().unwrap(), &path, &ancestors, out);
        } else if field.required == Some(true) {
            out.push((path, optional_ancestors.to_vec()));
        }
    }
}

/// True when `fields` declares a field at the exact dotted `segments` path
/// (existence only — required-ness on the declaring side is irrelevant,
/// the question is only whether the route's source ever PRODUCES it).
fn declares_path(fields: &[PayloadField], segments: &[&str]) -> bool {
    let Some((head, rest)) = segments.split_first() else {
        return true;
    };
    let Some(field) = fields.iter().find(|f| f.name == *head) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    // A declared struct (a `type` alias lowers with an `entity` marker AND its
    // properties) produces the nested fields it lists.
    let is_leaf = field.properties.as_ref().is_none_or(|p| p.is_empty());
    if is_leaf {
        false
    } else {
        declares_path(field.properties.as_ref().unwrap(), rest)
    }
}

/// True when a LITERAL object value declares a field at the exact dotted
/// `segments` path (existence only, same shape as `declares_path` but over
/// authored data rather than a declared `PayloadField` schema).
fn literal_declares_path(value: &SExpression, segments: &[&str]) -> bool {
    let Some((head, rest)) = segments.split_first() else {
        return true;
    };
    let SExpression::Object(map) = value else {
        return false;
    };
    let Some(v) = map.get(*head) else {
        return false;
    };
    if rest.is_empty() {
        true
    } else {
        literal_declares_path(v, rest)
    }
}

/// A source emit fired via a DEFINER KNOB (`emits { @config.action -> ... }`,
/// `EmitDefinition.definer_knob = Some("action")`) carries no `payloadSchema`
/// of its own — the atom's real payload contract lives in a COMPANION config
/// field declared `@payload(action)` (dependent on the same knob), whose
/// call-site literal `default` is the actual declared shape once composed
/// (e.g. `ui-button`'s `action`/`actionPayload` pair). `resolve_sigil_types`
/// stamps `payload_for` on that companion at the moment it resolves the
/// sentinel, so this reads back declared data only — no inference from
/// effect bodies, no name joins beyond the recorded `payload_for` link.
/// A non-literal (bound/computed) default declares nothing: only an object
/// LITERAL is data the validator can read without evaluating it.
fn definer_knob_companion_declares_path(
    source_td: &TraitDefinition,
    knob: &str,
    segments: &[&str],
) -> bool {
    let Some(config) = &source_td.config else {
        return false;
    };
    config.values().any(|field| {
        field.payload_for.as_deref() == Some(knob)
            && field.default.as_ref().is_some_and(|d| {
                matches!(d, SExpression::Object(_)) && literal_declares_path(d, segments)
            })
    })
}

pub(crate) fn validate_listen_route_starves_required_field(
    schema: &OrbitalSchema,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // (orbital, trait) -> its TraitDefinition, for direct source-contract
    // lookup. Mirrors `listens.rs`'s (orbital, trait) keying of the
    // resolvable scope.
    let mut traits_by_key: HashMap<(String, String), &TraitDefinition> = HashMap::new();
    let orbitals_in_schema: HashSet<String> =
        schema.orbitals.iter().map(|o| o.name.clone()).collect();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(td) = trait_ref {
                traits_by_key.insert((orbital.name.clone(), td.name.clone()), td);
            }
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital, source_trait) = match &listen.source {
                    Some(ListenSource::Trait { trait_name, .. }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                        ..
                    }) => (o.clone(), trait_name.clone()),
                    // No recorded trait source: nothing to compare a
                    // contract against.
                    Some(ListenSource::Any) | None => continue,
                };
                // Out of this validation scope (single-orbital run, sibling
                // not loaded) — unresolvable here, not this rule's finding.
                if !orbitals_in_schema.contains(&source_orbital) {
                    continue;
                }
                let Some(source_td) = traits_by_key.get(&(source_orbital, source_trait.clone()))
                else {
                    continue; // unresolved source trait — listens.rs's territory
                };
                let Some(source_emit) = source_td
                    .emits
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .find(|e| e.event == listen.event)
                else {
                    continue; // source declares no emit contract for this event — payload.rs's territory
                };

                let triggered = listen.triggers.as_deref().unwrap_or(listen.event.as_str());
                let Some(trigger_payload) = td
                    .state_machine
                    .events
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .find(|e| e.key == triggered)
                    .and_then(|e| e.payload_schema.as_ref())
                else {
                    continue; // listening trigger declares no payload schema
                };

                let mut required = Vec::new();
                required_leaf_paths(trigger_payload, "", &[], &mut required);

                let produced = |path: &str| -> bool {
                    let top_key = path.split('.').next().unwrap();
                    let segments: Vec<&str> = path.split('.').collect();
                    match &listen.payload_mapping {
                        // An explicit `with { }` REPLACES the delivered
                        // payload outright — only its own top-level keys
                        // land, regardless of what the source emits.
                        Some(mapping) => mapping.contains_key(top_key),
                        // Pass-through: either the source's declared emit
                        // contract IS what arrives, or — when the emit fires
                        // via a definer knob and so carries no payload
                        // schema of its own — its declared `@payload(<knob>)`
                        // companion config field's literal default is.
                        None => {
                            declares_path(
                                source_emit.payload_schema.as_deref().unwrap_or(&[]),
                                &segments,
                            ) || source_emit.definer_knob.as_deref().is_some_and(|knob| {
                                definer_knob_companion_declares_path(source_td, knob, &segments)
                            })
                        }
                    }
                };
                for (path, optional_ancestors) in &required {
                    let top_key = path.split('.').next().unwrap();
                    let owed = optional_ancestors.iter().all(|a| produced(a));
                    if owed && !produced(path) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::ListenRouteStarvesRequiredField,
                                format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                                format!(
                                    "listens route `{}.{} -> {}` in `{}.{}` requires `{}` but the source contract never produces it",
                                    source_trait, listen.event, triggered, orbital.name, td.name, path
                                ),
                            )
                            .with_suggestion(format!(
                                "Add `{}` to `{}`'s `emits {{ {} {{ ... }} }}` payload, or supply it explicitly via `with {{ {}: ... }}`",
                                path, source_trait, listen.event, top_key
                            )),
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
    use orbital_core_base::schema::{
        ConfigField, EmitDefinition, EntityDefinition, EntityRef, EventDefinition, EventScope,
        ListenDefinition, Orbital, StateMachine,
    };
    use std::collections::HashMap as Map;

    fn empty_state_machine_with_events(events: Vec<EventDefinition>) -> StateMachine {
        StateMachine {
            states: vec![],
            events: if events.is_empty() {
                None
            } else {
                Some(events)
            },
            transitions: vec![],
        }
    }

    fn trait_with(
        name: &str,
        emits: Option<Vec<EmitDefinition>>,
        listens: Option<Vec<ListenDefinition>>,
        events: Vec<EventDefinition>,
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
            linked_entity: None,
            emits,
            listens,
            state_machine: empty_state_machine_with_events(events),
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

    fn trait_with_config(
        name: &str,
        emits: Option<Vec<EmitDefinition>>,
        listens: Option<Vec<ListenDefinition>>,
        events: Vec<EventDefinition>,
        config: Option<Map<String, ConfigField>>,
    ) -> TraitReference {
        let TraitReference::Inline(mut td) = trait_with(name, emits, listens, events) else {
            unreachable!()
        };
        td.config = config;
        TraitReference::Inline(td)
    }

    /// A source emit fired via a definer knob (`EmitDefinition.definer_knob`)
    /// — no `payloadSchema` of its own; the real contract lives on a
    /// companion config field (see `payload_companion_field`).
    fn emit_via_knob(event: &str, knob: &str) -> EmitDefinition {
        EmitDefinition {
            definer_knob: Some(knob.to_string()),
            scope_overridden: None,
            event: event.to_string(),
            scope: Some(EventScope::External),
            description: None,
            synonyms: None,
            tier: None,
            payload_schema: None,
            event_id: None,
            payload_entity: None,
        }
    }

    /// The `@payload(<knob>)` companion config field `resolve_sigil_types`
    /// stamps `payload_for` on — its `default` is the call-site's resolved
    /// object literal (e.g. `ui-button`'s `actionPayload`).
    fn payload_companion_field(knob: &str, default: SExpression) -> ConfigField {
        ConfigField {
            field_type: "GeneratePayload".to_string(),
            default: Some(default),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: None,
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            nested_forwarded_from: Vec::new(),
            payload_for: Some(knob.to_string()),
            projected_from: None,
        }
    }

    fn emit(event: &str, payload: Option<Vec<PayloadField>>) -> EmitDefinition {
        EmitDefinition {
            definer_knob: None,
            scope_overridden: None,
            event: event.to_string(),
            scope: Some(EventScope::External),
            description: None,
            synonyms: None,
            tier: None,
            payload_schema: payload,
            event_id: None,
            payload_entity: None,
        }
    }

    fn event_def(key: &str, payload: Option<Vec<PayloadField>>) -> EventDefinition {
        EventDefinition {
            key: key.to_string(),
            id: None,
            name: None,
            description: None,
            synonyms: None,
            tier: None,
            payload_schema: payload,
            payload_entity: None,
            external: None,
            listened: None,
            inbound: None,
        }
    }

    fn field(name: &str, required: bool) -> PayloadField {
        PayloadField {
            values: None,
            name: name.to_string(),
            field_type: "string".to_string(),
            required: if required { Some(true) } else { None },
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        }
    }

    fn object_field(name: &str, required: bool, properties: Vec<PayloadField>) -> PayloadField {
        PayloadField {
            values: None,
            name: name.to_string(),
            field_type: "object".to_string(),
            required: if required { Some(true) } else { None },
            description: None,
            properties: Some(properties),
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        }
    }

    fn entity_field(name: &str, required: bool, entity: &str) -> PayloadField {
        PayloadField {
            values: None,
            name: name.to_string(),
            field_type: "object".to_string(),
            required: if required { Some(true) } else { None },
            description: None,
            properties: Some(vec![field("id", true)]),
            type_when: None,
            entity: Some(entity.to_string()),
            projected_from: None,
            items: None,
        }
    }

    fn listen_from(event: &str, source_trait: &str, triggers: &str) -> ListenDefinition {
        ListenDefinition {
            event: event.to_string(),
            event_id: None,
            triggers: Some(triggers.to_string()),
            triggers_id: None,
            scope: None,
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            source: Some(ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
        }
    }

    fn orbital_with(name: &str, traits: Vec<TraitReference>) -> Orbital {
        Orbital {
            spec: false,
            types: None,
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
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
                name: "Entity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
                invariants: None,
            }),
            auxiliary_entities: None,
            traits,
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
        }
    }

    fn schema_with(orbitals: Vec<Orbital>) -> OrbitalSchema {
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
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals,
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

    /// The chat shape: source `SAVE { data: { channel! } }`, listener
    /// `DO_CREATE { data: { content!, channel! } }` — exactly one error
    /// naming the starved nested leaf `data.content`.
    #[test]
    fn starved_nested_required_field_errors_once() {
        let source = trait_with(
            "ChatComposer",
            Some(vec![emit(
                "SAVE",
                Some(vec![object_field(
                    "data",
                    false,
                    vec![field("channel", true)],
                )]),
            )]),
            None,
            vec![],
        );
        let listener = trait_with(
            "ChatMessagePersistor",
            None,
            Some(vec![listen_from("SAVE", "ChatComposer", "DO_CREATE")]),
            vec![event_def(
                "DO_CREATE",
                Some(vec![object_field(
                    "data",
                    false,
                    vec![field("content", true), field("channel", true)],
                )]),
            )],
        );
        let schema = schema_with(vec![orbital_with("ChatOrbital", vec![source, listener])]);

        let result = validate_listen_route_starves_required_field(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ListenRouteStarvesRequiredField)
            .collect();
        assert_eq!(hits.len(), 1, "{:?}", result.errors);
        assert!(
            hits[0].message.contains("data.content"),
            "{}",
            hits[0].message
        );
    }

    /// riya open world: `OwLevelMerger`'s RIYA_LEVEL_DATA contract lists every
    /// level's optional containers (`baseCurves : [RiyaOwCurve]`, whose
    /// samples require `x`/`y`); the plain level never sends `baseCurves`, so
    /// nothing under it is owed. `starved_nested_required_field_errors_once`
    /// is the control — a container the source DOES send owes its children.
    #[test]
    fn an_optional_container_the_source_never_sends_owes_nothing() {
        let source = trait_with(
            "PlainData",
            Some(vec![emit("LEVEL_DATA", Some(vec![field("id", true)]))]),
            None,
            vec![],
        );
        let listener = trait_with(
            "Merger",
            None,
            Some(vec![listen_from("LEVEL_DATA", "PlainData", "LEVEL_DATA")]),
            vec![event_def(
                "LEVEL_DATA",
                Some(vec![
                    field("id", true),
                    object_field(
                        "baseCurves",
                        false,
                        vec![object_field("samples", true, vec![field("x", true)])],
                    ),
                ]),
            )],
        );
        let schema = schema_with(vec![orbital_with("World", vec![source, listener])]);
        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A complete route (source declares every required leaf) is silent.
    #[test]
    fn complete_route_is_silent() {
        let source = trait_with(
            "ChatComposer",
            Some(vec![emit(
                "SAVE",
                Some(vec![object_field(
                    "data",
                    false,
                    vec![field("content", true), field("channel", true)],
                )]),
            )]),
            None,
            vec![],
        );
        let listener = trait_with(
            "ChatMessagePersistor",
            None,
            Some(vec![listen_from("SAVE", "ChatComposer", "DO_CREATE")]),
            vec![event_def(
                "DO_CREATE",
                Some(vec![object_field(
                    "data",
                    false,
                    vec![field("content", true), field("channel", true)],
                )]),
            )],
        );
        let schema = schema_with(vec![orbital_with("ChatOrbital", vec![source, listener])]);

        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A `with { content: ... }` mapping supplies the required top-level
    /// key explicitly — the mapping replaces pass-through, so the source's
    /// (incomplete) contract is irrelevant once a mapping is present.
    #[test]
    fn explicit_mapping_supplying_the_key_is_silent() {
        let source = trait_with(
            "ChatComposer",
            Some(vec![emit("SAVE", Some(vec![]))]),
            None,
            vec![],
        );
        let mut listen = listen_from("SAVE", "ChatComposer", "DO_CREATE");
        let mut mapping = orbital_core_base::schema::SObject::new();
        mapping.insert(
            "content".to_string(),
            orbital_core_base::schema::SExpression::String("?text".to_string()),
        );
        listen.payload_mapping = Some(mapping);
        let listener = trait_with(
            "ChatMessagePersistor",
            None,
            Some(vec![listen]),
            vec![event_def("DO_CREATE", Some(vec![field("content", true)]))],
        );
        let schema = schema_with(vec![orbital_with("ChatOrbital", vec![source, listener])]);

        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// The source declares no emit contract for the event at all — that is
    /// `payload.rs`'s territory (unresolved source), not this rule's.
    #[test]
    fn source_with_no_emit_contract_is_silent() {
        let source = trait_with("ChatComposer", None, None, vec![]);
        let listener = trait_with(
            "ChatMessagePersistor",
            None,
            Some(vec![listen_from("SAVE", "ChatComposer", "DO_CREATE")]),
            vec![event_def("DO_CREATE", Some(vec![field("content", true)]))],
        );
        let schema = schema_with(vec![orbital_with("ChatOrbital", vec![source, listener])]);

        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// An entity-typed field on both sides is ONE leaf (existence by name),
    /// never expanded into its flattened `properties` — no error.
    #[test]
    fn entity_typed_field_on_both_sides_is_silent() {
        let source = trait_with(
            "ChatComposer",
            Some(vec![emit(
                "SAVE",
                Some(vec![entity_field("row", true, "ChatMessage")]),
            )]),
            None,
            vec![],
        );
        let listener = trait_with(
            "ChatMessagePersistor",
            None,
            Some(vec![listen_from("SAVE", "ChatComposer", "DO_CREATE")]),
            vec![event_def(
                "DO_CREATE",
                Some(vec![entity_field("row", true, "ChatMessage")]),
            )],
        );
        let schema = schema_with(vec![orbital_with("ChatOrbital", vec![source, listener])]);

        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// std-pong's `PongMechanic.BURST -> BURST`: the rally atom lowers its
    /// `particles : [Fx]` payload with an `entity: "Fx"` marker AND the alias's
    /// properties; the fx listener's local contract requires `particles.id`.
    /// The source declares the nested field, so it produces it.
    #[test]
    fn alias_marked_source_struct_produces_its_declared_nested_fields() {
        let mut particles = entity_field("particles", true, "Fx");
        particles.properties = Some(vec![field("id", true), field("ttl", true)]);
        let source = trait_with(
            "PongMechanic",
            Some(vec![emit("BURST", Some(vec![particles]))]),
            None,
            vec![],
        );
        let listener = trait_with(
            "PongFx",
            None,
            Some(vec![listen_from("BURST", "PongMechanic", "BURST")]),
            vec![event_def(
                "BURST",
                Some(vec![object_field(
                    "particles",
                    true,
                    vec![field("id", true), field("ttl", true)],
                )]),
            )],
        );
        let schema = schema_with(vec![orbital_with("PongOrbital", vec![source, listener])]);
        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// Control: a nested field the source struct does NOT declare still starves.
    #[test]
    fn alias_marked_source_struct_missing_a_nested_field_still_starves() {
        let mut particles = entity_field("particles", true, "Fx");
        particles.properties = Some(vec![field("id", true)]);
        let source = trait_with(
            "PongMechanic",
            Some(vec![emit("BURST", Some(vec![particles]))]),
            None,
            vec![],
        );
        let listener = trait_with(
            "PongFx",
            None,
            Some(vec![listen_from("BURST", "PongMechanic", "BURST")]),
            vec![event_def(
                "BURST",
                Some(vec![object_field(
                    "particles",
                    true,
                    vec![field("id", true), field("ttl", true)],
                )]),
            )],
        );
        let schema = schema_with(vec![orbital_with("PongOrbital", vec![source, listener])]);
        let result = validate_listen_route_starves_required_field(&schema);
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    }

    /// The std-coding-academy shape: a composed `ui-button`'s `GENERATE`
    /// fires via the `action` definer knob (no `payloadSchema` of its own),
    /// and its `actionPayload` companion (`payload_for: "action"`) carries
    /// the call-site's resolved `{ prompt: ... }` literal. The listener's
    /// trigger requires `prompt!` — declared via the companion, not the
    /// emit's own (empty) contract, so the route is complete. No error.
    #[test]
    fn definer_knob_companion_config_field_satisfies_required_field() {
        let mut config = Map::new();
        config.insert(
            "actionPayload".to_string(),
            payload_companion_field(
                "action",
                SExpression::Object(orbital_core_base::schema::SObject::from([(
                    "prompt".to_string(),
                    SExpression::String("@payload.text".to_string()),
                )])),
            ),
        );
        let source = trait_with_config(
            "SeqButtonHint",
            Some(vec![emit_via_knob("GENERATE", "action")]),
            None,
            vec![],
            Some(config),
        );
        let listener = trait_with(
            "SeqHintGeneration",
            None,
            Some(vec![listen_from("GENERATE", "SeqButtonHint", "GENERATE")]),
            vec![event_def("GENERATE", Some(vec![field("prompt", true)]))],
        );
        let schema = schema_with(vec![orbital_with(
            "SeqChallengeOrbital",
            vec![source, listener],
        )]);

        let result = validate_listen_route_starves_required_field(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// Same shape with the companion config field absent (or with no
    /// `payload_for` link) — the emit's own contract is empty and nothing
    /// else declares `prompt`, so exactly one error fires.
    #[test]
    fn definer_knob_emit_without_companion_still_starves() {
        let source = trait_with_config(
            "SeqButtonHint",
            Some(vec![emit_via_knob("GENERATE", "action")]),
            None,
            vec![],
            None,
        );
        let listener = trait_with(
            "SeqHintGeneration",
            None,
            Some(vec![listen_from("GENERATE", "SeqButtonHint", "GENERATE")]),
            vec![event_def("GENERATE", Some(vec![field("prompt", true)]))],
        );
        let schema = schema_with(vec![orbital_with(
            "SeqChallengeOrbital",
            vec![source, listener],
        )]);

        let result = validate_listen_route_starves_required_field(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ListenRouteStarvesRequiredField)
            .collect();
        assert_eq!(hits.len(), 1, "{:?}", result.errors);
        assert!(hits[0].message.contains("prompt"), "{}", hits[0].message);
    }
}
