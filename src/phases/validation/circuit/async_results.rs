//! `ORB_CIRCUIT_ASYNC_RESULT_DEAF_TARGET`: ported from the JS wiring lint's
//! `async-result-deaf-target` (warning there; error here per the 0/0 ruling).
//! Same-orbital circuit clause, the async-result-contract twin of
//! `closed_circuit.rs::validate_failure_arms`, scoped to the destination
//! state's WHOLE result surface rather than only its failure half.

use super::closed_circuit::arms_in;
use super::cross_orbital::collect_async_result_events;
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::StateMachine;
use std::collections::HashSet;

/// `ORB_CIRCUIT_ASYNC_RESULT_DEAF_TARGET`. For every state-CHANGING
/// transition (self-transitions excluded — the machine stays interactive,
/// a different, render-drop class), collect the async-result events its own
/// `fetch`/`persist` effects declare (`collect_async_result_events`); if the
/// destination state's own arms (`arms_in`: `from === to-state || from ===
/// "*"`) don't handle one of them, the requester departs to a state deaf to
/// its own operation's result. NOT client-bound gated — the JS predicate has
/// no `bound.has` check in this block, unlike its `failure-arm-missing`
/// sibling in `validate_closed_circuit`, so this is called unconditionally
/// there.
pub(crate) fn validate_async_result_deaf_target(
    state_machine: &StateMachine,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in state_machine.transitions.iter().enumerate() {
        // Self-transition: the machine stays in an interactive state, not a
        // stranded one. Mirrors the JS `from === to` guard; `StringOrArray`
        // generalizes a multi-`from` arm, so treat ANY shared state as a
        // self-transition for this arm.
        if transition.from.matches(transition.to.as_str()) {
            continue;
        }
        let Some(effects) = &transition.effects else {
            continue;
        };
        let results = collect_async_result_events(effects);
        if results.is_empty() {
            continue;
        }
        let handled: HashSet<&str> = arms_in(state_machine, &transition.to)
            .iter()
            .map(|t| t.event.as_str())
            .collect();
        let mut deaf: Vec<&str> = results
            .iter()
            .map(String::as_str)
            .filter(|e| !handled.contains(e))
            .collect();
        if deaf.is_empty() {
            continue;
        }
        deaf.sort_unstable(); // deterministic message order

        let effect_path = format!("{}.stateMachine.transitions[{}].effects", path, i);
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitAsyncResultDeafTarget,
                &effect_path,
                format!(
                    "transition {}--[{}]-->{} declares async result event(s) {:?} but state '{}' has no arm handling {} — the requester departs before it can hear its own operation's result",
                    transition.from,
                    transition.event,
                    transition.to,
                    deaf,
                    transition.to,
                    if deaf.len() == 1 { "it" } else { "them" }
                ),
            )
            .with_suggestion(format!(
                "Add a transition `from: '{}'` handling each of {:?} in state '{}' (or route the result to a state that already does).",
                transition.to, deaf, transition.to
            )),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{SExpression, StateDefinition, StringOrArray, TransitionDefinition};

    fn effects_from(json_val: serde_json::Value) -> Vec<SExpression> {
        serde_json::from_value(json_val).expect("effects fixture deserializes")
    }

    fn sm_transition(
        from: &str,
        to: &str,
        event: &str,
        effects: Vec<SExpression>,
    ) -> TransitionDefinition {
        TransitionDefinition {
            event_id: None,
            from: StringOrArray::Single(from.to_string()),
            to: to.to_string(),
            event: event.to_string(),
            guard: None,
            effects: if effects.is_empty() {
                None
            } else {
                Some(effects)
            },
            ensures: None,
        }
    }

    fn state(name: &str, is_initial: bool) -> StateDefinition {
        StateDefinition {
            name: name.to_string(),
            is_initial: if is_initial { Some(true) } else { None },
            is_terminal: None,
            description: None,
            time_bound_ms: None,
            holds: None,
        }
    }

    // --- ORB_CIRCUIT_ASYNC_RESULT_DEAF_TARGET ---

    #[test]
    fn async_result_deaf_target_when_destination_state_has_no_arm_for_success() {
        let sm = StateMachine {
            states: vec![state("browsing", true), state("loading", false)],
            events: None,
            transitions: vec![
                sm_transition(
                    "browsing",
                    "loading",
                    "REFRESH",
                    effects_from(serde_json::json!([
                        ["fetch", "Item", { "emit": { "success": "ItemLoaded", "failure": "ItemFailed" } }]
                    ])),
                ),
                // "loading" only handles ItemFailed, not ItemLoaded.
                sm_transition("loading", "browsing", "ItemFailed", vec![]),
            ],
        };
        let result = validate_async_result_deaf_target(&sm, "test");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitAsyncResultDeafTarget),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn async_result_deaf_target_silent_when_destination_handles_every_result_event() {
        let sm = StateMachine {
            states: vec![state("browsing", true), state("loading", false)],
            events: None,
            transitions: vec![
                sm_transition(
                    "browsing",
                    "loading",
                    "REFRESH",
                    effects_from(serde_json::json!([
                        ["fetch", "Item", { "emit": { "success": "ItemLoaded", "failure": "ItemFailed" } }]
                    ])),
                ),
                sm_transition("loading", "browsing", "ItemLoaded", vec![]),
                sm_transition("loading", "browsing", "ItemFailed", vec![]),
            ],
        };
        let result = validate_async_result_deaf_target(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn async_result_deaf_target_silent_on_self_transition() {
        let sm = StateMachine {
            states: vec![state("browsing", true)],
            events: None,
            transitions: vec![sm_transition(
                "browsing",
                "browsing",
                "REFRESH",
                effects_from(serde_json::json!([
                    ["fetch", "Item", { "emit": { "success": "ItemLoaded" } }]
                ])),
            )],
        };
        let result = validate_async_result_deaf_target(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn async_result_deaf_target_silent_for_call_service_result() {
        // collect_async_result_events is fetch/persist only — call-service
        // is a different (unscoped) contract, matching the JS asymmetry.
        let sm = StateMachine {
            states: vec![state("browsing", true), state("loading", false)],
            events: None,
            transitions: vec![sm_transition(
                "browsing",
                "loading",
                "REFRESH",
                effects_from(serde_json::json!([
                    ["call-service", "svc", { "emit": { "success": "SvcDone" } }]
                ])),
            )],
        };
        let result = validate_async_result_deaf_target(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
}
