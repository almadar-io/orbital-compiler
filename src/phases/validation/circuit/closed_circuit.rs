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
//! - Action-component mismatches (e.g., SAVE on table-view)
//! - State-inappropriate actions (e.g., form actions in browse state)
//! - Dead-end components (interactive components with no actions)
//!
//! Ported from `builder/packages/shared/src/orbitals/validation/closed-circuit.ts`.

use super::cross_orbital::collect_async_result_events;
use super::fired_events::{collect_schema_fired_events, FiredEvents};
use super::sync::content::is_content_body_pattern_type;
use super::sync::{
    get_invalid_actions_for_component, get_valid_actions_for_component,
    is_action_invalid_for_component, is_form_action, is_form_pattern, is_interactive_pattern,
    is_list_action, is_list_pattern,
};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::phases::embed_graph::{client_bound_traits, pages_embedding_children_of, EmbedGraph};
use crate::phases::page_content_owner::{
    is_content_main_writer, reduce_to_owners, resolve_page_content_owner, PageContentOwner,
};
use crate::phases::sexpr_json::sexpr_obj_to_json_map;
use crate::phases::validation::value_walk::all_sources;
use orbital_core_base::events::is_bus_excluded_event;
use orbital_core_base::schema::{
    json_value_leaves, ConfigField, Orbital, OrbitalSchema, PageDefinition, PageRef, SExpression,
    StateMachine, StringOrArray, TraitDefinition, TraitReference, TransitionDefinition,
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
    /// True when the action is record-scoped (see [`is_record_scoped`]): a
    /// per-row `itemActions` entry, or an `actions` entry on a pattern that
    /// binds one record. Its event name carries no form or list semantics.
    from_item_actions: bool,
    /// Path for error reporting
    path: String,
}

/// Validate all closed circuits in the schema
pub fn validate_all_circuits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(super::fetch_deliver::validate_fetch_deliver(schema));
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
        if super::mounts::is_program(schema, orbital) {
            ctx.merge(validate_client_bound_state_machines(
                i,
                orbital,
                &crate::phases::embed_graph::mounted_traits(&schema.orbitals, orbital),
            ));
        }
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = super::trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_closed_circuit(trait_def, &path, &orbital_handled);
                ctx.merge(result);
            }
        }
    }
}

use super::sync::content::is_main_slot_render_ui;

/// Validate closed circuit for a single trait. `orbital_handled` is the
/// union of EVERY sibling trait's transition triggers AND every event any
/// sibling `listens` for in the same orbital — the orphan-event check accepts
/// a sibling-handled (or emits-declared) action event as a closed circuit,
/// since the bus broadcasts orbital-wide.
pub fn validate_closed_circuit(
    trait_def: &TraitDefinition,
    path: &str,
    orbital_handled: &HashSet<String>,
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
    result.merge(validate_action_component_match(&item_actions, path));
    result.merge(validate_dead_end_components(&render_ui_effects, path));
    result.merge(validate_state_appropriate_actions(
        &item_actions,
        &render_ui_effects,
        path,
    ));

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
            slot: slot.into_inner(),
            pattern_type: pattern_type.map(|v| v.into_inner()),
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
            // Every declared value source of an actions config contributes
            // (both `if` branches, a sliced/sorted/concatenated list, a mapped
            // lambda body) — the runtime evaluates it at render time, so a
            // descriptor from any source is live. A source is a descriptor
            // list or one descriptor. Multi-leaf paths carry a `.branch[i]`
            // suffix: only one branch renders at a time, so cross-branch
            // repeats are not duplicates.
            let leaves = json_value_leaves(actions_value, &all_sources());
            let multi_leaf = leaves.len() > 1;
            for (leaf_idx, leaf) in leaves.into_iter().enumerate() {
                let actions = match leaf {
                    serde_json::Value::Array(actions) => actions.as_slice(),
                    serde_json::Value::Object(_) => std::slice::from_ref(leaf),
                    _ => continue,
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
                            from_item_actions: is_record_scoped(action_prop, &pattern_type),
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

/// A record-scoped action acts on one record — a list row's `itemActions`
/// entry, or an `actions` entry on a pattern whose `entity` prop declares
/// record cardinality (detail-panel). "Submit for approval" or "Cancel order"
/// there is a lifecycle event of that record, so the form/list name checks
/// below do not apply to it.
fn is_record_scoped(action_prop: &str, pattern_type: &str) -> bool {
    action_prop == "itemActions"
        || (action_prop == "actions" && crate::pattern_registry::binds_single_record(pattern_type))
}

/// Detect action-component mismatches
fn validate_action_component_match(
    item_actions: &[ItemActionInfo],
    _path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for action in item_actions {
        if action.from_item_actions {
            continue;
        }
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

/// The `.lolo` page line for the page at `path` with `trait_name` joined to its `->` list.
fn page_decl_with(orbital: &Orbital, path: &str, trait_name: &str) -> Option<String> {
    orbital.pages.iter().flatten().find_map(|page_ref| {
        let PageRef::Inline(page) = page_ref else {
            return None;
        };
        if page.path != path {
            return None;
        }
        let mut names = vec![trait_name.to_string()];
        for trait_ref in page.traits.iter().flatten() {
            match trait_ref {
                TraitReference::Inline(td) => names.push(td.name.clone()),
                TraitReference::Reference {
                    reference: Some(r), ..
                } => names.push(r.clone()),
                _ => {}
            }
        }
        Some(format!(
            "page \"{}\" as {} -> {}",
            page.path,
            page.name,
            names.join(", ")
        ))
    })
}

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
        let decls: Vec<String> = child_pages
            .iter()
            .filter_map(|path| page_decl_with(orbital, path, &trait_def.name))
            .collect();
        let suggestion = if child_pages.is_empty() {
            format!(
                "declare {name} on a page — its traits follow `->`, comma-separated, e.g. \
                 `page \"/path\" as SomePage -> {name}` — or embed it in a mounted trait's \
                 render-ui as @trait.{name}",
                name = trait_def.name
            )
        } else {
            format!(
                "add {} to the page decl already mounting one of its embedded children ({}): {}",
                trait_def.name,
                child_pages.join(", "),
                decls.join("; ")
            )
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitUnmounted,
                &path,
                format!(
                    "'{}.{}' owns a state machine ({} transition(s)) but is listed on no page (route or protocol endpoint) and no page's @trait embed closure reaches it — nothing mounts it, so it can never handle an event in the shipped app",
                    orbital.name, trait_def.name, transition_count
                ),
            )
            .with_suggestion(suggestion),
        );
    }

    result
}

/// `ORB_CIRCUIT_VIEWER_STRANDED` + `ORB_PAGE_CONTROL_OUTSIDE_SHELL`: both read
/// the SAME per-page content-owner fact (`page_content_owner::resolve_page_content_owner`),
/// computed ONCE per page here and shared between the two checks — never two
/// owner-model calls for one page. Ported from the JS wiring lint's
/// `viewer-stranded` and `unclaimed-main-writer`.
pub fn validate_page_content_ownership(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let fired = collect_schema_fired_events(schema);

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let Some(pages) = orbital.pages.as_ref() else {
            continue;
        };
        if pages.is_empty() {
            continue;
        }

        let graph = EmbedGraph::build(&orbital.traits);
        let channel_adjacency = graph.config_embedded_children();
        let embed_adjacency = graph.embedded_children();

        let traits_by_name: HashMap<&str, &TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some((td.name.as_str(), &**td)),
                _ => None,
            })
            .collect();
        let trait_index_by_name: HashMap<&str, usize> = orbital
            .traits
            .iter()
            .enumerate()
            .filter_map(|(j, t)| match t {
                TraitReference::Inline(td) => Some((td.name.as_str(), j)),
                _ => None,
            })
            .collect();

        let async_results = orbital_async_result_events(&traits_by_name);
        let affordance_events = affordance_events_of(&traits_by_name, &fired, &async_results);
        let all_producible = all_producible_events_of(&traits_by_name, &fired, &async_results);

        let client_bound = client_bound_traits(orbital);
        // Every target embedded (full config+state-machine graph) by a
        // source the client actually mounts — a second page-declared writer
        // reached this way is already claimed, not stray. Computed once per
        // orbital, mirrors the JS `claimedByBound`.
        let claimed_by_bound: HashSet<&str> = embed_adjacency
            .iter()
            .filter(|(source, _)| client_bound.contains(**source))
            .flat_map(|(_, targets)| targets.iter().copied())
            .collect();

        for (page_index, page_ref) in pages.iter().enumerate() {
            let PageRef::Inline(page) = page_ref else {
                continue;
            };
            let owner = resolve_page_content_owner(
                page,
                &traits_by_name,
                &channel_adjacency,
                &embed_adjacency,
            );

            if let PageContentOwner::Channel(name) | PageContentOwner::SoleWriter(name) = &owner {
                if let (Some(trait_def), Some(&j)) = (
                    traits_by_name.get(name.as_str()),
                    trait_index_by_name.get(name.as_str()),
                ) {
                    validate_viewer_stranded_for_owner(
                        i,
                        orbital,
                        j,
                        trait_def,
                        &affordance_events,
                        &all_producible,
                        &traits_by_name,
                        ctx,
                    );
                }
            }

            if matches!(
                owner,
                PageContentOwner::Channel(_) | PageContentOwner::Ambiguous(_)
            ) {
                validate_second_content_body(
                    i,
                    page_index,
                    page,
                    &owner,
                    &traits_by_name,
                    &channel_adjacency,
                    &embed_adjacency,
                    &claimed_by_bound,
                    ctx,
                );
            }
        }
    }
}

/// Async-result events (`fetch`/`persist` `emit:{success,failure,...}`
/// values) produced anywhere in this orbital — excluded from the
/// affordance-event set (an async result is a "response", never a
/// user-triggered affordance) but still counted toward `all_producible` (a
/// fetch's own success event is a valid exit from a loading screen).
fn orbital_async_result_events(traits: &HashMap<&str, &TraitDefinition>) -> HashSet<String> {
    let mut out = HashSet::new();
    for td in traits.values() {
        for transition in &td.state_machine.transitions {
            if let Some(effects) = transition.effects.as_deref() {
                out.extend(collect_async_result_events(effects));
            }
        }
        for tick in td.ticks.iter().flatten() {
            out.extend(collect_async_result_events(&tick.effects));
        }
    }
    out
}

fn admit_event(event: &str, out: &mut HashSet<String>, async_results: &HashSet<String>) {
    if event.is_empty() || async_results.contains(event) || is_bus_excluded_event(event) {
        return;
    }
    out.insert(event.to_string());
}

/// "User-triggered" event set for `viewer-stranded`: every event any trait
/// in this orbital can produce (`FiredEvents` control/effect producers plus
/// its declared `emits[]` contract) or listens for, closed transitively over
/// `listens` routes (a route's `triggers` becomes reachable once its source
/// `event` is), minus async-result and lifecycle events.
fn affordance_events_of(
    traits: &HashMap<&str, &TraitDefinition>,
    fired: &FiredEvents,
    async_results: &HashSet<String>,
) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for (name, td) in traits {
        for (t, e) in fired.by_control.iter().chain(fired.by_effect.iter()) {
            if t.as_str() == *name {
                admit_event(e, &mut out, async_results);
            }
        }
        for emit_def in td.emits.iter().flatten() {
            admit_event(&emit_def.event, &mut out, async_results);
        }
        for listen in td.listens.iter().flatten() {
            admit_event(&listen.event, &mut out, async_results);
        }
    }
    // Transitive closure: a route's `triggers` joins the set once its source
    // `event` is already in it — up to 8 passes, matching the JS fixpoint cap.
    for _ in 0..8 {
        let before = out.len();
        for td in traits.values() {
            for listen in td.listens.iter().flatten() {
                if out.contains(&listen.event) {
                    if let Some(triggers) = &listen.triggers {
                        out.insert(triggers.clone());
                    }
                }
            }
        }
        if out.len() == before {
            break;
        }
    }
    out
}

/// Superset of `affordance_events_of` used ONLY for the arm-exemption
/// "escapes to a state with another producible arm" test — includes
/// async-result events (a fetch's own success event is a valid exit),
/// unlike the affordance set itself.
fn all_producible_events_of(
    traits: &HashMap<&str, &TraitDefinition>,
    fired: &FiredEvents,
    async_results: &HashSet<String>,
) -> HashSet<String> {
    let mut out: HashSet<String> = async_results.clone();
    for (name, td) in traits {
        for (t, e) in fired.by_control.iter().chain(fired.by_effect.iter()) {
            if t.as_str() == *name {
                out.insert(e.clone());
            }
        }
        for emit_def in td.emits.iter().flatten() {
            out.insert(emit_def.event.clone());
        }
        for listen in td.listens.iter().flatten() {
            if let Some(triggers) = &listen.triggers {
                out.insert(triggers.clone());
            }
        }
    }
    out
}

/// True when `from` (a transition's `from`, single or list) contains `state`.
fn from_contains(from: &StringOrArray, state: &str) -> bool {
    match from {
        StringOrArray::Single(s) => s == state,
        StringOrArray::Multiple(list) => list.iter().any(|s| s == state),
    }
}

/// The general, registry-derived "way on" test the F31 live-probe
/// (`ui-minigolf-board` PUTT_UP, `std-chem-titration` DOSE_ADDED) forced onto
/// `ORB_CIRCUIT_VIEWER_STRANDED`'s predicate: NO label test (a missing
/// required label is `ORB_RUI` required-prop territory, not this check's),
/// NO `@`/`?` sigil distinction (a heuristic, dropped). A main-slot body
/// offers a way on iff it contains ANY of:
///   (a) a content-body pattern type (registry entity-kind prop);
///   (b) a registry event-kind prop DECLARED ON THIS NODE'S OWN PATTERN TYPE
///       (`event`/`event-ref`/`event-list`/`event-listen`/`callback`/
///       `event-map`, via `pattern_node_has_live_event_outlet` — scoped per
///       type, never a flat cross-pattern set) bound to a non-lifecycle
///       literal OR any binding (a binding is unknown = way on, never
///       presumed dead);
///   (c) an `@trait.X` embed WHOSE OWN main-slot render bodies (across every
///       transition and tick, same predicate, recursively) offer a way on —
///       corrected 2026-09-12 per the vendor-onboarding worked example: a
///       JSX-hoisted LEAF embed (`UiIcon.traits.IconRender`,
///       `UiTypography.traits.TypographyRender`) resolves to a real trait
///       whose body is icon/typography-only, so it must NOT short-circuit to
///       "way on" just for existing. Only a reference `traits` cannot
///       resolve at all (no Inline entry) is unknown = way on. `visiting`
///       cycle-guards by trait name (insert on entry, remove on exit — a
///       true cycle gets no credit at the repeated name; a diamond shape,
///       same trait reached twice via different paths with no cycle, is
///       still evaluated fully each time it's off the stack).
///   (d) a `@entity.<field>`/`@config.<knob>` string binding whose SAME-ARM
///       source (`scope`, built by [`RenderScope::for_effects`]) resolves to
///       a literal node tree — recurses on that tree instead of stopping
///       dead at the binding string. No same-arm literal source = unknown =
///       way on. `visiting_fields` cycle-guards by field name the same way
///       `visiting` does by trait name.
fn body_offers_way_on(
    node: &SExpression,
    traits: &HashMap<&str, &TraitDefinition>,
    scope: &RenderScope,
    visiting: &mut HashSet<String>,
    visiting_fields: &mut HashSet<String>,
) -> bool {
    match node {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let name = &rest[..end];
                return !name.is_empty() && trait_offers_way_on(name, traits, visiting);
            }
            if let Some(rest) = s.strip_prefix("@entity.") {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let field = &rest[..end];
                if field.is_empty() {
                    return false;
                }
                return scope.entity_field_way_on(field, traits, visiting, visiting_fields);
            }
            if let Some(rest) = s.strip_prefix("@config.") {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let knob = &rest[..end];
                if knob.is_empty() {
                    return false;
                }
                return scope.config_knob_way_on(knob, traits, visiting, visiting_fields);
            }
            false
        }
        SExpression::Array(items) => items
            .iter()
            .any(|i| body_offers_way_on(i, traits, scope, visiting, visiting_fields)),
        SExpression::Object(map) => {
            let pattern_type = match map.get("type") {
                Some(SExpression::String(t)) => Some(t.as_str()),
                _ => None,
            };
            if let Some(t) = pattern_type {
                if is_content_body_pattern_type(t) {
                    return true;
                }
                if pattern_node_has_live_event_outlet(t, map) {
                    return true;
                }
            }
            map.values()
                .any(|v| body_offers_way_on(v, traits, scope, visiting, visiting_fields))
        }
        _ => false,
    }
}

/// Same-arm resolution context for `@entity.<field>`/`@config.<knob>`
/// bindings hit while walking a render-ui body: an arm may compute its
/// interactive tree earlier in the SAME transition (`(set @entity.x
/// [...])`) then paint it via `(render-ui main { children: @entity.x })` —
/// `body_offers_way_on` must see through that binding, not stop dead at the
/// string.
struct RenderScope<'a> {
    /// Field name -> literal-tree candidates from the LAST `(set
    /// @entity.<field> <value>)` (found anywhere in the arm's effects, incl.
    /// nested in `let`/`do`/`if`) whose value resolves to a literal tree. An
    /// `if`-valued set contributes BOTH branches (union, not a single pick) —
    /// leniently, the same direction every other unknown in this predicate
    /// leans. A set whose value does not resolve to a literal tree is simply
    /// skipped, never clearing an earlier resolved candidate.
    entity_fields: HashMap<String, Vec<SExpression>>,
    config: Option<&'a orbital_core_base::collections::IndexMap<String, ConfigField>>,
}

impl<'a> RenderScope<'a> {
    fn for_effects(effects: &[SExpression], trait_def: &'a TraitDefinition) -> Self {
        RenderScope {
            entity_fields: collect_entity_field_literal_candidates(effects),
            config: trait_def.config.as_ref(),
        }
    }

    fn entity_field_way_on(
        &self,
        field: &str,
        traits: &HashMap<&str, &TraitDefinition>,
        visiting: &mut HashSet<String>,
        visiting_fields: &mut HashSet<String>,
    ) -> bool {
        let Some(candidates) = self.entity_fields.get(field) else {
            return true; // no same-arm literal source: unknown = way on
        };
        if !visiting_fields.insert(field.to_string()) {
            return false; // a field's own literal candidate re-references it: no credit
        }
        let hit = candidates
            .iter()
            .any(|c| body_offers_way_on(c, traits, self, visiting, visiting_fields));
        visiting_fields.remove(field);
        hit
    }

    fn config_knob_way_on(
        &self,
        knob: &str,
        traits: &HashMap<&str, &TraitDefinition>,
        visiting: &mut HashSet<String>,
        visiting_fields: &mut HashSet<String>,
    ) -> bool {
        let default = self
            .config
            .and_then(|c| c.get(knob))
            .and_then(|f| f.default.as_ref());
        match default {
            Some(value) if is_pattern_node_tree_shape(value) => {
                body_offers_way_on(value, traits, self, visiting, visiting_fields)
            }
            _ => true, // no literal config-forward default: unknown = way on
        }
    }
}

/// True when `expr` is a rendered pattern node (an object carrying `type`)
/// or an array whose every element is, recursively, such a node — the
/// literal-tree shape a `set`'s value must have to substitute for a
/// same-arm `@entity.<field>`/`@config.<knob>` binding. An empty array is
/// vacuously a (childless) literal tree.
fn is_pattern_node_tree_shape(expr: &SExpression) -> bool {
    match expr {
        SExpression::Object(map) => map.contains_key("type"),
        SExpression::Array(items) => items.iter().all(is_pattern_node_tree_shape),
        _ => false,
    }
}

/// Resolves a `set`'s value expression to its literal-tree candidates,
/// recursing through the value-position `let`/`do`/`if` wrappers exactly
/// like the effect-position ones `RUNTIME`'s `exec_effect_expr` (`kernel.rs`)
/// evaluates: `do`/`let`'s body is its last expression, `if` is BOTH
/// branches (union — we don't know which fires). A bare `@name` resolves
/// through `lets` (the flattened local-variable scope collected alongside);
/// anything else (a call to `array/map`, `object/get`, `str/concat`, …) is
/// opaque — `None`, not a false literal.
fn resolve_value_literal_candidates(
    expr: &SExpression,
    lets: &orbital_core_base::schema::SObject,
    depth: u32,
) -> Option<Vec<SExpression>> {
    if depth > 32 {
        return None;
    }
    match expr {
        SExpression::Object(map) if map.contains_key("type") => Some(vec![expr.clone()]),
        SExpression::String(s) => {
            let name = s.strip_prefix('@')?;
            if name.contains(['.', '[']) {
                return None; // a field/index path, not a bare local variable
            }
            resolve_value_literal_candidates(lets.get(name)?, lets, depth + 1)
        }
        SExpression::Array(items) => match items.first() {
            Some(SExpression::String(op)) if op == "if" => {
                let mut out = Vec::new();
                if let Some(t) = items.get(2) {
                    out.extend(
                        resolve_value_literal_candidates(t, lets, depth + 1).unwrap_or_default(),
                    );
                }
                if let Some(e) = items.get(3) {
                    out.extend(
                        resolve_value_literal_candidates(e, lets, depth + 1).unwrap_or_default(),
                    );
                }
                if out.is_empty() {
                    None
                } else {
                    Some(out)
                }
            }
            Some(SExpression::String(op)) if op == "do" => {
                resolve_value_literal_candidates(items.last()?, lets, depth + 1)
            }
            Some(SExpression::String(op)) if op == "let" => {
                let mut scope = lets.clone();
                if let Some(SExpression::Array(bindings)) = items.get(1) {
                    for binding in bindings {
                        if let SExpression::Array(pair) = binding {
                            if let (Some(SExpression::String(name)), Some(value)) =
                                (pair.first(), pair.get(1))
                            {
                                scope.insert(name.clone().into_inner(), value.clone());
                            }
                        }
                    }
                }
                resolve_value_literal_candidates(
                    items.iter().skip(2).next_back()?,
                    &scope,
                    depth + 1,
                )
            }
            _ if is_pattern_node_tree_shape(expr) => Some(vec![expr.clone()]),
            _ => None,
        },
        _ => None,
    }
}

/// Collects, in program order, a flattened local-variable scope (every
/// `let` binding found anywhere in `effects`, later occurrences winning,
/// regardless of `let`/`do`/`if` nesting) plus the literal-tree
/// candidates of the LAST `(set @entity.<field> <value>)` per field whose
/// value resolves (via `resolve_value_literal_candidates`, using that flat
/// scope). A `set` whose value does not resolve is skipped, not recorded —
/// it never clears a field's earlier resolved candidate.
fn collect_entity_field_literal_candidates(
    effects: &[SExpression],
) -> HashMap<String, Vec<SExpression>> {
    fn walk(
        expr: &SExpression,
        lets: &mut orbital_core_base::schema::SObject,
        fields: &mut HashMap<String, Vec<SExpression>>,
    ) {
        let SExpression::Array(items) = expr else {
            return;
        };
        if let Some(SExpression::String(op)) = items.first() {
            if op == "let" {
                if let Some(SExpression::Array(bindings)) = items.get(1) {
                    for binding in bindings {
                        if let SExpression::Array(pair) = binding {
                            if let (Some(SExpression::String(name)), Some(value)) =
                                (pair.first(), pair.get(1))
                            {
                                lets.insert(name.clone().into_inner(), value.clone());
                            }
                        }
                    }
                }
            } else if op == "set" {
                if let (Some(SExpression::String(target)), Some(value)) =
                    (items.get(1), items.get(2))
                {
                    if let Some(field) = target.strip_prefix("@entity.") {
                        if !field.contains(['.', '[']) {
                            if let Some(candidates) =
                                resolve_value_literal_candidates(value, lets, 0)
                            {
                                fields.insert(field.to_string(), candidates);
                            }
                        }
                    }
                }
            }
        }
        for item in items {
            walk(item, lets, fields);
        }
    }

    let mut lets = orbital_core_base::schema::SObject::new();
    let mut fields = HashMap::new();
    for effect in effects {
        walk(effect, &mut lets, &mut fields);
    }
    fields
}

/// True when `map` (a rendered pattern instance of type `pattern_type`)
/// declares a registry event-kind prop (`event`/`event-ref`/`event-listen`/
/// `callback`/`event-map`, or `event-list`'s inner `eventField`) whose bound
/// value is live — a non-lifecycle literal, or any binding (unknown = way
/// on).
///
/// Scoped to `pattern_type`'s OWN `propsSchema` — never a flat cross-pattern
/// set, as defense in depth against a per-pattern `children`/`renderItem`
/// COLLISION: checking a bare prop KEY against a schema-wide set (as
/// `fired_events::ui_event_prop_names()` does, correctly, for ITS OWN
/// different purpose — harvesting fired event names anywhere in a tree
/// without needing a node's own type) would treat every OTHER pattern's
/// unrelated nested-render-tree `children` prop (`stack`, `box`, …) as an
/// event outlet too, the moment ANY pattern tags its own `children`
/// `kind: "callback"` — the vendor-onboarding regression test below
/// reproduces exactly this shape of collision. (`data-grid`/`data-list`/
/// `table-view`/`calendar-grid`/`carousel`'s `children`/`renderItem` render
/// props themselves carry NO kind at all as of 2026-09-12 — a render-prop
/// that RETURNS a value is never a bus event outlet — but the scoping stays:
/// it is correct on its own terms, not merely a workaround for that one
/// mis-tag.) Mirrors `@almadar/core`'s per-type `eventKeyPropsOf`, widened to
/// also check `event-list`'s actual `eventField` (not every value in the
/// descriptor — only the field the registry names) per the F31 predicate's
/// inclusion of `event-list`.
fn pattern_node_has_live_event_outlet(
    pattern_type: &str,
    map: &orbital_core_base::schema::SObject,
) -> bool {
    let Some(entry) = crate::embedded::get_pattern(pattern_type) else {
        return false;
    };
    for (prop_name, prop) in &entry.props_schema {
        let Some(kind) = prop.kind.as_deref() else {
            continue;
        };
        match kind {
            // `event-map` (a dynamic-key object whose VALUES — not the prop
            // itself — are declared event keys: a d-pad's direction→event map
            // `control-grid.directionEvents`, a canvas's keycode→event map
            // `canvas.keyMap`) rides the same `event_prop_value_is_live` call
            // as the scalar kinds: that helper already recurses through an
            // Object's values, so a literal map is live iff any VALUE is, and
            // a whole-map binding (`directionEvents: "@config.directionEvents"`)
            // is unknown = way on, same as any other event-kind prop.
            "event" | "event-ref" | "event-listen" | "callback" | "event-map" => {
                if let Some(value) = map.get(prop_name.as_str()) {
                    if event_prop_value_is_live(value) {
                        return true;
                    }
                }
            }
            "event-list" => {
                let Some(SExpression::Array(items)) = map.get(prop_name.as_str()) else {
                    continue;
                };
                let field = prop.event_field.as_deref().unwrap_or("event");
                for item in items {
                    let SExpression::Object(descriptor) = item else {
                        continue;
                    };
                    if let Some(value) = descriptor.get(field) {
                        if event_prop_value_is_live(value) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// True when `name` resolves to an Inline trait whose own main-slot render
/// bodies (every transition's and every tick's effects — `initialEffects`
/// has no representation in the compiled schema, see `page_content_owner`'s
/// same finding) offer a way on. An unresolvable name (no Inline entry in
/// this orbital's `traits`) is unknown, not empty — counts as way on.
fn trait_offers_way_on(
    name: &str,
    traits: &HashMap<&str, &TraitDefinition>,
    visiting: &mut HashSet<String>,
) -> bool {
    let Some(trait_def) = traits.get(name) else {
        return true; // unresolved reference: unknown, never presumed dead
    };
    if !visiting.insert(name.to_string()) {
        return false; // already on the call stack — a cycle, no credit here
    }
    let mut found = false;
    // Kept as effect LISTS (one per transition/tick), not flattened across
    // arms, so `RenderScope::for_effects` only ever sees same-arm effects
    // when resolving an `@entity.<field>`/`@config.<knob>` binding below.
    let effect_lists = trait_def
        .state_machine
        .transitions
        .iter()
        .filter_map(|t| t.effects.as_deref())
        .chain(
            trait_def
                .ticks
                .iter()
                .flatten()
                .map(|t| t.effects.as_slice()),
        );
    'arms: for effects in effect_lists {
        for (idx, effect) in effects.iter().enumerate() {
            if !is_main_slot_render_ui(effect) {
                continue;
            }
            let SExpression::Array(arr) = effect else {
                continue;
            };
            let Some(body) = arr.get(2) else { continue };
            if matches!(body, SExpression::Null) {
                continue; // a deliberate clear
            }
            // Only effects BEFORE this render-ui, in program order — a
            // `set` appearing after it hasn't run yet and must not count.
            let scope = RenderScope::for_effects(&effects[..idx], trait_def);
            if body_offers_way_on(body, traits, &scope, visiting, &mut HashSet::new()) {
                found = true;
                break 'arms;
            }
        }
    }
    visiting.remove(name);
    found
}

/// A registry event-kind prop's value is "live" when it is a binding
/// (unknown at static-analysis time = way on) or a non-empty, non-lifecycle
/// literal. Recurses through wrapper shapes (`event-list` descriptor arrays,
/// `if`-conditional value trees) generically — leniently: a leaf reachable
/// only through a provably-dead `if` branch still counts, which only ADDS
/// way-on credit (favors fewer false-positive strand findings, the same
/// direction the F31 probe corrected).
fn event_prop_value_is_live(value: &SExpression) -> bool {
    match value {
        SExpression::String(s) => {
            s.starts_with('@') || (!s.is_empty() && !is_bus_excluded_event(s))
        }
        SExpression::Array(items) => items.iter().any(event_prop_value_is_live),
        SExpression::Object(map) => map.values().any(event_prop_value_is_live),
        _ => false,
    }
}

/// `ORB_CIRCUIT_VIEWER_STRANDED` for one page's content owner.
#[allow(clippy::too_many_arguments)]
fn validate_viewer_stranded_for_owner(
    orbital_index: usize,
    orbital: &Orbital,
    trait_index: usize,
    trait_def: &TraitDefinition,
    affordance_events: &HashSet<String>,
    all_producible: &HashSet<String>,
    traits: &HashMap<&str, &TraitDefinition>,
    ctx: &mut ValidationContext,
) {
    let path = super::trait_path(
        orbital_index,
        &orbital.name,
        trait_index,
        &orbital.traits[trait_index],
    );
    let mut seen: HashSet<String> = HashSet::new();

    for (ti, arm) in trait_def.state_machine.transitions.iter().enumerate() {
        let event = &arm.event;
        if is_bus_excluded_event(event) {
            continue;
        }
        if !affordance_events.contains(event) || seen.contains(event) {
            continue;
        }

        let effects = arm.effects.as_deref().unwrap_or(&[]);
        // The arm's own fetch/persist exempts it — a transient screen whose
        // result arm repaints.
        let runs_its_own_fetch_or_persist = effects.iter().any(|e| {
            matches!(e, SExpression::Array(items)
                if matches!(items.first(), Some(SExpression::String(op)) if op == "fetch" || op == "persist"))
        });
        if runs_its_own_fetch_or_persist {
            continue;
        }

        // A self-transition gets no credit; a departure to a state with
        // another non-lifecycle, producible arm is not a strand either.
        let leaves_the_state = !matches!(&arm.from, StringOrArray::Single(s) if s == &arm.to);
        if leaves_the_state {
            let escapes = trait_def.state_machine.transitions.iter().any(|other| {
                from_contains(&other.from, &arm.to)
                    && &other.event != event
                    && !is_bus_excluded_event(&other.event)
                    && all_producible.contains(&other.event)
            });
            if escapes {
                continue;
            }
        }

        for (idx, effect) in effects.iter().enumerate() {
            if !is_main_slot_render_ui(effect) {
                continue;
            }
            let SExpression::Array(arr) = effect else {
                continue;
            };
            let Some(body) = arr.get(2) else { continue };
            if matches!(body, SExpression::Null) {
                continue; // a deliberate clear
            }
            // Only effects BEFORE this render-ui, in program order — a
            // `set` appearing after it hasn't run yet and must not count.
            let scope = RenderScope::for_effects(&effects[..idx], trait_def);
            if body_offers_way_on(
                body,
                traits,
                &scope,
                &mut HashSet::new(),
                &mut HashSet::new(),
            ) {
                continue;
            }
            seen.insert(event.clone());
            ctx.add_error(
                ValidationError::error(
                    ErrorCode::CircuitViewerStranded,
                    format!("{}.stateMachine.transitions[{}]", path, ti),
                    format!(
                        "'{}' owns this page's content region, and '{}' repaints it with a body that carries no data surface, no wired control and no sibling embed — the screen the viewer came from is gone and there is nothing to click",
                        trait_def.name, event
                    ),
                )
                .with_suggestion(
                    "render this OVER the page instead of replacing it: move the body into a dedicated overlay trait writing only 'modal' (a modal-close arm rendered from the trait that owns 'main' is rejected by CIRCUIT_MODAL_EXIT_INCOMPLETE, which is why it needs its own trait), or give the arm a wired control back to the state it came from".to_string(),
                ),
            );
            break;
        }
    }
}

/// True when any transition or tick of `trait_def` paints a control into a `main` slot: a
/// pattern node whose event prop (registry `kind: event | event-ref`) carries a value.
fn paints_main(trait_def: &TraitDefinition) -> bool {
    fn has_control(node: &SExpression) -> bool {
        match node {
            SExpression::Object(map) => {
                let control = matches!(map.get("type"), Some(SExpression::String(t)) if crate::embedded::get_pattern(t).is_some_and(|p| {
                    p.props_schema.iter().any(|(prop, spec)| {
                        matches!(spec.kind.as_deref(), Some("event") | Some("event-ref"))
                            && matches!(map.get(prop), Some(SExpression::String(v)) if !v.is_empty())
                    })
                }));
                control || map.values().any(has_control)
            }
            SExpression::Array(items) => items.iter().any(has_control),
            _ => false,
        }
    }
    fn scan(node: &SExpression) -> bool {
        if is_main_slot_render_ui(node) {
            if let SExpression::Array(arr) = node {
                return arr.get(2).is_some_and(has_control);
            }
        }
        match node {
            SExpression::Array(items) => items.iter().any(scan),
            _ => false,
        }
    }
    trait_def
        .state_machine
        .transitions
        .iter()
        .flat_map(|t| t.effects.iter().flatten())
        .chain(
            trait_def
                .ticks
                .iter()
                .flatten()
                .flat_map(|tick| tick.effects.iter()),
        )
        .any(scan)
}

/// True when `from` designates one of `owners` through its config channel, directly or
/// through traits it channels in turn (a shell → a list → the list's row view).
fn channels_to(
    from: &str,
    owners: &[String],
    channel_adjacency: &crate::phases::embed_graph::EmbedAdjacency<'_>,
) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut walk: Vec<&str> = vec![from];
    while let Some(node) = walk.pop() {
        if !seen.insert(node) {
            continue;
        }
        for &child in channel_adjacency.get(node).into_iter().flatten() {
            if owners.iter().any(|o| o == child) {
                return true;
            }
            walk.push(child);
        }
    }
    false
}

/// True when `trait_def` paints a `template`-category pattern (a whole-page layout) into
/// `main`; a `@config.<knob>` pattern type resolves through the trait's own config.
fn paints_main_layout(trait_def: &TraitDefinition) -> bool {
    let resolve = |t: &str| -> Option<String> {
        match t.strip_prefix("@config.") {
            None => Some(t.to_string()),
            Some(knob) => (trait_def
                .config
                .as_ref()
                .and_then(|c| c.get(knob))
                .and_then(|f| match &f.default {
                    Some(SExpression::String(v)) => Some(v.clone()),
                    _ => None,
                })).map(|v| v.into_inner()),
        }
    };
    fn is_layout(node: &SExpression, resolve: &dyn Fn(&str) -> Option<String>) -> bool {
        match node {
            SExpression::Object(map) => {
                matches!(map.get("type"), Some(SExpression::String(t)) if resolve(t).is_some_and(|p| {
                    crate::embedded::patterns().get(&p).is_some_and(|e| e.category == "template")
                })) || map.values().any(|v| is_layout(v, resolve))
            }
            SExpression::Array(items) => items.iter().any(|i| is_layout(i, resolve)),
            _ => false,
        }
    }
    fn scan(node: &SExpression, resolve: &dyn Fn(&str) -> Option<String>) -> bool {
        if is_main_slot_render_ui(node) {
            if let SExpression::Array(arr) = node {
                if arr.get(2).is_some_and(|body| is_layout(body, resolve)) {
                    return true;
                }
            }
        }
        match node {
            SExpression::Array(items) => items.iter().any(|i| scan(i, resolve)),
            _ => false,
        }
    }
    trait_def
        .state_machine
        .transitions
        .iter()
        .flat_map(|t| t.effects.iter().flatten())
        .any(|e| scan(e, &resolve))
}

/// `ORB_PAGE_CONTROL_OUTSIDE_SHELL` for one page whose owner is `channel` or
/// `ambiguous` — a page-declared, inline, main-painting trait that is
/// not the owner, not embedded by any client-bound trait, and not kin to the
/// owner through either adjacency.
#[allow(clippy::too_many_arguments)]
fn validate_second_content_body(
    orbital_index: usize,
    page_index: usize,
    page: &PageDefinition,
    owner: &PageContentOwner,
    traits_by_name: &HashMap<&str, &TraitDefinition>,
    channel_adjacency: &crate::phases::embed_graph::EmbedAdjacency<'_>,
    embed_adjacency: &crate::phases::embed_graph::EmbedAdjacency<'_>,
    claimed_by_bound: &HashSet<&str>,
    ctx: &mut ValidationContext,
) {
    let owner_names: Vec<String> = match owner {
        PageContentOwner::Channel(n) => vec![n.clone()],
        PageContentOwner::Ambiguous(names) => names.clone(),
        _ => return,
    };

    let page_trait_names: Vec<&str> = page
        .traits
        .iter()
        .flatten()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.as_str()),
            TraitReference::Reference {
                reference, name, ..
            } => name.as_deref().or(reference.as_deref()),
        })
        .collect();
    // A page shell: a page trait that channels the owner and paints a page layout
    // (a `template`-category pattern) into `main` — anything else painting `main`
    // outside the channelled content stacks below that layout.
    let shell_channels_owner = matches!(owner, PageContentOwner::Channel(_))
        && page_trait_names.iter().any(|candidate| {
            channels_to(candidate, &owner_names, channel_adjacency)
                && traits_by_name
                    .get(candidate)
                    .is_some_and(|td| paints_main_layout(td))
        });

    for trait_ref in page.traits.iter().flatten() {
        let name = match trait_ref {
            TraitReference::Inline(td) => td.name.as_str(),
            TraitReference::Reference {
                reference, name, ..
            } => name.as_deref().or(reference.as_deref()).unwrap_or(""),
        };
        if name.is_empty() || owner_names.iter().any(|o| o == name) {
            continue;
        }
        if claimed_by_bound.contains(name) {
            continue;
        }
        let Some(trait_def) = traits_by_name.get(name) else {
            continue;
        };
        let content_writer = is_content_main_writer(trait_def);
        let stray_control = shell_channels_owner && paints_main(trait_def);
        if !content_writer && !stray_control {
            continue;
        }
        let related_to_owner = owner_names.iter().any(|owner_name| {
            let candidates = [owner_name.clone(), name.to_string()];
            reduce_to_owners(&candidates, channel_adjacency, embed_adjacency).len() == 1
        });
        if related_to_owner {
            continue;
        }
        if !content_writer {
            ctx.add_error(
                ValidationError::error(
                    ErrorCode::PageControlOutsideShell,
                    format!("orbitals[{}].pages[{}]", orbital_index, page_index),
                    format!(
                        "page \"{}\": {} paints into slot 'main', but the page's shell shows {} as its content and nothing there embeds {} — it stacks below the shell's layout instead of inside it",
                        page.path, name, owner_names.join(" / "), name
                    ),
                )
                .with_suggestion(format!(
                    "embed <trait.{}> in the content the shell shows (a stack set as its contentTrait), or remove {} from the page",
                    name, name
                )),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
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
                    holds: None,
                },
                StateDefinition {
                    name: "viewing".to_string(),
                    is_initial: Some(false),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                    holds: None,
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
                ensures: None,
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
            pattern_type: "table-view".to_string(),
            in_state: "idle".to_string(),
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
        // A non-record `actions` group: SAVE on an table-view toolbar is the
        // misplaced form button the affinity table catches.
        let item_actions = vec![ItemActionInfo {
            action: "SAVE".to_string(),
            event: "SAVE".to_string(),
            pattern_type: "table-view".to_string(),
            in_state: "idle".to_string(),
            from_item_actions: false,
            path: "test.path".to_string(),
        }];
        assert!(!validate_action_component_match(&item_actions, "test").is_valid());

        // The same name as a per-row action ("save this listing") is a record
        // lifecycle event and passes.
        let row = vec![ItemActionInfo {
            from_item_actions: true,
            ..item_actions[0].clone()
        }];
        assert!(validate_action_component_match(&row, "test").is_valid());
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

    fn record_action(action: &str, pattern: &str, record_scoped: bool) -> ItemActionInfo {
        ItemActionInfo {
            action: action.to_string(),
            event: action.to_string(),
            pattern_type: pattern.to_string(),
            in_state: "viewing".to_string(),
            from_item_actions: record_scoped,
            path: "test.path".to_string(),
        }
    }

    #[test]
    fn test_record_scope_is_declared_registry_data() {
        // detail-panel's `entity` prop declares `cardinality: "record"`, so its
        // `actions` act on that one record; an `actions` group on a pattern
        // without a record-cardinality entity prop is not record-scoped.
        assert!(is_record_scoped("itemActions", "data-list"));
        assert!(is_record_scoped("actions", "detail-panel"));
        assert!(!is_record_scoped("actions", "page-header"));
        assert!(!is_record_scoped("actions", "alert"));
    }

    #[test]
    fn test_detail_panel_lifecycle_actions_named_submit_or_cancel_pass() {
        // "Submit for approval" / "Cancel order" on a record's detail panel are
        // lifecycle events of that record, not form buttons.
        let actions = vec![
            record_action("SUBMIT", "detail-panel", true),
            record_action("CANCEL", "detail-panel", true),
        ];
        let renders = vec![render_ui_info("detail-panel", "viewing")];
        assert!(validate_action_component_match(&actions, "test").is_valid());
        assert!(validate_state_appropriate_actions(&actions, &renders, "test").is_valid());
    }

    #[test]
    fn test_row_action_named_cancel_passes_affinity_too() {
        // The affinity table lists CANCEL as invalid on data-list; a per-row
        // "cancel this item" action must not trip it any more than the
        // state-appropriateness check.
        let actions = vec![record_action("CANCEL", "data-list", true)];
        assert!(validate_action_component_match(&actions, "test").is_valid());
    }

    #[test]
    fn test_non_record_actions_group_keeps_affinity_check() {
        // A page-header `actions` group is not record-scoped: SAVE there is
        // still the misplaced form button the affinity table exists to catch.
        let actions = vec![record_action("SAVE", "page-header", false)];
        assert!(validate_action_component_match(&actions, "test")
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CircuitActionComponentMismatch));
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

    // --- ORB_T_TRAIT_UNMOUNTED ---

    fn orbital_json(traits: Vec<serde_json::Value>, pages: serde_json::Value) -> Orbital {
        use orbital_core_base::schema::EntityRef;
        let traits: Vec<TraitReference> = traits
            .into_iter()
            .map(|t| serde_json::from_value(t).expect("trait reference fixture deserializes"))
            .collect();
        let pages: Vec<orbital_core_base::schema::PageRef> =
            serde_json::from_value(pages).expect("pages fixture deserializes");
        Orbital {
            spec: false,
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
            os_grants: None,
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
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }]
            }
        })
    }

    #[test]
    fn client_unbound_state_machine_hit_when_no_page_or_embed_reaches_the_trait() {
        let orbital = orbital_json(
            vec![
                inline_trait_with_a_transition("Bound"),
                inline_trait_with_a_transition("Orphan"),
            ],
            serde_json::json!([{ "name": "Home", "path": "/home", "traits": [{ "ref": "Bound" }] }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitUnmounted && e.message.contains("Orphan")),
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

    fn unmounted_in(orbitals: Vec<Orbital>) -> bool {
        let mut schema: OrbitalSchema =
            serde_json::from_value(serde_json::json!({ "name": "p", "orbitals": [] })).unwrap();
        schema.orbitals = orbitals;
        let mut ctx = ValidationContext::new();
        validate_all_circuits(&schema, &mut ctx);
        ctx.result
            .errors
            .iter()
            .any(|error| error.code == ErrorCode::TraitUnmounted)
    }

    #[test]
    fn an_atom_with_no_pages_may_mount_nothing() {
        let atom = orbital_json(
            vec![inline_trait_with_a_transition("Worker")],
            serde_json::json!([]),
        );
        assert!(!unmounted_in(vec![atom]));
    }

    #[test]
    fn control_a_trait_on_no_page_of_an_orbital_with_pages_is_unmounted() {
        let orbital = orbital_json(
            vec![
                inline_trait_with_a_transition("Home"),
                inline_trait_with_a_transition("Worker"),
            ],
            serde_json::json!([{ "name": "H", "path": "/h", "traits": [{ "ref": "Home" }] }]),
        );
        assert!(unmounted_in(vec![orbital]));
    }

    #[test]
    fn edge_an_organism_orbital_without_pages_is_a_program() {
        let atom = orbital_json(
            vec![inline_trait_with_a_transition("Worker")],
            serde_json::json!([]),
        );
        let mut sibling = orbital_json(vec![], serde_json::json!([]));
        sibling.name = "Sibling".to_string();
        assert!(unmounted_in(vec![atom, sibling]));
    }

    #[test]
    fn a_trait_another_orbitals_page_lists_is_mounted() {
        let mut front = orbital_json(vec![inline_trait_with_a_transition("Front")], serde_json::json!([]));
        front.name = "FrontOrbital".to_string();
        let app = orbital_json(
            vec![inline_trait_with_a_transition("Home")],
            serde_json::json!([{ "name": "H", "path": "/h", "traits": [{ "ref": "Home" }, { "ref": "Front" }] }]),
        );
        assert!(!unmounted_in(vec![front, app]));
    }

    #[test]
    fn control_a_trait_no_orbitals_page_lists_is_unmounted() {
        let mut front = orbital_json(vec![inline_trait_with_a_transition("Front")], serde_json::json!([]));
        front.name = "FrontOrbital".to_string();
        let app = orbital_json(
            vec![inline_trait_with_a_transition("Home")],
            serde_json::json!([{ "name": "H", "path": "/h", "traits": [{ "ref": "Home" }] }]),
        );
        assert!(unmounted_in(vec![front, app]));
    }

    #[test]
    fn edge_a_trait_on_a_protocol_endpoint_is_mounted() {
        let orbital = orbital_json(
            vec![
                inline_trait_with_a_transition("Home"),
                inline_trait_with_a_transition("Worker"),
            ],
            serde_json::json!([
                { "name": "H", "path": "/h", "traits": [{ "ref": "Home" }] },
                { "name": "B", "path": "", "protocol": "boot", "traits": [{ "ref": "Worker" }] }
            ]),
        );
        assert!(!unmounted_in(vec![orbital]));
    }

    /// A trait with an empty `stateMachine.transitions` has nothing to bind
    /// — not a defect, so it must stay silent even though it is unbound.
    #[test]
    fn client_unbound_state_machine_silent_when_transitions_are_empty() {
        let orbital = orbital_json(
            vec![serde_json::json!({
                "name": "Stateless",
                "linkedEntity": "Item",
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
            .find(|e| e.code == ErrorCode::TraitUnmounted)
            .unwrap_or_else(|| panic!("expected a finding: {:?}", result.errors));
        assert!(
            hit.suggestion
                .as_deref()
                .is_some_and(|s| s.contains("/home")),
            "{:?}",
            hit.suggestion
        );
        // The exact `.lolo` page line to write: the trait joins the page's `->` list.
        assert!(
            hit.suggestion
                .as_deref()
                .is_some_and(|s| s.contains("page \"/home\" as Home -> Wrapper, Child")),
            "{:?}",
            hit.suggestion
        );
    }

    /// Control: with no page mounting a child, the suggestion still spells the page syntax.
    #[test]
    fn client_unbound_state_machine_suggestion_spells_page_syntax_without_a_child_page() {
        let orbital = orbital_json(
            vec![inline_trait_with_a_transition("Orphan")],
            serde_json::json!([{ "name": "Home", "path": "/home" }]),
        );
        let bound = client_bound_traits(&orbital);
        let result = validate_client_bound_state_machines(0, &orbital, &bound);
        let hit = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::TraitUnmounted)
            .unwrap_or_else(|| panic!("expected a finding: {:?}", result.errors));
        assert!(
            hit.suggestion
                .as_deref()
                .is_some_and(|s| s.contains("-> Orphan") && s.contains("@trait.Orphan")),
            "{:?}",
            hit.suggestion
        );
    }

    // --- ORB_CIRCUIT_VIEWER_STRANDED ---

    fn schema_of(orbital: Orbital) -> OrbitalSchema {
        OrbitalSchema {
            type_contract_version: None,
            resolved: None,            types: None,
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
            config: None,
            orbitals: vec![orbital],
            metadata: None,
            schema_version: None,
            ledger: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        }
    }

    /// A sole-content-writing owner trait whose only user-triggered arm
    /// (`STRAND`) repaints `main` with `body`. `STRAND` is admitted into the
    /// affordance set via a sibling `Trigger` trait's rendered `action`
    /// prop, matching how the JS's `affordanceEvents` is seeded from a
    /// rendered control anywhere in the orbital, not necessarily the owner's
    /// own arm.
    fn owner_and_trigger(body: serde_json::Value) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "Owner",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "browsing", "isInitial": true }],
                    "transitions": [
                        { "from": "browsing", "to": "browsing", "event": "INIT",
                          "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] },
                        { "from": "browsing", "to": "browsing", "event": "STRAND",
                          "effects": [["render-ui", "main", body]] }
                    ]
                }
            }),
            serde_json::json!({
                "name": "Trigger",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT",
                          "effects": [["render-ui", "aux", { "type": "button", "action": "STRAND" }]] }
                    ]
                }
            }),
        ]
    }

    /// `owner_and_trigger`, plus extra Inline trait definitions in the same
    /// orbital — for embedding a real, resolvable leaf trait from the
    /// owner's STRAND body.
    fn owner_and_trigger_with(
        body: serde_json::Value,
        extra: Vec<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        let mut traits = owner_and_trigger(body);
        traits.extend(extra);
        traits
    }

    fn page_with(traits: &[&str]) -> serde_json::Value {
        serde_json::json!([{
            "name": "Home", "path": "/home",
            "traits": traits.iter().map(|t| serde_json::json!({ "ref": t })).collect::<Vec<_>>()
        }])
    }

    fn viewer_stranded_hits(orbital: Orbital) -> Vec<ValidationError> {
        let schema = schema_of(orbital);
        let mut ctx = ValidationContext::new();
        validate_page_content_ownership(&schema, &mut ctx);
        ctx.result
            .errors
            .into_iter()
            .filter(|e| e.code == ErrorCode::CircuitViewerStranded)
            .collect()
    }

    #[test]
    fn viewer_stranded_fires_on_a_body_with_no_data_surface_no_control_no_embed() {
        let orbital = orbital_json(
            owner_and_trigger(serde_json::json!({ "type": "typography", "content": "Loading…" })),
            page_with(&["Owner", "Trigger"]),
        );
        let hits = viewer_stranded_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
        assert!(hits[0].message.contains("Owner"));
    }

    #[test]
    fn viewer_stranded_silent_when_the_body_is_a_content_body_pattern() {
        let orbital = orbital_json(
            owner_and_trigger(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// F31 predicate revision: a LABELLESS button wired to a live,
    /// non-lifecycle event via a registry event-kind prop IS a way on now —
    /// the JS's labelled-control test is gone.
    #[test]
    fn viewer_stranded_silent_when_the_body_has_a_labelless_wired_control() {
        let orbital = orbital_json(
            owner_and_trigger(serde_json::json!({ "type": "button", "action": "GO_BACK" })),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// `kind: "event-map"` (2026-09-12): a d-pad/canvas control whose ONLY
    /// affordance is a direction/key→event object (`control-grid.
    /// directionEvents`) is a live way on — the mis-registered gap that read
    /// `ui-winter-ski-board`'s STEER_LEFT/STEER_RIGHT as a dead end even
    /// though the rendered control-grid dispatches them.
    #[test]
    fn viewer_stranded_silent_when_the_body_has_a_live_event_map_control() {
        let orbital = orbital_json(
            owner_and_trigger(serde_json::json!({
                "type": "control-grid",
                "kind": "dpad",
                "directionEvents": { "left": "STEER_LEFT", "right": "STEER_RIGHT" }
            })),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    #[test]
    fn viewer_stranded_fires_when_the_event_map_prop_is_absent() {
        // A control-grid rendered as pure decoration (no directionEvents at
        // all) offers no way on — the event-map kind only credits a NODE
        // that actually carries the prop.
        let orbital = orbital_json(
            owner_and_trigger(serde_json::json!({ "type": "control-grid", "kind": "dpad" })),
            page_with(&["Owner", "Trigger"]),
        );
        assert_eq!(viewer_stranded_hits(orbital).len(), 1);
    }

    #[test]
    fn viewer_stranded_silent_when_the_body_embeds_an_unresolvable_trait() {
        // "Anything" has no Inline entry anywhere in this orbital — an
        // unresolvable reference is unknown, never presumed dead.
        let orbital = orbital_json(
            owner_and_trigger(
                serde_json::json!({ "type": "stack", "children": ["@trait.Anything"] }),
            ),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// F31 correction (vendor-onboarding worked example): a JSX-hoisted LEAF
    /// embed resolves to a real trait whose own body is icon/typography-only
    /// — no content-body pattern, no event-kind prop, no further embed — so
    /// the embed must NOT short-circuit to "way on" just for existing.
    #[test]
    fn viewer_stranded_fires_when_the_embedded_trait_is_a_leaf_with_no_way_on() {
        let leaf = serde_json::json!({
            "name": "LeafIcon",
            "linkedEntity": "Item",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT",
                      "effects": [["render-ui", "main", { "type": "icon", "name": "check-circle" }]] }
                ]
            }
        });
        let orbital = orbital_json(
            owner_and_trigger_with(
                serde_json::json!({ "type": "stack", "children": ["@trait.LeafIcon"] }),
                vec![leaf],
            ),
            page_with(&["Owner", "Trigger"]),
        );
        let hits = viewer_stranded_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
    }

    /// The recursive half of the same correction: an embed of a trait whose
    /// OWN body has a wired control (a registry event-kind prop bound to a
    /// live event) IS a way on — the recursion finds it one level down.
    #[test]
    fn viewer_stranded_silent_when_the_embedded_trait_has_its_own_wired_control() {
        let leaf = serde_json::json!({
            "name": "LeafButton",
            "linkedEntity": "Item",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT",
                      "effects": [["render-ui", "main", { "type": "button", "action": "GO_BACK" }]] }
                ]
            }
        });
        let orbital = orbital_json(
            owner_and_trigger_with(
                serde_json::json!({ "type": "stack", "children": ["@trait.LeafButton"] }),
                vec![leaf],
            ),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    #[test]
    fn viewer_stranded_silent_when_the_body_is_null_a_deliberate_clear() {
        let orbital = orbital_json(
            owner_and_trigger(serde_json::Value::Null),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    #[test]
    fn viewer_stranded_silent_when_the_arm_runs_its_own_fetch() {
        let traits = vec![
            serde_json::json!({
                "name": "Owner",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "browsing", "isInitial": true }],
                    "transitions": [
                        { "from": "browsing", "to": "browsing", "event": "INIT",
                          "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] },
                        { "from": "browsing", "to": "browsing", "event": "STRAND",
                          "effects": [
                            ["fetch", { "entity": "Item", "emit": { "success": "ItemLoaded" } }],
                            ["render-ui", "main", { "type": "typography", "content": "Loading…" }]
                          ] }
                    ]
                }
            }),
            serde_json::json!({
                "name": "Trigger",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT",
                          "effects": [["render-ui", "aux", { "type": "button", "action": "STRAND" }]] }
                    ]
                }
            }),
        ];
        let orbital = orbital_json(traits, page_with(&["Owner", "Trigger"]));
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    #[test]
    fn viewer_stranded_silent_when_the_arm_escapes_to_a_state_with_another_producible_arm() {
        let traits = vec![
            serde_json::json!({
                "name": "Owner",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [
                        { "name": "browsing", "isInitial": true },
                        { "name": "waiting" }
                    ],
                    "transitions": [
                        { "from": "browsing", "to": "browsing", "event": "INIT",
                          "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] },
                        { "from": "browsing", "to": "waiting", "event": "STRAND",
                          "effects": [["render-ui", "main", { "type": "typography", "content": "Starting…" }]] },
                        { "from": "waiting", "to": "browsing", "event": "STARTED",
                          "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] }
                    ]
                },
                "listens": [{ "event": "Sibling.STARTED_BROADCAST", "triggers": "STARTED" }]
            }),
            serde_json::json!({
                "name": "Trigger",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT",
                          "effects": [["render-ui", "aux", { "type": "button", "action": "STRAND" }]] }
                    ]
                }
            }),
            serde_json::json!({
                "name": "Sibling",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT",
                          "effects": [["emit", "STARTED_BROADCAST"]] }
                    ]
                }
            }),
        ];
        let orbital = orbital_json(traits, page_with(&["Owner", "Trigger", "Sibling"]));
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// `owner_and_trigger`, but the STRAND arm's full effects list is
    /// caller-supplied — for the same-arm `@entity.<field>` substitution
    /// tests below, where STRAND needs a `set` before its `render-ui`.
    fn owner_and_trigger_with_strand_effects(effects: serde_json::Value) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "Owner",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "browsing", "isInitial": true }],
                    "transitions": [
                        { "from": "browsing", "to": "browsing", "event": "INIT",
                          "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] },
                        { "from": "browsing", "to": "browsing", "event": "STRAND", "effects": effects }
                    ]
                }
            }),
            serde_json::json!({
                "name": "Trigger",
                "linkedEntity": "Item",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT",
                          "effects": [["render-ui", "aux", { "type": "button", "action": "STRAND" }]] }
                    ]
                }
            }),
        ]
    }

    /// (a) The arm sets `renderChildren` to a literal tree with a wired
    /// button earlier in the SAME transition, then paints it via a binding
    /// — `std-logic-truthtable`'s exact shape. No strand: the substituted
    /// tree offers a way on.
    #[test]
    fn viewer_stranded_silent_when_a_same_arm_set_binds_a_wired_literal() {
        let effects = serde_json::json!([
            ["set", "@entity.renderChildren", [{ "type": "button", "action": "GO_BACK" }]],
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// (b) Same shape, but the substituted literal carries only typography
    /// — no data surface, no control, no embed. A real strand.
    #[test]
    fn viewer_stranded_fires_when_a_same_arm_set_binds_a_dead_literal() {
        let effects = serde_json::json!([
            ["set", "@entity.renderChildren", [{ "type": "typography", "content": "Loading…" }]],
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        let hits = viewer_stranded_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
    }

    /// (c) No same-transition `set` for the bound field at all — unknown,
    /// never presumed dead.
    #[test]
    fn viewer_stranded_silent_when_the_bound_field_has_no_same_arm_set() {
        let effects = serde_json::json!([
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// (d) The `set` sits nested inside `let`/`do`/`if` (`std-learn-classify`'s
    /// shape, minus the local-let-variable indirection) — still resolves.
    #[test]
    fn viewer_stranded_silent_when_the_set_is_nested_in_let_do_if() {
        let effects = serde_json::json!([
            ["let", [["x", 1]],
                ["do",
                    ["if", true,
                        ["set", "@entity.renderChildren", [{ "type": "button", "action": "GO_BACK" }]],
                        null
                    ]
                ]
            ],
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// (e) A `set` AFTER the `render-ui` hasn't run yet by the time the
    /// body paints — it must not count, even though its literal is dead
    /// (which would otherwise wrongly flip an unknown into a false strand).
    #[test]
    fn viewer_stranded_silent_when_the_set_comes_after_the_render_ui() {
        let effects = serde_json::json!([
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }],
            ["set", "@entity.renderChildren", [{ "type": "typography", "content": "Loading…" }]]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// Local-let-variable indirection: the `set`'s value is `(if cond A B)`
    /// where A/B are themselves `let`-bound names (`std-learn-classify`'s
    /// actual shape — `roundView` set to `(if @entity.roundComplete
    /// @summaryView @itemView)`). Both branches resolve through the flat
    /// local scope; either offering a way on is enough.
    #[test]
    fn viewer_stranded_silent_when_the_if_branches_are_let_bound_locals() {
        let effects = serde_json::json!([
            ["let", [
                ["summaryView", [{ "type": "typography", "content": "done" }]],
                ["itemView", [{ "type": "button", "action": "GO_BACK" }]]
            ],
                ["do", ["set", "@entity.renderChildren", ["if", "@entity.roundComplete", "@summaryView", "@itemView"]]]
            ],
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        assert!(viewer_stranded_hits(orbital).is_empty());
    }

    /// Both `if` branches (local-let-bound) are dead — still a strand.
    #[test]
    fn viewer_stranded_fires_when_both_if_branches_are_dead_let_bound_locals() {
        let effects = serde_json::json!([
            ["let", [
                ["summaryView", [{ "type": "typography", "content": "done" }]],
                ["itemView", [{ "type": "typography", "content": "loading" }]]
            ],
                ["do", ["set", "@entity.renderChildren", ["if", "@entity.roundComplete", "@summaryView", "@itemView"]]]
            ],
            ["render-ui", "main", { "type": "box", "children": "@entity.renderChildren" }]
        ]);
        let orbital = orbital_json(
            owner_and_trigger_with_strand_effects(effects),
            page_with(&["Owner", "Trigger"]),
        );
        let hits = viewer_stranded_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
    }

    fn control_outside_shell_hits(orbital: Orbital) -> Vec<ValidationError> {
        let schema = schema_of(orbital);
        let mut ctx = ValidationContext::new();
        validate_page_content_ownership(&schema, &mut ctx);
        ctx.result
            .errors
            .into_iter()
            .filter(|e| e.code == ErrorCode::PageControlOutsideShell)
            .collect()
    }

    /// A page shell: channels `Catalog` through `contentTrait` and paints a template-category
    /// layout (`dashboard-layout`, named through its `layoutPattern` knob) into `main`.
    fn channel_shell() -> serde_json::Value {
        channel_rendering("dashboard-layout")
    }

    fn channel_rendering(pattern: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "Shell", "linkedEntity": "Item",
            "config": {
                "contentTrait": { "type": "trait", "default": "@trait.Catalog" },
                "layoutPattern": { "type": "pattern", "default": pattern }
            },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT",
                    "effects": [["render-ui", "main", { "type": "box", "children": [{ "type": "@config.layoutPattern", "appName": "Items" }] }]] }]
            }
        })
    }

    fn catalog_rendering(body: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": "Catalog", "linkedEntity": "Item",
            "stateMachine": {
                "states": [{ "name": "browsing", "isInitial": true }],
                "transitions": [{ "from": "browsing", "to": "browsing", "event": "INIT", "effects": [["render-ui", "main", body]] }]
            }
        })
    }

    fn add_button(slot: &str, body: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": "AddButton", "linkedEntity": "Item",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [["render-ui", slot, body]] }]
            }
        })
    }

    fn button_body() -> serde_json::Value {
        serde_json::json!({ "type": "button", "label": "Add", "action": "ADD" })
    }

    /// The shell channels its body to `Catalog`; `AddButton` paints a control into `main`
    /// that nothing embeds — it stacks below the shell's layout instead of inside it.
    #[test]
    fn control_outside_shell_fires_for_an_unembedded_control_on_a_channel_page() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", button_body()),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        let hits = control_outside_shell_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
        assert!(
            hits[0].message.contains("AddButton") && hits[0].message.contains("Catalog"),
            "{}",
            hits[0].message
        );
    }

    /// The shell channels a list that channels its own row view: the page's owner is the row
    /// view, reached from the shell through two channels — the button is still outside.
    #[test]
    fn control_outside_shell_fires_through_a_chain_of_channels() {
        let mut shell = channel_shell();
        shell["config"]["contentTrait"]["default"] = serde_json::json!("@trait.List");
        let list = serde_json::json!({
            "name": "List", "linkedEntity": "Item",
            "config": { "rowTrait": { "type": "trait", "default": "@trait.Catalog" } },
            "stateMachine": {
                "states": [{ "name": "browsing", "isInitial": true }],
                "transitions": [{ "from": "browsing", "to": "browsing", "event": "INIT", "effects": [["render-ui", "main", { "type": "stack", "children": ["@trait.Catalog"] }]] }]
            }
        });
        let orbital = orbital_json(
            vec![
                shell,
                list,
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", button_body()),
            ],
            page_with(&["Shell", "List", "Catalog", "AddButton"]),
        );
        let hits = control_outside_shell_hits(orbital);
        assert_eq!(hits.len(), 1, "{:?}", hits);
        assert!(hits[0].message.contains("AddButton"));
    }

    /// Control: the content trait stacks the button inside the shell (`@trait.AddButton`).
    #[test]
    fn control_outside_shell_silent_when_the_content_embeds_the_control() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(
                    serde_json::json!({ "type": "stack", "children": ["@trait.AddButton", { "type": "data-grid", "entity": "?data" }] }),
                ),
                add_button("main", button_body()),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Control: no shell (a sole writer owns `main`) — siblings stack in page order and stay on screen.
    #[test]
    fn control_outside_shell_silent_without_a_channel_owner() {
        let orbital = orbital_json(
            vec![
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", button_body()),
            ],
            page_with(&["Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Control: a channel that is not a page layout (a list channelling its own row view) —
    /// a control after it stacks in the page's flow and stays on screen.
    #[test]
    fn control_outside_shell_silent_when_the_channel_is_not_a_page_layout() {
        let orbital = orbital_json(
            vec![
                channel_rendering("data-list"),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", button_body()),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Edge: a trait painting only an overlay slot is not in the page's flow.
    #[test]
    fn control_outside_shell_silent_for_an_overlay_writer() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("modal", button_body()),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Edge: an empty placeholder (a dialog restoring `main` with a bare box) carries no control.
    #[test]
    fn control_outside_shell_silent_for_an_empty_placeholder() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", serde_json::json!({ "type": "box" })),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Edge: a deliberate clear of `main` paints nothing.
    #[test]
    fn control_outside_shell_silent_for_a_clear() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button("main", serde_json::Value::Null),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }

    /// Edge: a second CONTENT body is not a stray control.
    #[test]
    fn control_outside_shell_leaves_content_bodies_to_the_second_body_rule() {
        let orbital = orbital_json(
            vec![
                channel_shell(),
                catalog_rendering(serde_json::json!({ "type": "data-grid", "entity": "?data" })),
                add_button(
                    "main",
                    serde_json::json!({ "type": "data-grid", "entity": "?data" }),
                ),
            ],
            page_with(&["Shell", "Catalog", "AddButton"]),
        );
        assert!(control_outside_shell_hits(orbital).is_empty());
    }
}
