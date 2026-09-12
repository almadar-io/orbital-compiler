//! Closed Circuit Validation
//!
//! Validates the "Closed Circuit" pattern where:
//! 1. Trait state machines fire render_ui effects
//! 2. Pattern components render UI with actions
//! 3. User actions emit events back to the event bus
//! 4. Traits listen and dispatch to state machine, completing the circuit
//!
//! This validator detects:
//! - Orphan events (events emitted by UI with no handler)
//! - Duplicate events (same event in same slot)
//! - Action-component mismatches (e.g., SAVE on entity-table)
//! - State-inappropriate actions (e.g., form actions in browse state)
//! - Dead-end components (interactive components with no actions)
//!
//! Ported from `builder/packages/shared/src/orbitals/validation/closed-circuit.ts`.

use super::async_results::validate_async_result_deaf_target;
use super::cross_orbital::collect_failure_events;
use super::fired_events::ui_event_prop_names;
use super::sync::patterns::get_prop_types;
use super::sync::{
    get_invalid_actions_for_component, get_valid_actions_for_component,
    is_action_invalid_for_component, is_form_action, is_form_pattern, is_interactive_pattern,
    is_list_action, is_list_pattern,
};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::phases::embed_graph::{client_bound_traits, pages_embedding_children_of};
use crate::phases::sexpr_json::sexpr_obj_to_json_map;
use orbital_core::events::is_bus_excluded_event;
use orbital_core::schema::{
    json_conditional_leaf_values, Orbital, OrbitalSchema, SExpression, StateMachine,
    StringOrArray, TraitDefinition, TraitReference, TransitionDefinition,
};
use orbital_core::slots::SELF_OVERLAY_PATTERN_TYPES;
use std::collections::{HashMap, HashSet};

/// Information about a render-ui effect
#[derive(Debug, Clone)]
struct RenderUIInfo {
    /// Target slot
    slot: String,
    /// Pattern type
    pattern_type: Option<String>,
    /// Props including itemActions
    props: Option<serde_json::Map<String, serde_json::Value>>,
    /// Target state
    to_state: String,
    /// Path for error reporting
    path: String,
}

/// Information about an item action
#[derive(Debug, Clone)]
struct ItemActionInfo {
    /// Action name (e.g., VIEW, EDIT, DELETE)
    action: String,
    /// Event emitted when action is triggered
    event: String,
    /// Pattern type where action is defined
    pattern_type: String,
    /// State where this action is available
    in_state: String,
    /// Slot where this action appears
    slot: String,
    /// True when declared in an `itemActions` array (a per-row action on the
    /// declaring component) rather than an `actions` button group.
    from_item_actions: bool,
    /// Path for error reporting
    path: String,
}

/// Validate all closed circuits in the schema
pub fn validate_all_circuits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        // The bus broadcasts UI-fired events orbital-wide: an action event a
        // SIBLING trait's transition handles (the ui-page-header `actions`
        // shape — the header renders the button, the catalog trait owns
        // CREATE) closes the circuit just as an own-transition would
        // (C-ORPHAN-EVENT-PAGEHEADER-ACTIONS).
        let inline_traits = || {
            orbital.traits.iter().filter_map(|tr| match tr {
                TraitReference::Inline(td) => Some(td),
                _ => None,
            })
        };
        let mut orbital_handled: HashSet<String> = inline_traits()
            .flat_map(|td| extract_handled_events(&td.state_machine))
            .collect();
        // A `listens` route consumes the event under its OWN local name:
        // `listens { ModerationDecisionBrowse.DELETE -> REVERT }` means this
        // orbital handles DELETE — by firing REVERT. Counting only transition
        // triggers misses every route whose target is renamed, so a wired
        // affordance reads as orphaned (std-forum's ModQueueOrbital has no
        // DELETE transition anywhere, only this route). The consumed event is
        // `listens[].event`; `triggers` is already covered above, since it
        // fires a real transition.
        orbital_handled.extend(
            inline_traits()
                .flat_map(|td| td.listens.iter().flatten())
                .map(|l| l.event.clone()),
        );
        // Client-bound traits — page-declared plus the transitive `@trait.X`
        // embed closure. The six ported wiring-lint kinds gating on this
        // (failure-arm pair, modal-shell pair, filter-field-never-written,
        // dead-bodiless-action) exactly as the JS did, for faithfulness: a
        // trait the client never mounts can't strand a user on a dead
        // control. `client-unbound-state-machine` reads the SAME set the
        // opposite way — it fires precisely when a trait is NOT in it.
        let bound = client_bound_traits(orbital);
        ctx.merge(validate_client_bound_state_machines(i, orbital, &bound));
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = super::trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_closed_circuit(trait_def, &path, &orbital_handled, &bound);
                ctx.merge(result);
                ctx.merge(validate_render_action_lifecycle_events(
                    trait_def,
                    &orbital.name,
                    &path,
                ));
                ctx.merge(validate_lifecycle_emit_effects(
                    trait_def,
                    &orbital.name,
                    &path,
                ));
            }
        }
    }
}

/// Overlay slots that require exit transitions. Canonical list lives in
/// `orbital_core::slots` (shared with the desktop renderer); the validator
/// requires every path leaving a state that mounts one of these to clear it
/// with `(render-ui <slot> null)` — otherwise the overlay keeps the previous
/// state's content mounted across the transition (gap #15 in
/// `docs/Almadar_Std_Verification.md`).
use orbital_core::slots::OVERLAY_SLOTS;
use super::sync::content::is_main_slot_render_ui;

/// Events that are valid exit events for overlay slots
const OVERLAY_EXIT_EVENTS: &[&str] = &["CLOSE", "CANCEL", "DISMISS", "SAVE", "SUBMIT", "CONFIRM"];

/// Validate closed circuit for a single trait. `orbital_handled` is the
/// union of EVERY sibling trait's transition triggers AND every event any
/// sibling `listens` for in the same orbital — the orphan-event check accepts
/// a sibling-handled (or emits-declared) action event as a closed circuit,
/// since the bus broadcasts orbital-wide.
pub fn validate_closed_circuit(
    trait_def: &TraitDefinition,
    path: &str,
    orbital_handled: &HashSet<String>,
    bound: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let state_machine = &trait_def.state_machine;

    // Extract information
    let render_ui_effects = extract_render_ui_effects(trait_def, path);
    let item_actions = extract_item_actions(&render_ui_effects);
    let handled_events = extract_handled_events(state_machine);
    let declared_emits: HashSet<String> = trait_def
        .emits
        .iter()
        .flatten()
        .map(|e| e.event.clone())
        .collect();

    // Run validations
    result.merge(validate_orphan_events(
        &item_actions,
        &handled_events,
        orbital_handled,
        &declared_emits,
        path,
    ));
    result.merge(validate_dead_handlers(&item_actions, state_machine, path));
    result.merge(validate_duplicate_events(&item_actions, path));
    result.merge(validate_action_component_match(&item_actions, path));
    result.merge(validate_dead_end_components(&render_ui_effects, path));
    result.merge(validate_unreachable_states(state_machine, path));
    result.merge(validate_state_appropriate_actions(
        &item_actions,
        &render_ui_effects,
        path,
    ));

    // Overlay exit transitions are ERRORS - users will be stuck without them
    result.merge(validate_overlay_exit_transitions(
        &render_ui_effects,
        state_machine,
        path,
    ));

    // Modal-to-inline transitions should include both modal null + main render
    result.merge(validate_modal_exit_completeness(
        &render_ui_effects,
        state_machine,
        path,
    ));

    // L2.8: every exit out of an overlay-mounted state must explicitly
    // clear the overlay slot. Generalises CircuitModalExitIncomplete
    // beyond the modal+main cleanup to "did you actually close the
    // overlay?" — gap #15 / #3 in the std-verification doc.
    result.merge(validate_overlay_close_on_exit(
        &render_ui_effects,
        state_machine,
        path,
    ));

    // Dead-end states are warnings, not errors
    result.merge(validate_dead_end_states(state_machine, path));

    // Ported wiring-lint kind (sub-wave L2) — NOT client-bound gated, the
    // JS predicate has no `bound.has` check in this block (unlike its
    // `failure-arm-missing` sibling below).
    result.merge(validate_async_result_deaf_target(state_machine, path));

    // Ported wiring-lint kinds (sub-wave L1) — a trait the client never
    // mounts (not reachable from any page's `@trait.X` embed closure) can't
    // strand a user, so these gate on `bound` exactly as the JS did.
    if bound.contains(&trait_def.name) {
        result.merge(validate_failure_arms(state_machine, path));
        result.merge(validate_modal_shell_overlay_contract(state_machine, path));
        result.merge(validate_dead_bodiless_actions(state_machine, path));
        // Ported wiring-lint kind (sub-wave L2) — client-bound, same reason.
    }

    result
}

/// Extract render-ui effects from a trait's state machine
fn extract_render_ui_effects(trait_def: &TraitDefinition, base_path: &str) -> Vec<RenderUIInfo> {
    let mut results = Vec::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                if let Some(info) = parse_render_ui_effect(effect, transition, i, j, base_path) {
                    results.push(info);
                }
            }
        }
    }

    results
}

/// Parse a render-ui effect from an s-expression
fn parse_render_ui_effect(
    effect: &SExpression,
    transition: &TransitionDefinition,
    trans_idx: usize,
    effect_idx: usize,
    base_path: &str,
) -> Option<RenderUIInfo> {
    if let SExpression::Array(arr) = effect {
        if arr.is_empty() {
            return None;
        }

        // Check operator
        let operator = match &arr[0] {
            SExpression::String(s) => s.as_str(),
            _ => return None,
        };

        if operator != "render-ui" {
            return None;
        }

        // Get slot
        let slot = if arr.len() > 1 {
            match &arr[1] {
                SExpression::String(s) => s.clone(),
                _ => return None,
            }
        } else {
            return None;
        };

        // Get pattern
        let (pattern_type, props) = if arr.len() > 2 {
            match &arr[2] {
                SExpression::Object(obj) => {
                    let pattern_type = obj.get("type").and_then(|v| {
                        if let SExpression::String(s) = v {
                            Some(s.clone())
                        } else {
                            None
                        }
                    });
                    // Convert SExpression object to serde_json Map for props
                    let props = sexpr_obj_to_json_map(obj);
                    (pattern_type, Some(props))
                }
                _ => (None, None),
            }
        } else {
            (None, None)
        };

        Some(RenderUIInfo {
            slot,
            pattern_type,
            props,
            to_state: transition.to.clone(),
            path: format!(
                "{}.stateMachine.transitions[{}].effects[{}]",
                base_path, trans_idx, effect_idx
            ),
        })
    } else {
        None
    }
}

/// Extract item actions from render-ui effects
/// Checks both `itemActions` (for tables/lists) and `actions` (for headers/buttons/forms)
fn extract_item_actions(render_ui_effects: &[RenderUIInfo]) -> Vec<ItemActionInfo> {
    let mut results = Vec::new();

    for render_ui in render_ui_effects {
        let Some(props) = &render_ui.props else {
            continue;
        };
        let Some(pattern_type) = &render_ui.pattern_type else {
            continue;
        };

        // Check both itemActions (tables, lists) and actions (page-header, form, toolbar)
        for action_prop in ["itemActions", "actions"] {
            let Some(actions_value) = props.get(action_prop) else {
                continue;
            };
            // A conditional (`if`-wrapped) actions config contributes the
            // union of both branches — the runtime evaluates the condition
            // at render time, so descriptors in either branch are live.
            // Multi-leaf paths carry a `.branch[i]` suffix: only one branch
            // renders at a time, so cross-branch repeats are not duplicates.
            let leaves = json_conditional_leaf_values(actions_value);
            let multi_leaf = leaves.len() > 1;
            for (leaf_idx, leaf) in leaves.into_iter().enumerate() {
                let serde_json::Value::Array(actions) = leaf else {
                    continue;
                };

                for action_value in actions {
                    let serde_json::Value::Object(action_obj) = action_value else {
                        continue;
                    };

                    // Get action name: prefer "action" field, fallback to "event", then "label"
                    let action = action_obj
                        .get("action")
                        .or_else(|| action_obj.get("event"))
                        .or_else(|| action_obj.get("label"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let event = action_obj
                        .get("event")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let (Some(action), Some(event)) = (action, event) {
                        results.push(ItemActionInfo {
                            action,
                            event,
                            pattern_type: pattern_type.clone(),
                            in_state: render_ui.to_state.clone(),
                            slot: render_ui.slot.clone(),
                            from_item_actions: action_prop == "itemActions",
                            path: if multi_leaf {
                                format!(
                                    "{}.props.{}.branch[{}]",
                                    render_ui.path, action_prop, leaf_idx
                                )
                            } else {
                                format!("{}.props.{}", render_ui.path, action_prop)
                            },
                        });
                    }
                }
            }
        }
    }

    results
}

/// Extract all events handled by transitions
fn extract_handled_events(state_machine: &StateMachine) -> HashSet<String> {
    state_machine
        .transitions
        .iter()
        .map(|t| t.event.clone())
        .collect()
}

/// Get incoming and outgoing transitions for each state
fn get_state_transitions(
    state_machine: &StateMachine,
) -> HashMap<String, (Vec<&TransitionDefinition>, Vec<&TransitionDefinition>)> {
    let mut result: HashMap<String, (Vec<&TransitionDefinition>, Vec<&TransitionDefinition>)> =
        HashMap::new();

    // Initialize all states
    for state in &state_machine.states {
        result.insert(state.name.clone(), (Vec::new(), Vec::new()));
    }

    // Populate transitions
    for transition in &state_machine.transitions {
        if let Some((incoming, _)) = result.get_mut(&transition.to) {
            incoming.push(transition);
        }
        // Handle StringOrArray for from states
        let from_states = match &transition.from {
            orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
            orbital_core::schema::StringOrArray::Multiple(arr) => {
                arr.iter().map(|s| s.as_str()).collect()
            }
        };
        for from_state in from_states {
            if let Some((_, outgoing)) = result.get_mut(from_state) {
                outgoing.push(transition);
            }
        }
    }

    result
}

/// Detect orphan events (events emitted by UI with no transition handler).
/// A sibling trait's transition or the owner's declared emit closes the
/// circuit too — the bus broadcasts orbital-wide, and an emits-declared
/// action event is an outward contract for another orbital's listener.
fn validate_orphan_events(
    item_actions: &[ItemActionInfo],
    handled_events: &HashSet<String>,
    orbital_handled: &HashSet<String>,
    declared_emits: &HashSet<String>,
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for action in item_actions {
        if !handled_events.contains(&action.event)
            && !orbital_handled.contains(&action.event)
            && !declared_emits.contains(&action.event)
        {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitOrphanEvent,
                    &action.path,
                    format!(
                        "Action '{}' emits event '{}' which has no transition handler",
                        action.action, action.event
                    ),
                )
                .with_suggestion(format!(
                    "Add a transition with event '{}' or remove this action",
                    action.event
                ))
                .with_valid_values(handled_events.iter().cloned().collect()),
            );
        }
    }

    result
}

/// Detect dead handlers: render-ui action fires an event that has no matching
/// transition from the state where the handler renders.
///
/// Unlike `validate_orphan_events` (which checks if the event exists ANYWHERE),
/// this checks if the event has a transition from the SPECIFIC state where the
/// action button is rendered. A button in state "viewing" that fires "EDIT"
/// is a dead handler if there's no transition `from: "viewing", event: "EDIT"`.
fn validate_dead_handlers(
    item_actions: &[ItemActionInfo],
    state_machine: &StateMachine,
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build map: state → set of events with transitions from that state
    let mut state_events: HashMap<&str, HashSet<&str>> = HashMap::new();
    for transition in &state_machine.transitions {
        for from_state in transition.from.states() {
            state_events
                .entry(from_state)
                .or_default()
                .insert(&transition.event);
        }
    }

    // Also collect wildcard transitions (from: "*") — these apply to all states
    let wildcard_events: HashSet<&str> = state_machine
        .transitions
        .iter()
        .filter(|t| t.from.matches("*"))
        .map(|t| t.event.as_str())
        .collect();

    for action in item_actions {
        // Skip if the event is a global orphan (already caught by validate_orphan_events)
        let has_any_transition = state_machine
            .transitions
            .iter()
            .any(|t| t.event == action.event);
        if !has_any_transition {
            continue;
        }

        // Check if this event has a transition from this specific state
        let events_from_state = state_events.get(action.in_state.as_str());
        let has_transition_from_state = events_from_state
            .map(|events| events.contains(action.event.as_str()))
            .unwrap_or(false);
        let has_wildcard = wildcard_events.contains(action.event.as_str());

        if !has_transition_from_state && !has_wildcard {
            // Find which states DO have this transition (for helpful suggestion)
            let valid_states: Vec<String> = state_machine
                .transitions
                .iter()
                .filter(|t| t.event == action.event)
                .flat_map(|t| t.from.states().into_iter().map(|s| s.to_string()))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitDeadHandler,
                    &action.path,
                    format!(
                        "Action '{}' fires event '{}' in state '{}' but no transition handles '{}' from '{}'",
                        action.action, action.event, action.in_state, action.event, action.in_state
                    ),
                )
                .with_suggestion(format!(
                    "Add a transition `from: '{}', event: '{}'` or move this render-ui to a state that handles '{}' (valid states: {})",
                    action.in_state, action.event, action.event,
                    if valid_states.is_empty() { "none".to_string() } else { valid_states.join(", ") }
                )),
            );
        }
    }

    result
}

/// Detect duplicate events within a single render-ui effect
///
/// Duplicates are only a problem if the SAME render-ui effect has the same event
/// multiple times in its actions array. Different transitions rendering the same
/// UI to the same state is valid - only one transition fires at a time.
fn validate_duplicate_events(item_actions: &[ItemActionInfo], _path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut seen: HashMap<String, &ItemActionInfo> = HashMap::new();

    for action in item_actions {
        // Extract the base path (up to the effect) to group actions from the same render-ui
        // Path format: "orbitals[X].traits[Y].stateMachine.transitions[Z].effects[W].props.actions"
        // We want to group by everything up to "props.actions" to detect duplicates within same effect
        let effect_base_path = action
            .path
            .rsplit_once(".props.")
            .map(|(base, _)| base)
            .unwrap_or(&action.path);

        // Key includes the effect path to only flag duplicates within the same render-ui effect,
        // plus the conditional-branch discriminator (empty for literal lists): only one branch
        // of an `if`-wrapped list renders, so a repeat across branches is not a duplicate.
        let branch = action
            .path
            .rsplit_once(".branch[")
            .map(|(_, b)| b)
            .unwrap_or("");
        let key = format!(
            "{}:{}:{}:{}",
            effect_base_path, branch, action.slot, action.event
        );

        if let Some(existing) = seen.get(&key) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitDuplicateEvent,
                    &action.path,
                    format!(
                        "Event '{}' is emitted multiple times in the same render-ui effect (actions '{}' and '{}')",
                        action.event, existing.action, action.action
                    ),
                )
                .with_suggestion("Remove duplicate action or use unique event names"),
            );
        } else {
            seen.insert(key, action);
        }
    }

    result
}

/// Detect action-component mismatches
fn validate_action_component_match(
    item_actions: &[ItemActionInfo],
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for action in item_actions {
        if is_action_invalid_for_component(&action.action, &action.pattern_type) {
            let valid_actions = get_valid_actions_for_component(&action.pattern_type);
            let invalid_actions = get_invalid_actions_for_component(&action.pattern_type);

            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitActionComponentMismatch,
                    &action.path,
                    format!(
                        "Action '{}' is not valid on '{}' component",
                        action.action, action.pattern_type
                    ),
                )
                .with_suggestion(format!(
                    "Valid actions for '{}': {}. Invalid: {}",
                    action.pattern_type,
                    valid_actions.join(", "),
                    invalid_actions.join(", ")
                ))
                .with_valid_values(valid_actions.into_iter().map(|s| s.to_string()).collect()),
            );
        }
    }

    result
}

/// Detect dead-end components (interactive components with no actions)
fn validate_dead_end_components(
    render_ui_effects: &[RenderUIInfo],
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for render_ui in render_ui_effects {
        let Some(pattern_type) = &render_ui.pattern_type else {
            continue;
        };

        if !is_interactive_pattern(pattern_type) {
            continue;
        }

        // Check for itemActions (tables, lists) or actions (headers, forms)
        let has_actions = render_ui.props.as_ref().is_some_and(|p| {
            let has_item_actions = p
                .get("itemActions")
                .and_then(|v| v.as_array())
                .map(|arr| !arr.is_empty())
                .unwrap_or(false);
            let has_actions = p
                .get("actions")
                .and_then(|v| v.as_array())
                .map(|arr| !arr.is_empty())
                .unwrap_or(false);
            has_item_actions || has_actions
        });

        if !has_actions {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitDeadEndComponent,
                    &render_ui.path,
                    format!(
                        "'{}' component in slot '{}' state '{}' has no actions - users cannot interact with it",
                        pattern_type, render_ui.slot, render_ui.to_state
                    ),
                )
                .with_suggestion("Add itemActions or actions array with events like VIEW, EDIT, DELETE"),
            );
        }
    }

    result
}

/// Detect unreachable states (states with no incoming transitions except initial)
fn validate_unreachable_states(state_machine: &StateMachine, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Find initial state
    let initial_state = state_machine
        .states
        .iter()
        .find(|s| s.is_initial == Some(true))
        .map(|s| s.name.as_str());

    let state_transitions = get_state_transitions(state_machine);

    for (state_name, (incoming, _)) in &state_transitions {
        // Skip initial state
        if initial_state == Some(state_name.as_str()) {
            continue;
        }

        if incoming.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitUnreachableState,
                    format!("{}.stateMachine.states", path),
                    format!(
                        "State '{}' has no incoming transitions and cannot be reached",
                        state_name
                    ),
                )
                .with_suggestion("Add a transition to this state or remove it"),
            );
        }
    }

    result
}

/// Validate that overlay slots (modal, drawer) have exit transitions
///
/// When a transition renders content to an overlay slot (modal/drawer), the target state
/// MUST have at least one outgoing transition that leads to a state that doesn't render
/// to the same overlay slot (i.e., a way to exit the overlay).
///
/// This is critical for the closed circuit pattern - users will be stuck if they can't
/// exit an overlay.
fn validate_overlay_exit_transitions(
    render_ui_effects: &[RenderUIInfo],
    state_machine: &StateMachine,
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build a map of state -> outgoing transitions
    let state_transitions = get_state_transitions(state_machine);

    // Build a set of transitions that CLOSE overlays (render null to overlay slot)
    // These are valid exit transitions regardless of target state
    let mut closing_transitions: HashSet<(&str, &str)> = HashSet::new(); // (from_state, event)
    for transition in &state_machine.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if is_overlay_close_effect(effect) {
                    let from_states = match &transition.from {
                        orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
                        orbital_core::schema::StringOrArray::Multiple(arr) => {
                            arr.iter().map(|s| s.as_str()).collect()
                        }
                    };
                    for from_state in from_states {
                        closing_transitions.insert((from_state, &transition.event));
                    }
                }
            }
        }
    }

    // Build a map of state -> slots it renders to (excluding null renders)
    let mut state_to_slots: HashMap<&str, HashSet<&str>> = HashMap::new();
    for render_ui in render_ui_effects {
        // Only track non-null pattern renders
        if render_ui.pattern_type.is_some() {
            state_to_slots
                .entry(&render_ui.to_state)
                .or_default()
                .insert(&render_ui.slot);
        }
    }

    // Find states that render to overlay slots
    for render_ui in render_ui_effects {
        // Only check overlay slots with actual content (not null closes)
        if !OVERLAY_SLOTS.contains(&render_ui.slot.as_str()) {
            continue;
        }

        // Skip null renders (modal closures)
        if render_ui.pattern_type.is_none() {
            continue;
        }

        let target_state = &render_ui.to_state;
        let overlay_slot = &render_ui.slot;

        // Check if the target state has an exit transition
        // An exit transition is one that:
        // 1. Goes to a different state AND closes the overlay (renders null), OR
        // 2. Goes to a state that doesn't render to this overlay slot
        let has_exit_transition = state_transitions
            .get(target_state.as_str())
            .map(|(_, outgoing)| {
                outgoing.iter().any(|t| {
                    // Check if this transition closes the overlay
                    if closing_transitions.contains(&(target_state.as_str(), t.event.as_str())) {
                        return true;
                    }

                    // If transition goes to a different state, check if that state
                    // renders to the same overlay slot
                    if &t.to != target_state {
                        // Get slots the target state renders to
                        let target_slots = state_to_slots.get(t.to.as_str());

                        // If target state doesn't render to this overlay slot, it's an exit
                        match target_slots {
                            Some(slots) => !slots.contains(overlay_slot.as_str()),
                            // If we don't know what slots it renders to, assume it's a valid exit
                            // (conservative approach - don't over-flag)
                            None => true,
                        }
                    } else {
                        // Self-transition - not an exit unless it closes the overlay
                        false
                    }
                })
            })
            .unwrap_or(false);

        if !has_exit_transition {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitNoOverlayExit,
                    &render_ui.path,
                    format!(
                        "State '{}' renders to '{}' slot but has no exit transition. Users will be stuck in this overlay.",
                        target_state, render_ui.slot
                    ),
                )
                .with_suggestion(format!(
                    "Add a transition from '{}' that goes to a state not rendering to '{}' slot. Common exit events: {}",
                    target_state,
                    overlay_slot,
                    OVERLAY_EXIT_EVENTS.join(", ")
                ))
                .with_valid_values(OVERLAY_EXIT_EVENTS.iter().map(|s| s.to_string()).collect()),
            );
        }
    }

    result
}

/// Check if an effect is a render-ui that closes an overlay (renders null to modal/drawer)
fn is_overlay_close_effect(effect: &SExpression) -> bool {
    if let SExpression::Array(arr) = effect {
        if arr.len() >= 3 {
            // Check operator is "render-ui"
            let is_render_ui = matches!(&arr[0], SExpression::String(s) if s == "render-ui");

            // Check slot is overlay
            let is_overlay_slot =
                matches!(&arr[1], SExpression::String(s) if OVERLAY_SLOTS.contains(&s.as_str()));

            // Check pattern is null
            let is_null_pattern = matches!(&arr[2], SExpression::Null);

            return is_render_ui && is_overlay_slot && is_null_pattern;
        }
    }
    false
}

/// Validate that modal-to-inline transitions include both render-ui("modal", null)
/// and render-ui("main", ...). Without both, the main slot goes stale after modal close.
fn validate_modal_exit_completeness(
    render_ui_effects: &[RenderUIInfo],
    state_machine: &StateMachine,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build state → overlay slot map (which states render to modal/drawer)
    let mut overlay_states: HashSet<&str> = HashSet::new();
    for rui in render_ui_effects {
        if OVERLAY_SLOTS.contains(&rui.slot.as_str()) && rui.pattern_type.is_some() {
            overlay_states.insert(&rui.to_state);
        }
    }

    if overlay_states.is_empty() {
        return result;
    }

    // Build state → inline set (states that render to "main" slot)
    let mut inline_states: HashSet<&str> = HashSet::new();
    for rui in render_ui_effects {
        if rui.slot == "main" && rui.pattern_type.is_some() {
            inline_states.insert(&rui.to_state);
        }
    }

    // Check transitions that go FROM an overlay state TO an inline state
    for transition in &state_machine.transitions {
        let from_is_overlay = overlay_states.iter().any(|s| transition.from.matches(s));
        let to_is_inline = inline_states.contains(transition.to.as_str());

        if !from_is_overlay || !to_is_inline {
            continue;
        }

        // This transition exits a modal to an inline state
        // Check effects for both render-ui("modal", null) and render-ui("main", ...)
        let effects = match &transition.effects {
            Some(effs) => effs,
            None => continue,
        };

        let has_modal_null = effects.iter().any(is_overlay_close_effect);
        let has_main_render = effects.iter().any(|e| {
            if let SExpression::Array(arr) = e {
                if arr.len() >= 3 {
                    let is_render_ui =
                        matches!(&arr[0], SExpression::String(s) if s == "render-ui");
                    let is_main = matches!(&arr[1], SExpression::String(s) if s == "main");
                    let is_not_null = !matches!(&arr[2], SExpression::Null);
                    return is_render_ui && is_main && is_not_null;
                }
            }
            false
        });

        if has_modal_null && !has_main_render {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitModalExitIncomplete,
                    format!(
                        "{}.transitions[{}->{}->{}]",
                        path, transition.from, transition.event, transition.to
                    ),
                    format!(
                        "Transition {}--[{}]-->{} closes modal but doesn't re-render main slot. Main view will show stale content.",
                        transition.from, transition.event, transition.to
                    ),
                )
                .with_suggestion(
                    "Add render-ui(\"main\", {...}) alongside render-ui(\"modal\", null) to refresh the main view after closing the modal".to_string()
                ),
            );
        }
    }

    result
}

/// L2.8: when a transition leaves a state that mounts an overlay slot
/// and lands in a state that does NOT mount the same overlay, the
/// transition's effects must include `(render-ui <slot> null)` to
/// explicitly close the overlay. Without that explicit clear, the
/// overlay keeps the previous state's content mounted across the
/// transition — which manifests as gap #15 ("modal slot not clearing
/// after SAVE") in the std-verification doc.
///
/// Generalisation of the existing `validate_modal_exit_completeness`
/// (which checks the inverse case: "you closed the modal but didn't
/// re-render main"). Together they cover both halves of the modal-
/// exit contract; this one extends to drawer + overlay slots too.
///
/// Skipped when the trait has no overlay-mounting state (vacuous).
/// Self-transitions on overlay states (open → open) skip — the
/// overlay is supposed to stay mounted.
fn validate_overlay_close_on_exit(
    render_ui_effects: &[RenderUIInfo],
    state_machine: &StateMachine,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build state → set-of-overlay-slots-mounted map.
    let mut overlay_slots_per_state: HashMap<&str, HashSet<&str>> = HashMap::new();
    for rui in render_ui_effects {
        if OVERLAY_SLOTS.contains(&rui.slot.as_str()) && rui.pattern_type.is_some() {
            overlay_slots_per_state
                .entry(rui.to_state.as_str())
                .or_default()
                .insert(rui.slot.as_str());
        }
    }

    if overlay_slots_per_state.is_empty() {
        return result;
    }

    for transition in &state_machine.transitions {
        let from_states: Vec<&str> = match &transition.from {
            orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
            orbital_core::schema::StringOrArray::Multiple(v) => {
                v.iter().map(|s| s.as_str()).collect()
            }
        };
        for from in from_states {
            let Some(mounted_overlays) = overlay_slots_per_state.get(from) else {
                continue;
            };
            // Self-transition on the overlay state — overlay stays.
            if from == transition.to.as_str() {
                continue;
            }
            // What overlays does the destination state mount?
            let dest_overlays = overlay_slots_per_state
                .get(transition.to.as_str())
                .cloned()
                .unwrap_or_default();

            // For each overlay mounted at `from`, if the destination
            // doesn't mount the same overlay, the transition must
            // explicitly clear it.
            for overlay in mounted_overlays {
                if dest_overlays.contains(overlay) {
                    continue;
                }
                let effects = match &transition.effects {
                    Some(effs) => effs,
                    None => &Vec::new()[..],
                };
                let has_close = effects.iter().any(|e| {
                    if let SExpression::Array(arr) = e {
                        if arr.len() >= 3 {
                            let is_render_ui =
                                matches!(&arr[0], SExpression::String(s) if s == "render-ui");
                            let is_this_slot =
                                matches!(&arr[1], SExpression::String(s) if s == overlay);
                            let is_null = matches!(&arr[2], SExpression::Null);
                            return is_render_ui && is_this_slot && is_null;
                        }
                    }
                    false
                });
                if !has_close {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CircuitModalExitIncomplete,
                            format!(
                                "{}.transitions[{}->{}->{}]",
                                path, transition.from, transition.event, transition.to
                            ),
                            format!(
                                "Transition {}--[{}]-->{} leaves a state mounting '{}' but doesn't include `(render-ui {} null)` to close it. The overlay will stay mounted across the transition.",
                                transition.from, transition.event, transition.to, overlay, overlay,
                            ),
                        )
                        .with_suggestion(format!(
                            "Add `(render-ui {} null)` to the transition's effects to explicitly close the overlay before the new state renders.",
                            overlay
                        )),
                    );
                }
            }
        }
    }

    result
}

/// Detect dead-end states (states with no outgoing transitions)
fn validate_dead_end_states(state_machine: &StateMachine, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect states explicitly marked as terminal
    let terminal_states: HashSet<&str> = state_machine
        .states
        .iter()
        .filter(|s| s.is_terminal == Some(true))
        .map(|s| s.name.as_str())
        .collect();

    let state_transitions = get_state_transitions(state_machine);

    for (state_name, (_, outgoing)) in &state_transitions {
        // Skip states marked as terminal
        if terminal_states.contains(state_name.as_str()) {
            continue;
        }

        if outgoing.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CircuitDeadEndState,
                    format!("{}.stateMachine.states", path),
                    format!(
                        "State '{}' has no outgoing transitions - it may be a dead end",
                        state_name
                    ),
                )
                .with_suggestion(
                    "Add outgoing transitions or mark as terminal state with isTerminal: true",
                ),
            );
        }
    }

    result
}

// ============================================================================
// Ported wiring-lint kinds (sub-wave L1): failure-arm pair + modal-shell pair
// ============================================================================

/// Transitions whose `from === state || from === '*'` — the arms a
/// transition landing IN `state` can be handled by. Mirrors the JS wiring
/// lint's `armsIn`/`handledIn` (`t.from === state || t.from === '*'`)
/// exactly; distinct from `get_state_transitions`'s per-state map, which
/// indexes wildcard arms separately rather than folding them into every
/// state's bucket.
pub(crate) fn arms_in<'a>(
    state_machine: &'a StateMachine,
    state: &str,
) -> Vec<&'a TransitionDefinition> {
    state_machine
        .transitions
        .iter()
        .filter(|t| t.from.matches(state) || t.from.matches("*"))
        .collect()
}

/// True iff `effects` contains a `["render-ui", <any slot>, <non-null>]`
/// node ANYWHERE in the tree, including nested inside `if`/`do` — i.e. this
/// arm actually paints something for the user to see. Mirrors
/// `armPaintsSomething`. Only descends through Arrays (control-flow nodes
/// are arrays; a render-ui's own payload object is a data tree, not a
/// nested effect), the same traversal shape `has_render_ui_effects`
/// (`trait_validator.rs`) uses for a shallower, `if`-only version of this
/// same question.
fn arm_paints_something(effects: &[SExpression]) -> bool {
    fn scan(node: &SExpression) -> bool {
        let SExpression::Array(arr) = node else {
            return false;
        };
        if arr.len() >= 3
            && matches!(&arr[0], SExpression::String(s) if s == "render-ui")
            && !matches!(&arr[2], SExpression::Null)
        {
            return true;
        }
        arr.iter().any(scan)
    }
    effects.iter().any(scan)
}

/// Every `["render-ui", <overlay slot>, payload]` node anywhere in `effects`
/// (including nested inside `if`/`do`), paired with its payload. Scoped to
/// `orbital_core::slots::OVERLAY_SLOTS` (modal/drawer/overlay) — deliberately
/// widened from the JS wiring lint's literal `'modal'|'drawer'` check (D3:
/// the single owner for "which slots the generic portal shell wraps" is
/// `OVERLAY_SLOTS`, not a second two-slot literal). Mirrors
/// `overlaySlotRenderUiPayloads`.
fn overlay_slot_render_ui_payloads(effects: &[SExpression]) -> Vec<&SExpression> {
    fn scan<'a>(node: &'a SExpression, out: &mut Vec<&'a SExpression>) {
        let SExpression::Array(arr) = node else {
            return;
        };
        if arr.len() >= 3 {
            if let (SExpression::String(op), SExpression::String(slot)) = (&arr[0], &arr[1]) {
                if op == "render-ui" && OVERLAY_SLOTS.contains(&slot.as_str()) {
                    out.push(&arr[2]);
                    return;
                }
            }
        }
        for item in arr {
            scan(item, out);
        }
    }
    let mut out = Vec::new();
    for effect in effects {
        scan(effect, &mut out);
    }
    out
}

/// True for a render-ui payload that is an actual pattern config — excludes
/// `null` (an overlay-close) and any string (a `RenderBinding`/`@trait.X`
/// marker) so `.get("type")` is meaningful. Mirrors `isPatternPayload`.
fn is_pattern_payload(payload: &SExpression) -> bool {
    !matches!(payload, SExpression::Null | SExpression::String(_))
}

/// The `type:` field of a pattern-config payload, if any.
fn pattern_type_of(payload: &SExpression) -> Option<&str> {
    if let SExpression::Object(map) = payload {
        if let Some(SExpression::String(t)) = map.get("type") {
            return Some(t.as_str());
        }
    }
    None
}

/// True when `payload` — or ANY node nested inside it (a layout container's
/// `children`, or any other nested field) — carries `slideOver: true`: a
/// pattern that self-manages its own overlay chrome. Recurses through the
/// whole payload tree (both Array and Object) since a self-overlaying
/// pattern usually arrives wrapped in a layout `Stack`. Mirrors
/// `containsSelfOverlayNode`. This walks a resolved DATA tree (the render-ui
/// payload), not effect code, so — unlike `arm_paints_something`'s
/// Array-only descent — it also recurses into Objects.
fn is_self_overlay_node(node: &SExpression) -> bool {
    let SExpression::Object(map) = node else {
        return false;
    };
    if matches!(map.get("slideOver"), Some(SExpression::Bool(true))) {
        return true;
    }
    matches!(
        map.get("type"),
        Some(SExpression::String(t)) if SELF_OVERLAY_PATTERN_TYPES.contains(&t.as_str())
    )
}

/// Whether an overlay-slot `payload` stacks a second overlay chrome under the
/// slot shell. The ROOT node counts only through `slideOver: true`: a root
/// whose TYPE is self-overlaying (`modal`, `confirm-dialog`) is rendered BARE
/// by the slot shell (`UISlotRenderer` skips its Modal wrapper for
/// `SELF_OVERLAY_PATTERN_TYPES`), so it paints exactly one chrome — that is
/// how the generated `ui-modal`/`ui-confirm-dialog` molecules work. Below the
/// root, any self-overlaying node (type or `slideOver`) reachable through
/// `children` or a registry-declared `node`-typed prop (e.g. flip-card's
/// `front`/`back`) is a second chrome inside the shell's. Descends only
/// render-NODE positions, the same positions `validate_pattern` walks for
/// nested pattern configs: NEVER a data prop (`entity`, `fields`, `filter`,
/// `config`, an S-expression call) — a `slideOver: true` key sitting inside a
/// merged row/data payload paints nothing and must stay silent.
fn contains_self_overlay_node(payload: &SExpression) -> bool {
    if let SExpression::Object(map) = payload {
        if matches!(map.get("slideOver"), Some(SExpression::Bool(true))) {
            return true;
        }
    }
    contains_nested_self_overlay_node(payload)
}

fn contains_nested_self_overlay_node(payload: &SExpression) -> bool {
    match payload {
        SExpression::Object(map) => {
            let pattern_type = match map.get("type") {
                Some(SExpression::String(t)) => Some(t.as_str()),
                _ => None,
            };
            map.iter().any(|(key, value)| {
                let is_node_prop = key == "children"
                    || pattern_type
                        .and_then(|pt| get_prop_types(pt, key))
                        .is_some_and(|types| types.contains(&"node"));
                is_node_prop && node_or_descendant_is_self_overlay(value)
            })
        }
        SExpression::Array(items) => items.iter().any(node_or_descendant_is_self_overlay),
        _ => false,
    }
}

fn node_or_descendant_is_self_overlay(node: &SExpression) -> bool {
    is_self_overlay_node(node) || contains_nested_self_overlay_node(node)
}

/// `ORB_CIRCUIT_FAILURE_ARM_MISSING` / `ORB_CIRCUIT_FAILURE_ARM_RENDERS_NOTHING`.
///
/// For every transition whose effects declare an `emit.failure` event, the
/// destination state must have an arm handling that event (missing = no arm
/// at all; renders-nothing = an arm exists but paints nothing) — the async
/// result contract's failure half is otherwise a live wire terminating in
/// nothing. Declared-data only: `collect_failure_events` reads the effect
/// tuple's own `emit: {failure: X}` map, `arms_in`/`arm_paints_something`
/// read transitions and effects structurally. Ported from the JS wiring
/// lint's `failure-arm-missing` / `failure-arm-renders-nothing` (same arm
/// lookup, one pass, two-way branch).
fn validate_failure_arms(state_machine: &StateMachine, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in state_machine.transitions.iter().enumerate() {
        let Some(effects) = &transition.effects else {
            continue;
        };
        let failure_events = collect_failure_events(effects);
        if failure_events.is_empty() {
            continue;
        }
        let arms_at_target = arms_in(state_machine, &transition.to);
        for failure_event in &failure_events {
            let handling_arms: Vec<&&TransitionDefinition> = arms_at_target
                .iter()
                .filter(|t| &t.event == failure_event)
                .collect();
            let effect_path = format!("{}.stateMachine.transitions[{}].effects", path, i);
            if handling_arms.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CircuitFailureArmMissing,
                        &effect_path,
                        format!(
                            "transition {}--[{}]-->{} declares `emit: {{ failure: {} }}` but state '{}' has no arm handling '{}' — the failure is never caught",
                            transition.from, transition.event, transition.to, failure_event, transition.to, failure_event
                        ),
                    )
                    .with_suggestion(format!(
                        "Add a transition `from: '{}', event: '{}'` that handles the failure (e.g. render an error toast).",
                        transition.to, failure_event
                    )),
                );
            } else if !handling_arms
                .iter()
                .any(|t| t.effects.as_deref().is_some_and(arm_paints_something))
            {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CircuitFailureArmRendersNothing,
                        &effect_path,
                        format!(
                            "state '{}' handles failure event '{}' but its arm renders nothing — the user never sees the failure",
                            transition.to, failure_event
                        ),
                    )
                    .with_suggestion(format!(
                        "Add `(render-ui toast {{ type: alert, variant: error, message: ... }})` (or another slot) to the '{}' arm.",
                        failure_event
                    )),
                );
            }
        }
    }

    result
}

/// `ORB_CIRCUIT_MODAL_SHELL_CLOSE_DEAF` / `ORB_CIRCUIT_SELF_OVERLAY_IN_SHELL`.
///
/// Shares one walk over `overlay_slot_render_ui_payloads` (D2: the modal pair
/// is the structural inverse of itself — one condition needs the shell's
/// dismiss control to land somewhere, the other needs the shell to NOT wrap
/// a pattern that already manages its own overlay chrome):
/// - close-deaf: a non-self-overlaying pattern rendered into an overlay slot
///   whose destination state has no `CLOSE`/`CANCEL` arm — the generic
///   portal shell's own X/Escape/overlay-click control fires an event
///   nothing handles.
/// - self-overlay: ANY pattern (regardless of the shell check above) whose
///   payload tree carries `slideOver: true` anywhere — the generic shell
///   wraps it too, stacking two overlay chromes.
///
/// Complementary to (not a replacement for) `validate_overlay_exit_transitions`
/// / `validate_modal_exit_completeness`, which check a broader, different
/// contract (`docs` for the full distinction). Ported from `modal-shell-
/// close-deaf` / `self-overlay-inside-modal-shell`.
fn validate_modal_shell_overlay_contract(
    state_machine: &StateMachine,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in state_machine.transitions.iter().enumerate() {
        let Some(effects) = &transition.effects else {
            continue;
        };
        for payload in overlay_slot_render_ui_payloads(effects) {
            let effect_path = format!("{}.stateMachine.transitions[{}].effects", path, i);

            if contains_self_overlay_node(payload) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CircuitSelfOverlayInShell,
                        &effect_path,
                        format!(
                            "transition {}--[{}]-->{} renders a self-overlaying pattern (`slideOver: true`, or a nested `modal`/`confirm-dialog`) inside an overlay slot payload — the generic portal shell wraps it too, stacking two overlay chromes",
                            transition.from, transition.event, transition.to
                        ),
                    )
                    .with_suggestion(
                        "Render the inner content pattern into the slot instead (the shell already provides the chrome and the X), or make the self-overlaying pattern the slot payload's ROOT node, which the shell renders bare.".to_string(),
                    ),
                );
            }

            if !is_pattern_payload(payload) {
                continue;
            }
            let is_self_overlay = pattern_type_of(payload)
                .is_some_and(|t| SELF_OVERLAY_PATTERN_TYPES.contains(&t));
            if is_self_overlay {
                continue;
            }
            let handled = arms_in(state_machine, &transition.to);
            let has_close_or_cancel = handled
                .iter()
                .any(|t| t.event == "CLOSE" || t.event == "CANCEL");
            if !has_close_or_cancel {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CircuitModalShellCloseDeaf,
                        &effect_path,
                        format!(
                            "state '{}' mounts an overlay-slot pattern but has no `CLOSE`/`CANCEL` arm — the shell's own X/Escape/overlay-click control fires into nothing",
                            transition.to
                        ),
                    )
                    .with_suggestion(format!(
                        "Add a `CLOSE` (or `CANCEL`) arm in state '{}' that clears the overlay slot.",
                        transition.to
                    )),
                );
            }
        }
    }

    result
}

/// Validate state-inappropriate actions
fn validate_state_appropriate_actions(
    item_actions: &[ItemActionInfo],
    render_ui_effects: &[RenderUIInfo],
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build a map of state -> pattern types rendered in that state
    let mut state_patterns: HashMap<&str, HashSet<&str>> = HashMap::new();
    for render_ui in render_ui_effects {
        if let Some(pattern_type) = &render_ui.pattern_type {
            state_patterns
                .entry(&render_ui.to_state)
                .or_default()
                .insert(pattern_type.as_str());
        }
    }

    for action in item_actions {
        // A per-row `itemActions` entry is a row action on its declaring
        // component, never a form/list button — its event name carries no
        // form or list semantics (a data-list row legitimately named CANCEL
        // is "cancel this item", not a form cancel). Only `actions` button
        // groups are subject to the state-appropriateness name checks.
        if action.from_item_actions {
            continue;
        }
        let patterns_in_state = state_patterns.get(action.in_state.as_str());

        // Check form actions in non-form states
        if is_form_action(&action.action) {
            if let Some(patterns) = patterns_in_state {
                let has_form_pattern = patterns.iter().any(|p| is_form_pattern(p));
                if !has_form_pattern && !patterns.is_empty() {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CircuitActionComponentMismatch,
                            &action.path,
                            format!(
                                "Form action '{}' in state '{}' which renders non-form patterns: {:?}",
                                action.action, action.in_state, patterns
                            ),
                        )
                        .with_suggestion("Form actions should be used in states that render form patterns"),
                    );
                }
            }
        }

        // Check list actions in non-list states
        if is_list_action(&action.action) {
            if let Some(patterns) = patterns_in_state {
                let has_list_pattern = patterns.iter().any(|p| is_list_pattern(p));
                if !has_list_pattern && !patterns.is_empty() {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CircuitActionComponentMismatch,
                            &action.path,
                            format!(
                                "List action '{}' in state '{}' which renders non-list patterns: {:?}",
                                action.action, action.in_state, patterns
                            ),
                        )
                        .with_suggestion("List actions should be used in states that render list/table patterns"),
                    );
                }
            }
        }
    }

    result
}

/// ORB_RENDER_ACTION_LIFECYCLE_EVENT: a rendered affordance's event-bearing
/// prop names a lifecycle event (`INIT`/`LOAD`/`$MOUNT`). The bus never
/// re-delivers these (`orbital_core::events::BUS_EXCLUDED_EVENTS`), so the
/// control does nothing when operated — dead on click.
///
/// Walks every render-ui effect's whole props tree (covers a top-level
/// prop like `action`/`submitEvent`/`onChange` AND a nested descriptor —
/// `itemActions`/`actions`/literal `children` — since the registry's
/// event-prop vocabulary, `ui_event_prop_names()`, is keyed by NAME across
/// every pattern, not scoped to the outer pattern's own `propsSchema`).
fn validate_render_action_lifecycle_events(
    trait_def: &TraitDefinition,
    orbital_name: &str,
    base_path: &str,
) -> ValidationResult {
    let render_ui_effects = extract_render_ui_effects(trait_def, base_path);
    validate_render_action_lifecycle_events_for_effects(
        &render_ui_effects,
        orbital_name,
        &trait_def.name,
    )
}

/// Effects-level half of `validate_render_action_lifecycle_events`, split
/// out so tests can drive it with fabricated `RenderUIInfo`s the same way
/// every other check in this file is tested — never touches `listens`,
/// which is a wholly separate `TraitDefinition` field this rule never reads.
fn validate_render_action_lifecycle_events_for_effects(
    render_ui_effects: &[RenderUIInfo],
    orbital_name: &str,
    trait_name: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let event_keys = ui_event_prop_names();

    for render_ui in render_ui_effects {
        let Some(props) = &render_ui.props else {
            continue;
        };
        let outer_pattern_type = render_ui.pattern_type.as_deref().unwrap_or("<unknown>");
        let mut hits = Vec::new();
        for (key, v) in props.iter() {
            if event_keys.contains(key) {
                if let serde_json::Value::String(s) = v {
                    if is_bus_excluded_event(s) {
                        hits.push((key.clone(), s.clone(), outer_pattern_type.to_string()));
                    }
                }
            }
            find_lifecycle_targeted_props(v, outer_pattern_type, event_keys, &mut hits);
        }
        for (prop_key, event, pattern_type) in hits {
            result.add_error(
                ValidationError::error(
                    ErrorCode::RenderActionLifecycleEvent,
                    format!("{}.props.{}", render_ui.path, prop_key),
                    format!(
                        "'{}.{}': prop '{}' on rendered '{}' targets lifecycle event '{}' — the bus never delivers it",
                        orbital_name, trait_name, prop_key, pattern_type, event
                    ),
                )
                .with_suggestion(
                    "route a named event (e.g. REFRESH) whose arm re-enters via `EVENT -> loading`; \
                     lifecycle events are never affordance targets",
                ),
            );
        }
    }

    result
}

/// Recursively collect `(prop key, event, owning pattern type)` triples
/// whose key is a registry event-bearing prop name and whose value is a
/// literal string the bus never delivers. Descends through nested
/// descriptor objects/arrays (`itemActions`, `actions`, literal `children`)
/// the same way the props tree is actually shaped, tracking the nearest
/// enclosing `type:` so a `Button action={INIT}` nested inside a `stack`'s
/// `children` reports `button`, not the outer container.
fn find_lifecycle_targeted_props(
    value: &serde_json::Value,
    pattern_type: &str,
    event_keys: &HashSet<String>,
    out: &mut Vec<(String, String, String)>,
) {
    match value {
        serde_json::Value::Object(obj) => {
            let pattern_type = obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or(pattern_type);
            for (key, v) in obj.iter() {
                if event_keys.contains(key) {
                    if let serde_json::Value::String(s) = v {
                        if is_bus_excluded_event(s) {
                            out.push((key.clone(), s.clone(), pattern_type.to_string()));
                        }
                    }
                }
                find_lifecycle_targeted_props(v, pattern_type, event_keys, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                find_lifecycle_targeted_props(item, pattern_type, event_keys, out);
            }
        }
        _ => {}
    }
}

/// `ORB_CIRCUIT_LIFECYCLE_EMIT_UNDELIVERED`: sibling production site to
/// [`validate_render_action_lifecycle_events`] — that check catches a
/// RENDERED affordance whose event-bearing prop targets a lifecycle event;
/// this one catches the same dead end reached through an explicit `(emit
/// <event>)` effect instead. Both read the same `is_bus_excluded_event`
/// fact. Not client-bound gated: the emit is undelivered whoever runs it.
///
/// Walks each transition's effects recursively (through `if`/`do`, the same
/// descent as `collect_failure_events`) — the JS twin only scanned the top
/// level, which was a hole, not a semantic: a nested `(emit INIT)` is
/// exactly as undelivered as a bare one.
fn validate_lifecycle_emit_effects(
    trait_def: &TraitDefinition,
    orbital_name: &str,
    base_path: &str,
) -> ValidationResult {
    fn collect_lifecycle_emits<'a>(expr: &'a SExpression, out: &mut Vec<&'a str>) {
        let SExpression::Array(items) = expr else {
            return;
        };
        if let [SExpression::String(op), SExpression::String(event), ..] = items.as_slice() {
            if op == "emit" && is_bus_excluded_event(event) {
                out.push(event);
            }
        }
        for item in items {
            collect_lifecycle_emits(item, out);
        }
    }

    let mut result = ValidationResult::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let Some(effects) = &transition.effects else {
            continue;
        };
        for (j, effect) in effects.iter().enumerate() {
            let mut emitted = Vec::new();
            collect_lifecycle_emits(effect, &mut emitted);
            for event in emitted {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CircuitLifecycleEmitUndelivered,
                        format!("{}.stateMachine.transitions[{}].effects[{}]", base_path, i, j),
                        format!(
                            "'{}.{}': transition {}--[{}]-->{} emits lifecycle event '{}' — the bus never delivers it",
                            orbital_name, trait_def.name, transition.from, transition.event, transition.to, event
                        ),
                    )
                    .with_suggestion(
                        "route a named event whose arm re-enters via `EVENT -> ...`, or inline the target state's own effects directly into this arm; lifecycle events are never delivered through (emit ...)",
                    ),
                );
            }
        }
    }

    result
}

/// `ORB_CIRCUIT_CLIENT_UNBOUND_STATE_MACHINE`: an inline trait owns a
/// non-empty `stateMachine.transitions` but is absent from
/// `client_bound_traits(orbital)` — no page declares it and no page's
/// `@trait.X` embed closure reaches it, so the client never mounts a state
/// machine for it and it can never handle an event in the shipped app.
/// `client_bound_traits` (`embed_graph.rs`) already computes the exact
/// page-seeded BFS the JS predicate reads (`wiring-lint.ts:850-873`); this
/// check is a direct consumer, run once per orbital rather than per-trait
/// since it needs the whole trait list to report by exclusion.
fn validate_client_bound_state_machines(
    orbital_index: usize,
    orbital: &Orbital,
    bound: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (j, trait_ref) in orbital.traits.iter().enumerate() {
        let TraitReference::Inline(trait_def) = trait_ref else {
            continue;
        };
        let transition_count = trait_def.state_machine.transitions.len();
        if transition_count == 0 {
            continue;
        }
        if bound.contains(&trait_def.name) {
            continue;
        }
        let path = super::trait_path(orbital_index, &orbital.name, j, trait_ref);
        // Courtesy suggestion only — not part of the predicate above. Names
        // pages that already declare, directly, one of THIS trait's own
        // `@trait.X` children (mirrors the JS's own `childPages`), since
        // that page is the natural place to also add this trait.
        let child_pages = pages_embedding_children_of(orbital, &trait_def.name);
        let suggestion = if child_pages.is_empty() {
            format!(
                "declare {} on a page or embed it via @trait.{}",
                trait_def.name, trait_def.name
            )
        } else {
            format!(
                "add {} to the page decl already mounting one of its embedded children ({})",
                trait_def.name,
                child_pages.join(", ")
            )
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitClientUnboundStateMachine,
                &path,
                format!(
                    "'{}.{}' owns a state machine ({} transition(s)) but is in no page decl and no page's @trait embed closure — the client never binds it, so it can never handle an event in the shipped app",
                    orbital.name, trait_def.name, transition_count
                ),
            )
            .with_suggestion(suggestion),
        );
    }

    result
}

/// `ORB_CIRCUIT_DEAD_BODILESS_ACTION`: client-bound gated (an unbound
/// trait's arms never run in the shipped client at all). `painted` is the
/// set of `to` states reached by a transition whose effects paint a main
/// slot; an arm that moves `from -> to` (not a self-transition) with EMPTY
/// effects out of a `from` state some other arm already painted fires — the
/// machine leaves that state but the screen keeps its old paint, so every
/// affordance routed here is dead. Ported from the JS wiring lint's
/// `dead-bodiless-action`; a `from` list is evaluated per element (the JS
/// twin's `===`/`Set` comparisons silently skipped every list-form arm).
fn validate_dead_bodiless_actions(state_machine: &StateMachine, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    let painted: HashSet<&str> = state_machine
        .transitions
        .iter()
        .filter(|t| {
            t.effects
                .as_deref()
                .is_some_and(|effects| effects.iter().any(is_main_slot_render_ui))
        })
        .map(|t| t.to.as_str())
        .collect();

    for (i, transition) in state_machine.transitions.iter().enumerate() {
        if transition.effects.as_deref().is_some_and(|e| !e.is_empty()) {
            continue;
        }
        // Each element of a `from` list is a real source state: the arm is
        // dead from every painted one it actually leaves (a self-edge keeps
        // its paint and is not a departure).
        let from_states: Vec<&str> = match &transition.from {
            StringOrArray::Single(s) => vec![s.as_str()],
            StringOrArray::Multiple(list) => list.iter().map(String::as_str).collect(),
        };
        let leaves_a_painted_state = from_states
            .iter()
            .any(|s| *s != transition.to && painted.contains(s));
        if !leaves_a_painted_state {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitDeadBodilessAction,
                format!("{}.stateMachine.transitions[{}]", path, i),
                format!(
                    "'{}' moves {} -> {} but carries no effects — the machine leaves {} while the screen keeps its paint, so every affordance routed here is dead and the surface now disagrees with the state it is in",
                    transition.event, transition.from, transition.to, transition.from
                ),
            )
            .with_suggestion(format!(
                "give the arm the body '{}' should show — copy {}'s INIT render (and its fetch, if any) into the arm. An (emit INIT) is not a substitute: the bus never delivers lifecycle events",
                transition.to, transition.to
            )),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
    };

    fn create_test_state_machine() -> StateMachine {
        StateMachine {
            states: vec![
                StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
                StateDefinition {
                    name: "viewing".to_string(),
                    is_initial: Some(false),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: None,
            transitions: vec![TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "viewing".to_string(),
                event: "VIEW".to_string(),
                guard: None,
                effects: None,
            }],
        }
    }

    #[test]
    fn test_extract_handled_events() {
        let sm = create_test_state_machine();
        let events = extract_handled_events(&sm);
        assert!(events.contains("VIEW"));
    }

    #[test]
    fn test_orphan_event_detection() {
        let item_actions = vec![ItemActionInfo {
            action: "VIEW".to_string(),
            event: "VIEW".to_string(),
            pattern_type: "entity-table".to_string(),
            in_state: "idle".to_string(),
            slot: "main".to_string(),
            from_item_actions: true,
            path: "test.path".to_string(),
        }];

        let empty: HashSet<String> = HashSet::new();
        let handled_events: HashSet<String> = HashSet::new(); // Empty - no handlers
        let result = validate_orphan_events(&item_actions, &handled_events, &empty, &empty, "test");
        assert!(!result.is_valid()); // Should fail - orphan event

        let mut handled_events_with_view = HashSet::new();
        handled_events_with_view.insert("VIEW".to_string());
        let result2 = validate_orphan_events(
            &item_actions,
            &handled_events_with_view,
            &empty,
            &empty,
            "test",
        );
        assert!(result2.is_valid()); // Should pass - event is handled

        // A SIBLING trait's transition closes the circuit too (page-header
        // actions shape); so does a declared emit.
        let mut sibling_handled = HashSet::new();
        sibling_handled.insert("VIEW".to_string());
        let result3 = validate_orphan_events(
            &item_actions,
            &handled_events,
            &sibling_handled,
            &empty,
            "test",
        );
        assert!(
            result3.is_valid(),
            "sibling-handled event must close the circuit"
        );

        let mut declared = HashSet::new();
        declared.insert("VIEW".to_string());
        let result4 =
            validate_orphan_events(&item_actions, &handled_events, &empty, &declared, "test");
        assert!(
            result4.is_valid(),
            "emits-declared event must close the circuit"
        );
    }

    #[test]
    fn test_action_component_mismatch() {
        let item_actions = vec![ItemActionInfo {
            action: "SAVE".to_string(), // SAVE is invalid on entity-table
            event: "SAVE".to_string(),
            pattern_type: "entity-table".to_string(),
            in_state: "idle".to_string(),
            slot: "main".to_string(),
            from_item_actions: true,
            path: "test.path".to_string(),
        }];

        let result = validate_action_component_match(&item_actions, "test");
        assert!(!result.is_valid()); // SAVE is invalid on entity-table
    }

    fn render_ui_info(pattern_type: &str, to_state: &str) -> RenderUIInfo {
        RenderUIInfo {
            slot: "main".to_string(),
            pattern_type: Some(pattern_type.to_string()),
            props: None,
            to_state: to_state.to_string(),
            path: "test.path".to_string(),
        }
    }

    fn item_action(action: &str, in_state: &str, from_item_actions: bool) -> ItemActionInfo {
        ItemActionInfo {
            action: action.to_string(),
            event: action.to_string(),
            pattern_type: "data-list".to_string(),
            in_state: in_state.to_string(),
            slot: "main".to_string(),
            from_item_actions,
            path: "test.path".to_string(),
        }
    }

    #[test]
    fn test_row_action_named_cancel_is_not_a_form_action() {
        // A data-list row action named CANCEL ("cancel this appointment") in a
        // state with no form pattern must NOT trip the form-action name check.
        let actions = vec![item_action("CANCEL", "idle", true)];
        let renders = vec![render_ui_info("data-list", "idle")];
        let result = validate_state_appropriate_actions(&actions, &renders, "test");
        assert!(result.is_valid());
    }

    #[test]
    fn test_button_group_cancel_in_non_form_state_still_flagged() {
        // The same name in an `actions` button group (e.g. on an alert) keeps
        // the state-appropriateness check.
        let actions = vec![item_action("CANCEL", "idle", false)];
        let renders = vec![render_ui_info("alert", "idle")];
        let result = validate_state_appropriate_actions(&actions, &renders, "test");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CircuitActionComponentMismatch));
    }

    #[test]
    fn test_unreachable_state() {
        let sm = StateMachine {
            states: vec![
                StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
                StateDefinition {
                    name: "orphan".to_string(), // No incoming transitions
                    is_initial: Some(false),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: None,
            transitions: vec![], // No transitions
        };

        let result = validate_unreachable_states(&sm, "test");
        assert!(!result.is_valid()); // orphan state is unreachable
    }

    fn json_descriptor(event: &str) -> serde_json::Value {
        serde_json::json!({"name": "Edit", "action": event, "event": event})
    }

    fn render_ui_info_with_item_actions(
        pattern_type: &str,
        item_actions: serde_json::Value,
    ) -> RenderUIInfo {
        let mut props = serde_json::Map::new();
        props.insert("itemActions".to_string(), item_actions);
        RenderUIInfo {
            slot: "main".to_string(),
            pattern_type: Some(pattern_type.to_string()),
            props: Some(props),
            to_state: "idle".to_string(),
            path: "test.path".to_string(),
        }
    }

    /// `extract_item_actions` on a conditional (`if`-wrapped) `itemActions`
    /// value must union descriptors from BOTH branches — a descriptor that
    /// only fires in one branch is still a legal action.
    #[test]
    fn extract_item_actions_unions_conditional_branches() {
        let item_actions = serde_json::json!([
            "if",
            ["=", "@user.role", "reader"],
            [json_descriptor("EDIT_ITEM")],
            [json_descriptor("VIEW_ITEM")]
        ]);
        let render_ui_effects = vec![render_ui_info_with_item_actions("data-list", item_actions)];
        let actions = extract_item_actions(&render_ui_effects);
        let events: HashSet<String> = actions.into_iter().map(|a| a.event).collect();
        assert!(
            events.contains("EDIT_ITEM"),
            "then-branch missing: {:?}",
            events
        );
        assert!(
            events.contains("VIEW_ITEM"),
            "else-branch missing: {:?}",
            events
        );
    }

    /// Literal (non-conditional) `itemActions` extraction is unaffected —
    /// byte-identical to the pre-fix direct-array path.
    #[test]
    fn extract_item_actions_still_reads_literal_list() {
        let item_actions = serde_json::json!([json_descriptor("EDIT_ITEM")]);
        let render_ui_effects = vec![render_ui_info_with_item_actions("data-list", item_actions)];
        let actions = extract_item_actions(&render_ui_effects);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].event, "EDIT_ITEM");
        assert_eq!(actions[0].path, "test.path.props.itemActions");
    }

    /// The same event in BOTH branches of a conditional list is not a
    /// duplicate — only one branch renders at a time. The same event twice
    /// WITHIN one branch still is.
    #[test]
    fn duplicate_events_are_branch_scoped() {
        let cross_branch = serde_json::json!([
            "if",
            ["=", "@user.role", "reader"],
            [json_descriptor("VIEW_ITEM")],
            [json_descriptor("VIEW_ITEM"), json_descriptor("EDIT_ITEM")]
        ]);
        let effects = vec![render_ui_info_with_item_actions("data-list", cross_branch)];
        let result = validate_duplicate_events(&extract_item_actions(&effects), "test");
        assert!(
            result.is_valid(),
            "cross-branch repeat flagged as duplicate: {:?}",
            result.errors
        );

        let within_branch = serde_json::json!([
            "if",
            ["=", "@user.role", "reader"],
            [json_descriptor("VIEW_ITEM"), json_descriptor("VIEW_ITEM")],
            [json_descriptor("EDIT_ITEM")]
        ]);
        let effects = vec![render_ui_info_with_item_actions("data-list", within_branch)];
        let result = validate_duplicate_events(&extract_item_actions(&effects), "test");
        assert!(!result.is_valid(), "within-branch duplicate not flagged");
    }

    fn render_ui_info_with_props(pattern_type: &str, props: serde_json::Value) -> RenderUIInfo {
        let serde_json::Value::Object(props) = props else {
            panic!("props must be a JSON object");
        };
        RenderUIInfo {
            slot: "main".to_string(),
            pattern_type: Some(pattern_type.to_string()),
            props: Some(props),
            to_state: "idle".to_string(),
            path: "test.path".to_string(),
        }
    }

    /// `action: INIT` on a button is dead — the bus never re-delivers INIT.
    #[test]
    fn dead_lifecycle_action_on_button_errors() {
        let render_ui =
            render_ui_info_with_props("button", serde_json::json!({ "action": "INIT" }));
        let result =
            validate_render_action_lifecycle_events_for_effects(&[render_ui], "TestOrbital", "TestTrait");
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::RenderActionLifecycleEvent)
            .collect();
        assert_eq!(hits.len(), 1, "{:?}", result.errors);
    }

    /// An `itemActions[].event` targeting INIT is the same defect, one level
    /// nested — the registry's event-prop vocabulary is name-keyed across
    /// the whole props tree, not scoped to the outer pattern's own props.
    #[test]
    fn dead_lifecycle_item_action_errors() {
        let item_actions = serde_json::json!([{ "event": "INIT" }]);
        let render_ui_effects = vec![render_ui_info_with_item_actions("data-list", item_actions)];
        let result = validate_render_action_lifecycle_events_for_effects(
            &render_ui_effects,
            "TestOrbital",
            "TestTrait",
        );
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderActionLifecycleEvent));
    }

    /// A named, non-lifecycle event is a legitimate live affordance.
    #[test]
    fn live_named_action_is_not_flagged() {
        let render_ui =
            render_ui_info_with_props("button", serde_json::json!({ "action": "REFRESH" }));
        let result =
            validate_render_action_lifecycle_events_for_effects(&[render_ui], "TestOrbital", "TestTrait");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A `listens { X.EVT -> INIT }` route lives entirely in
    /// `TraitDefinition.listens` — a structurally separate field this rule
    /// never reads (it only walks render-ui props trees), so a trait whose
    /// only wiring for INIT is such a route must stay silent even through
    /// the full `TraitDefinition`-level entry point.
    #[test]
    fn listens_route_to_init_is_not_an_affordance() {
        let trait_def = TraitDefinition {
            name: "TestTrait".to_string(),
            id: None,
            entity_rebindable: None,
            entity_contract: None,
            effect_row: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
            emits: None,
            listens: Some(vec![orbital_core::schema::ListenDefinition {
                event: "EVT".to_string(),
                event_id: None,
                triggers: Some("INIT".to_string()),
                triggers_id: None,
                scope: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
                source: Some(orbital_core::schema::ListenSource::Trait {
                    trait_name: "X".to_string(),
                    trait_id: None,
                }),
            }]),
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
                    event: "INIT".to_string(),
                    event_id: None,
                    guard: None,
                    effects: None,
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            local: false,
            source_behavior: None,
            source_entity_definition: None,
            type_params: None,
            type_args: None,
        };

        let result = validate_render_action_lifecycle_events(&trait_def, "TestOrbital", "test.path");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    // --- ORB_CIRCUIT_FAILURE_ARM_MISSING / ORB_CIRCUIT_FAILURE_ARM_RENDERS_NOTHING ---

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
            effects: if effects.is_empty() { None } else { Some(effects) },
        }
    }

    fn state(name: &str) -> StateDefinition {
        StateDefinition {
            name: name.to_string(),
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        }
    }

    #[test]
    fn failure_arm_missing_when_no_arm_handles_the_failure_event() {
        let sm = StateMachine {
            states: vec![state("loading")],
            events: None,
            transitions: vec![sm_transition(
                "loading",
                "loading",
                "INIT",
                effects_from(serde_json::json!([
                    ["fetch", "Item", { "emit": { "failure": "LoadFailed" } }]
                ])),
            )],
        };
        let result = validate_failure_arms(&sm, "test");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CircuitFailureArmMissing));
    }

    #[test]
    fn failure_arm_renders_nothing_when_the_handling_arm_is_mute() {
        let sm = StateMachine {
            states: vec![state("loading"), state("error")],
            events: None,
            transitions: vec![
                sm_transition(
                    "loading",
                    "loading",
                    "INIT",
                    effects_from(serde_json::json!([
                        ["fetch", "Item", { "emit": { "failure": "LoadFailed" } }]
                    ])),
                ),
                sm_transition(
                    "loading",
                    "error",
                    "LoadFailed",
                    effects_from(serde_json::json!([["render-ui", "main", null]])),
                ),
            ],
        };
        let result = validate_failure_arms(&sm, "test");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitFailureArmRendersNothing),
            "{:?}",
            result.errors
        );
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitFailureArmMissing),
            "an existing (if mute) arm is not ALSO 'missing'"
        );
    }

    #[test]
    fn failure_arm_silent_when_the_arm_paints_a_toast() {
        let sm = StateMachine {
            states: vec![state("loading"), state("error")],
            events: None,
            transitions: vec![
                sm_transition(
                    "loading",
                    "loading",
                    "INIT",
                    effects_from(serde_json::json!([
                        ["fetch", "Item", { "emit": { "failure": "LoadFailed" } }]
                    ])),
                ),
                sm_transition(
                    "loading",
                    "error",
                    "LoadFailed",
                    effects_from(serde_json::json!([
                        ["render-ui", "toast", { "type": "alert", "variant": "error" }]
                    ])),
                ),
            ],
        };
        let result = validate_failure_arms(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    // --- ORB_CIRCUIT_MODAL_SHELL_CLOSE_DEAF / ORB_CIRCUIT_SELF_OVERLAY_IN_SHELL ---

    #[test]
    fn modal_shell_close_deaf_when_no_close_or_cancel_arm_exists() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", { "type": "detail-panel" }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CircuitModalShellCloseDeaf));
    }

    /// A `SELF_OVERLAY_PATTERN_TYPES` body (`type: "modal"`) manages its own
    /// dismiss chrome — the generic shell's close-deaf contract does not
    /// apply to it, even with no `CLOSE`/`CANCEL` arm.
    #[test]
    fn modal_shell_close_deaf_silent_for_self_overlay_pattern_type() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", { "type": "modal" }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitModalShellCloseDeaf),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn modal_shell_close_deaf_silent_when_a_cancel_arm_exists() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![
                sm_transition(
                    "idle",
                    "open",
                    "OPEN",
                    effects_from(serde_json::json!([
                        ["render-ui", "modal", { "type": "form" }]
                    ])),
                ),
                sm_transition("open", "idle", "CANCEL", effects_from(serde_json::json!([]))),
            ],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn self_overlay_hit_when_a_nested_child_has_slide_over_true() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", {
                        "type": "stack",
                        "children": [{ "type": "detail-panel", "slideOver": true }]
                    }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitSelfOverlayInShell),
            "{:?}",
            result.errors
        );
    }

    /// A `slideOver: true` key sitting inside a DATA expression (here, the
    /// form-section's `entity:` merge) is not a rendered node — `entity` is
    /// not `children` and is not declared `node`-typed, so the walk must
    /// never reach it.
    #[test]
    fn self_overlay_silent_when_slide_over_is_inside_a_data_expression() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", {
                        "type": "form-section",
                        "entity": ["object/merge", "@entity.seedRow",
                            ["object/merge", "?row", { "slideOver": true }]]
                    }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitSelfOverlayInShell),
            "{:?}",
            result.errors
        );
    }

    /// The modal-slot payload's ROOT is a `SELF_OVERLAY_PATTERN_TYPES` node
    /// (`type: "modal"`): the slot shell renders it bare (no Modal wrapper),
    /// so exactly one chrome paints — the generated `ui-modal` molecule's
    /// shape. Silent.
    #[test]
    fn self_overlay_silent_when_the_root_payload_is_a_self_overlay_type_the_shell_unwraps() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", {
                        "type": "modal",
                        "children": [{ "type": "form-section" }]
                    }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitSelfOverlayInShell),
            "{:?}",
            result.errors
        );
    }

    /// A `modal` NESTED under a plain stack in the modal slot: the shell wraps
    /// the stack in its Modal, and the inner modal paints a second chrome.
    #[test]
    fn self_overlay_hit_when_a_self_overlay_type_is_nested_under_the_shell() {
        let sm = StateMachine {
            states: vec![state("idle"), state("open")],
            events: None,
            transitions: vec![sm_transition(
                "idle",
                "open",
                "OPEN",
                effects_from(serde_json::json!([
                    ["render-ui", "modal", {
                        "type": "stack",
                        "children": [{ "type": "modal", "children": [{ "type": "form-section" }] }]
                    }]
                ])),
            )],
        };
        let result = validate_modal_shell_overlay_contract(&sm, "test");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitSelfOverlayInShell),
            "{:?}",
            result.errors
        );
    }

    // --- ORB_CIRCUIT_LIFECYCLE_EMIT_UNDELIVERED ---

    fn trait_def_json(json_val: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(json_val).expect("trait definition fixture deserializes")
    }

    #[test]
    fn lifecycle_emit_hit_when_a_transition_explicitly_emits_a_lifecycle_event() {
        let trait_def = trait_def_json(serde_json::json!({
            "name": "TestTrait",
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{
                    "from": "idle", "to": "idle", "event": "RETRY",
                    "effects": [["emit", "INIT"]]
                }]
            }
        }));
        let result = validate_lifecycle_emit_effects(&trait_def, "TestOrbital", "test.path");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitLifecycleEmitUndelivered),
            "{:?}",
            result.errors
        );
    }

    /// A named, non-lifecycle emit is a legitimate cross-trait signal.
    #[test]
    fn lifecycle_emit_silent_for_a_named_event() {
        let trait_def = trait_def_json(serde_json::json!({
            "name": "TestTrait",
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{
                    "from": "idle", "to": "idle", "event": "SAVE",
                    "effects": [["emit", "SAVED"]]
                }]
            }
        }));
        let result = validate_lifecycle_emit_effects(&trait_def, "TestOrbital", "test.path");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn lifecycle_emit_hit_when_the_emit_is_nested_inside_an_if() {
        let trait_def = trait_def_json(serde_json::json!({
            "name": "TestTrait",
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{
                    "from": "idle", "to": "idle", "event": "RETRY",
                    "effects": [["if", true, [["emit", "INIT"]], []]]
                }]
            }
        }));
        let result = validate_lifecycle_emit_effects(&trait_def, "TestOrbital", "test.path");
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert_eq!(result.errors[0].code, ErrorCode::CircuitLifecycleEmitUndelivered);
    }

    // --- ORB_CIRCUIT_CLIENT_UNBOUND_STATE_MACHINE ---

    fn orbital_json(traits: Vec<serde_json::Value>, pages: serde_json::Value) -> Orbital {
        use orbital_core::schema::EntityRef;
        let traits: Vec<TraitReference> = traits
            .into_iter()
            .map(|t| serde_json::from_value(t).expect("trait reference fixture deserializes"))
            .collect();
        let pages: Vec<orbital_core::schema::PageRef> =
            serde_json::from_value(pages).expect("pages fixture deserializes");
        Orbital {
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Reference("Item".to_string()),
            auxiliary_entities: None,
            traits,
            pages: Some(pages),
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

    fn inline_trait_with_a_transition(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }]
            }
        })
    }

    #[test]
    fn client_unbound_state_machine_hit_when_no_page_or_embed_reaches_the_trait() {
        let orbital = orbital_json(
            vec![inline_trait_with_a_transition("Bound"), inline_trait_with_a_transition("Orphan")],
            serde_json::json!([{ "name": "Home", "path": "/home", "traits": [{ "ref": "Bound" }] }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitClientUnboundStateMachine
                    && e.message.contains("Orphan")),
            "{:?}",
            result.errors
        );
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.message.contains("'TestOrbital.Bound'")),
            "a bound trait must stay silent: {:?}",
            result.errors
        );
    }

    /// A trait with an empty `stateMachine.transitions` has nothing to bind
    /// — not a defect, so it must stay silent even though it is unbound.
    #[test]
    fn client_unbound_state_machine_silent_when_transitions_are_empty() {
        let orbital = orbital_json(
            vec![serde_json::json!({
                "name": "Stateless",
                "linkedEntity": "Item",
                "category": "interaction",
                "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] }
            })],
            serde_json::json!([{ "name": "Home", "path": "/home" }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A trait a page declares directly must stay silent.
    #[test]
    fn client_unbound_state_machine_silent_when_bound_by_page_decl() {
        let orbital = orbital_json(
            vec![inline_trait_with_a_transition("Bound")],
            serde_json::json!([{ "name": "Home", "path": "/home", "traits": [{ "ref": "Bound" }] }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// The suggestion names a page already mounting one of the orphan's own
    /// embedded children, when computable (mirrors the JS's `childPages`).
    #[test]
    fn client_unbound_state_machine_suggestion_names_a_page_when_computable() {
        let wrapper = serde_json::json!({
            "name": "Wrapper",
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{
                    "from": "idle", "to": "idle", "event": "INIT",
                    "effects": [["render-ui", "main", { "children": ["@trait.Child"] }]]
                }]
            }
        });
        let child = serde_json::json!({
            "ref": "Up.traits.Child", "name": "Child",
            "config": {}
        });
        let orbital = orbital_json(
            vec![wrapper, child],
            serde_json::json!([{ "name": "Home", "path": "/home", "traits": [{ "ref": "Child" }] }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        let hit = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::CircuitClientUnboundStateMachine)
            .unwrap_or_else(|| panic!("expected a finding: {:?}", result.errors));
        assert!(
            hit.suggestion.as_deref().is_some_and(|s| s.contains("/home")),
            "{:?}",
            hit.suggestion
        );
    }

    // --- ORB_CIRCUIT_DEAD_BODILESS_ACTION ---

    #[test]
    fn dead_bodiless_action_hit_when_an_arm_leaves_a_painted_state_with_no_effects() {
        let sm = StateMachine {
            states: vec![state("loading"), state("browsing"), state("opened")],
            events: None,
            transitions: vec![
                sm_transition(
                    "loading",
                    "browsing",
                    "ChatMessageLoaded",
                    effects_from(serde_json::json!([["render-ui", "main", { "type": "data-list" }]])),
                ),
                sm_transition("browsing", "opened", "OPEN", effects_from(serde_json::json!([]))),
            ],
        };
        let result = validate_dead_bodiless_actions(&sm, "test");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitDeadBodilessAction),
            "{:?}",
            result.errors
        );
    }

    /// A `from` list is judged per element: `[browsing, idle] OPEN -> opened`
    /// with no effects is dead from the painted `browsing`, whatever `idle` is.
    #[test]
    fn dead_bodiless_action_hit_when_a_from_list_contains_a_painted_state() {
        let mut arm = sm_transition("idle", "opened", "OPEN", effects_from(serde_json::json!([])));
        arm.from = StringOrArray::Multiple(vec!["browsing".to_string(), "idle".to_string()]);
        let sm = StateMachine {
            states: vec![state("loading"), state("browsing"), state("idle"), state("opened")],
            events: None,
            transitions: vec![
                sm_transition(
                    "loading",
                    "browsing",
                    "ChatMessageLoaded",
                    effects_from(serde_json::json!([["render-ui", "main", { "type": "data-list" }]])),
                ),
                arm,
            ],
        };
        let result = validate_dead_bodiless_actions(&sm, "test");
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert_eq!(result.errors[0].code, ErrorCode::CircuitDeadBodilessAction);
    }

    /// A self-transition leaves the current paint standing — not a defect.
    #[test]
    fn dead_bodiless_action_silent_for_a_self_transition() {
        let sm = StateMachine {
            states: vec![state("browsing")],
            events: None,
            transitions: vec![
                sm_transition(
                    "browsing",
                    "browsing",
                    "ChatMessageLoaded",
                    effects_from(serde_json::json!([["render-ui", "main", { "type": "data-list" }]])),
                ),
                sm_transition("browsing", "browsing", "REFRESH", effects_from(serde_json::json!([]))),
            ],
        };
        let result = validate_dead_bodiless_actions(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// An arm with real effects is not bodiless, even leaving a painted state.
    #[test]
    fn dead_bodiless_action_silent_when_the_arm_has_effects() {
        let sm = StateMachine {
            states: vec![state("browsing"), state("opened")],
            events: None,
            transitions: vec![
                sm_transition(
                    "loading",
                    "browsing",
                    "ChatMessageLoaded",
                    effects_from(serde_json::json!([["render-ui", "main", { "type": "data-list" }]])),
                ),
                sm_transition(
                    "browsing",
                    "opened",
                    "OPEN",
                    effects_from(serde_json::json!([["render-ui", "main", { "type": "detail-panel" }]])),
                ),
            ],
        };
        let result = validate_dead_bodiless_actions(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// `from` was never painted by anything — nothing is stranded.
    #[test]
    fn dead_bodiless_action_silent_when_the_from_state_was_never_painted() {
        let sm = StateMachine {
            states: vec![state("idle"), state("gone")],
            events: None,
            transitions: vec![sm_transition("idle", "gone", "LEAVE", effects_from(serde_json::json!([])))],
        };
        let result = validate_dead_bodiless_actions(&sm, "test");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// Client-bound gated at the call site: an unbound trait's arms never
    /// run in the shipped client, so the check must not fire even for the
    /// exact shape that trips `dead_bodiless_action_hit_...` above.
    #[test]
    fn dead_bodiless_action_silent_when_the_trait_is_not_client_bound() {
        let trait_def = trait_def_json(serde_json::json!({
            "name": "Orphan",
            "linkedEntity": "Item",
            "category": "interaction",
            "stateMachine": {
                "states": [{ "name": "loading", "isInitial": true }, { "name": "browsing" }, { "name": "opened" }],
                "transitions": [
                    {
                        "from": "loading", "to": "browsing", "event": "ChatMessageLoaded",
                        "effects": [["render-ui", "main", { "type": "data-list" }]]
                    },
                    { "from": "browsing", "to": "opened", "event": "OPEN", "effects": [] }
                ]
            }
        }));
        let empty: HashSet<String> = HashSet::new();
        let bound = HashSet::new(); // trait absent — not client-bound
        let result = validate_closed_circuit(&trait_def, "test.path", &empty, &bound);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CircuitDeadBodilessAction),
            "{:?}",
            result.errors
        );
    }
}
