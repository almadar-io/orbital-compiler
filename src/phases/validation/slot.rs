//! Slot Validation
//!
//! Validates slot coverage, contention, and nesting.
//! Ported from `builder/packages/shared/src/orbitals/validation/slot-validator.ts`.

use super::{
    render_ui::{extract_slot, is_render_ui_effect},
    Domain, ErrorCode, ValidationContext, ValidationError, ValidationResult,
};
use orbital_core::schema::{OrbitalSchema, StateMachine, TraitDefinition, TraitReference};
use std::collections::{HashMap, HashSet, VecDeque};

/// Slots that cannot contain other slots
pub const LEAF_SLOTS: &[&str] = &["toast", "overlay"];

/// Slots that can only be used in game context. `hud-left`/`hud-right` dropped —
/// not part of `VALID_SLOTS` (no canonical `UI_SLOTS` entry, zero `.lolo` usage).
pub const GAME_ONLY_SLOTS: &[&str] = &["hud-top", "hud-bottom"];

/// Validate all slots in a schema
pub fn validate_all_slots(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(pages) = &orbital.pages {
            for (j, page_ref) in pages.iter().enumerate() {
                let path = format!("orbitals[{}].pages[{}]", i, j);

                // Only validate inline pages, not references
                if let Some(page) = page_ref.as_inline() {
                    // Collect all inline traits for this page
                    // Note: Only inline traits are supported. Referenced traits should be defined inline within orbitals.
                    let page_traits: Vec<&TraitDefinition> = page
                        .traits
                        .as_ref()
                        .map(|traits| {
                            traits
                                .iter()
                                .filter_map(|tr| match tr {
                                    TraitReference::Inline(def) => Some(def),
                                    TraitReference::Reference { .. } => {
                                        // Referenced traits are no longer supported at schema level.
                                        // Traits should be defined inline within orbitals.
                                        None
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    // Validate slots for this page
                    let result = validate_page_slots(&page_traits, &path, ctx.domain);
                    ctx.merge(result);
                }
            }
        }
    }
}

/// Validate slots for a page
pub fn validate_page_slots(
    traits: &[&TraitDefinition],
    path: &str,
    domain: Domain,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect all slots rendered by traits on initial state
    let mut initial_slots: HashMap<String, Vec<String>> = HashMap::new(); // slot -> [trait names]
    let mut all_slots: HashSet<String> = HashSet::new();

    for trait_def in traits {
        let initial_state = get_initial_state(trait_def);

        for transition in &trait_def.state_machine.transitions {
            // Check if this transition is from initial state on INIT event
            let is_init_transition = transition.event == "INIT"
                && initial_state
                    .as_ref()
                    .map(|s| matches_state(&transition.from, s))
                    .unwrap_or(false);

            if let Some(effects) = &transition.effects {
                for effect in effects {
                    if is_render_ui_effect(effect) {
                        if let Some(slot) = extract_slot(effect) {
                            all_slots.insert(slot.clone());

                            if is_init_transition {
                                initial_slots
                                    .entry(slot)
                                    .or_default()
                                    .push(trait_def.name.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // Check main slot coverage
    if !initial_slots.contains_key("main") && !traits.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SlotMainNotCovered,
                path,
                "Main slot is not covered by any trait on page load",
            )
            .with_suggestion("Ensure at least one trait renders to 'main' slot in INIT transition"),
        );
    }

    // Check slot contention (multiple traits rendering to same slot)
    for (slot, trait_names) in &initial_slots {
        if trait_names.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::SlotContention,
                    path,
                    format!(
                        "Slot '{}' has contention: multiple traits render to it: {:?}",
                        slot, trait_names
                    ),
                )
                .with_suggestion("Each slot should be owned by exactly one trait"),
            );
        }
    }

    // Per-state slot contention (non-INIT reachable states).
    //
    // For every trait, compute the set of states reachable from that trait's
    // initial state, then bucket render-ui effects by (from_state, slot). If a
    // bucket has >1 trait contributing, that's runtime contention.
    //
    // The initial-state bucket is skipped here because `initial_slots` above
    // already covers it (and keeps error-severity via `SlotContention`). The
    // non-INIT version is emitted as a warning (`SlotContentionRuntime`) per
    // the "warnings first, graduate to errors" policy.
    let mut runtime_slots: HashMap<(String, String), Vec<String>> = HashMap::new();

    for trait_def in traits {
        let initial_state = get_initial_state(trait_def);
        let reachable = match initial_state.as_deref() {
            Some(init) => reachable_states(&trait_def.state_machine, init),
            None => continue,
        };

        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                let from_states = transition_from_states(&transition.from);
                for from_state in &from_states {
                    // Skip the trait's initial state — INIT-slot rule is handled above.
                    // The wildcard "*" is treated as "all reachable states"; iterate them
                    // separately so the warning identifies a real state name.
                    if *from_state == "*" {
                        for state in &reachable {
                            if initial_state.as_deref() == Some(state.as_str()) {
                                continue;
                            }
                            for effect in effects {
                                if is_render_ui_effect(effect) {
                                    if let Some(slot) = extract_slot(effect) {
                                        runtime_slots
                                            .entry((state.clone(), slot))
                                            .or_default()
                                            .push(trait_def.name.clone());
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    if initial_state.as_deref() == Some(*from_state) {
                        continue;
                    }
                    if !reachable.contains(*from_state) {
                        continue;
                    }

                    for effect in effects {
                        if is_render_ui_effect(effect) {
                            if let Some(slot) = extract_slot(effect) {
                                runtime_slots
                                    .entry(((*from_state).to_string(), slot))
                                    .or_default()
                                    .push(trait_def.name.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    for ((state, slot), mut trait_names) in runtime_slots {
        // De-dup: a trait may have multiple transitions rendering the same slot
        // in the same state. That isn't contention — only distinct traits count.
        trait_names.sort();
        trait_names.dedup();
        if trait_names.len() > 1 {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::SlotContentionRuntime,
                    path,
                    format!(
                        "Slot '{}' in state '{}' has contention: traits {:?} all render to it.",
                        slot, state, trait_names
                    ),
                )
                .with_suggestion("Each slot should be owned by exactly one trait per state"),
            );
        }
    }

    // Check HUD slots in non-game context
    if domain != Domain::Game {
        for slot in &all_slots {
            if GAME_ONLY_SLOTS.contains(&slot.as_str()) {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::SlotHudNonGame,
                        path,
                        format!("Slot '{}' is typically used in game context", slot),
                    )
                    .with_suggestion("HUD slots are intended for game UIs"),
                );
            }
        }
    }

    // Validate slot nesting
    result.merge(validate_slot_nesting(&all_slots, path));

    result
}

/// Validate slot nesting rules
fn validate_slot_nesting(slots: &HashSet<String>, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check for modal in modal (if both modal and drawer are used, warn)
    let overlay_slots: Vec<&str> = slots
        .iter()
        .filter(|s| *s == "modal" || *s == "drawer" || *s == "overlay")
        .map(|s| s.as_str())
        .collect();

    if overlay_slots.len() > 1 {
        result.add_error(
            ValidationError::warning(
                ErrorCode::SlotInvalidNesting,
                path,
                format!(
                    "Multiple overlay-type slots used: {:?}. Consider if nesting is intended.",
                    overlay_slots
                ),
            )
            .with_suggestion("Avoid nesting modals within modals"),
        );
    }

    result
}

/// Get the initial state of a trait
fn get_initial_state(trait_def: &TraitDefinition) -> Option<String> {
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
///
/// Inlined here rather than pulled from `closed_circuit.rs` to keep `slot.rs`
/// self-contained — the BFS is small and the two modules have different needs
/// (closed-circuit uses an incoming-transition heuristic).
fn reachable_states(state_machine: &StateMachine, initial: &str) -> HashSet<String> {
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
            if !fires_from_current {
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
fn transition_from_states(from: &orbital_core::schema::StringOrArray) -> Vec<&str> {
    match from {
        orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
        orbital_core::schema::StringOrArray::Multiple(arr) => {
            arr.iter().map(|s| s.as_str()).collect()
        }
    }
}

/// Check if a transition 'from' matches a state name
fn matches_state(from: &orbital_core::schema::StringOrArray, state: &str) -> bool {
    match from {
        orbital_core::schema::StringOrArray::Single(s) => s == state || s == "*",
        orbital_core::schema::StringOrArray::Multiple(states) => {
            states.iter().any(|s| s == state || s == "*")
        }
    }
}

/// Validate main slot coverage for a specific set of traits and state
pub fn validate_main_slot_coverage(
    traits: &[&TraitDefinition],
    state: &str,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut has_main = false;

    for trait_def in traits {
        for transition in &trait_def.state_machine.transitions {
            if matches_state(&transition.from, state) {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        if is_render_ui_effect(effect) {
                            if let Some(slot) = extract_slot(effect) {
                                if slot == "main" {
                                    has_main = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !has_main && !traits.is_empty() {
        result.add_error(
            ValidationError::warning(
                ErrorCode::SlotMainNotCovered,
                path,
                format!("Main slot may not be covered in state '{}'", state),
            )
            .with_suggestion("Ensure main slot is rendered in all visual states"),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        SExpression, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
    };

    fn create_trait_with_render_ui(name: &str, slot: &str) -> TraitDefinition {
        TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
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
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("render-ui".to_string()),
                        SExpression::String(slot.to_string()),
                        SExpression::Object(
                            [("type".to_string(), SExpression::String("card".to_string()))]
                                .into_iter()
                                .collect(),
                        ),
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
        }
    }

    #[test]
    fn test_main_slot_covered() {
        let trait_def = create_trait_with_render_ui("TestTrait", "main");
        let traits = vec![&trait_def];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_main_slot_not_covered() {
        let trait_def = create_trait_with_render_ui("TestTrait", "sidebar");
        let traits = vec![&trait_def];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SlotMainNotCovered));
    }

    #[test]
    fn test_slot_contention() {
        let trait1 = create_trait_with_render_ui("Trait1", "main");
        let trait2 = create_trait_with_render_ui("Trait2", "main");
        let traits = vec![&trait1, &trait2];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SlotContention));
    }

    #[test]
    fn test_hud_slot_in_business() {
        let trait_def = create_trait_with_render_ui("TestTrait", "hud-top");
        let main_trait = create_trait_with_render_ui("MainTrait", "main");
        let traits = vec![&trait_def, &main_trait];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SlotHudNonGame));
    }

    #[test]
    fn test_hud_slot_in_game() {
        let trait_def = create_trait_with_render_ui("TestTrait", "hud-top");
        let main_trait = create_trait_with_render_ui("MainTrait", "main");
        let traits = vec![&trait_def, &main_trait];

        let result = validate_page_slots(&traits, "test", Domain::Game);
        // Should not warn about HUD in game context
        assert!(!result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SlotHudNonGame));
    }

    // ---- Per-state slot contention (ORB_SLOT_CONTENTION_RUNTIME) -----------

    /// Build a trait whose state machine is:
    ///   idle -- INIT --> idle (renders `init_slot` on init if provided)
    ///   idle -- `event_to_second` --> second (renders `second_slot` in `second`)
    fn create_two_state_trait(
        name: &str,
        init_slot: Option<&str>,
        event_to_second: &str,
        second_slot: &str,
    ) -> TraitDefinition {
        let mut transitions = Vec::new();
        if let Some(slot) = init_slot {
            transitions.push(TransitionDefinition {
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(vec![SExpression::Array(vec![
                    SExpression::String("render-ui".to_string()),
                    SExpression::String(slot.to_string()),
                    SExpression::Object(
                        [("type".to_string(), SExpression::String("card".to_string()))]
                            .into_iter()
                            .collect(),
                    ),
                ])]),
            });
        }
        transitions.push(TransitionDefinition {
            from: StringOrArray::Single("idle".to_string()),
            to: "second".to_string(),
            event: event_to_second.to_string(),
            guard: None,
            effects: Some(vec![SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String(second_slot.to_string()),
                SExpression::Object(
                    [("type".to_string(), SExpression::String("card".to_string()))]
                        .into_iter()
                        .collect(),
                ),
            ])]),
        });
        // Loop transition from `second` so it stays reachable and also renders.
        transitions.push(TransitionDefinition {
            from: StringOrArray::Single("second".to_string()),
            to: "second".to_string(),
            event: format!("REFRESH_{}", name),
            guard: None,
            effects: Some(vec![SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String(second_slot.to_string()),
                SExpression::Object(
                    [("type".to_string(), SExpression::String("card".to_string()))]
                        .into_iter()
                        .collect(),
                ),
            ])]),
        });

        TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![
                    StateDefinition {
                        name: "idle".to_string(),
                        is_initial: Some(true),
                        is_terminal: None,
                        description: None,
                        time_bound_ms: None,
                    },
                    StateDefinition {
                        name: "second".to_string(),
                        is_initial: None,
                        is_terminal: None,
                        description: None,
                        time_bound_ms: None,
                    },
                ],
                events: None,
                transitions,
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
        }
    }

    /// Two traits, both render `main` but on different reachable states
    /// (trait1 in "idle", trait2 in "second"). Because the grouping key is
    /// (state, slot), the two renders don't collide.
    #[test]
    fn test_slot_no_contention_across_distinct_states() {
        // trait1: main @ idle (INIT), something else @ second
        let trait1 = create_two_state_trait("Trait1", Some("main"), "GO", "sidebar");
        // trait2: sidebar @ idle (INIT), main @ second — reachable but different state from trait1's main render
        let trait2 = create_two_state_trait("Trait2", Some("sidebar"), "GO", "main");
        let traits = vec![&trait1, &trait2];

        let result = validate_page_slots(&traits, "test", Domain::Business);

        // Neither trait has an INIT-time conflict on `main` (trait1 renders main at INIT,
        // trait2 renders sidebar at INIT), so no SlotContention error.
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContention),
            "Unexpected INIT-slot contention: {:?}",
            result.errors
        );
        // Different states means no per-state contention either.
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContentionRuntime),
            "Unexpected runtime-slot contention: {:?}",
            result.errors
        );
    }

    /// One trait renders `main` on INIT, the other renders `modal` on INIT.
    /// Different slots, no contention at all.
    #[test]
    fn test_slot_no_contention_different_slots_on_init() {
        let trait_main = create_trait_with_render_ui("MainTrait", "main");
        let trait_modal = create_trait_with_render_ui("ModalTrait", "modal");
        let traits = vec![&trait_main, &trait_modal];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContention),
            "Unexpected INIT slot contention: {:?}",
            result.errors
        );
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContentionRuntime),
            "Unexpected runtime slot contention: {:?}",
            result.errors
        );
    }

    /// Two traits, each rendering `main` in the shared reachable post-INIT
    /// state `second`. Expect ORB_SLOT_CONTENTION_RUNTIME (warning).
    #[test]
    fn test_slot_runtime_contention_post_init_same_state() {
        // Both render `main` at INIT? No — that would fire the original INIT error.
        // Instead render different INIT slots but same `main` at the `second` state.
        let trait1 = create_two_state_trait("Trait1", Some("main"), "GO", "main");
        // trait2 also renders main at INIT? We want ONLY a runtime conflict.
        // Give trait2 a non-main INIT slot so INIT check passes.
        let trait2 = create_two_state_trait("Trait2", Some("sidebar"), "GO", "main");
        let traits = vec![&trait1, &trait2];

        let result = validate_page_slots(&traits, "test", Domain::Business);

        // INIT check: trait1 has main @ idle, trait2 has sidebar @ idle — no INIT error.
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContention),
            "Should not fire INIT contention: {:?}",
            result.errors
        );
        // Runtime check: both render main in `second` state.
        let runtime_warnings: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::SlotContentionRuntime)
            .collect();
        assert!(
            !runtime_warnings.is_empty(),
            "Expected runtime contention warning. Warnings: {:?}",
            result.errors
        );
        // The message should mention the state "second".
        assert!(
            runtime_warnings
                .iter()
                .any(|w| w.message.contains("second")),
            "Runtime contention message should name state 'second': {:?}",
            runtime_warnings
        );
    }

    /// The original INIT-state contention rule must keep its error severity.
    #[test]
    fn test_init_slot_contention_still_errors() {
        let trait1 = create_trait_with_render_ui("Trait1", "main");
        let trait2 = create_trait_with_render_ui("Trait2", "main");
        let traits = vec![&trait1, &trait2];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(!result.is_valid());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContention),
            "INIT contention should remain an error: {:?}",
            result.errors
        );
        // The runtime warning should NOT duplicate the INIT-state finding:
        // initial state "idle" is excluded from the runtime pass by design.
        assert!(
            !result.all_issues().any(|e| {
                e.code == ErrorCode::SlotContentionRuntime && e.message.contains("idle")
            }),
            "Runtime pass should not re-report the INIT state: {:?}",
            result.errors
        );
    }

    /// Unreachable states must NOT contribute to runtime contention.
    #[test]
    fn test_slot_runtime_contention_skips_unreachable_states() {
        // Craft a trait with an unreachable state `orphan` that also renders `main`.
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
                    name: "orphan".to_string(),
                    is_initial: None,
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: None,
            transitions: vec![
                TransitionDefinition {
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("render-ui".to_string()),
                        SExpression::String("sidebar".to_string()),
                        SExpression::Object(
                            [("type".to_string(), SExpression::String("card".to_string()))]
                                .into_iter()
                                .collect(),
                        ),
                    ])]),
                },
                // No transition leads to `orphan`, so this render must be ignored.
                TransitionDefinition {
                    from: StringOrArray::Single("orphan".to_string()),
                    to: "orphan".to_string(),
                    event: "TICK".to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("render-ui".to_string()),
                        SExpression::String("main".to_string()),
                        SExpression::Object(
                            [("type".to_string(), SExpression::String("card".to_string()))]
                                .into_iter()
                                .collect(),
                        ),
                    ])]),
                },
            ],
        };
        let trait_orphan = TraitDefinition {
            name: "TraitOrphan".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: sm,
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
        };
        // Second trait renders `main` at INIT to satisfy main-slot coverage and
        // give us a potential contender. If the BFS is correct, contention
        // with the orphan render won't trigger.
        let trait_main = create_trait_with_render_ui("TraitMain", "main");
        let traits = vec![&trait_orphan, &trait_main];

        let result = validate_page_slots(&traits, "test", Domain::Business);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SlotContentionRuntime),
            "Unreachable orphan render should not contribute to contention: {:?}",
            result.errors
        );
    }
}
