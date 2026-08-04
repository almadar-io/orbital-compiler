//! Theme Validation
//!
//! Validates theme definitions at the orbital level.
//! Themes can be inline definitions or references to imported themes.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use crate::embedded::builtin_theme_names;
use orbital_core::schema::{OrbitalSchema, ThemeDefinition, ThemeRef};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static THEME_REF_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*\.theme$").unwrap());

/// Validate all themes in a schema
pub fn validate_all_themes(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // The valid-theme set for references = built-in `@almadar-ui/themes/`
    // registry (baked) ∪ this program's own orbital-declared inline themes.
    let inline_names: HashSet<&str> = schema
        .orbitals
        .iter()
        .filter_map(|o| o.theme.as_ref())
        .filter_map(|t| t.as_inline())
        .map(|t| t.name.as_str())
        .collect();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(theme_ref) = &orbital.theme {
            let path = format!("orbitals[{}].theme", i);
            let result = validate_theme_ref(theme_ref, &path, &inline_names);
            ctx.merge(result);
        }
    }
}

/// Validate a theme reference (inline or reference string).
///
/// A `Reference` is valid when it names a theme in the built-in
/// `@almadar-ui/themes/` registry OR one of the program's own
/// orbital-declared inline themes. The legacy `Alias.theme` import form is
/// also accepted (resolved by the inline phase). Anything else is
/// `ORB_X_UNKNOWN_THEME`.
pub fn validate_theme_ref(
    theme_ref: &ThemeRef,
    path: &str,
    inline_names: &HashSet<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match theme_ref {
        ThemeRef::Inline(theme_def) => {
            let theme_result = validate_theme_definition(theme_def, path);
            result.merge(theme_result);
        }
        ThemeRef::Reference(ref_str) => {
            if !is_valid_theme_reference(ref_str, inline_names) {
                result.add_error(ValidationError::error(
                    ErrorCode::UnknownTheme,
                    path,
                    format!(
                        "Unknown theme '{}'. Must be a built-in theme name (e.g. linear-clean, terminal) or an orbital-declared inline theme.",
                        ref_str
                    ),
                ));
            }
        }
    }

    result
}

/// Validate an inline theme definition
pub fn validate_theme_definition(theme: &ThemeDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Theme name is required
    if theme.name.is_empty() {
        result.add_error(ValidationError::error(
            ErrorCode::EntityMissingName,
            format!("{}.name", path),
            "Theme name is required",
        ));
    }

    // Validate color tokens are valid hex colors
    if let Some(colors) = &theme.tokens.colors {
        for (key, value) in colors {
            if !is_valid_css_color(value) {
                result.add_error(ValidationError::warning(
                    ErrorCode::InvalidValue,
                    format!("{}.tokens.colors.{}", path, key),
                    format!("Color value '{}' may not be a valid CSS color", value),
                ));
            }
        }
    }

    // Validate radii tokens are valid CSS values
    if let Some(radii) = &theme.tokens.radii {
        for (key, value) in radii {
            if !is_valid_css_length(value) {
                result.add_error(ValidationError::warning(
                    ErrorCode::InvalidValue,
                    format!("{}.tokens.radii.{}", path, key),
                    format!("Radius value '{}' may not be a valid CSS length", value),
                ));
            }
        }
    }

    // Validate spacing tokens
    if let Some(spacing) = &theme.tokens.spacing {
        for (key, value) in spacing {
            if !is_valid_css_length(value) {
                result.add_error(ValidationError::warning(
                    ErrorCode::InvalidValue,
                    format!("{}.tokens.spacing.{}", path, key),
                    format!("Spacing value '{}' may not be a valid CSS length", value),
                ));
            }
        }
    }

    // Validate variants
    if let Some(variants) = &theme.variants {
        for (variant_name, variant) in variants {
            let variant_path = format!("{}.variants.{}", path, variant_name);

            // Validate variant colors
            if let Some(colors) = &variant.colors {
                for (key, value) in colors {
                    if !is_valid_css_color(value) {
                        result.add_error(ValidationError::warning(
                            ErrorCode::InvalidValue,
                            format!("{}.colors.{}", variant_path, key),
                            format!("Color value '{}' may not be a valid CSS color", value),
                        ));
                    }
                }
            }
        }
    }

    result
}

/// Check if a theme reference is valid: a built-in registry name, an
/// orbital-declared inline theme name, or the legacy `Alias.theme` import
/// form (resolved by the inline phase).
fn is_valid_theme_reference(ref_str: &str, inline_names: &HashSet<&str>) -> bool {
    if THEME_REF_REGEX.is_match(ref_str) {
        return true;
    }
    if inline_names.contains(ref_str) {
        return true;
    }
    builtin_theme_names().iter().any(|n| n == ref_str)
}

/// Check if a value is a valid CSS color (hex, rgb, hsl, named)
fn is_valid_css_color(value: &str) -> bool {
    let value = value.trim();

    // Hex colors: #RGB, #RRGGBB, #RRGGBBAA
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }

    // RGB/RGBA
    if value.starts_with("rgb") {
        return true; // Basic check - could be more sophisticated
    }

    // HSL/HSLA
    if value.starts_with("hsl") {
        return true;
    }

    // CSS variables
    if value.starts_with("var(") {
        return true;
    }

    // Named colors - just accept any alphabetic string
    value.chars().all(|c| c.is_alphabetic() || c == '-')
}

/// Check if a value is a valid CSS length (px, rem, em, %, etc.)
fn is_valid_css_length(value: &str) -> bool {
    let value = value.trim();

    // Zero is always valid
    if value == "0" {
        return true;
    }

    // CSS variables
    if value.starts_with("var(") {
        return true;
    }

    // Common units: px, rem, em, %, vh, vw, pt, ch
    let units = ["px", "rem", "em", "%", "vh", "vw", "pt", "ch", "ex"];

    for unit in units {
        if let Some(num_part) = value.strip_suffix(unit) {
            if num_part.parse::<f64>().is_ok() {
                return true;
            }
        }
    }

    // Could also be a number (unitless for line-height, etc.)
    value.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::ThemeTokens;

    #[test]
    fn test_valid_theme_reference() {
        let empty: HashSet<&str> = HashSet::new();
        // Legacy Alias.theme import form still accepted.
        assert!(is_valid_theme_reference("Ocean.theme", &empty));
        assert!(is_valid_theme_reference("MyTheme.theme", &empty));
        assert!(!is_valid_theme_reference("ocean.theme", &empty)); // must start with uppercase
        assert!(!is_valid_theme_reference("Ocean.themes", &empty)); // must be .theme
        // Built-in registry names accepted.
        assert!(is_valid_theme_reference("linear-clean", &empty));
        assert!(is_valid_theme_reference("terminal", &empty));
        // Orbital-declared inline theme names accepted.
        let mut inline = HashSet::new();
        inline.insert("my-custom");
        assert!(is_valid_theme_reference("my-custom", &inline));
        // Unknown rejected.
        assert!(!is_valid_theme_reference("does-not-exist", &empty));
    }

    #[test]
    fn test_valid_css_color() {
        assert!(is_valid_css_color("#fff"));
        assert!(is_valid_css_color("#ffffff"));
        assert!(is_valid_css_color("#0ea5e9"));
        assert!(is_valid_css_color("rgb(255, 0, 0)"));
        assert!(is_valid_css_color("hsl(0, 100%, 50%)"));
        assert!(is_valid_css_color("var(--primary)"));
        assert!(is_valid_css_color("red"));
        assert!(is_valid_css_color("transparent"));
    }

    #[test]
    fn test_valid_css_length() {
        assert!(is_valid_css_length("0"));
        assert!(is_valid_css_length("4px"));
        assert!(is_valid_css_length("1rem"));
        assert!(is_valid_css_length("50%"));
        assert!(is_valid_css_length("var(--spacing-md)"));
    }

    #[test]
    fn test_validate_theme_definition() {
        let theme = ThemeDefinition {
            name: "ocean".to_string(),
            tokens: ThemeTokens {
                colors: Some(
                    [("primary".to_string(), "#0ea5e9".to_string())]
                        .into_iter()
                        .collect(),
                ),
                radii: Some(
                    [("md".to_string(), "8px".to_string())]
                        .into_iter()
                        .collect(),
                ),
                spacing: None,
                typography: None,
                shadows: None,
                density: None,
                type_scale: None,
                motion: None,
                iconography: None,
                elevation: None,
                geometry: None,
            },
            variants: None,
        };

        let result = validate_theme_definition(&theme, "test");
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_theme_missing_name() {
        let theme = ThemeDefinition {
            name: "".to_string(),
            tokens: ThemeTokens {
                colors: None,
                radii: None,
                spacing: None,
                typography: None,
                shadows: None,
                density: None,
                type_scale: None,
                motion: None,
                iconography: None,
                elevation: None,
                geometry: None,
            },
            variants: None,
        };

        let result = validate_theme_definition(&theme, "test");
        assert!(!result.is_valid());
    }
}
