use super::{collect_all_traits, extract_emit_event, transition_can_propagate};
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EventScope, ListenSource, OrbitalSchema};
use std::collections::{HashMap, HashSet};

/// Validate circular event dependencies.
/// Checks both `emits` declarations AND emit effects in transitions.
pub(crate) fn validate_circular_dependencies(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Event-level graph: a node is (trait, event its arm receives). An arm's
    // emits reach the arms their routes trigger; only a chain that returns to
    // an arm it left is a loop. Request/response between two traits on
    // different events is not (std-platformer-board-2d: JUMP -> FELL).
    let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
    // (trait, event) pairs whose `emits` declaration is `scope: external`
    let mut external_emits: HashSet<(String, String)> = HashSet::new();
    // (emitting trait, orbital, arm event, emitted event, arm guarded)
    let mut arm_emits: Vec<(String, String, String, String, bool)> = Vec::new();

    fn walk_emits(expr: &orbital_core::schema::SExpression, out: &mut HashSet<String>) {
        if let Some(event) = extract_emit_event(expr) {
            out.insert(event);
        }
        if let orbital_core::schema::SExpression::Array(items) = expr {
            for item in items {
                walk_emits(item, out);
            }
        }
    }

    let all_traits = collect_all_traits(schema);
    for (orbital, trait_def) in &all_traits {
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                if emit.scope == Some(EventScope::External) {
                    external_emits.insert((trait_def.name.clone(), emit.event.clone()));
                }
            }
        }
        for transition in &trait_def.state_machine.transitions {
            let effects = transition.effects.as_deref().unwrap_or(&[]);
            let mut out = HashSet::new();
            for effect in effects {
                walk_emits(effect, &mut out);
            }
            out.extend(super::collect_async_result_events(effects));
            for emitted in out {
                arm_emits.push((
                    trait_def.name.clone(),
                    orbital.to_string(),
                    transition.event.clone(),
                    emitted,
                    transition.guard.is_some(),
                ));
            }
        }
    }

    // Lifecycle triggers (`INIT`, `LOAD`, `RESET`) are "reset/refresh on
    // signal", not a data dependency: the state machine reset breaks the loop.
    // An edge crosses orbitals only when the event genuinely crosses (the
    // emitter declares it `scope: external`, or the listen is externally
    // scoped / sourced at another orbital).
    let lifecycle_triggers: std::collections::HashSet<&str> =
        ["INIT", "LOAD", "RESET"].into_iter().collect();
    for (listener_orbital, trait_def) in &all_traits {
        let Some(listens) = &trait_def.listens else {
            continue;
        };
        for listen in listens {
            let trigger = listen.triggers.as_deref().unwrap_or(&listen.event);
            if lifecycle_triggers.contains(trigger) || !transition_can_propagate(trait_def, trigger)
            {
                continue;
            }
            for (emitter, emitter_orbital, arm, emitted, arm_guarded) in &arm_emits {
                // A guarded edge names the loop's exit, so only unguarded edges can close an error cycle.
                if *arm_guarded || listen.guard.is_some() {
                    continue;
                }
                if emitted != &listen.event || emitter == &trait_def.name {
                    continue;
                }
                let source_ok = match &listen.source {
                    None | Some(ListenSource::Any) => true,
                    Some(ListenSource::Trait { trait_name, .. }) => trait_name == emitter,
                    Some(ListenSource::Orbital {
                        orbital,
                        trait_name,
                        ..
                    }) => orbital == emitter_orbital && trait_name == emitter,
                };
                if !source_ok {
                    continue;
                }
                if emitter_orbital != listener_orbital {
                    let crosses = external_emits.contains(&(emitter.clone(), listen.event.clone()))
                        || listen.scope == Some(EventScope::External)
                        || matches!(listen.source, Some(ListenSource::Orbital { .. }));
                    if !crosses {
                        continue;
                    }
                }
                dependencies
                    .entry(format!("{emitter}.{arm}"))
                    .or_default()
                    .insert(format!("{}.{}", trait_def.name, trigger));
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
                        SExpression::String("APPROVE".to_string()),
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
                local: false,
                seed_mock: false,
                locale_instances: None,
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

    fn routed(json: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(json).unwrap()
    }

    fn two_traits(intent_arm_emits: &str) -> OrbitalSchema {
        routed(serde_json::json!({
            "name": "Plat",
            "orbitals": [{
                "name": "PlatOrbital",
                "entity": { "name": "Body", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [
                    {
                        "name": "Intent",
                        "linkedEntity": "Body",
                        "listens": [{ "event": "FELL", "source": { "kind": "trait", "trait": "Authority" }, "triggers": "FELL" }],
                        "stateMachine": {
                            "states": [{ "name": "s", "isInitial": true }],
                            "transitions": [
                                { "from": "s", "to": "s", "event": "KEY", "effects": [["emit", "JUMP", {}]] },
                                { "from": "s", "to": "s", "event": "FELL", "effects": [["emit", intent_arm_emits, {}]] }
                            ]
                        }
                    },
                    {
                        "name": "Authority",
                        "linkedEntity": "Body",
                        "listens": [{ "event": "JUMP", "source": { "kind": "trait", "trait": "Intent" }, "triggers": "JUMP" }],
                        "stateMachine": {
                            "states": [{ "name": "s", "isInitial": true }],
                            "transitions": [{ "from": "s", "to": "s", "event": "JUMP", "effects": [["emit", "FELL", {}]] }]
                        }
                    }
                ]
            }]
        }))
    }

    /// std-platformer-board-2d: the intent trait sends JUMP to the body
    /// authority, which reports FELL back; the FELL arm raises something
    /// else. Request/response on different events is not a loop.
    #[test]
    fn request_response_between_two_traits_is_not_a_cycle() {
        let result = validate_circular_dependencies(&two_traits("BURST"));
        assert!(result.is_valid(), "{:?}", result.errors);
    }

    /// Control: the FELL arm re-emits JUMP, so the chain closes into a real loop.
    #[test]
    fn an_event_chain_that_closes_is_a_cycle() {
        let result = validate_circular_dependencies(&two_traits("JUMP"));
        assert!(!result.is_valid());
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Exit {
        Listen,
        Arm,
    }

    /// Ring of `events.len()` traits: `T{i}` handles `events[i]` and emits the next event; `T{i+1}` listens it from `T{i}`.
    /// `orbital_of(i)` places each trait; `guard` puts an exit on ring position `i`.
    fn ring(
        events: &[&str],
        orbital_of: impl Fn(usize) -> usize,
        guard: Option<(usize, Exit)>,
    ) -> OrbitalSchema {
        let n = events.len();
        let when = serde_json::json!(["<", "@entity.attempts", 3]);
        let mut orbitals: Vec<Vec<serde_json::Value>> = vec![vec![], vec![]];
        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = events[(i + 1) % n];
            let mut listen = serde_json::json!({
                "event": events[i],
                "triggers": events[i],
                "source": if orbital_of(prev) == orbital_of(i) {
                    serde_json::json!({ "kind": "trait", "trait": format!("T{prev}") })
                } else {
                    serde_json::json!({ "kind": "orbital", "orbital": format!("O{}", orbital_of(prev)), "trait": format!("T{prev}") })
                }
            });
            let mut arm = serde_json::json!({ "from": "s", "to": "s", "event": events[i], "effects": [["emit", next, {}]] });
            match guard {
                Some((g, Exit::Listen)) if g == i => listen["guard"] = when.clone(),
                Some((g, Exit::Arm)) if g == i => arm["guard"] = when.clone(),
                _ => {}
            }
            orbitals[orbital_of(i)].push(serde_json::json!({
                "name": format!("T{i}"),
                "linkedEntity": "Job",
                "emits": [{ "event": next, "scope": "external" }],
                "listens": [listen],
                "stateMachine": { "states": [{ "name": "s", "isInitial": true }], "transitions": [arm] }
            }));
        }
        let orbitals: Vec<serde_json::Value> = orbitals
            .into_iter()
            .enumerate()
            .filter(|(_, traits)| !traits.is_empty())
            .map(|(o, traits)| serde_json::json!({
                "name": format!("O{o}"),
                "entity": { "name": "Job", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": traits
            }))
            .collect();
        routed(serde_json::json!({ "name": "Ring", "orbitals": orbitals }))
    }

    fn is_cycle(schema: &OrbitalSchema) -> bool {
        validate_circular_dependencies(schema)
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalCircularDependency)
    }

    #[test]
    fn an_unguarded_two_trait_loop_is_rejected() {
        assert!(is_cycle(&ring(&["SEND", "ACK"], |_| 0, None)));
    }

    #[test]
    fn a_listen_when_on_the_loop_names_its_exit_and_validates_silently() {
        let result =
            validate_circular_dependencies(&ring(&["SEND", "ACK"], |_| 0, Some((1, Exit::Listen))));
        assert!(
            result.errors.is_empty() && result.warnings.is_empty(),
            "{result:?}"
        );
    }

    #[test]
    fn a_transition_guard_on_the_loop_names_its_exit_too() {
        assert!(!is_cycle(&ring(
            &["SEND", "ACK"],
            |_| 0,
            Some((0, Exit::Arm))
        )));
    }

    #[test]
    fn one_guarded_edge_is_enough_on_a_longer_loop_and_every_position_counts() {
        let events = ["A", "B", "C", "D"];
        assert!(is_cycle(&ring(&events, |_| 0, None)));
        for i in 0..events.len() {
            for exit in [Exit::Listen, Exit::Arm] {
                assert!(
                    !is_cycle(&ring(&events, |_| 0, Some((i, exit)))),
                    "position {i}"
                );
            }
        }
    }

    #[test]
    fn a_guard_on_another_arm_of_the_same_event_does_not_exit_the_loop() {
        let mut schema = ring(&["SEND", "ACK"], |_| 0, None);
        let TraitReference::Inline(t) = &mut schema.orbitals[0].traits[1] else {
            panic!("inline trait");
        };
        let mut guarded = t.state_machine.transitions[0].clone();
        guarded.guard = Some(SExpression::Bool(false));
        t.state_machine.transitions.push(guarded);
        assert!(is_cycle(&schema), "the unguarded arm still closes the loop");
    }

    #[test]
    fn a_loop_through_a_lifecycle_trigger_stays_exempt() {
        assert!(!is_cycle(&ring(&["RESET", "ACK"], |_| 0, None)));
        assert!(
            is_cycle(&ring(&["RESTART", "ACK"], |_| 0, None)),
            "only the three lifecycle names"
        );
    }

    #[test]
    fn a_trait_listening_to_its_own_emit_is_not_a_cross_trait_edge() {
        assert!(!is_cycle(&ring(&["PING"], |_| 0, None)));
    }

    #[test]
    fn a_cross_orbital_loop_follows_the_same_guard_rule() {
        assert!(is_cycle(&ring(&["SEND", "ACK"], |i| i, None)));
        assert!(!is_cycle(&ring(
            &["SEND", "ACK"],
            |i| i,
            Some((0, Exit::Listen))
        )));
    }
}
