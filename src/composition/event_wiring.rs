//! Event Wiring
//!
//! Applies cross-orbital event wiring to orbital definitions.
//! Adds emits/listens declarations to traits so they can communicate
//! across orbital boundaries.
//!
//! Faithful Rust port of `packages/almadar-core/src/builders/event-wiring.ts`.

use super::compose_behaviors::ComposeError;
use std::sync::Arc;
use orbital_core_base::schema::{
    EmitDefinition, EventScope, ListenDefinition, Orbital, TraitDefinition, TraitReference,
};
use serde::{Deserialize, Serialize};

// ============================================================================
// Types
// ============================================================================

/// A single event wiring entry connecting two traits across orbitals.
///
/// Mirrors the TypeScript `EventWiringEntry` interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventWiringEntry {
    /// Source trait name or orbital name
    pub from: String,
    /// Event name (UPPER_SNAKE_CASE)
    pub event: String,
    /// Target trait name or orbital name
    pub to: String,
    /// Event to trigger on the listener side
    pub triggers: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Find an inline trait by name across all orbitals and return a mutable
/// reference. Only inline traits can be mutated; reference traits are skipped.
///
/// Mirrors `findInlineTrait` from the TS implementation, but returns a
/// mutable borrow because Rust does not have shared mutability.
fn find_inline_trait_mut<'a>(
    orbitals: &'a mut [Orbital],
    name: &str,
) -> Option<&'a mut TraitDefinition> {
    for orbital in orbitals.iter_mut() {
        for trait_ref in orbital.traits.iter_mut() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                if trait_def.name == name {
                    return Some(Arc::make_mut(trait_def));
                }
            }
        }
    }
    None
}

/// Check if an emit already exists in the list.
fn has_emit(emits: &[EmitDefinition], event: &str) -> bool {
    emits.iter().any(|e| e.event == event)
}

/// Check if a listen already exists in the list.
fn has_listen(listens: &[ListenDefinition], event: &str, triggers: &str) -> bool {
    listens
        .iter()
        .any(|l| l.event == event && l.triggers.as_deref() == Some(triggers))
}

// ============================================================================
// Public API
// ============================================================================

/// Apply event wiring to orbital definitions.
///
/// For each wiring entry:
/// 1. Find the source trait and add an external emit (if not already present)
/// 2. Find the target trait and add an external listen (if not already present)
///
/// The TS implementation deep-clones the input and returns a new array. The
/// Rust port mirrors this by mutating a slice of `Orbital`s in place. Callers
/// pass a cloned vec (or `&mut Vec<Orbital>` via deref coercion) when they
/// want immutability.
///
/// Returns `Result<(), ComposeError>` for symmetry with the rest of the
/// composition API even though the typed mutation cannot currently fail.
pub fn apply_event_wiring(
    orbitals: &mut [Orbital],
    wiring: &[EventWiringEntry],
) -> Result<(), ComposeError> {
    for entry in wiring {
        // Wire the source: add emit
        if let Some(source_trait) = find_inline_trait_mut(orbitals, &entry.from) {
            if source_trait.emits.is_none() {
                source_trait.emits = Some(Vec::new());
            }
            let emits = source_trait.emits.as_mut().expect("just initialized");
            if !has_emit(emits, &entry.event) {
                emits.push(EmitDefinition {
                    // Synthesized from an explicit wiring entry, not from a
                    // config knob — so this is an obligation, not an offer.
                    definer_knob: None,
                    event: entry.event.clone(),
                    description: None,
                    synonyms: None,
                    tier: None,
                    scope: Some(EventScope::External),
                    payload_schema: None,
                    event_id: None,
                    scope_overridden: None,
                    payload_entity: None,
                });
            }
        }

        // Wire the target: add listen
        if let Some(target_trait) = find_inline_trait_mut(orbitals, &entry.to) {
            if target_trait.listens.is_none() {
                target_trait.listens = Some(Vec::new());
            }
            let listens = target_trait.listens.as_mut().expect("just initialized");
            if !has_listen(listens, &entry.event, &entry.triggers) {
                listens.push(ListenDefinition {
                    event: entry.event.clone(),
                    triggers: Some(entry.triggers.clone()),
                    scope: Some(EventScope::External),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_mapping: None,
                    guard: None,
                    source: None,
                    event_id: None,
                    triggers_id: None,
                });
            }
        }
    }

    Ok(())
}
