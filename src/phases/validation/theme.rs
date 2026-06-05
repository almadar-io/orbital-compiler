//! Theme Validation
//!
//! Validates theme definitions at the orbital level.
//! Themes can be inline definitions or references to imported themes.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use orbital_core::schema::{OrbitalSchema, ThemeDefinition, ThemeRef};

/// Validate all themes in a schema
pub fn validate_all_themes(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(theme_ref) = &orbital.theme {
            let path = format!("orbitals[{}].theme", i);
            let result = validate_theme_ref(theme_ref, &path);
            ctx.merge(result);
        }
    }
}

/// Validate a theme reference (inline or reference string)
pub fn validate_theme_ref(theme_ref: &ThemeRef, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    match theme_ref {
        ThemeRef::Inline(theme_def) => {
            let theme_result = validate_theme_definition(theme_def, path);
            result.merge(theme_result);
        }
        ThemeRef::Reference(ref_str) => {
            // Validate reference format: "Alias.theme"
            if !is_valid_theme_reference(ref_str) {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidReference,
                    path,
                    format!(
                        "Invalid theme reference '{}'. Must be in format 'Alias.theme'",
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
            &format!("{}.name", path),
            "Theme name is required",
        ));
    }

    // Validate color tokens are valid hex colors
    if let Some(colors) = &theme.tokens.colors {
        for (key, value) in colors {
            if !is_valid_css_color(value) {
                result.add_error(ValidationError::warning(
                    ErrorCode::InvalidValue,
                    &format!("{}.tokens.colors.{}", path, key),
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
                    &format!("{}.tokens.radii.{}", path, key),
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
                    &format!("{}.tokens.spacing.{}", path, key),
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
                            &format!("{}.colors.{}", variant_path, key),
                            format!("Color value '{}' may not be a valid CSS color", value),
                        ));
                    }
                }
            }
        }
    }

    result
}

/// Check if a theme reference is valid format: "Alias.theme"
fn is_valid_theme_reference(ref_str: &str) -> bool {
    let pattern = regex::Regex::new(r"^[A-Z][a-zA-Z0-9]*\.theme$").unwrap();
    pattern.is_match(ref_str)
}

/// Check if a value is a valid CSS color (hex, rgb, hsl, named)
fn is_valid_css_color(value: &str) -> bool {
    let value = value.trim();

    // Hex colors: #RGB, #RRGGBB, #RRGGBBAA
    if value.starts_with('#') {
        let hex = &value[1..];
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
        if value.ends_with(unit) {
            let num_part = &value[..value.len() - unit.len()];
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
        assert!(is_valid_theme_reference("Ocean.theme"));
        assert!(is_valid_theme_reference("MyTheme.theme"));
        assert!(!is_valid_theme_reference("ocean.theme")); // must start with uppercase
        assert!(!is_valid_theme_reference("Ocean.themes")); // must be .theme
        assert!(!is_valid_theme_reference("Ocean")); // missing .theme
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
