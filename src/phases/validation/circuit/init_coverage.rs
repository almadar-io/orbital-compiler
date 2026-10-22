//! `ORB_CIRCUIT_INIT_UNCOVERED_STATE` (G-ORB-186): a page trait that initializes on `INIT` must
//! handle `INIT` in every state it can reach. Trait state survives unmount, so a page revisited
//! while its trait sits in a state with no `INIT` arm renders the stale state and does nothing.

use super::slot::{get_initial_state, reachable_states, transition_from_states};
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::TraitDefinition;

const INIT: &str = "INIT";

pub(crate) fn validate_init_coverage(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(initial) = get_initial_state(trait_def) else {
        return result;
    };
    let machine = &trait_def.state_machine;
    let handles_init = |state: &str| {
        machine.transitions.iter().any(|t| {
            t.event == INIT
                && transition_from_states(&t.from)
                    .iter()
                    .any(|s| *s == state || *s == "*")
        })
    };
    if !handles_init(&initial) {
        return result;
    }
    let reachable = reachable_states(machine, &initial);
    for state in &machine.states {
        if !reachable.contains(&state.name) || handles_init(&state.name) {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitInitUncoveredState,
                format!("{path}.stateMachine.states.{}", state.name),
                format!(
                    "Page trait '{}' handles INIT in '{initial}' but not in reachable state '{}': revisiting the page from there does nothing",
                    trait_def.name, state.name
                ),
            )
            .with_suggestion(format!(
                "Add an `INIT` arm to state '{}' that re-initializes the trait (as '{initial}' does)",
                state.name
            )),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(transitions: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Page",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }, { "name": "open" }, { "name": "orphan" }],
                "transitions": transitions
            }
        }))
        .expect("trait json")
    }

    fn uncovered(td: &TraitDefinition) -> Vec<String> {
        validate_init_coverage(td, "t")
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::CircuitInitUncoveredState)
            .map(|e| e.path.clone())
            .collect()
    }

    #[test]
    fn a_reachable_state_without_init_is_reported() {
        let td = machine(serde_json::json!([
            { "from": "idle", "to": "idle", "event": "INIT" },
            { "from": "idle", "to": "open", "event": "OPEN" }
        ]));
        assert_eq!(uncovered(&td), vec!["t.stateMachine.states.open"]);
    }

    #[test]
    fn control_an_init_arm_in_every_reachable_state_passes() {
        let td = machine(serde_json::json!([
            { "from": "idle", "to": "idle", "event": "INIT" },
            { "from": "idle", "to": "open", "event": "OPEN" },
            { "from": "open", "to": "idle", "event": "INIT" }
        ]));
        assert!(uncovered(&td).is_empty());
    }

    #[test]
    fn a_wildcard_init_arm_covers_every_state() {
        let td = machine(serde_json::json!([
            { "from": "*", "to": "idle", "event": "INIT" },
            { "from": "idle", "to": "open", "event": "OPEN" }
        ]));
        assert!(uncovered(&td).is_empty());
    }

    #[test]
    fn a_trait_that_does_not_init_in_its_initial_state_is_not_checked() {
        let td = machine(serde_json::json!([
            { "from": "idle", "to": "open", "event": "OPEN" }
        ]));
        assert!(uncovered(&td).is_empty());
    }

    #[test]
    fn a_state_reached_only_through_a_dead_guard_is_not_reported() {
        let td = machine(serde_json::json!([
            { "from": "idle", "to": "idle", "event": "INIT" },
            { "from": "idle", "to": "open", "event": "OPEN", "guard": ["=", "panel", "document"] }
        ]));
        assert!(uncovered(&td).is_empty());
    }
}
