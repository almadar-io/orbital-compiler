//! Theme Validation
//!
//! Validates theme definitions at the orbital level.
//! Themes can be inline definitions or references to imported themes.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use crate::embedded::builtin_theme_names;
use orbital_core_base::schema::{
    OrbitalSchema, SExpression, ThemeDefinition, ThemeRef, TraitDefinition, TypeScaleTokens,
};
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
    for (i, key) in schema.themes.iter().flatten().enumerate() {
        let path = format!("themes[{}]", i);
        ctx.merge(validate_theme_ref(
            &ThemeRef::Reference(key.clone()),
            &path,
            &inline_names,
        ));
    }
    ctx.merge(validate_data_themes(schema));
}

/// A theme key's base: `"clay-dark"` and `"clay-light"` are both `"clay"`, whose stylesheet carries both modes.
fn theme_base(key: &str) -> &str {
    key.strip_suffix("-dark")
        .or_else(|| key.strip_suffix("-light"))
        .unwrap_or(key)
}

/// Every theme base the program declares: its app and orbital `theme` (a key, or an inline theme's name) and its
/// `themes` list.
fn declared_theme_bases(schema: &OrbitalSchema) -> HashSet<String> {
    let refs = schema
        .theme
        .iter()
        .chain(schema.orbitals.iter().filter_map(|o| o.theme.as_ref()));
    refs.filter_map(|t| match t {
        ThemeRef::Reference(key) if !key.is_empty() => Some(theme_base(key).to_string()),
        ThemeRef::Inline(def) if !def.name.is_empty() => Some(def.name.clone()),
        _ => None,
    })
    .chain(
        schema
            .themes
            .iter()
            .flatten()
            .map(|k| theme_base(k).to_string()),
    )
    .collect()
}

/// The literal theme a `data-theme` value names: the string itself, or the literal default of the `@config` knob it
/// reads. `None` for an unset knob, any other binding, or an expression.
fn literal_data_theme<'a>(
    value: &'a SExpression,
    trait_def: &'a TraitDefinition,
) -> Option<&'a str> {
    let SExpression::String(s) = value else {
        return None;
    };
    let literal = match s.strip_prefix("@config.") {
        Some(knob) => match trait_def.config.as_ref()?.get(knob)?.default.as_ref()? {
            SExpression::String(v) => v.as_str(),
            _ => return None,
        },
        None => s.as_str(),
    };
    (!literal.is_empty() && !literal.starts_with('@')).then_some(literal)
}

fn collect_data_themes<'a>(
    expr: &'a SExpression,
    trait_def: &'a TraitDefinition,
    found: &mut Vec<&'a str>,
) {
    match expr {
        SExpression::Object(map) => {
            for (key, value) in map.iter() {
                if key == "data-theme" {
                    if let Some(theme) = literal_data_theme(value, trait_def) {
                        found.push(theme);
                    }
                }
                collect_data_themes(value, trait_def, found);
            }
        }
        SExpression::Array(items) => items
            .iter()
            .for_each(|item| collect_data_themes(item, trait_def, found)),
        _ => {}
    }
}

/// `ORB_THEME_UNDECLARED` (G-UI-106): a `data-theme` resolving to a literal theme the program does not declare. A
/// compiled client ships only the declared themes, so that frame would be unstyled.
pub fn validate_data_themes(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let declared = declared_theme_bases(schema);
    for (trait_def, path) in super::inline_traits(schema) {
        let mut found = Vec::new();
        for transition in &trait_def.state_machine.transitions {
            for effect in transition.effects.iter().flatten() {
                collect_data_themes(effect, trait_def, &mut found);
            }
        }
        for tick in trait_def.ticks.iter().flatten() {
            for effect in &tick.effects {
                collect_data_themes(effect, trait_def, &mut found);
            }
        }
        for field in trait_def.config.iter().flat_map(|c| c.values()) {
            if let Some(default) = &field.default {
                collect_data_themes(default, trait_def, &mut found);
            }
        }
        found.sort_unstable();
        found.dedup();
        for theme in found
            .into_iter()
            .filter(|t| !declared.contains(theme_base(t)))
        {
            result.add_error(
                ValidationError::error(
                    ErrorCode::ThemeUndeclared,
                    path.clone(),
                    format!("'{}' shows a frame in theme \"{}\", which the program does not declare", trait_def.name, theme),
                )
                .with_suggestion(format!(
                    "add \"{}\" to the app's `themes: [...]` (the themes a program switches to inside a page)",
                    theme
                )),
            );
        }
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

    check_font_stacks(
        theme.tokens.type_scale.as_ref(),
        theme.tokens.typography.as_ref(),
        &format!("{}.tokens", path),
        &mut result,
    );

    // Validate variants
    if let Some(variants) = &theme.variants {
        for (variant_name, variant) in variants {
            let variant_path = format!("{}.variants.{}", path, variant_name);
            check_font_stacks(
                variant.type_scale.as_ref(),
                variant.typography.as_ref(),
                &variant_path,
                &mut result,
            );

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

/// Every font stack a theme declares (`typeScale` families, legacy
/// `typography.font-family*`) must be well formed — a malformed stack voids
/// the whole CSS declaration and the font silently falls back. Twin of
/// `@almadar/core`'s `isWellFormedFontStack`.
fn check_font_stacks(
    type_scale: Option<&TypeScaleTokens>,
    typography: Option<&orbital_core_base::schema::ThemeMap>,
    path: &str,
    result: &mut ValidationResult,
) {
    let mut stacks: Vec<(String, &str)> = Vec::new();
    if let Some(ts) = type_scale {
        for (key, value) in [
            ("displayFamily", &ts.display_family),
            ("bodyFamily", &ts.body_family),
            ("monoFamily", &ts.mono_family),
        ] {
            if let Some(v) = value {
                stacks.push((format!("{}.typeScale.{}", path, key), v.as_str()));
            }
        }
    }
    if let Some(map) = typography {
        for (key, value) in map {
            if key.starts_with("font-family") {
                stacks.push((format!("{}.typography.{}", path, key), value.as_str()));
            }
        }
    }
    for (at, stack) in stacks {
        if !is_well_formed_font_stack(stack) {
            result.add_error(ValidationError::error(
                ErrorCode::InvalidValue,
                at,
                format!(
                    "Malformed CSS font-family stack '{}' — separate every family with a comma and fully quote multi-word names",
                    stack
                ),
            ));
        }
    }
}

/// See `check_font_stacks`. Items are comma-separated at paren depth 0; each is
/// a fully quoted string, a bare identifier sequence, or a `var(...)`.
fn is_well_formed_font_stack(stack: &str) -> bool {
    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth: i32 = 0;
    for ch in stack.chars() {
        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            items.push(current.trim().to_string());
            current.clear();
            continue;
        }
        current.push(ch);
    }
    if quote.is_some() || depth != 0 {
        return false;
    }
    items.push(current.trim().to_string());
    items.iter().all(|item| {
        FONT_QUOTED_DOUBLE.is_match(item)
            || FONT_QUOTED_SINGLE.is_match(item)
            || FONT_BARE.is_match(item)
            || FONT_VAR.is_match(item)
    })
}

static FONT_QUOTED_DOUBLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^"[^"]+"$"#).unwrap());
static FONT_QUOTED_SINGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^'[^']+'$").unwrap());
static FONT_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-?[A-Za-z][\w-]*( [A-Za-z0-9][\w-]*)*$").unwrap());
static FONT_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^var\(--[\w-]+(,.*)?\)$").unwrap());

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
    use orbital_core_base::schema::ThemeTokens;

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
            display_name: None,
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
                diagram: None,
                scene: None,
                surface: None,
            },
            variants: None,
        };

        let result = validate_theme_definition(&theme, "test");
        assert!(result.is_valid());
    }

    #[test]
    fn font_stack_well_formedness() {
        for ok in [
            "\"Inter\", -apple-system, sans-serif",
            "'Kenney Pixel', ui-sans-serif",
            "Times New Roman, serif",
            "var(--font-family-display, var(--font-family))",
            "monospace",
        ] {
            assert!(is_well_formed_font_stack(ok), "should accept {ok}");
        }
        for bad in [
            "\"Bangers\"\"Comic Sans MS\", cursive",
            "\"MedievalSharp\"Georgia, serif",
            "\"Inter\", , serif",
            "\"Inter, serif",
            "",
            "\"Inter\",",
            "var(--font-family",
        ] {
            assert!(!is_well_formed_font_stack(bad), "should reject {bad}");
        }
    }

    #[test]
    fn malformed_font_stack_in_a_theme_is_an_error_and_a_clean_one_is_not() {
        let theme: ThemeDefinition = serde_json::from_value(serde_json::json!({
            "name": "t",
            "tokens": {
                "typeScale": { "displayFamily": "\"Bangers\"\"Comic Sans MS\"" },
                "typography": { "font-family": "\"Inter\", sans-serif", "letter-spacing": "\"x\"\"y\"" }
            },
            "variants": { "dark": { "typography": { "font-family": "\"A\"B" } } }
        }))
        .unwrap();
        let paths: Vec<String> = validate_theme_definition(&theme, "theme")
            .errors
            .iter()
            .map(|e| e.path.clone())
            .collect();
        assert_eq!(
            paths,
            vec![
                "theme.tokens.typeScale.displayFamily".to_string(),
                "theme.variants.dark.typography.font-family".to_string(),
            ]
        );
    }

    #[test]
    fn test_validate_theme_missing_name() {
        let theme = ThemeDefinition {
            name: "".to_string(),
            display_name: None,
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
                diagram: None,
                scene: None,
                surface: None,
            },
            variants: None,
        };

        let result = validate_theme_definition(&theme, "test");
        assert!(!result.is_valid());
    }
}

/// A `data-theme` that resolves to a literal theme must name one the program declares
/// (its app or orbital `theme`, or its `themes` list), since a compiled client ships only those (G-UI-106).
#[cfg(test)]
mod data_theme_declared_tests {
    use super::*;

    fn site(app_extra: &str, data_theme: &str, demo_default: &str) -> OrbitalSchema {
        serde_json::from_str(&format!(
            r#"{{
          "name": "site", "version": "1.0.0", "theme": "almadar-light"{app_extra},
          "orbitals": [{{
            "name": "Home",
            "entity": {{ "name": "V", "persistence": "runtime", "fields": [{{ "name": "id", "type": "string" }}] }},
            "traits": [{{
              "name": "Tour", "linkedEntity": "V",
              "config": {{ "demoTheme": {{ "type": "string", "default": "{demo_default}" }} }},
              "stateMachine": {{
                "states": [{{ "name": "idle", "isInitial": true }}],
                "events": [{{ "key": "INIT", "name": "INIT" }}],
                "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                  "effects": [["render-ui", "main", {{ "type": "box", "children": [{{ "type": "box", "data-theme": "{data_theme}", "children": [] }}] }}]] }}]
              }}
            }}]
          }}]
        }}"#
        ))
        .expect("fixture")
    }

    fn codes(schema: &OrbitalSchema) -> Vec<ErrorCode> {
        validate_data_themes(schema)
            .errors
            .into_iter()
            .map(|e| e.code)
            .collect()
    }

    #[test]
    fn a_config_bound_theme_the_program_never_declares_is_an_error() {
        let errors = validate_data_themes(&site("", "@config.demoTheme", "clay-dark")).errors;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::ThemeUndeclared);
        assert!(
            errors[0].message.contains("clay-dark"),
            "{}",
            errors[0].message
        );
    }

    #[test]
    fn control_the_same_theme_declared_in_themes_is_clean() {
        assert!(codes(&site(
            r#", "themes": ["clay-dark"]"#,
            "@config.demoTheme",
            "clay-dark"
        ))
        .is_empty());
    }

    #[test]
    fn control_the_app_theme_s_other_mode_is_declared_with_it() {
        assert!(codes(&site("", "@config.demoTheme", "almadar-dark")).is_empty());
    }

    #[test]
    fn edge_a_literal_data_theme_is_checked_too() {
        assert_eq!(
            codes(&site("", "terminal-dark", "")),
            vec![ErrorCode::ThemeUndeclared]
        );
    }

    #[test]
    fn edge_an_unset_knob_and_a_binding_are_not_theme_names() {
        assert!(codes(&site("", "@config.demoTheme", "")).is_empty());
        assert!(codes(&site("", "@currentTheme", "")).is_empty());
    }

    #[test]
    fn plain_validation_reports_an_undeclared_theme_once() {
        let mut ctx = ValidationContext::default();
        validate_all_themes(&site("", "@config.demoTheme", "clay-dark"), &mut ctx);
        let errors = ctx.into_result().errors;
        assert_eq!(
            errors
                .iter()
                .filter(|e| e.code == ErrorCode::ThemeUndeclared)
                .count(),
            1,
            "{errors:?}"
        );
        let finals = super::super::final_program::validate_final_program(&site(
            "",
            "@config.demoTheme",
            "clay-dark",
        ))
        .errors;
        assert!(
            finals.iter().all(|e| e.code != ErrorCode::ThemeUndeclared),
            "the final pass must not report it again"
        );
    }

    #[test]
    fn edge_a_themes_entry_must_name_a_real_theme() {
        let mut ctx = ValidationContext::default();
        validate_all_themes(
            &site(r#", "themes": ["zz-unknown-dark"]"#, "", ""),
            &mut ctx,
        );
        let errors = ctx.into_result().errors;
        assert!(
            errors.iter().any(|e| e.code == ErrorCode::UnknownTheme),
            "{errors:?}"
        );
    }
}
