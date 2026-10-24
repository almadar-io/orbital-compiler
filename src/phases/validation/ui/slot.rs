//! Slot Validation
//!
//! Tick-scoped slot contention (`ORB_SLOT_CONTENTION_TICK`) and the page-trait helpers it shares.
//! Coverage, contention and nesting live in the std lint `std-orb-lint-slot`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{
    Orbital, OrbitalSchema, PageDefinition, StateMachine, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet, VecDeque};

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

/// Resolve a page's trait mounts to this orbital's own trait definitions.
///
/// A `page "/x" -> A, B` declaration lowers to
/// `page.traits = [{"ref":"A"},{"ref":"B"}]` — NAME references into the orbital's
/// trait list, never inline definitions. Any page-scoped check that reads
/// `page.traits` and keeps only `TraitReference::Inline` therefore sees an empty
/// list on every real program and silently checks nothing, which is how
/// `validate_page_slots` came to have never fired on the corpus. Resolve through
/// the reference instead.
///
/// Mounts that name a trait the orbital does not define are dropped here;
/// reporting an undefined trait is `ORB_T_UNDEFINED_TRAIT`'s job, not a slot
/// check's.
pub(crate) fn resolve_page_traits<'a>(
    orbital: &'a Orbital,
    page: &'a PageDefinition,
) -> Vec<&'a TraitDefinition> {
    let by_name: HashMap<&str, &TraitDefinition> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(def) => Some((def.name.as_str(), def)),
            TraitReference::Reference { .. } => None,
        })
        .collect();

    page.traits
        .as_ref()
        .map(|mounts| {
            mounts
                .iter()
                .filter_map(|mount| match mount {
                    TraitReference::Inline(def) => Some(def),
                    TraitReference::Reference {
                        reference, name, ..
                    } => {
                        // A page mount names a sibling trait; `reference` carries
                        // the `Alias.traits.Name` form only for imports.
                        let key = name
                            .as_deref()
                            .or_else(|| reference.as_deref())
                            .unwrap_or_default();
                        by_name.get(key).copied()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every trait mounted on `page` that repaints `slot` from inside a `ticks`
/// block, skipping any whose tick guard is provably dead.
///
/// Reads the inferred effect row rather than re-walking the effect tree: the row
/// already resolved `@config.<knob>` slots at inline, and re-deriving it here
/// would be a second implementation of the same question.
fn tick_slot_writers<'a>(
    traits: &[&'a TraitDefinition],
) -> HashMap<String, Vec<&'a TraitDefinition>> {
    let mut writers: HashMap<String, Vec<&TraitDefinition>> = HashMap::new();
    for trait_def in traits {
        if !has_live_tick(trait_def) {
            continue;
        }
        let Some(row) = trait_def.effect_row.as_ref() else {
            continue;
        };
        let mut slots: Vec<&str> = row.tick_rendered_slots().collect();
        slots.sort();
        slots.dedup();
        for slot in slots {
            writers.entry(slot.to_string()).or_default().push(trait_def);
        }
    }
    writers
}

/// True when at least one of the trait's ticks can actually run.
///
/// A call site that sets `config { running: false }` on all but one composed
/// trait is using the sanctioned way to keep a single owner for a resource; at
/// inline that knob is substituted into the tick guard, which folds to a literal
/// `false`. Flagging those would flag the remedy.
fn has_live_tick(trait_def: &TraitDefinition) -> bool {
    let Some(ticks) = trait_def.ticks.as_ref() else {
        return false;
    };
    ticks.iter().any(|tick| {
        tick.guard
            .as_ref()
            .map(|g| !g.is_statically_false())
            .unwrap_or(true)
    })
}

/// Two or more traits on one page repainting one slot every frame.
///
/// This is the tick-scoped sibling of the lint's `ORB_SLOT_CONTENTION`, and the tick scoping is
/// what makes it decidable. Traits that name the same slot from TRANSITIONS
/// routinely never coexist — a modal over the catalog behind it, a detail panel
/// swapped by state — which is why the transition-scoped check has to be narrow.
/// A `ticks` body carries no such alternation: it runs on every frame of the
/// clock, so two of them on one slot paint together, and the runtime stacks the
/// writers into one column rather than letting either win.
pub fn validate_tick_slot_contention(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let Some(pages) = &orbital.pages else {
            continue;
        };
        for (j, page_ref) in pages.iter().enumerate() {
            let Some(page) = page_ref.as_inline() else {
                continue;
            };
            let mounted = resolve_page_traits(orbital, page);
            if mounted.len() < 2 {
                continue;
            }

            let mut conflicts: Vec<(String, Vec<&TraitDefinition>)> = tick_slot_writers(&mounted)
                .into_iter()
                .filter(|(_, writers)| writers.len() > 1)
                .collect();
            conflicts.sort_by(|a, b| a.0.cmp(&b.0));

            for (slot, writers) in conflicts {
                let mut names: Vec<&str> = writers.iter().map(|t| t.name.as_str()).collect();
                names.sort();
                let path = format!("orbitals[{}].pages[{}]", i, j);
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SlotContentionTick,
                        &path,
                        format!(
                            "page \"{}\": traits {} both render into slot '{}' from inside a ticks block, and no tick guard switches either one off — a ticks block runs EVERY frame, so they paint slot '{}' on every frame and the runtime stacks them into one growing column instead of showing either",
                            page.path,
                            format_trait_list(&names),
                            slot,
                            slot
                        ),
                    )
                    .with_suggestion(format!(
                        "Give exactly one trait slot '{}': set config {{ running: false }} on the others at this call site, or send them to different slots by making the slot a config knob on the atom (write (render-ui @config.surface ...) in the atom's .lolo, declare `surface` in its config, then set a different value per trait here)",
                        slot
                    )),
                );
            }
        }
    }

    ctx.merge(result);
}

/// `'A' and 'B'` for two, `'A', 'B' and 'C'` beyond — the message names every
/// participant, never "multiple traits", so a reader knows exactly what to edit.
fn format_trait_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("'{only}'"),
        [rest @ .., last] => {
            let head = rest
                .iter()
                .map(|n| format!("'{n}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{head} and '{last}'")
        }
    }
}
