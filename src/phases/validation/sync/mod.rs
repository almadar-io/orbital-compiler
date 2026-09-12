//! Synced Definitions from TypeScript
//!
//! This module contains pattern and service definitions synced from the TypeScript
//! source of truth at `builder/packages/shared/src/orbitals/`.
//!
//! To regenerate, run: `npm run sync:rust-validation` from builder/packages/shared
//!
//! DO NOT EDIT MANUALLY - This file is auto-generated.

pub mod action_affinity;
// Hand-written (not pattern-sync generated, unlike its siblings above/below):
// content-grade classification over the same registry data `patterns.rs`
// exposes. See its own header for why it lives here rather than in
// `patterns.rs` itself.
pub mod content;
pub mod icons;
pub mod patterns;
pub mod services;

pub use action_affinity::{
    get_invalid_actions_for_component, get_valid_actions_for_component,
    is_action_invalid_for_component, is_form_action, is_form_pattern, is_interactive_pattern,
    is_list_action, is_list_pattern, ACTION_AFFINITY, FORM_ACTIONS, FORM_PATTERNS,
    INTERACTIVE_PATTERNS, LIST_ACTIONS, LIST_PATTERNS,
};
pub use content::{is_content_body_pattern, is_content_body_pattern_type, is_main_slot_render_ui};
pub use icons::{
    find_similar_icons, is_common_icon, kebab_to_pascal, normalize_icon_name, COMMON_LUCIDE_ICONS,
};
pub use patterns::{
    get_pattern_category, get_pattern_props, get_pattern_valid_props, is_valid_pattern,
    PATTERN_CATEGORIES, PATTERN_TYPES,
};
pub use services::{is_valid_service_type, RESERVED_SOCKET_EVENTS};

use std::collections::{HashMap, HashSet};

/// Extra patterns loaded from a project-local patterns.json file.
///
/// Merged with core patterns during validation so the compiler accepts
/// project-specific design system components in render-ui effects.
#[derive(Debug, Clone, Default)]
pub struct ExtraPatterns {
    /// Valid pattern type names
    pub types: HashSet<String>,
    /// Pattern type → set of valid prop names
    pub props_schema: HashMap<String, HashSet<String>>,
}

impl ExtraPatterns {
    /// Check if a pattern type exists in this extra set
    pub fn contains_pattern(&self, pattern_type: &str) -> bool {
        self.types.contains(pattern_type)
    }

    /// Get valid props for a pattern from this extra set
    pub fn get_valid_props(&self, pattern_type: &str) -> Option<&HashSet<String>> {
        self.props_schema.get(pattern_type)
    }
}
