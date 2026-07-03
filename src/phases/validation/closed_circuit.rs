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

use super::sync::{
    get_invalid_actions_for_component, get_valid_actions_for_component,
    is_action_invalid_for_component, is_form_action, is_form_pattern, is_interactive_pattern,
    is_list_action, is_list_pattern,
};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::phases::sexpr_json::sexpr_obj_to_json_map;
use orbital_core::schema::{
    OrbitalSchema, SExpression, StateMachine, TraitDefinition, TraitReference, TransitionDefinition,
};
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
    /// Path for error reporting
    path: String,
}

/// Validate all closed circuits in the schema
pub fn validate_all_circuits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_closed_circuit(trait_def, &path);
                ctx.merge(result);
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

/// Events that are valid exit events for overlay slots
const OVERLAY_EXIT_EVENTS: &[&str] = &["CLOSE", "CANCEL", "DISMISS", "SAVE", "SUBMIT", "CONFIRM"];

/// Validate closed circuit for a single trait
pub fn validate_closed_circuit(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    let state_machine = &trait_def.state_machine;

    // Extract information
    let render_ui_effects = extract_render_ui_effects(trait_def, path);
    let item_actions = extract_item_actions(&render_ui_effects);
    let handled_events = extract_handled_events(state_machine);

    // Run validations
    result.merge(validate_orphan_events(&item_actions, &handled_events, path));
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
            let serde_json::Value::Array(actions) = actions_value else {
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
                        path: format!("{}.props.{}", render_ui.path, action_prop),
                    });
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

/// Detect orphan events (events emitted by UI with no transition handler)
fn validate_orphan_events(
    item_actions: &[ItemActionInfo],
    handled_events: &HashSet<String>,
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for action in item_actions {
        if !handled_events.contains(&action.event) {
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
                ValidationError::warning(
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

        // Key includes the effect path to only flag duplicates within the same render-ui effect
        let key = format!("{}:{}:{}", effect_base_path, action.slot, action.event);

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
                ValidationError::warning(
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
                ValidationError::warning(
                    ErrorCode::CircuitModalExitIncomplete,
                    format!(
                        "{}.transitions[{:?}->{}->{}]",
                        path, transition.from, transition.event, transition.to
                    ),
                    format!(
                        "Transition {:?}--[{}]-->{} closes modal but doesn't re-render main slot. Main view will show stale content.",
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
                        ValidationError::warning(
                            ErrorCode::CircuitModalExitIncomplete,
                            format!(
                                "{}.transitions[{:?}->{}->{}]",
                                path, transition.from, transition.event, transition.to
                            ),
                            format!(
                                "Transition {:?}--[{}]-->{} leaves a state mounting '{}' but doesn't include `(render-ui {} null)` to close it. The overlay will stay mounted across the transition.",
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
                ValidationError::warning(
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
        let patterns_in_state = state_patterns.get(action.in_state.as_str());

        // Check form actions in non-form states
        if is_form_action(&action.action) {
            if let Some(patterns) = patterns_in_state {
                let has_form_pattern = patterns.iter().any(|p| is_form_pattern(p));
                if !has_form_pattern && !patterns.is_empty() {
                    result.add_error(
                        ValidationError::warning(
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
                        ValidationError::warning(
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
            path: "test.path".to_string(),
        }];

        let handled_events: HashSet<String> = HashSet::new(); // Empty - no handlers
        let result = validate_orphan_events(&item_actions, &handled_events, "test");
        assert!(!result.is_valid()); // Should fail - orphan event

        let mut handled_events_with_view = HashSet::new();
        handled_events_with_view.insert("VIEW".to_string());
        let result2 = validate_orphan_events(&item_actions, &handled_events_with_view, "test");
        assert!(result2.is_valid()); // Should pass - event is handled
    }

    #[test]
    fn test_action_component_mismatch() {
        let item_actions = vec![ItemActionInfo {
            action: "SAVE".to_string(), // SAVE is invalid on entity-table
            event: "SAVE".to_string(),
            pattern_type: "entity-table".to_string(),
            in_state: "idle".to_string(),
            slot: "main".to_string(),
            path: "test.path".to_string(),
        }];

        let result = validate_action_component_match(&item_actions, "test");
        assert!(!result.is_valid()); // SAVE is invalid on entity-table
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
}
