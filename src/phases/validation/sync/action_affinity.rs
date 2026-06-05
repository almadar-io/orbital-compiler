//! Action-Component Affinity Rules
//!
//! Synced from `builder/packages/shared/src/orbitals/std/behaviors/action-affinity.ts`.
//!
//! Defines which actions are valid on which components in the Closed Circuit pattern.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Action affinity definition for a component
#[derive(Debug, Clone)]
pub struct ActionAffinity {
    /// Actions that are valid on this component
    pub valid: HashSet<&'static str>,
    /// Actions that should NEVER appear on this component
    pub invalid: HashSet<&'static str>,
}

impl ActionAffinity {
    fn new(valid: &[&'static str], invalid: &[&'static str]) -> Self {
        Self {
            valid: valid.iter().copied().collect(),
            invalid: invalid.iter().copied().collect(),
        }
    }
}

/// The action-component affinity matrix.
/// Defines which actions belong on which components.
pub static ACTION_AFFINITY: LazyLock<HashMap<&'static str, ActionAffinity>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    // Display components - view data and trigger navigation/editing
    map.insert(
        "entity-table",
        ActionAffinity::new(
            &["VIEW", "EDIT", "DELETE", "SELECT", "SORT", "PAGE"],
            &["SAVE", "CANCEL", "SUBMIT", "CREATE", "CLOSE"],
        ),
    );
    map.insert(
        "entity-list",
        ActionAffinity::new(
            &["VIEW", "EDIT", "DELETE", "SELECT"],
            &["SAVE", "CANCEL", "SUBMIT", "CREATE", "CLOSE"],
        ),
    );
    map.insert(
        "entity-cards",
        ActionAffinity::new(
            &["VIEW", "EDIT", "DELETE", "SELECT"],
            &["SAVE", "CANCEL", "SUBMIT", "CREATE", "CLOSE"],
        ),
    );
    map.insert(
        "card-grid",
        ActionAffinity::new(
            &["VIEW", "EDIT", "DELETE", "SELECT"],
            &["SAVE", "CANCEL", "SUBMIT", "CREATE", "CLOSE"],
        ),
    );

    // Header components - page-level actions
    map.insert(
        "page-header",
        ActionAffinity::new(
            &["CREATE", "REFRESH", "EXPORT", "IMPORT", "BACK", "FILTER"],
            &["SAVE", "VIEW", "EDIT", "DELETE", "SUBMIT"],
        ),
    );

    // Form components - data entry and submission
    map.insert(
        "form",
        ActionAffinity::new(
            &[
                "SAVE",
                "CANCEL",
                "SUBMIT",
                "CLOSE",
                "RESET",
                "FIELD_CHANGE",
                "FIELD_BLUR",
            ],
            &["VIEW", "DELETE", "CREATE", "SELECT", "EDIT"],
        ),
    );
    map.insert(
        "form-section",
        ActionAffinity::new(
            &["FIELD_CHANGE", "FIELD_BLUR"],
            &["SAVE", "VIEW", "DELETE", "CREATE", "SELECT", "EDIT"],
        ),
    );
    map.insert(
        "form-actions",
        ActionAffinity::new(
            &["SAVE", "CANCEL", "SUBMIT", "RESET"],
            &["VIEW", "DELETE", "CREATE", "SELECT", "EDIT"],
        ),
    );

    // Detail components - view single entity with edit/delete options
    map.insert(
        "detail-panel",
        ActionAffinity::new(
            &["EDIT", "DELETE", "CLOSE", "BACK"],
            &["SAVE", "CREATE", "SELECT", "SUBMIT"],
        ),
    );
    map.insert(
        "entity-detail",
        ActionAffinity::new(
            &["EDIT", "DELETE", "CLOSE", "BACK"],
            &["SAVE", "CREATE", "SELECT", "SUBMIT"],
        ),
    );

    // Container components - open/close states
    map.insert(
        "modal",
        ActionAffinity::new(
            &["CLOSE", "CONFIRM", "CANCEL"],
            &["VIEW", "CREATE", "EDIT", "DELETE"],
        ),
    );
    map.insert(
        "modal-container",
        ActionAffinity::new(
            &["CLOSE", "CONFIRM", "CANCEL"],
            &["VIEW", "CREATE", "EDIT", "DELETE"],
        ),
    );
    map.insert(
        "drawer",
        ActionAffinity::new(&["CLOSE"], &["VIEW", "CREATE", "DELETE"]),
    );

    // Navigation components
    map.insert(
        "tabs",
        ActionAffinity::new(
            &["SELECT_TAB"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "tab-bar",
        ActionAffinity::new(
            &["SELECT_TAB"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "wizard-navigation",
        ActionAffinity::new(
            &["NEXT", "PREV", "GO_TO", "COMPLETE"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "wizard-progress",
        ActionAffinity::new(
            &["GO_TO"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT", "NEXT", "PREV"],
        ),
    );

    // Filter components
    map.insert(
        "filter-group",
        ActionAffinity::new(
            &["SET_FILTER", "CLEAR_FILTER", "CLEAR_ALL"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "search-bar",
        ActionAffinity::new(
            &["SEARCH", "CLEAR_SEARCH"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "search-input",
        ActionAffinity::new(
            &["SEARCH", "CLEAR_SEARCH"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );

    // Pagination components
    map.insert(
        "pagination",
        ActionAffinity::new(
            &["NEXT_PAGE", "PREV_PAGE", "GO_TO_PAGE", "SET_PAGE_SIZE"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );

    // Confirmation components
    map.insert(
        "confirm-dialog",
        ActionAffinity::new(
            &["CONFIRM", "CANCEL"],
            &["VIEW", "CREATE", "EDIT", "DELETE", "SAVE"],
        ),
    );

    // Empty/Loading states
    map.insert(
        "empty-state",
        ActionAffinity::new(
            &["CREATE"], // Often has a "Create first item" CTA
            &["SAVE", "VIEW", "EDIT", "DELETE", "CANCEL"],
        ),
    );
    map.insert(
        "loading-state",
        ActionAffinity::new(&[], &["SAVE", "VIEW", "EDIT", "DELETE", "CREATE", "CANCEL"]),
    );

    // Stats/Dashboard - view only
    map.insert(
        "stats",
        ActionAffinity::new(&[], &["SAVE", "VIEW", "EDIT", "DELETE", "CREATE", "CANCEL"]),
    );

    // Game components
    map.insert(
        "game-canvas",
        ActionAffinity::new(
            &["INPUT", "PAUSE", "UNPAUSE"],
            &["SAVE", "CREATE", "DELETE"],
        ),
    );
    map.insert(
        "game-hud",
        ActionAffinity::new(&["PAUSE"], &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"]),
    );
    map.insert(
        "game-controls",
        ActionAffinity::new(
            &["INPUT", "ACTION"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "game-menu",
        ActionAffinity::new(
            &["START", "OPTIONS", "QUIT", "SELECT"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "game-pause-overlay",
        ActionAffinity::new(
            &["RESUME", "QUIT", "OPTIONS"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );
    map.insert(
        "game-over-screen",
        ActionAffinity::new(
            &["RETRY", "QUIT", "MAIN_MENU"],
            &["SAVE", "CREATE", "DELETE", "VIEW", "EDIT"],
        ),
    );

    map
});

/// Interactive patterns that typically need itemActions
pub static INTERACTIVE_PATTERNS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["entity-table", "entity-list", "entity-cards", "card-grid"]
        .into_iter()
        .collect()
});

/// Form-related patterns
pub static FORM_PATTERNS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["form", "form-section", "form-actions"]
        .into_iter()
        .collect()
});

/// List/table patterns
pub static LIST_PATTERNS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["entity-table", "entity-list", "entity-cards", "card-grid"]
        .into_iter()
        .collect()
});

/// Form-related actions
pub static FORM_ACTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "SAVE",
        "CANCEL",
        "SUBMIT",
        "RESET",
        "FIELD_CHANGE",
        "FIELD_BLUR",
    ]
    .into_iter()
    .collect()
});

/// List-related actions
pub static LIST_ACTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["SELECT", "SELECT_ALL", "PAGE", "SORT", "FILTER"]
        .into_iter()
        .collect()
});

/// Check if an action is explicitly invalid for a component
pub fn is_action_invalid_for_component(action: &str, component: &str) -> bool {
    if let Some(affinity) = ACTION_AFFINITY.get(component) {
        affinity.invalid.contains(action)
    } else {
        false
    }
}

/// Get valid actions for a component
pub fn get_valid_actions_for_component(component: &str) -> Vec<&'static str> {
    if let Some(affinity) = ACTION_AFFINITY.get(component) {
        affinity.valid.iter().copied().collect()
    } else {
        Vec::new()
    }
}

/// Get invalid actions for a component
pub fn get_invalid_actions_for_component(component: &str) -> Vec<&'static str> {
    if let Some(affinity) = ACTION_AFFINITY.get(component) {
        affinity.invalid.iter().copied().collect()
    } else {
        Vec::new()
    }
}

/// Check if a pattern is interactive (typically needs itemActions)
pub fn is_interactive_pattern(pattern_type: &str) -> bool {
    INTERACTIVE_PATTERNS.contains(pattern_type)
}

/// Check if a pattern is form-related
pub fn is_form_pattern(pattern_type: &str) -> bool {
    FORM_PATTERNS.contains(pattern_type)
}

/// Check if a pattern is list-related
pub fn is_list_pattern(pattern_type: &str) -> bool {
    LIST_PATTERNS.contains(pattern_type)
}

/// Check if an action is form-related
pub fn is_form_action(action: &str) -> bool {
    FORM_ACTIONS.contains(action)
}

/// Check if an action is list-related
pub fn is_list_action(action: &str) -> bool {
    LIST_ACTIONS.contains(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_affinity() {
        assert!(is_action_invalid_for_component("SAVE", "entity-table"));
        assert!(!is_action_invalid_for_component("VIEW", "entity-table"));
        assert!(is_action_invalid_for_component("DELETE", "form"));
    }

    #[test]
    fn test_valid_actions() {
        let actions = get_valid_actions_for_component("entity-table");
        assert!(actions.contains(&"VIEW"));
        assert!(actions.contains(&"EDIT"));
        assert!(actions.contains(&"DELETE"));
    }

    #[test]
    fn test_interactive_patterns() {
        assert!(is_interactive_pattern("entity-table"));
        assert!(is_interactive_pattern("entity-list"));
        assert!(!is_interactive_pattern("form"));
    }

    #[test]
    fn test_form_patterns() {
        assert!(is_form_pattern("form"));
        assert!(is_form_pattern("form-section"));
        assert!(!is_form_pattern("entity-table"));
    }
}
