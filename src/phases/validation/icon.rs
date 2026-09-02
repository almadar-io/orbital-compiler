//! Icon Validation
//!
//! Validates icon names used in patterns (page-header actions, buttons, etc.)
//! against Lucide icon naming conventions.
//!
//! Ported from `builder/packages/shared/src/orbitals/validation/icon-validator.ts`.

use super::sync::{find_similar_icons, is_common_icon, COMMON_LUCIDE_ICONS};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition};
use regex::Regex;
use std::sync::LazyLock;

/// Valid kebab-case icon name pattern (e.g., 'plus', 'trash-2', 'chevron-right')
static KEBAB_CASE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap());

/// Valid PascalCase component name pattern (e.g., 'Plus', 'Trash2', 'ChevronRight')
static PASCAL_CASE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Validate all icons in the schema
pub fn validate_all_icons(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate icons in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_icons(trait_def, &path);
        ctx.merge(result);
    }
}

/// Validate all icons in a trait
fn validate_trait_icons(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check render_ui effects for icon props
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                result.merge(validate_effect_icons(effect, &effect_path));
            }
        }
    }

    result
}

/// Validate icons in an effect
fn validate_effect_icons(effect: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::Array(arr) = effect {
        if arr.is_empty() {
            return result;
        }

        // Check if this is a render-ui effect
        let is_render_ui = matches!(&arr[0], SExpression::String(s) if s == "render-ui");

        if is_render_ui && arr.len() > 2 {
            // Third element is the pattern object
            if let SExpression::Object(pattern) = &arr[2] {
                result.merge(validate_pattern_icons(
                    pattern,
                    &format!("{}.pattern", path),
                ));
            }
        }
    }

    result
}

/// Validate icons in a pattern object
fn validate_pattern_icons(
    pattern: &std::collections::HashMap<String, SExpression>,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check direct icon prop
    if let Some(icon) = pattern.get("icon") {
        result.merge(validate_icon_value(icon, &format!("{}.icon", path)));
    }

    // Check actions array
    if let Some(SExpression::Array(actions)) = pattern.get("actions") {
        for (i, action) in actions.iter().enumerate() {
            if let SExpression::Object(action_obj) = action {
                if let Some(icon) = action_obj.get("icon") {
                    result.merge(validate_icon_value(
                        icon,
                        &format!("{}.actions[{}].icon", path, i),
                    ));
                }
            }
        }
    }

    // Check itemActions array
    if let Some(SExpression::Array(item_actions)) = pattern.get("itemActions") {
        for (i, action) in item_actions.iter().enumerate() {
            if let SExpression::Object(action_obj) = action {
                if let Some(icon) = action_obj.get("icon") {
                    result.merge(validate_icon_value(
                        icon,
                        &format!("{}.itemActions[{}].icon", path, i),
                    ));
                }
            }
        }
    }

    // Check nested pattern objects recursively
    for (key, value) in pattern {
        if let SExpression::Object(nested) = value {
            result.merge(validate_pattern_icons(nested, &format!("{}.{}", path, key)));
        } else if let SExpression::Array(arr) = value {
            for (i, item) in arr.iter().enumerate() {
                if let SExpression::Object(nested) = item {
                    result.merge(validate_pattern_icons(
                        nested,
                        &format!("{}.{}[{}]", path, key, i),
                    ));
                }
            }
        }
    }

    result
}

/// Validate a single icon value
fn validate_icon_value(icon: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    let icon_name = match icon {
        SExpression::String(s) => s,
        _ => {
            result.add_error(ValidationError::error(
                ErrorCode::IconInvalidFormat,
                path,
                "Icon value must be a string",
            ));
            return result;
        }
    };

    // Binding refs (`@card.icon`, `@payload.icon`, `@config.foo`, …) skip
    // every shape check below — the string is a runtime path, not a literal
    // icon name. The icon's resolved value gets validated when it lands in
    // the rendered pattern at runtime; the L2 binding-type validator (gap
    // #6) is the right home for typing the path itself.
    if icon_name.starts_with('@') {
        return result;
    }

    // Empty string is invalid
    if icon_name.trim().is_empty() {
        result.add_error(ValidationError::error(
            ErrorCode::IconEmpty,
            path,
            "Icon name cannot be empty",
        ));
        return result;
    }

    // Check naming convention
    let is_kebab = KEBAB_CASE_PATTERN.is_match(icon_name);
    let is_pascal = PASCAL_CASE_PATTERN.is_match(icon_name);

    if !is_kebab && !is_pascal {
        let suggestions: Vec<String> = COMMON_LUCIDE_ICONS
            .iter()
            .take(10)
            .map(|s| s.to_string())
            .collect();

        result.add_error(
            ValidationError::error(
                ErrorCode::IconInvalidFormat,
                path,
                format!(
                    "Icon '{}' has invalid format. Use kebab-case (e.g., 'plus', 'trash-2') or PascalCase (e.g., 'Plus', 'Trash2')",
                    icon_name
                ),
            )
            .with_valid_values(suggestions),
        );
        return result;
    }

    // Check if it's a common icon
    if !is_common_icon(icon_name) {
        let similar = find_similar_icons(icon_name, 3);
        let mut error = ValidationError::error(
            ErrorCode::IconNotFound,
            path,
            format!(
                "Icon '{}' may not exist in Lucide. Check https://lucide.dev/icons for available icons",
                icon_name
            ),
        );

        if !similar.is_empty() {
            error = error.with_suggestion(format!("Did you mean: {}?", similar.join(", ")));
            error = error.with_valid_values(similar.iter().map(|s| s.to_string()).collect());
        }

        result.add_error(error);
    }

    result
}

/// Validate an icon name (public API)
pub fn validate_icon_name(icon_name: &str, path: &str) -> ValidationResult {
    validate_icon_value(&SExpression::String(icon_name.to_string()), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_kebab_case_icons() {
        let result = validate_icon_name("plus", "test");
        assert!(result.is_valid());

        let result = validate_icon_name("trash-2", "test");
        assert!(result.is_valid());

        let result = validate_icon_name("chevron-right", "test");
        assert!(result.is_valid());
    }

    #[test]
    fn test_valid_pascal_case_icons() {
        let result = validate_icon_name("Plus", "test");
        assert!(result.is_valid());

        let result = validate_icon_name("ChevronRight", "test");
        // May warn as "uncommon" but format is valid
        assert!(
            result.errors.is_empty()
                || result
                    .errors
                    .iter()
                    .all(|e| e.code != ErrorCode::IconInvalidFormat)
        );
    }

    #[test]
    fn test_invalid_format() {
        let result = validate_icon_name("PLUS_ICON", "test");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::IconInvalidFormat));

        let result = validate_icon_name("plus icon", "test");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::IconInvalidFormat));
    }

    #[test]
    fn test_empty_icon() {
        let result = validate_icon_name("", "test");
        assert!(result.errors.iter().any(|e| e.code == ErrorCode::IconEmpty));

        let result = validate_icon_name("   ", "test");
        assert!(result.errors.iter().any(|e| e.code == ErrorCode::IconEmpty));
    }

    #[test]
    fn test_uncommon_icon_warning() {
        let result = validate_icon_name("nonexistent-icon", "test");
        // Should have a warning about uncommon icon
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::IconNotFound));
    }

    #[test]
    fn test_similar_icon_suggestions() {
        let result = validate_icon_name("trsh", "test");
        // Should suggest "trash" or "trash-2"
        if let Some(warning) = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::IconNotFound)
        {
            assert!(warning.suggestion.is_some() || warning.valid_values.is_some());
        }
    }
}
