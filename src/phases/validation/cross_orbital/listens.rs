use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{ListenSource, OrbitalSchema, TraitReference};
use std::collections::{HashMap, HashSet};

/// Listens-integrity backstop (error).
///
/// Defense-in-depth duplicate of the L1 `LowerError::ListenSourceUnresolved`
/// check, run on the resolved IR so `use`-imported behaviors that inline
/// listens after lowering are also covered. Reads the typed
/// `ListenSource` enum produced by `split_listen_source`; never reparses
/// the original event string.
pub(crate) fn validate_listens_integrity(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let mut trait_emits: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut orbitals_in_schema: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        orbitals_in_schema.insert(orbital.name.clone());
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let entry = trait_emits
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(emits) = &td.emits {
                for emit in emits {
                    entry.insert(emit.event.clone());
                }
            }
            // Producible-but-undeclared events: effect emit-option maps and
            // render-ui pattern event props (e.g. search-input/filter-group
            // `event` — the component fires `UI:<value>`). Same crediting as
            // the orphan-listener emitter index; without it a scoped listen on
            // a configured event name false-positives as unresolved.
            entry.extend(super::event_wiring::collect_effect_produced_events(td));
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
                    Some(ListenSource::Any) | None => continue,
                };
                // Single-orbital validation context: when the listen's source
                // orbital isn't part of this schema, we can't resolve it. Skip
                // — the composed-schema validation pass catches truly unresolved
                // cross-orbital references once all sibling orbitals are loaded.
                if !orbitals_in_schema.contains(&source_orbital) {
                    continue;
                }
                let resolved = trait_emits
                    .get(&(source_orbital.clone(), source_trait.clone()))
                    .map(|events| events.contains(&listen.event))
                    .unwrap_or(false);
                if !resolved {
                    // A bare `Trait.EVENT` source resolves against the
                    // listener's OWN orbital only. When the named trait does
                    // declare the event in a SIBLING orbital, the qualified
                    // `Orbital.Trait.EVENT` form already routes there on every
                    // path (static + both runtimes) — name it.
                    let sibling_hit = matches!(
                        &listen.source,
                        Some(ListenSource::Trait { .. })
                    )
                    .then(|| {
                        trait_emits.iter().find_map(|((o, t), events)| {
                            (t == &source_trait
                                && o != &source_orbital
                                && events.contains(&listen.event))
                            .then(|| o.clone())
                        })
                    })
                    .flatten();
                    let suggestion = match sibling_hit {
                        Some(o) => format!(
                            "Trait '{}' declares event '{}' in sibling orbital '{}' — a bare \
                             `Trait.EVENT` listen only resolves within this orbital; write the \
                             qualified form `{}.{}.{}`.",
                            source_trait, listen.event, o, o, source_trait, listen.event
                        ),
                        None => "Either declare the event in the source trait's `emits` block, or \
                                 remove the listens entry. Cross-orbital channels require the source \
                                 trait to declare the emit explicitly."
                            .to_string(),
                    };
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CrossOrbitalListenSourceUnresolved,
                            format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                            format!(
                                "Listen entry in '{}.{}' references source '{}.{}' event '{}' \
                                 which is not declared on the source trait.",
                                orbital.name, td.name, source_orbital, source_trait, listen.event
                            ),
                        )
                        .with_suggestion(suggestion),
                    );
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
        EntityDefinition, EntityRef, ListenDefinition, Orbital, SExpression, StateDefinition,
        StateMachine, TraitDefinition, TransitionDefinition,
    };
    use std::collections::HashMap as Map;

    fn minimal_schema(traits: Vec<TraitReference>) -> OrbitalSchema {
        OrbitalSchema {
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
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
                    name: "Task".to_string(),
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
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
        }
    }

    fn bare_trait(name: &str) -> TraitDefinition {
        TraitDefinition {
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: None,
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
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    /// A trait whose only transition renders a `data-list` with a conditional
    /// (`if`-wrapped) `itemActions` config: the then-branch fires
    /// `EDIT_ITEM`, the else-branch fires `VIEW_ITEM`.
    fn trait_with_conditional_item_actions(name: &str) -> TraitDefinition {
        let descriptor = |event: &str| {
            let mut dm: Map<String, SExpression> = Map::new();
            dm.insert("name".to_string(), SExpression::String("Edit".to_string()));
            dm.insert("event".to_string(), SExpression::String(event.to_string()));
            SExpression::Object(dm)
        };
        let item_actions = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.role".to_string()),
            SExpression::Array(vec![descriptor("EDIT_ITEM")]),
            SExpression::Array(vec![descriptor("VIEW_ITEM")]),
        ]);
        let mut node: Map<String, SExpression> = Map::new();
        node.insert("type".to_string(), SExpression::String("data-list".to_string()));
        node.insert("itemActions".to_string(), item_actions);
        let render_effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(node),
        ]);

        let mut td = bare_trait(name);
        td.state_machine = StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }],
            events: None,
            transitions: vec![TransitionDefinition {
                from: orbital_core::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                event_id: None,
                guard: None,
                effects: Some(vec![render_effect]),
            }],
        };
        td
    }

    fn scoped_listener(name: &str, source_trait: &str, event: &str) -> TraitDefinition {
        let mut td = bare_trait(name);
        td.listens = Some(vec![ListenDefinition {
            event: event.to_string(),
            triggers: Some("REFRESH".to_string()),
            scope: None,
            source: Some(ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            event_id: None,
            triggers_id: None,
        }]);
        td
    }

    /// A listen on an event fired only inside the else-branch of a
    /// conditional `itemActions` config resolves without
    /// `ORB_X_LISTEN_SOURCE_UNRESOLVED` — the runtime evaluates the
    /// conditional at render time, so either branch is a legal listen
    /// source.
    #[test]
    fn listen_on_conditional_branch_only_event_resolves() {
        let schema = minimal_schema(vec![
            TraitReference::Inline(trait_with_conditional_item_actions("TaskList")),
            TraitReference::Inline(scoped_listener("TaskDetail", "TaskList", "VIEW_ITEM")),
        ]);
        let result = validate_listens_integrity(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalListenSourceUnresolved),
            "listen on the conditional's else-branch-only event must resolve; got {:?}",
            result.errors
        );
    }
}
