//! Trait Validation
//!
//! Validates trait references, categories, state machines, and transitions.
//! Ported from `builder/packages/shared/src/orbitals/validation/trait-validator.ts`.

use super::{
    is_std_trait, render_ui::is_render_ui_effect, trait_path, ErrorCode, ValidationContext,
    ValidationError, ValidationResult, VALID_TRAIT_CATEGORIES,
};
use orbital_core::schema::{
    EntityDefinition, Orbital, OrbitalSchema, Persistence, SExpression, StateMachine,
    StringOrArray, TraitCategory, TraitDefinition, TraitReference, TransitionDefinition,
};
use std::collections::{HashMap, HashSet};

/// Validate all traits in a schema
pub fn validate_all_traits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate trait references in orbitals
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let path = trait_path(i, &orbital.name, j, trait_ref);
            let result =
                validate_trait_reference(trait_ref, &path, &ctx.trait_names, &ctx.entity_names);
            ctx.merge(result);
        }
    }

    // Validate `events` rename maps on surviving trait references against the
    // referenced trait's known event set. The inline phase raises
    // `InlineError::EventRenameUnknownKey` for `uses`-resolved references, but a
    // reference that survives inline unchanged (no `ref:` string, or an
    // un-inlined named reference) carries an unchecked `events` map — raw `.orb`
    // / agent output can author a bogus rename key here. This pass closes that
    // gap with `ErrorCode::EventRenameUnknownKey`.
    let trait_events = build_trait_event_sets(schema);
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let path = trait_path(i, &orbital.name, j, trait_ref);
            ctx.merge(validate_event_renames(trait_ref, &path, &trait_events));
        }
    }

    // Check persistent entities read by interaction traits are fetched on INIT
    // by at least one trait in the orbital (entity-scoped, not per-trait: the
    // datasource idiom loads an entity once; sibling render traits are pure
    // consumers and must not each carry a duplicate fetch).
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let result = validate_orbital_init_fetch(orbital, i, &ctx.entity_definitions);
        ctx.merge(result);
    }

    // Enforce `@rebindable`: a trait imported from an atom and rebound to a
    // different entity via `linkedEntity` is only legal when the atom's
    // binding opted in (`-> @rebindable Entity`); the rebind target must also
    // provide the trait's contract `requires` fields. Runs on the resolved
    // (post-inline) schema where the marker + contract survive on the cloned
    // Inline trait and `source_entity_definition` holds the atom's original
    // entity. See docs/Almadar_Rabit_Gaps.md (RABIT-STD-1 follow-up).
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_entity_rebind(trait_def, &path, &ctx.entity_definitions);
                ctx.merge(result);
            }
        }
    }

    // Validate trait uniqueness across orbitals
    let uniqueness_result = validate_trait_uniqueness(schema);
    ctx.merge(uniqueness_result);
}

/// Enforce the `@rebindable` opt-in + inferred field contract on a resolved
/// (post-inline) trait. A trait is a "rebind" when it was imported from an
/// atom (`source_behavior` set) and its `linked_entity` differs from the
/// atom's own entity (`source_entity_definition.name`). Only then do the
/// rules apply:
///   1. the atom's binding must be `entity_rebindable` (else `ORB_T_ENTITY_NOT_REBINDABLE`);
///   2. the target entity must have every field in `entity_contract.requires`
///      (else `ORB_T_REBIND_MISSING_FIELDS`). `provides` are NOT required —
///      the trait provisions those itself.
fn validate_entity_rebind(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Only imported traits can be rebinds. A trait authored directly on the
    // orbital (no source_behavior) declaring `linkedEntity` is a definition,
    // not a rebind, and is exempt.
    let Some(source) = &trait_def.source_behavior else {
        return result;
    };
    let Some(linked) = &trait_def.linked_entity else {
        return result;
    };
    let source_entity = trait_def
        .source_entity_definition
        .as_ref()
        .map(|e| e.name.as_str());
    let is_rebind = source_entity.map(|n| n != linked.as_str()).unwrap_or(false);
    if !is_rebind {
        return result;
    }

    if trait_def.entity_rebindable != Some(true) {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitEntityNotRebindable,
                format!("{}.linkedEntity", path),
                format!(
                    "Trait '{}' (from {}) has a fixed entity binding and cannot be rebound to '{}'; the atom's binding is not marked `@rebindable`.",
                    trait_def.name, source.behavior, linked
                ),
            )
            .with_suggestion(
                "Mark the atom trait's binding `-> @rebindable Entity` in its .lolo source, or remove the linkedEntity rebind.",
            ),
        );
        return result;
    }

    if let Some(contract) = &trait_def.entity_contract {
        if !contract.requires.is_empty() {
            if let Some(target) = entity_definitions.get(linked.as_str()) {
                let have: HashSet<&str> = target
                    .fields
                    .iter()
                    .filter_map(|f| f.name.as_deref())
                    .collect();
                let missing: Vec<String> = contract
                    .requires
                    .iter()
                    .filter(|r| !have.contains(r.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::RebindMissingEntityFields,
                            format!("{}.linkedEntity", path),
                            format!(
                                "Entity '{}' is missing fields required by trait '{}': {}",
                                linked,
                                trait_def.name,
                                missing.join(", ")
                            ),
                        )
                        .with_suggestion(
                            "Add the missing fields to the entity, or rebind to an entity that already has them.",
                        ),
                    );
                }
            }
        }
    }

    result
}

/// Validate a trait reference (inline or external)
pub fn validate_trait_reference(
    trait_ref: &TraitReference,
    path: &str,
    known_traits: &HashSet<String>,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match trait_ref {
        TraitReference::Inline(trait_def) => {
            // Validate inline trait definition
            result.merge(validate_trait_definition(trait_def, path, known_entities));
        }
        TraitReference::Reference {
            reference,
            name,
            linked_entity,
            ..
        } => {
            // Get the trait name from reference or name field
            let trait_name = reference.as_ref().or(name.as_ref());

            if let Some(name) = trait_name {
                // Check if it's a known trait (custom or std library)
                if !known_traits.contains(name) && !is_std_trait(name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitUndefined,
                            path,
                            format!("Trait '{}' is not defined", name),
                        )
                        .with_suggestion(
                            "Define traits inline within orbitals or use a std library trait",
                        ),
                    );
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidFormat,
                        path,
                        "Trait reference must have 'ref' or 'name' property",
                    )
                    .with_suggestion("Add ref: 'TraitName' to reference a defined trait"),
                );
            }

            // Validate linked entity if specified
            if let Some(linked) = linked_entity {
                if !known_entities.contains(linked) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EntityInvalidRelation,
                            format!("{}.linkedEntity", path),
                            format!("Linked entity '{}' does not exist", linked),
                        )
                        .with_suggestion("Ensure the linked entity is defined in the schema"),
                    );
                }
            }
        }
    }

    result
}

/// Build a `trait name -> declared event-key set` map from every INLINE trait in
/// the schema. The event set is the union of declared `emits`, transition
/// triggers, and `state_machine.events[].key` — the same union the inline phase
/// validates rename source keys against.
fn build_trait_event_sets(schema: &OrbitalSchema) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let entry = map.entry(trait_def.name.clone()).or_default();
                if let Some(emits) = &trait_def.emits {
                    for e in emits {
                        entry.insert(e.event.clone());
                    }
                }
                for t in &trait_def.state_machine.transitions {
                    entry.insert(t.event.clone());
                }
                if let Some(events) = &trait_def.state_machine.events {
                    for e in events {
                        entry.insert(e.key.clone());
                    }
                }
                // Effect emit-maps + render-ui event props — the same union the
                // inline phase's RULE 3 checks rename source keys against, so
                // the two checks can never disagree on what counts as known.
                entry.extend(super::cross_orbital::collect_effect_produced_events(
                    trait_def,
                ));
            }
        }
    }
    map
}

/// Validate the `events` rename map on a `TraitReference::Reference` against the
/// referenced trait's known event set. Each `from` key that names no declared
/// event silently no-ops the rename at runtime — surface it as
/// `ORB_T_EVENT_RENAME_UNKNOWN_KEY` instead. Skips silently when the referenced
/// trait's event set is unknown (external / std trait not present inline in this
/// schema) — those are covered by the inline phase when resolved.
fn validate_event_renames(
    trait_ref: &TraitReference,
    path: &str,
    trait_events: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let TraitReference::Reference {
        reference,
        name,
        events: Some(events),
        ..
    } = trait_ref
    else {
        return result;
    };

    // Resolve the referenced trait name: the `ref:`'s trailing `.Member`
    // segment, else the `name` field.
    let target_name = reference
        .as_deref()
        .and_then(|r| r.rsplit('.').next())
        .or(name.as_deref());
    let Some(target_name) = target_name else {
        return result;
    };
    let Some(known) = trait_events.get(target_name) else {
        // Unknown target event set (external/std) — defer to the inline phase.
        return result;
    };

    for from in events.keys() {
        if from.is_empty() {
            continue;
        }
        if !known.contains(from) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EventRenameUnknownKey,
                    path,
                    format!(
                        "events rename key '{}' does not name any event declared by trait '{}';                          the rename silently no-ops.",
                        from, target_name
                    ),
                )
                .with_suggestion(
                    "Use an event key the referenced trait declares (an emit, a transition                      trigger, or a stateMachine.events entry).",
                ),
            );
        }
    }

    result
}

/// Validate a full trait definition
pub fn validate_trait_definition(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Trait must have a name
    if trait_def.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitUndefined,
                format!("{}.name", path),
                "Trait name is required",
            )
            .with_suggestion("Add a descriptive name for the trait"),
        );
    }

    // Validate trait category if specified
    if let Some(category) = &trait_def.category {
        let category_str = category_to_string(category);
        if !VALID_TRAIT_CATEGORIES.contains(&category_str.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitInvalidCategory,
                    format!("{}.category", path),
                    format!("Invalid trait category: '{}'", category_str),
                )
                .with_valid_values(
                    VALID_TRAIT_CATEGORIES
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
            );
        }
    }

    // Validate linked entity if specified
    if let Some(linked) = &trait_def.linked_entity {
        if !known_entities.contains(linked) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidRelation,
                    format!("{}.linkedEntity", path),
                    format!("Linked entity '{}' does not exist", linked),
                )
                .with_suggestion("Ensure the linked entity is defined in the schema"),
            );
        }
    }

    // Validate state machine
    let sm_result = validate_state_machine(&trait_def.state_machine, path, trait_def);
    result.merge(sm_result);

    // Validate render_ui effects for interaction traits
    if is_interaction_trait(trait_def) {
        let has_render_ui = has_render_ui_effects(&trait_def.state_machine);
        if !has_render_ui {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::TraitMissingRenderUI,
                    path,
                    format!(
                        "Interaction trait '{}' should have render_ui effects",
                        trait_def.name
                    ),
                )
                .with_suggestion("Add render_ui effects to transitions to render UI components"),
            );
        }
    }

    // Validate `trait`-typed config field defaults — must be `@trait.<TraitName>`
    // strings. Catches malformed defaults at the field declaration so authors
    // get a precise error rather than a downstream `ORB_RUI_INVALID_PATTERN`
    // failure when the config flows into render-ui children.
    if let Some(config) = &trait_def.config {
        for (field_name, cf) in config.iter() {
            if cf.field_type != "trait" {
                continue;
            }
            let Some(default) = &cf.default else { continue };
            match default {
                SExpression::String(s) => {
                    if !is_trait_ref_string(s) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::BindingTraitInvalidFormat,
                                format!("{}.config.{}.default", path, field_name),
                                format!(
                                    "trait-typed config field '{}' has invalid default '{}' — must be `@trait.<TraitName>`",
                                    field_name, s
                                ),
                            )
                            .with_suggestion(
                                "Set the default to a `@trait.PascalCaseName` reference, or remove the default and require the consumer to override.",
                            ),
                        );
                    }
                }
                _ => {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingTraitInvalidFormat,
                            format!("{}.config.{}.default", path, field_name),
                            format!(
                                "trait-typed config field '{}' default must be a string `@trait.<TraitName>` reference",
                                field_name
                            ),
                        ),
                    );
                }
            }
        }
    }

    result
}

/// `^@trait\.[A-Z][a-zA-Z0-9]*$` — accepted shape for trait references.
fn is_trait_ref_string(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("@trait.") else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Validate a state machine
pub fn validate_state_machine(
    sm: &StateMachine,
    path: &str,
    trait_def: &TraitDefinition,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let sm_path = format!("{}.stateMachine", path);

    // Must have at least one state
    if sm.states.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitNoInitialState,
                format!("{}.states", sm_path),
                "State machine must have at least one state",
            )
            .with_suggestion("Add at least one state to the state machine"),
        );
        return result;
    }

    // Collect state names and check for duplicates
    let mut state_names: HashSet<&str> = HashSet::new();
    let mut initial_states: Vec<&str> = Vec::new();

    for (i, state) in sm.states.iter().enumerate() {
        let state_path = format!("{}.states[{}]", sm_path, i);

        // Check for duplicate state names
        if !state_names.insert(&state.name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitDuplicateState,
                    state_path,
                    format!("Duplicate state name: '{}'", state.name),
                )
                .with_suggestion("Each state must have a unique name"),
            );
        }

        // Track initial states
        if state.is_initial == Some(true) {
            initial_states.push(&state.name);
        }
    }

    // Check for exactly one initial state
    if initial_states.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitNoInitialState,
                format!("{}.states", sm_path),
                "State machine must have an initial state",
            )
            .with_suggestion("Mark one state with isInitial: true"),
        );
    } else if initial_states.len() > 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitMultipleInitialStates,
                format!("{}.states", sm_path),
                format!("Multiple initial states found: {:?}", initial_states),
            )
            .with_suggestion("Only one state should be marked as initial"),
        );
    }

    // Collect event names (using key field)
    let event_names: HashSet<&str> = sm
        .events
        .as_ref()
        .map(|events| events.iter().map(|e| e.key.as_str()).collect())
        .unwrap_or_default();

    // Validate transitions
    let transitions_result =
        validate_transitions(&sm.transitions, &sm_path, &state_names, &event_names);
    result.merge(transitions_result);

    // Check for INIT transition on initial state (for interaction traits)
    if is_interaction_trait(trait_def) && !initial_states.is_empty() {
        let has_init = sm
            .transitions
            .iter()
            .any(|t| t.event == "INIT" && matches_state(&t.from, initial_states[0]));

        if !has_init {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::TraitMissingInitTransition,
                    format!("{}.transitions", sm_path),
                    format!(
                        "Interaction trait should have INIT transition on initial state '{}'",
                        initial_states[0]
                    ),
                )
                .with_suggestion("Add a transition: { from: 'initialState', to: 'initialState', event: 'INIT', effects: [...] }"),
            );
        }
    }

    // Reachability: every declared state must be reachable from the initial
    // state via some transition path. Mirrors the verifier's BFS exclusions —
    // skip `from == "*"` wildcards (they don't define real topology edges) and
    // skip the boot `INIT` self-loop on the initial state. Dead states surface
    // only as a generic "uncovered" in the verifier today; catch them here.
    if !initial_states.is_empty() {
        let initial = initial_states[0];
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for transition in &sm.transitions {
            // Skip ONLY the boot INIT *self-loop* on the initial state (it adds
            // no reachability). A `loading --INIT--> loaded` boot transition that
            // moves to a DIFFERENT state IS a real auto-fired reachability edge
            // (the runtime fires INIT on mount), so it must be followed — the
            // walker skips re-firing it, but for reachability `loaded` is reached.
            if transition.event == "INIT"
                && matches_state(&transition.from, initial)
                && transition.to.as_str() == initial
            {
                continue;
            }
            let froms: Vec<&str> = match &transition.from {
                StringOrArray::Single(s) => vec![s.as_str()],
                StringOrArray::Multiple(states) => states.iter().map(|s| s.as_str()).collect(),
            };
            for from in froms {
                if from == "*" {
                    continue;
                }
                adjacency
                    .entry(from)
                    .or_default()
                    .push(transition.to.as_str());
            }
        }

        let mut reached: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&str> = vec![initial];
        reached.insert(initial);
        while let Some(state) = queue.pop() {
            if let Some(targets) = adjacency.get(state) {
                for &target in targets {
                    if reached.insert(target) {
                        queue.push(target);
                    }
                }
            }
        }

        for state in &sm.states {
            if !reached.contains(state.name.as_str()) {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::TraitStateUnreachable,
                        format!("{}.states", sm_path),
                        format!(
                            "State '{}' is unreachable from the initial state '{}' via any transition path",
                            state.name, initial
                        ),
                    )
                    .with_suggestion(
                        "Add a transition that reaches this state, or remove the dead state",
                    ),
                );
            }
        }
    }

    result
}

/// Validate transitions
pub fn validate_transitions(
    transitions: &[TransitionDefinition],
    path: &str,
    state_names: &HashSet<&str>,
    event_names: &HashSet<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Track seen transitions for duplicate detection
    let mut seen_transitions: HashMap<(&str, &str), Vec<usize>> = HashMap::new();

    for (i, transition) in transitions.iter().enumerate() {
        let trans_path = format!("{}.transitions[{}]", path, i);

        // Validate 'from' states
        match &transition.from {
            StringOrArray::Single(state) => {
                if state != "*" && !state_names.contains(state.as_str()) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitInvalidTransition,
                            format!("{}.from", trans_path),
                            format!("Source state '{}' does not exist", state),
                        )
                        .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
                    );
                }

                // Track for duplicate detection
                let key = (state.as_str(), transition.event.as_str());
                seen_transitions.entry(key).or_default().push(i);
            }
            StringOrArray::Multiple(states) => {
                for state in states {
                    if state != "*" && !state_names.contains(state.as_str()) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidTransition,
                                format!("{}.from", trans_path),
                                format!("Source state '{}' does not exist", state),
                            )
                            .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
                        );
                    }
                }
            }
        }

        // Validate 'to' state
        if !state_names.contains(transition.to.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitInvalidTransition,
                    format!("{}.to", trans_path),
                    format!("Target state '{}' does not exist", transition.to),
                )
                .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
            );
        }

        // Validate event is defined (if events are declared)
        if !event_names.is_empty()
            && transition.event != "INIT"
            && !event_names.contains(transition.event.as_str())
        {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::TraitUndefinedEvent,
                    format!("{}.event", trans_path),
                    format!(
                        "Event '{}' is not defined in events array",
                        transition.event
                    ),
                )
                .with_suggestion("Add the event to the stateMachine.events array"),
            );
        }
    }

    // Check for duplicate transitions (same from/event without guards)
    for ((from, event), indices) in seen_transitions {
        if indices.len() > 1 {
            // Check if all have guards
            let guarded: Vec<_> = indices
                .iter()
                .filter(|&&i| transitions[i].guard.is_some())
                .collect();

            if guarded.len() < indices.len() - 1 {
                // More than one unguarded transition
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitDuplicateTransition,
                        format!("{}.transitions", path),
                        format!(
                            "Duplicate transitions from '{}' on event '{}' without guards",
                            from, event
                        ),
                    )
                    .with_suggestion("Add guard conditions to distinguish transitions"),
                );
            }
        }
    }

    result
}

/// Validate trait uniqueness across orbitals
pub fn validate_trait_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut trait_to_orbital: HashMap<&str, Vec<&str>> = HashMap::new();

    // Collect all inline trait names with their orbitals
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                trait_to_orbital
                    .entry(&trait_def.name)
                    .or_default()
                    .push(&orbital.name);
            }
        }
    }

    // Check for duplicates (same trait name in multiple orbitals)
    for (trait_name, orbitals) in trait_to_orbital {
        if orbitals.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitDuplicateName,
                    "orbitals",
                    format!(
                        "Trait '{}' is defined in multiple orbitals: {:?}",
                        trait_name, orbitals
                    ),
                )
                .with_suggestion(
                    "Use unique trait names per orbital or define traits inline within orbitals",
                ),
            );
        }
    }

    result
}

/// Check if a trait is an interaction trait
fn is_interaction_trait(trait_def: &TraitDefinition) -> bool {
    match &trait_def.category {
        Some(TraitCategory::Interaction) => true,
        None => true, // Default is interaction
        _ => false,
    }
}

/// Check if state machine has render_ui effects. Looks through `if`
/// wrappers — a conditional whose branches render still renders.
fn has_render_ui_effects(sm: &StateMachine) -> bool {
    fn renders(expr: &SExpression) -> bool {
        if is_render_ui_effect(expr) {
            return true;
        }
        if let SExpression::Array(arr) = expr {
            if let Some(SExpression::String(op)) = arr.first() {
                if op == "if" {
                    return arr.iter().skip(2).any(renders);
                }
            }
        }
        false
    }
    for transition in &sm.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if renders(effect) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if an S-expression is a fetch effect for the given entity
fn is_fetch_effect(expr: &SExpression, entity_name: &str) -> bool {
    match expr {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let (SExpression::String(op), SExpression::String(entity)) = (&arr[0], &arr[1]) {
                (op == "fetch" || op == "ref") && entity == entity_name
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Entity names fetched (`["fetch", E]` / `["ref", E]`) by INIT transitions
/// leaving the state machine's initial state.
fn init_fetch_targets(sm: &StateMachine) -> Vec<&str> {
    let Some(initial) = sm
        .states
        .iter()
        .find(|s| s.is_initial.unwrap_or(false))
        .map(|s| s.name.as_str())
    else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for t in &sm.transitions {
        if t.event != "INIT" || !matches_state(&t.from, initial) {
            continue;
        }
        let Some(effs) = &t.effects else { continue };
        for eff in effs {
            if let SExpression::Array(arr) = eff {
                if arr.len() >= 2 {
                    if let (SExpression::String(op), SExpression::String(entity)) =
                        (&arr[0], &arr[1])
                    {
                        if op == "fetch" || op == "ref" {
                            targets.push(entity.as_str());
                        }
                    }
                }
            }
        }
    }
    targets
}

/// ORB_T_INIT_MISSING_FETCH, scoped per (orbital, persistent entity): fires
/// once when interaction traits read a persistent entity but NO trait in the
/// orbital fetches it on INIT — i.e. the orbital has no datasource for data
/// its UI displays. A single fetch anywhere in the orbital satisfies every
/// consumer; requiring one per trait would mandate N duplicate fetches.
fn validate_orbital_init_fetch(
    orbital: &Orbital,
    orbital_index: usize,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut fetched: HashSet<&str> = HashSet::new();
    // (entity, first consumer trait index) in trait order → deterministic output
    let mut consumers: Vec<(&str, usize)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for (j, trait_ref) in orbital.traits.iter().enumerate() {
        let TraitReference::Inline(trait_def) = trait_ref else {
            continue;
        };
        for target in init_fetch_targets(&trait_def.state_machine) {
            fetched.insert(target);
        }
        if !is_interaction_trait(trait_def) {
            continue;
        }
        let Some(linked) = &trait_def.linked_entity else {
            continue;
        };
        let is_persistent = entity_definitions
            .get(linked)
            .map(|e| matches!(e.persistence, None | Some(Persistence::Persistent)))
            .unwrap_or(false);
        if !is_persistent {
            continue;
        }
        // Same applicability as the original per-trait rule: only traits with
        // a resolvable initial state participate.
        let has_initial = trait_def
            .state_machine
            .states
            .iter()
            .any(|s| s.is_initial.unwrap_or(false));
        if !has_initial {
            continue;
        }
        if seen.insert(linked.as_str()) {
            consumers.push((linked.as_str(), j));
        }
    }

    for (entity, j) in consumers {
        if fetched.contains(entity) {
            continue;
        }
        result.add_error(
            ValidationError::warning(
                ErrorCode::TraitInitMissingFetch,
                format!(
                    "orbitals[{}].traits[{}].stateMachine.transitions",
                    orbital_index, j
                ),
                format!(
                    "no trait in this orbital fetches persistent entity \"{}\" on INIT — interaction traits read it but nothing loads it",
                    entity
                ),
            )
            .with_suggestion(format!(
                "Add [\"fetch\", \"{}\"] to one datasource trait's INIT transition; sibling render traits read the loaded data and do not each need their own fetch",
                entity
            )),
        );
    }
    result
}

/// Check if a transition 'from' matches a state name
fn matches_state(from: &StringOrArray, state: &str) -> bool {
    match from {
        StringOrArray::Single(s) => s == state || s == "*",
        StringOrArray::Multiple(states) => states.iter().any(|s| s == state || s == "*"),
    }
}

/// Convert TraitCategory to string
fn category_to_string(category: &TraitCategory) -> String {
    match category {
        TraitCategory::Interaction => "interaction".to_string(),
        TraitCategory::Integration => "integration".to_string(),
        TraitCategory::Lifecycle => "lifecycle".to_string(),
        TraitCategory::GameCore => "game-core".to_string(),
        TraitCategory::GameEntity => "game-entity".to_string(),
        TraitCategory::GameUi => "game-ui".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EntityFieldContract, EventDefinition, FieldDefinition, FieldType, SourceBehaviorMetadata,
        StateDefinition,
    };

    fn create_valid_state_machine() -> StateMachine {
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
                    name: "active".to_string(),
                    is_initial: None,
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: Some(vec![
                EventDefinition {
                    id: None,
                    key: "INIT".to_string(),
                    name: Some("Initialize".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                },
                EventDefinition {
                    id: None,
                    key: "START".to_string(),
                    name: Some("Start".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                },
            ]),
            transitions: vec![
                // INIT self-transition with a render-ui effect — required for
                // Interaction traits to pass the validator (TraitMissingInitTransition +
                // TraitMissingRenderUI errors fire otherwise).
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![orbital_core::schema::SExpression::Array(vec![
                        orbital_core::schema::SExpression::String("render-ui".to_string()),
                        orbital_core::schema::SExpression::String("main".to_string()),
                        orbital_core::schema::SExpression::Object({
                            let mut m = std::collections::HashMap::new();
                            m.insert(
                                "type".to_string(),
                                orbital_core::schema::SExpression::String("box".to_string()),
                            );
                            m
                        }),
                    ])]),
                },
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "active".to_string(),
                    event: "START".to_string(),
                    guard: None,
                    effects: None,
                },
            ],
        }
    }

    fn create_valid_trait() -> TraitDefinition {
        TraitDefinition {
            type_params: None,
            type_args: None,
            name: "TestTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: Some(TraitCategory::Interaction),
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: create_valid_state_machine(),
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
        }
    }

    #[test]
    fn test_valid_trait() {
        let trait_def = create_valid_trait();
        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        // May have warnings but no errors
        assert!(
            result.errors.iter().all(|e| e.is_warning()),
            "Unexpected errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_trait_no_initial_state() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states[0].is_initial = None;

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitNoInitialState));
    }

    #[test]
    fn test_trait_multiple_initial_states() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states[1].is_initial = Some(true);

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitMultipleInitialStates));
    }

    #[test]
    fn test_trait_state_unreachable_fires() {
        // 3 states: idle (initial) -> active via START; `orphan` has no
        // incoming transition, so it is unreachable from the initial state.
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states.push(StateDefinition {
            name: "orphan".to_string(),
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        });

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitStateUnreachable
                    && e.message.contains("orphan")),
            "Expected ORB_T_STATE_UNREACHABLE for 'orphan', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_trait_state_all_reachable_no_fire() {
        // idle (initial) -> active is fully reachable; no unreachable state.
        let trait_def = create_valid_trait();
        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitStateUnreachable),
            "Did not expect ORB_T_STATE_UNREACHABLE, got: {:?}",
            result.errors
        );
    }

    // ---- @rebindable enforcement (validate_entity_rebind) ----

    fn make_entity(name: &str, fields: &[&str]) -> EntityDefinition {
        EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: name.to_string(),
            collection: None,
            persistence: None,
            fields: fields
                .iter()
                .map(|f| FieldDefinition {
                    name: Some(f.to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: None,
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                })
                .collect(),
            instances: None,
            id: None,
        }
    }

    /// A trait imported from std-browse (atom entity `BrowseItem`) and rebound
    /// to `Listing`.
    fn rebound_trait(rebindable: Option<bool>, requires: &[&str]) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.source_behavior = Some(SourceBehaviorMetadata {
            behavior: "std/behaviors/std-browse".to_string(),
            alias: "Browse".to_string(),
            original_name: "BrowseItemBrowse".to_string(),
        });
        t.source_entity_definition = Some(make_entity("BrowseItem", &[]));
        t.linked_entity = Some("Listing".to_string());
        t.entity_rebindable = rebindable;
        if !requires.is_empty() {
            t.entity_contract = Some(EntityFieldContract {
                requires: requires.iter().map(|s| s.to_string()).collect(),
                provides: vec![],
            });
        }
        t
    }

    #[test]
    fn test_rebind_not_rebindable_rejected() {
        let t = rebound_trait(None, &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEntityNotRebindable));
    }

    #[test]
    fn test_rebind_rebindable_allowed() {
        let t = rebound_trait(Some(true), &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new());
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_rebind_missing_required_field_rejected() {
        let t = rebound_trait(Some(true), &["sku"]);
        let mut defs = HashMap::new();
        defs.insert("Listing".to_string(), make_entity("Listing", &["title"])); // no `sku`
        let result = validate_entity_rebind(&t, "test", &defs);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RebindMissingEntityFields));
    }

    #[test]
    fn test_rebind_required_field_present_ok() {
        let t = rebound_trait(Some(true), &["sku"]);
        let mut defs = HashMap::new();
        defs.insert(
            "Listing".to_string(),
            make_entity("Listing", &["sku", "title"]),
        );
        let result = validate_entity_rebind(&t, "test", &defs);
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_inline_authored_binding_exempt() {
        // No source_behavior = authored directly on the orbital, not a rebind.
        let mut t = create_valid_trait();
        t.linked_entity = Some("Listing".to_string());
        t.entity_rebindable = None;
        let result = validate_entity_rebind(&t, "test", &HashMap::new());
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_trait_duplicate_state() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states.push(StateDefinition {
            name: "idle".to_string(), // Duplicate
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        });

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitDuplicateState));
    }

    #[test]
    fn test_trait_invalid_transition_target() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.transitions[0].to = "nonexistent".to_string();

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitInvalidTransition));
    }

    #[test]
    fn test_trait_undefined_reference() {
        let trait_ref = TraitReference::Reference {
            type_args: None,
            id: None,
            reference: Some("NonExistentTrait".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        };

        let result = validate_trait_reference(&trait_ref, "test", &HashSet::new(), &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitUndefined));
    }

    #[test]
    fn test_std_trait_reference() {
        let trait_ref = TraitReference::Reference {
            type_args: None,
            id: None,
            reference: Some("std/Form".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        };

        let result = validate_trait_reference(&trait_ref, "test", &HashSet::new(), &HashSet::new());
        assert!(result.is_valid());
    }

    fn event_ref(
        reference: Option<&str>,
        name: Option<&str>,
        renames: &[(&str, &str)],
    ) -> TraitReference {
        let mut events = HashMap::new();
        for (k, v) in renames {
            events.insert(k.to_string(), v.to_string());
        }
        TraitReference::Reference {
            type_args: None,
            id: None,
            reference: reference.map(str::to_string),
            name: name.map(str::to_string),
            linked_entity: None,
            config: None,
            events: Some(events),
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        }
    }

    fn known_events() -> HashMap<String, HashSet<String>> {
        let mut m = HashMap::new();
        m.insert(
            "TestTrait".to_string(),
            ["INIT", "START"].into_iter().map(str::to_string).collect(),
        );
        m
    }

    #[test]
    fn test_event_rename_unknown_key_via_ref() {
        // `Alias.traits.TestTrait` resolves to TestTrait; BOGUS isn't declared.
        let r = event_ref(Some("Alias.traits.TestTrait"), None, &[("BOGUS", "X")]);
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EventRenameUnknownKey));
    }

    #[test]
    fn test_event_rename_unknown_key_via_name() {
        let r = event_ref(None, Some("TestTrait"), &[("NOPE", "Y")]);
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EventRenameUnknownKey));
    }

    #[test]
    fn test_event_rename_known_key_passes() {
        let r = event_ref(
            Some("Alias.traits.TestTrait"),
            None,
            &[("INIT", "ADD"), ("START", "GO")],
        );
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result.is_valid());
    }

    #[test]
    fn test_event_rename_unknown_target_trait_skips() {
        // Target trait not in the event-set map (external/std) — defer to inline.
        let r = event_ref(
            Some("Alias.traits.ExternalTrait"),
            None,
            &[("WHATEVER", "Z")],
        );
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result.is_valid());
    }

    // ---- entity-scoped INIT fetch (validate_orbital_init_fetch) ----

    fn make_orbital(traits: Vec<TraitDefinition>) -> Orbital {
        Orbital {
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: orbital_core::schema::EntityRef::Inline(make_entity("Item", &["id"])),
            auxiliary_entities: None,
            traits: traits.into_iter().map(TraitReference::Inline).collect(),
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    fn make_consumer(name: &str) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.name = name.to_string();
        t.linked_entity = Some("Item".to_string());
        t
    }

    fn make_datasource(name: &str) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.name = name.to_string();
        t.category = Some(TraitCategory::Lifecycle);
        t.linked_entity = Some("Item".to_string());
        t.state_machine.transitions[0]
            .effects
            .get_or_insert_with(Vec::new)
            .push(SExpression::Array(vec![
                SExpression::String("fetch".to_string()),
                SExpression::String("Item".to_string()),
            ]));
        t
    }

    fn entity_defs() -> HashMap<String, EntityDefinition> {
        let mut m = HashMap::new();
        m.insert("Item".to_string(), make_entity("Item", &["id"]));
        m
    }

    #[test]
    fn test_init_fetch_fires_once_per_entity_when_nothing_fetches() {
        let orbital = make_orbital(vec![make_consumer("ViewA"), make_consumer("ViewB")]);
        let result = validate_orbital_init_fetch(&orbital, 0, &entity_defs());
        let fetch_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitInitMissingFetch)
            .collect();
        assert_eq!(
            fetch_errors.len(),
            1,
            "expected ONE entity-scoped error, got: {:?}",
            fetch_errors
        );
        assert!(fetch_errors[0].message.contains("Item"));
    }

    #[test]
    fn test_init_fetch_satisfied_by_sibling_datasource() {
        let orbital = make_orbital(vec![
            make_consumer("ViewA"),
            make_consumer("ViewB"),
            make_datasource("DataSource"),
        ]);
        let result = validate_orbital_init_fetch(&orbital, 0, &entity_defs());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitInitMissingFetch),
            "sibling datasource fetch must satisfy every consumer, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_init_fetch_skips_runtime_entities() {
        let mut defs = entity_defs();
        defs.get_mut("Item").unwrap().persistence = Some(Persistence::Runtime);
        let orbital = make_orbital(vec![make_consumer("ViewA")]);
        let result = validate_orbital_init_fetch(&orbital, 0, &defs);
        assert!(result
            .errors
            .iter()
            .all(|e| e.code != ErrorCode::TraitInitMissingFetch));
    }
}
