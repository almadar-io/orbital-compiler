//! Listen-trigger-matched backstop (error).
//!
//! A `listens { Src.EVENT -> TRIGGER }` resolves to an internal TRIGGER the
//! trait must react to. If no transition in the SAME trait fires on TRIGGER,
//! the listen is dead: the bus delivers the event but the state machine has no
//! handler, so the wire silently drops. The L1 (`orbital-lolo`) author-time
//! check covers hand-authored `.lolo`; this post-inline pass also catches the
//! `events: { OLD: NEW }`-rename variant, where the rewrite renamed the
//! transition event but a hand-authored TRIGGER (or vice versa) drifted apart.
//!
//! Runs POST-inline so `use`-imported atoms/molecules are checked too.

use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, TraitReference};
use std::collections::HashSet;

/// Validate that every `listens[].triggers` resolves to a transition in the
/// same trait's state machine.
pub(crate) fn validate_listen_triggers(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };

            // The set of events the trait reacts to: every transition's
            // triggering event. Sourced from `state_machine.transitions[].event`
            // (same access pattern as `cross_orbital/event_wiring.rs:65-69`).
            let transition_events: HashSet<&str> = td
                .state_machine
                .transitions
                .iter()
                .map(|t| t.event.as_str())
                .collect();

            for listen in listens {
                // The internal trigger defaults to the listened event when no
                // explicit `-> TRIGGER` was authored.
                let trigger = listen.triggers.as_deref().unwrap_or(&listen.event);
                if !transition_events.contains(trigger) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::ListenTriggerUnmatched,
                            format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                            format!(
                                "Listen entry in '{}.{}' triggers '{}' but no transition in the \
                                 same trait reacts to it.",
                                orbital.name, td.name, trigger
                            ),
                        )
                        .with_suggestion(
                            "Add a transition whose event matches the listen trigger, or fix the \
                             `-> TRIGGER` to name an event the trait's state machine handles.",
                        ),
                    );
                }
            }
        }
    }

    result
}
