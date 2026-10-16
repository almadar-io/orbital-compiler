//! Layout Strategy Detection
//!
//! Auto-detects the best layout strategy for a composed application based on
//! the number of orbitals and their event wiring topology.
//!
//! Faithful Rust port of `packages/almadar-core/src/builders/layout-strategy.ts`.

use super::event_wiring::EventWiringEntry;
use orbital_core_base::schema::Orbital;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ============================================================================
// Types
// ============================================================================

/// Layout strategy for the composed application.
///
/// - `Single`: One orbital, one page
/// - `Tabs`: 2-4 orbitals with no sequential chain
/// - `Sidebar`: 5+ orbitals (navigation-heavy)
/// - `Dashboard`: Single page with all orbitals visible
/// - `WizardFlow`: Sequential event chain detected (A -> B -> C)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutStrategy {
    Sidebar,
    Tabs,
    Dashboard,
    #[serde(rename = "wizard-flow")]
    WizardFlow,
    Single,
}

// ============================================================================
// Chain Detection
// ============================================================================

/// Detect whether the event wiring forms a sequential chain.
///
/// A sequential chain exists when orbital A's `from` feeds into B's `to`,
/// and B's `from` feeds into C's `to`, forming A -> B -> C.
///
/// We build a directed graph from wiring entries and look for a chain that
/// covers 3+ nodes (the minimum for a meaningful wizard flow).
///
/// IMPORTANT: This function must mirror the TypeScript algorithm exactly,
/// including its iteration order. The TS code uses `Map` and `Set`, which
/// preserve insertion order in JavaScript. To match that behavior we use
/// insertion-ordered Vec-based structures rather than hash collections.
fn has_sequential_chain(wiring: &[EventWiringEntry]) -> bool {
    if wiring.len() < 2 {
        return false;
    }

    // Build adjacency: from -> Vec<to> (preserving insertion order, like JS Map+Set).
    // We also track an `all_targets` set used to find roots.
    let mut adjacency_keys: Vec<String> = Vec::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    let mut adjacency_target_sets: HashMap<String, HashSet<String>> = HashMap::new();
    let mut all_targets: HashSet<String> = HashSet::new();

    for entry in wiring {
        if !adjacency.contains_key(&entry.from) {
            adjacency_keys.push(entry.from.clone());
            adjacency.insert(entry.from.clone(), Vec::new());
            adjacency_target_sets.insert(entry.from.clone(), HashSet::new());
        }
        let targets_vec = adjacency.get_mut(&entry.from).expect("just inserted");
        let targets_set = adjacency_target_sets
            .get_mut(&entry.from)
            .expect("just inserted");
        if targets_set.insert(entry.to.clone()) {
            targets_vec.push(entry.to.clone());
        }
        all_targets.insert(entry.to.clone());
    }

    // Find roots (nodes that are sources but never targets), preserving the
    // order in which `from` keys were first observed.
    let mut roots: Vec<String> = Vec::new();
    for from in &adjacency_keys {
        if !all_targets.contains(from) {
            roots.push(from.clone());
        }
    }

    // If no clear root, fall back to all sources in insertion order.
    let starts: Vec<String> = if !roots.is_empty() {
        roots
    } else {
        adjacency_keys.clone()
    };

    // Walk from each start and measure chain length.
    for start in starts {
        let mut current = start.clone();
        let mut length = 1usize;
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(current.clone());

        loop {
            let nexts = adjacency.get(&current);
            let Some(nexts) = nexts else { break };
            if nexts.is_empty() {
                break;
            }

            // Follow the first unvisited successor (linear chain).
            let mut advanced = false;
            for next in nexts {
                if !visited.contains(next) {
                    visited.insert(next.clone());
                    current = next.clone();
                    length += 1;
                    advanced = true;
                    break;
                }
            }

            if !advanced {
                break;
            }
        }

        // A chain of 3+ nodes qualifies as wizard-flow.
        if length >= 3 {
            return true;
        }
    }

    false
}

// ============================================================================
// Public API
// ============================================================================

/// Detect the best layout strategy based on orbital count and event wiring.
///
/// Heuristic:
/// 1. Sequential event chain detected -> `WizardFlow`
/// 2. 1 orbital -> `Single`
/// 3. 2-4 orbitals -> `Tabs`
/// 4. 5+ orbitals -> `Sidebar`
pub fn detect_layout_strategy(
    orbitals: &[Orbital],
    event_wiring: Option<&[EventWiringEntry]>,
) -> LayoutStrategy {
    detect_layout_strategy_inner(orbitals.len(), event_wiring)
}

/// Variant of `detect_layout_strategy` that operates on a slice of JSON
/// orbital values. Used by the JSON-based compose pipeline so that input
/// fields are preserved untouched.
pub fn detect_layout_strategy_from_values(
    orbitals: &[Value],
    event_wiring: Option<&[EventWiringEntry]>,
) -> LayoutStrategy {
    detect_layout_strategy_inner(orbitals.len(), event_wiring)
}

fn detect_layout_strategy_inner(
    count: usize,
    event_wiring: Option<&[EventWiringEntry]>,
) -> LayoutStrategy {
    // Check for sequential chain first (takes priority).
    if let Some(wiring) = event_wiring {
        if !wiring.is_empty() && has_sequential_chain(wiring) {
            return LayoutStrategy::WizardFlow;
        }
    }

    if count <= 1 {
        return LayoutStrategy::Single;
    }

    if count <= 4 {
        return LayoutStrategy::Tabs;
    }

    LayoutStrategy::Sidebar
}
