//! Synthetic Listener Injection (DEPRECATED)
//!
//! This phase is no longer needed. The `ref` operator handles intra-orbital
//! entity reactivity via the EntityStore. Cross-orbital communication uses
//! the event bus (`emit`/`listens`) as before.
//!
//! The function is kept as a no-op to avoid breaking the compiler pipeline.
//! It will be removed entirely in a future cleanup.

use orbital_core::ir::OirModule;

/// Check if an event name is a synthetic DATA_CHANGED event (legacy)
pub fn is_synthetic_event(event: &str) -> bool {
    event.ends_with(":DATA_CHANGED") || event.starts_with("__REFRESH_")
}

/// No-op. Synthetic listeners are replaced by the `ref` operator.
pub fn inject_synthetic_listeners(_module: &mut OirModule) {
    // Intentionally empty. The ref operator handles entity reactivity.
}
