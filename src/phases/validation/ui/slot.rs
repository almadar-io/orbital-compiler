//! Slot Validation
//!
//! State-machine helpers for the program validators. Coverage, contention and
//! nesting live in the std lint `std-orb-lint-slot`.

use orbital_core_base::schema::{StateMachine, TraitDefinition};
use std::collections::{HashSet, VecDeque};

/// Get the initial state of a trait
pub(crate) fn get_initial_state(trait_def: &TraitDefinition) -> Option<String> {
    trait_def
        .state_machine
        .states
        .iter()
        .find(|s| s.is_initial == Some(true))
        .map(|s| s.name.clone())
}

/// Compute the set of states reachable from `initial` by walking transitions.
///
/// Wildcard (`"*"`) `from` values mean "any state can transition this way", so
/// they contribute `to` states as reachable from every state already visited.
/// An arm whose guard is statically false never fires, so it reaches nothing.
///
/// Inlined here rather than pulled from `closed_circuit.rs` to keep `slot.rs`
/// self-contained — the BFS is small and the two modules have different needs
/// (closed-circuit uses an incoming-transition heuristic).
pub(crate) fn reachable_states(state_machine: &StateMachine, initial: &str) -> HashSet<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    visited.insert(initial.to_string());
    queue.push_back(initial.to_string());

    while let Some(current) = queue.pop_front() {
        for transition in &state_machine.transitions {
            let from_states = transition_from_states(&transition.from);
            let fires_from_current = from_states
                .iter()
                .any(|s| *s == current.as_str() || *s == "*");
            if !fires_from_current
                || transition
                    .guard
                    .as_ref()
                    .is_some_and(|g| g.is_statically_false())
            {
                continue;
            }
            if !visited.contains(&transition.to) {
                visited.insert(transition.to.clone());
                queue.push_back(transition.to.clone());
            }
        }
    }

    visited
}

/// Return every `from` state as a flat slice of `&str`.
pub(crate) fn transition_from_states(from: &orbital_core_base::schema::StringOrArray) -> Vec<&str> {
    match from {
        orbital_core_base::schema::StringOrArray::Single(s) => vec![s.as_str()],
        orbital_core_base::schema::StringOrArray::Multiple(arr) => {
            arr.iter().map(|s| s.as_str()).collect()
        }
    }
}
