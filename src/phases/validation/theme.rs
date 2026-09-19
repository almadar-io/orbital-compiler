//! Theme Validation
//!
//! Validates theme definitions at the orbital level.
//! Themes can be inline definitions or references to imported themes.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use crate::embedded::builtin_theme_names;
use orbital_core::schema::{OrbitalSchema, SExpression, ThemeDefinition, ThemeRef, TraitReference};
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
    if let Some(theme_ref) = &schema.theme {
        let result = validate_theme_ref(theme_ref, "theme", &inline_names);
        ctx.merge(result);
    }
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(theme_ref) = &orbital.theme {
            let path = format!("orbitals[{}].theme", i);
            let result = validate_theme_ref(theme_ref, &path, &inline_names);
            ctx.merge(result);
        }
    }
    ctx.merge(validate_theme_pin_matches_app(schema));
}

/// The schema's app-level `theme "<key>"` declaration, if any — a bare
/// reference string, or an inline theme's own `name`. `None` (empty string
/// included) means the app declares no app-level theme, in which case there
/// is nothing to contradict.
fn app_theme_key(schema: &OrbitalSchema) -> Option<&str> {
    match schema.theme.as_ref()? {
        ThemeRef::Reference(s) if !s.is_empty() => Some(s.as_str()),
        ThemeRef::Inline(def) if !def.name.is_empty() => Some(def.name.as_str()),
        _ => None,
    }
}

/// A trait config's `theme` default, when it is a LITERAL, NON-EMPTY pin
/// that is not a binding. `""` (an empty knob default) and any `@…` binding
/// are unpinned — an orbital riding the ambient default via `@currentTheme`
/// must stay silent, which is what stops the JS `app-theme-divergent`'s
/// 47-hit false positive (an empty string counted as a pin).
fn theme_pin_of(default: Option<&SExpression>) -> Option<&str> {
    match default? {
        SExpression::String(s) if !s.is_empty() && !s.starts_with('@') => Some(s.as_str()),
        _ => None,
    }
}

/// A trait pins `config.theme` to a literal that contradicts the schema's
/// app-level `theme` declaration.
///
/// App-level branch only (JS `app-theme-divergent`, L1116-1133): with an
/// app-level theme declared, an unpinned orbital inherits it through
/// `@currentTheme`, so only a pin that CONTRADICTS the app value is drift.
/// The JS dominant-vote branch (no app-level theme: flag whichever orbital
/// doesn't match the plurality pin) is a heuristic over which theme "wins" —
/// not ported; retired per owner ruling. JS twin: the app-level half of
/// `app-theme-divergent` in `wiring-lint.ts`.
fn validate_theme_pin_matches_app(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(app_theme) = app_theme_key(schema) else {
        return result;
    };
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let mut pinned: Vec<&str> = Vec::new();
        for trait_ref in &orbital.traits {
            let config = match trait_ref {
                TraitReference::Inline(td) => td.config.as_ref(),
                TraitReference::Reference { config, .. } => config.as_ref(),
            };
            let Some(pin) = config
                .and_then(|c| c.get("theme"))
                .and_then(|f| theme_pin_of(f.default.as_ref()))
            else {
                continue;
            };
            if !pinned.contains(&pin) {
                pinned.push(pin);
            }
        }
        let mut divergent: Vec<&str> = pinned.into_iter().filter(|t| *t != app_theme).collect();
        if divergent.is_empty() {
            continue;
        }
        divergent.sort_unstable();
        let quoted = divergent
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ");
        result.add_error(
            ValidationError::error(
                ErrorCode::ThemePinContradictsApp,
                format!("orbitals[{} ({})].theme", oi, orbital.name),
                format!(
                    "{} pins theme {} while the app declares \"{}\" at the app level",
                    orbital.name, quoted, app_theme
                ),
            )
            .with_suggestion(
                "remove the config pin so the orbital inherits the app theme via @currentTheme \
                 — or, for a deliberate per-orbital override, declare it as the orbital's own \
                 `theme \"<key>\"`",
            ),
        );
    }
    result
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
                        "Unknown theme '{}'. Must be a built-in theme key (the full data-theme value, e.g. linear-clean-light, terminal-dark, game-adventure-dark) or an orbital-declared inline theme.",
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
                result.add_error(ValidationError::error(
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
                result.add_error(ValidationError::error(
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
                result.add_error(ValidationError::error(
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
                        result.add_error(ValidationError::error(
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
                                                                    // Built-in registry keys accepted (full <base>-<variant> data-theme values).
        assert!(is_valid_theme_reference("linear-clean-light", &empty));
        assert!(is_valid_theme_reference("terminal-dark", &empty));
        // Bare base names are NOT valid (the CSS defines only suffixed keys).
        assert!(!is_valid_theme_reference("linear-clean", &empty));
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
                color: None,
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
                illustration: None,
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
                color: None,
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
                illustration: None,
            },
            variants: None,
        };

        let result = validate_theme_definition(&theme, "test");
        assert!(!result.is_valid());
    }
}

#[cfg(test)]
mod theme_pin_contradicts_app_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    fn findings(json: &str) -> Vec<ValidationError> {
        validate_theme_pin_matches_app(&schema_from_json(json)).errors
    }

    /// An unpinned orbital (empty config on its layout ref-trait) rides the
    /// app theme via `@currentTheme` — clean. A pin that matches the app
    /// theme is clean too.
    const UNPINNED_ORBITALS_ARE_CLEAN: &str = r#"{
      "name": "fixture", "version": "1.0.0", "theme": "linear-clean-light",
      "orbitals": [
        {
          "name": "TaskOrbital",
          "entity": { "name": "W", "fields": [] },
          "traits": [{ "ref": "std-app-layout#AppLayout", "name": "TaskLayout",
            "config": { "theme": { "type": "string", "default": "linear-clean-light" } } }]
        },
        {
          "name": "RosterOrbital",
          "entity": { "name": "V", "fields": [] },
          "traits": [{ "ref": "std-app-layout#AppLayout", "name": "RosterLayout" }]
        }
      ]
    }"#;

    const CONTRADICTING_PIN: &str = r#"{
      "name": "fixture", "version": "1.0.0", "theme": "linear-clean-light",
      "orbitals": [
        {
          "name": "TaskOrbital",
          "entity": { "name": "W", "fields": [] },
          "traits": [{ "ref": "std-app-layout#AppLayout", "name": "TaskLayout" }]
        },
        {
          "name": "OddOrbital",
          "entity": { "name": "V", "fields": [] },
          "traits": [{ "ref": "std-app-layout#AppLayout", "name": "OddLayout",
            "config": { "theme": { "type": "string", "default": "terminal-dark" } } }]
        }
      ]
    }"#;

    /// `""` is an unset knob default, not a pin — the JS false positive this
    /// port fixes (47 corpus hits were exactly this shape).
    const EMPTY_STRING_PIN_IS_UNPINNED: &str = r#"{
      "name": "fixture", "version": "1.0.0", "theme": "linear-clean-light",
      "orbitals": [{
        "name": "OddOrbital",
        "entity": { "name": "V", "fields": [] },
        "traits": [{ "ref": "std-app-layout#AppLayout", "name": "OddLayout",
          "config": { "theme": { "type": "string", "default": "" } } }]
      }]
    }"#;

    /// A `@currentTheme`-shaped binding is a live read, not a literal pin.
    const BINDING_PIN_IS_UNPINNED: &str = r#"{
      "name": "fixture", "version": "1.0.0", "theme": "linear-clean-light",
      "orbitals": [{
        "name": "OddOrbital",
        "entity": { "name": "V", "fields": [] },
        "traits": [{ "ref": "std-app-layout#AppLayout", "name": "OddLayout",
          "config": { "theme": { "type": "string", "default": "@currentTheme" } } }]
      }]
    }"#;

    #[test]
    fn unpinned_and_matching_orbitals_are_clean_when_app_declares_a_theme() {
        assert!(findings(UNPINNED_ORBITALS_ARE_CLEAN).is_empty());
    }

    #[test]
    fn flags_a_config_pin_contradicting_the_app_theme() {
        let errors = findings(CONTRADICTING_PIN);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::ThemePinContradictsApp);
        assert!(errors[0].message.contains("OddOrbital"));
        assert!(errors[0].message.contains("terminal-dark"));
        assert!(errors[0].message.contains("linear-clean-light"));
    }

    #[test]
    fn an_empty_string_pin_is_unpinned_not_divergent() {
        assert!(findings(EMPTY_STRING_PIN_IS_UNPINNED).is_empty());
    }

    #[test]
    fn a_binding_pin_is_unpinned_not_divergent() {
        assert!(findings(BINDING_PIN_IS_UNPINNED).is_empty());
    }

    #[test]
    fn no_app_level_theme_declared_is_silent() {
        let schema = schema_from_json(r#"{
          "name": "fixture", "version": "1.0.0",
          "orbitals": [{
            "name": "OddOrbital",
            "entity": { "name": "V", "fields": [] },
            "traits": [{ "ref": "std-app-layout#AppLayout", "name": "OddLayout",
              "config": { "theme": { "type": "string", "default": "terminal-dark" } } }]
          }]
        }"#);
        assert!(validate_theme_pin_matches_app(&schema).errors.is_empty());
    }
}
