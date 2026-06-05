use super::{collect_all_traits, extract_emit_event, transition_can_propagate};
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::OrbitalSchema;
use std::collections::{HashMap, HashSet};

/// Validate circular event dependencies.
/// Checks both `emits` declarations AND emit effects in transitions.
pub(crate) fn validate_circular_dependencies(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build dependency graph: trait -> [traits it depends on (via listen -> emit)]
    let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
    let mut emitters: HashMap<String, HashSet<String>> = HashMap::new(); // event -> {emitting traits}

    // Collect emitters from both declarations and transition effects
    let all_traits = collect_all_traits(schema);
    for trait_def in &all_traits {
        // From emits declarations
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                emitters
                    .entry(emit.event.clone())
                    .or_default()
                    .insert(trait_def.name.clone());
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
                            .insert(trait_def.name.clone());
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
    // Skip also when source and listening trait are in different
    // orbitals — orbital scope already isolates the bus key, so a
    // "depends on" edge across orbitals only matters when the listening
    // trait actually drives state on that emit (which the cycle check
    // can't tell from the static graph). Cross-orbital cycles still
    // surface via the `cross_orbital` ORB_X_LISTEN_SOURCE_UNRESOLVED
    // path when they're truly broken.
    let lifecycle_triggers: std::collections::HashSet<&str> =
        ["INIT", "LOAD", "RESET"].into_iter().collect();
    for trait_def in &all_traits {
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
                    for emitter in emitter_traits {
                        if emitter != &trait_def.name {
                            dependencies
                                .entry(trait_def.name.clone())
                                .or_default()
                                .insert(emitter.clone());
                        }
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
