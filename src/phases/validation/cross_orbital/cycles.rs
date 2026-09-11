use super::{collect_all_traits, extract_emit_event, transition_can_propagate};
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EventScope, ListenSource, OrbitalSchema};
use std::collections::{HashMap, HashSet};

/// Validate circular event dependencies.
/// Checks both `emits` declarations AND emit effects in transitions.
pub(crate) fn validate_circular_dependencies(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build dependency graph: trait -> [traits it depends on (via listen -> emit)]
    let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
    // event -> {(emitting trait, owning orbital)}
    let mut emitters: HashMap<String, HashSet<(String, String)>> = HashMap::new();
    // (trait, event) pairs whose `emits` declaration is `scope: external`
    let mut external_emits: HashSet<(String, String)> = HashSet::new();

    // Collect emitters from both declarations and transition effects
    let all_traits = collect_all_traits(schema);
    for (orbital, trait_def) in &all_traits {
        // From emits declarations
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                emitters
                    .entry(emit.event.clone())
                    .or_default()
                    .insert((trait_def.name.clone(), orbital.to_string()));
                if emit.scope == Some(EventScope::External) {
                    external_emits.insert((trait_def.name.clone(), emit.event.clone()));
                }
            }
        }
        // From transition effects (["emit", "EVENT_NAME"])
        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    if let Some(event_name) = extract_emit_event(effect) {
                        emitters
                            .entry(event_name)
                            .or_default()
                            .insert((trait_def.name.clone(), orbital.to_string()));
                    }
                }
            }
        }
    }

    // Build dependency edges: trait A listens to event X → trait B emits X → A depends on B
    //
    // Skip listens whose `triggers` resolves to a lifecycle event (`INIT`,
    // `LOAD`). Those represent a "reset/refresh on signal" pattern, not a
    // data dependency: when ContactCreate emits CONTACT_CREATED and
    // ContactBrowse responds with INIT (re-fetch its list), the state
    // machine reset breaks the bus loop. Counting them as edges flagged
    // every legitimate browse/create cycle as a dependency cycle, even
    // though the runtime can't loop infinitely (INIT just re-fetches).
    //
    // Skip also when the emitting and listening traits are in different
    // orbitals, UNLESS the event genuinely crosses: orbital scope isolates
    // the bus key, so a non-external emit is never visible on another
    // orbital's bus at runtime. An edge counts cross-orbital only when the
    // emitter declares the event `scope: external`, or the listen itself is
    // externally scoped / sourced at another orbital. Without this, two
    // instances of one atom (e.g. std-mod-queue mounted in two orbitals)
    // false-positive as a cycle on their shared event names.
    let lifecycle_triggers: std::collections::HashSet<&str> =
        ["INIT", "LOAD", "RESET"].into_iter().collect();
    for (listener_orbital, trait_def) in &all_traits {
        if let Some(listens) = &trait_def.listens {
            for listen in listens {
                let trigger = listen.triggers.as_deref().unwrap_or(&listen.event);
                if lifecycle_triggers.contains(trigger) {
                    continue;
                }
                // Skip terminal listens: if the trigger's transition
                // produces no emit effects (and no emit declarations
                // beyond the transition body), the edge can't propagate
                // an event back into the source emitter's listens. The
                // pagination pattern relies on this — Pagination listens
                // to Browse.BrowseItemLoaded with an ITEMS_LOADED
                // transition that only `set`s totalPages and renders the
                // pagination control; nothing it does can drive a fire-
                // loop, so flagging the structural cycle Browse↔Pagination
                // would be a false positive.
                if !transition_can_propagate(trait_def, trigger) {
                    continue;
                }
                if let Some(emitter_traits) = emitters.get(&listen.event) {
                    for (emitter, emitter_orbital) in emitter_traits {
                        if emitter == &trait_def.name {
                            continue;
                        }
                        if emitter_orbital != listener_orbital {
                            let crosses = external_emits
                                .contains(&(emitter.clone(), listen.event.clone()))
                                || listen.scope == Some(EventScope::External)
                                || matches!(listen.source, Some(ListenSource::Orbital { .. }));
                            if !crosses {
                                continue;
                            }
                        }
                        dependencies
                            .entry(trait_def.name.clone())
                            .or_default()
                            .insert(emitter.clone());
                    }
                }
            }
        }
    }

    // Detect cycles using DFS
    let mut visited: HashSet<&str> = HashSet::new();
    let mut rec_stack: HashSet<&str> = HashSet::new();

    for trait_name in dependencies.keys() {
        if let Some(cycle) = detect_cycle(trait_name, &dependencies, &mut visited, &mut rec_stack) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalCircularDependency,
                    "traits",
                    format!("Circular event dependency detected: {}", cycle.join(" -> ")),
                )
                .with_suggestion(
                    "Break the cycle by removing one of the listener/emitter relationships",
                ),
            );
            break; // Only report first cycle
        }
    }

    result
}

/// Detect cycle in dependency graph using DFS
pub(crate) fn detect_cycle<'a>(
    node: &'a str,
    graph: &'a HashMap<String, HashSet<String>>,
    visited: &mut HashSet<&'a str>,
    rec_stack: &mut HashSet<&'a str>,
) -> Option<Vec<String>> {
    if rec_stack.contains(node) {
        // Found cycle
        return Some(vec![node.to_string()]);
    }

    if visited.contains(node) {
        return None;
    }

    visited.insert(node);
    rec_stack.insert(node);

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            if let Some(mut cycle) = detect_cycle(dep, graph, visited, rec_stack) {
                cycle.insert(0, node.to_string());
                return Some(cycle);
            }
        }
    }

    rec_stack.remove(node);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EmitDefinition, EntityDefinition, EntityRef, ListenDefinition, Orbital, SExpression,
        StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
        TransitionDefinition,
    };

    fn emit(event: &str, scope: Option<EventScope>) -> EmitDefinition {
        EmitDefinition {
            definer_knob: None,
            scope_overridden: None,
            event: event.to_string(),
            scope,
            description: None,
            synonyms: None,
            tier: None,
            payload_schema: None,
            event_id: None,
            payload_entity: None,
        }
    }

    fn listen(event: &str, scope: Option<EventScope>) -> ListenDefinition {
        ListenDefinition {
            event: event.to_string(),
            event_id: None,
            triggers: None,
            triggers_id: None,
            scope,
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            source: None,
        }
    }

    /// A trait that emits AND listens to `APPROVE`, with a propagating
    /// transition on it (emit effect) so the cycle check counts the edge.
    fn mod_trait(
        name: &str,
        emit_scope: Option<EventScope>,
        listen_scope: Option<EventScope>,
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
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![emit("APPROVE", emit_scope)]),
            listens: Some(vec![listen("APPROVE", listen_scope)]),
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: None,
                transitions: vec![TransitionDefinition {
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "APPROVE".to_string(),
                    event_id: None,
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("emit".to_string()),
                        SExpression::String("APPROVED".to_string()),
                    ])]),
                }],
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
        })
    }

    fn orbital_with(name: &str, traits: Vec<TraitReference>) -> Orbital {
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
            reference: None,
            config: None,
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
            config: None,
        }
    }

    // Two orbitals mounting the same atom shape (std-mod-queue twice): both
    // traits emit and listen to the same non-external event names. Orbital
    // scope isolates the bus, so no edge crosses and no cycle is reported.
    #[test]
    fn cross_orbital_shared_event_names_no_cycle() {
        let schema = schema_with(vec![
            orbital_with("PMOrbital", vec![mod_trait("TaskModeration", None, None)]),
            orbital_with(
                "DashboardOrbital",
                vec![mod_trait("DashboardModeration", None, None)],
            ),
        ]);

        let result = validate_circular_dependencies(&schema);
        assert!(
            result.is_valid(),
            "cross-orbital non-external edges must not form a cycle; got {:?}",
            result.errors
        );
    }

    // Same shape, but the emitters mark the event `scope: external` — the
    // event genuinely crosses orbital boundaries, so the cycle still fires.
    #[test]
    fn cross_orbital_external_emit_cycle_still_flagged() {
        let schema = schema_with(vec![
            orbital_with(
                "PMOrbital",
                vec![mod_trait(
                    "TaskModeration",
                    Some(EventScope::External),
                    None,
                )],
            ),
            orbital_with(
                "DashboardOrbital",
                vec![mod_trait(
                    "DashboardModeration",
                    Some(EventScope::External),
                    None,
                )],
            ),
        ]);

        let result = validate_circular_dependencies(&schema);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalCircularDependency));
    }

    // Same shape, but the LISTENERS are externally scoped — the listen side
    // reaching across orbitals also counts as a genuine cross, cycle fires.
    #[test]
    fn cross_orbital_external_listen_cycle_still_flagged() {
        let schema = schema_with(vec![
            orbital_with(
                "PMOrbital",
                vec![mod_trait(
                    "TaskModeration",
                    None,
                    Some(EventScope::External),
                )],
            ),
            orbital_with(
                "DashboardOrbital",
                vec![mod_trait(
                    "DashboardModeration",
                    None,
                    Some(EventScope::External),
                )],
            ),
        ]);

        let result = validate_circular_dependencies(&schema);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalCircularDependency));
    }

    // Same-orbital reflexive pair: unchanged behavior, still an error.
    #[test]
    fn same_orbital_cycle_unchanged() {
        let schema = schema_with(vec![orbital_with(
            "TaskOrbital",
            vec![
                mod_trait("TaskModeration", None, None),
                mod_trait("DashboardModeration", None, None),
            ],
        )]);

        let result = validate_circular_dependencies(&schema);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalCircularDependency));
    }
}
