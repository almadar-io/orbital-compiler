//! Config field type validation for `slot` and `pattern`, plus
//! structural shape validation against the typed declaration carriers
//! (`items` / `properties` / `values`) populated by the lolo lowerer.
//!
//! Checks that config fields typed `slot` have defaults in `VALID_SLOTS`,
//! that config fields typed `pattern` have defaults in `PATTERN_TYPES`,
//! and that every config field's `default` value matches its declared
//! structural shape (array vs scalar, object vs primitive, enum
//! membership, per-element / per-property typing recursively).
//! Also validates config overrides in trait references (TODO: requires
//! inline resolution for the upstream declaration lookup).

use orbital_core::schema::{
    ConfigField, FieldDefinition, FieldType, OrbitalSchema, SExpression, TraitReference,
};

use super::error::{ErrorCode, ValidationError};
use super::sync::patterns::is_valid_pattern;
use super::{ValidationContext, ValidationResult, VALID_SLOTS};

/// Validate all `slot` and `pattern` config fields in the schema.
pub fn validate_all_config_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let path_prefix = format!("orbitals[{}].traits[{}]", oi, ti);
            match trait_ref {
                TraitReference::Inline(trait_def) => {
                    if let Some(config) = &trait_def.config {
                        for (field_name, field) in config.iter() {
                            let field_path = format!("{}.config({})", path_prefix, field_name);
                            result.merge(validate_config_field(field, &field_path));
                        }
                    }
                }
                TraitReference::Reference {
                    config: Some(config),
                    ..
                } => {
                    for (field_name, cf) in config.iter() {
                        let field_path = format!("{}.config({})", path_prefix, field_name);
                        // When the call-site entry is a full annotated declaration
                        // (not a plain wiring), validate its `@tier` and structural
                        // shape — the same checks that run for atom declarations.
                        // Plain wiring entries (`field_type == "unknown"`) are
                        // skipped: the base atom's declaration validates the type;
                        // we only have a value here with no declared shape.
                        if cf.field_type != "unknown" {
                            result.merge(validate_config_field(cf, &field_path));
                        }
                        // Suppress unused-variable warnings; the TODO below remains.
                        // TODO: After inline resolution, cross-check the override
                        // value against the base atom's declared config field type.
                        let _ = (field_name, field_path);
                    }
                }
                _ => {}
            }
        }
    }

    ctx.merge(result);
}

/// Validate a single config field's default value against its declared type.
fn validate_config_field(
    field: &orbital_core::schema::ConfigField,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Reject `[object]` (and the bare `object` variant) as a declared
    // type — both lower to an untyped `Array<Json>` / `Json` that the
    // strict-lint floor catches as `EventPayloadValue` / `Record<string,
    // FieldValue | undefined>[]` in emitted TS. The lolo author should
    // declare a named struct alias (e.g. `type FieldConfig = { name:
    // string, label: string, variant: string }`) and use `[FieldConfig]`
    // so the codegen carries real types end-to-end.
    //
    // The `render-ui` type is the deliberate exception: it carries a
    // recursive render-ui SExpr that's validated via the existing
    // render-ui validator + emitted as a proper RenderTree TS type by
    // the std-ts factory-signature codegen — not an untyped Json bag.
    let ft = field.field_type.as_str();
    if (ft == "[object]" || ft == "object") && ft != "render-ui" {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitConfigObjectArrayForbidden,
                path,
                format!(
                    "Config field uses banned type '{}'. Declare a named struct alias (e.g. `type FieldConfig = {{ name: string, label: string }}`) and use the alias (`[FieldConfig]`) so the catalog + TS codegen carry real types. For inline render-ui trees, use the `render-ui` type instead.",
                    ft
                ),
            )
            .with_suggestion(
                "Author the shape: `type <Name> = { field1: <T1>, field2: <T2> }`, then use `[<Name>]` (for arrays) or `<Name>` (for single objects) on the config field. For render-ui trees use `: render-ui`.",
            ),
        );
    }

    // Structural shape check — runs against the typed carriers
    // (`items` / `properties` / `values`) populated by the lolo lowerer.
    // Skips fields with no `default` and skips opaque types we don't
    // know how to validate (`object` / `unknown` / etc.).
    if let Some(default) = &field.default {
        result.merge(check_config_default_shape(field, default, path));
    }

    // `@tier` annotation — defense-in-depth. The parser already
    // restricts the accepted sigil to one of `label | description |
    // synonyms | tier`, but the tier *value* string is free-form at
    // parse time. The L2 rule pins it to the five allowed buckets so
    // any malformed `.orb` IR that bypasses the parser also fails fast.
    // `policy` (governance: audit/access/approval/erasure/notify) and
    // `infra` (operational: lifecycle/reminder/cascade/cross-reference)
    // are decision kinds shown by default alongside `domain`.
    if let Some(tier) = &field.tier {
        const VALID_TIERS: &[&str] = &["domain", "policy", "infra", "presentation", "internal"];
        if !VALID_TIERS.contains(&tier.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigTierInvalid,
                    path,
                    format!(
                        "Config field `@tier` value '{}' is invalid. Allowed values: {}.",
                        tier,
                        VALID_TIERS.join(", ")
                    ),
                )
                .with_suggestion(format!("Use one of: {}.", VALID_TIERS.join(", "))),
            );
        }
    }

    // `render-ui` typed config fields carry a full render-ui SExpr tree
    // as their default — variants override it with their own tree. The
    // tree is validated by the same render-ui validator that runs on
    // inline `(render-ui main { ... })` effects; it knows to skip
    // `@`-bindings, validate pattern names, recurse children, etc.
    //
    // Note: we do NOT call into render_ui::validate_render_ui_effect
    // here because that function expects the full `(render-ui slot
    // { tree })` envelope. The default is already the tree itself;
    // the existing render-ui binding context isn't available at config
    // declaration time (entity defs, extra patterns). Trust the
    // default's shape here — defense-in-depth lives in the call-site
    // override validation, which runs with full context.
    if field.field_type == "render-ui" {
        if let Some(default) = &field.default {
            if !matches!(default, SExpression::Object(_) | SExpression::String(_)) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        "Config field typed `render-ui` must default to an object literal (a render-ui tree) or a string binding (e.g. `@config.someTree`).",
                    )
                    .with_suggestion(
                        "Author the default as an object literal: `bodyContent : render-ui = { type: \"stack\", children: [...] }`.",
                    ),
                );
            }
        }
        return result;
    }

    let default_str = match &field.default {
        Some(SExpression::String(s)) => s.as_str(),
        _ => return result, // Non-string defaults can't be slot/pattern values
    };

    match field.field_type.as_str() {
        "slot" => {
            if !VALID_SLOTS.contains(&default_str) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidSlotDefault,
                        path,
                        format!(
                            "Config field typed `slot` has invalid default '{}' — must be one of: {}",
                            default_str,
                            VALID_SLOTS.join(", ")
                        ),
                    )
                    .with_valid_values(VALID_SLOTS.iter().map(|s| s.to_string()).collect()),
                );
            }
        }
        "pattern" => {
            if !is_valid_pattern(default_str) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidPatternDefault,
                        path,
                        format!(
                            "Config field typed `pattern` has invalid default '{}' — must be a known pattern type",
                            default_str
                        ),
                    )
                    .with_suggestion("Use a known pattern type or ensure custom pattern is registered"),
                );
            }
        }
        _ => {}
    }

    result
}

/// Walk the declared shape (a `ConfigField` carrying `field_type` +
/// optional `items` / `properties` / `values`) and check the actual
/// `default` value against it.
fn check_config_default_shape(
    field: &ConfigField,
    default: &SExpression,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    // Reject `@config.*` as a config default: it is self-referential (the
    // default resolves against the very config block that declares it) and
    // drives unbounded substitution at inline time. Config defaults must be
    // concrete values; a consumer overrides via the call-site config block.
    if let SExpression::String(s) = default {
        if s.starts_with("@config.") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "config default `{}` is self-referential; config defaults must be concrete values",
                        s
                    ),
                )
                    .with_suggestion(
                        "replace the @config.* binding with a concrete literal, or drop the default and override at the call site".to_string(),
                    ),
            );
            return result;
        }
    }
    // Array slot — `field_type` is `[T]` (string tag) and `items` carries
    // the per-element shape. Verify the value is an array, then recurse
    // each element through the `FieldDefinition` validator.
    if field.field_type.starts_with('[') && field.field_type.ends_with(']') {
        let SExpression::Array(items) = default else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "config default has type '{}' but the value isn't an array",
                        field.field_type
                    ),
                )
                .with_suggestion(format!(
                    "supply an array literal matching {}",
                    field.field_type
                )),
            );
            return result;
        };
        if let Some(item_schema) = &field.items {
            for (i, elem) in items.iter().enumerate() {
                let elem_path = format!("{}[{}]", path, i);
                result.merge(check_field_def_shape(item_schema, elem, &elem_path));
            }
        }
        return result;
    }
    // Closed-string enum (lolo `"a" | "b" | "c"` lowers to
    // `field_type: "string"` + `values: ["a","b","c"]`). Default must be
    // one of the listed strings.
    if let Some(values) = &field.values {
        if let SExpression::String(s) = default {
            if !s.starts_with('@') && !values.iter().any(|v| v == s) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "config default '{}' isn't a legal enum value (allowed: {})",
                            s,
                            values.join(", ")
                        ),
                    )
                    .with_valid_values(values.clone()),
                );
            }
            return result;
        }
    }
    // Primitive leaf type check.
    result.merge(check_primitive_shape(&field.field_type, default, path));
    result
}

/// Walk a `FieldDefinition` (used by `ConfigField.items` /
/// `ConfigField.properties[k]`) against a concrete `SExpression`. Same
/// rules as `check_config_default_shape` but typed against
/// `FieldDefinition` instead of `ConfigField`.
fn check_field_def_shape(
    schema: &FieldDefinition,
    value: &SExpression,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    match schema.field_type {
        FieldType::Array => {
            let SExpression::Array(items) = value else {
                result.add_error(ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    "expected an array".to_string(),
                ));
                return result;
            };
            if let Some(inner) = &schema.items {
                for (i, elem) in items.iter().enumerate() {
                    let elem_path = format!("{}[{}]", path, i);
                    result.merge(check_field_def_shape(inner, elem, &elem_path));
                }
            }
        }
        FieldType::Object => {
            let SExpression::Object(map) = value else {
                result.add_error(ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    "expected an object".to_string(),
                ));
                return result;
            };
            if let Some(props) = &schema.properties {
                for (key, prop_schema) in props {
                    let prop_path = format!("{}.{}", path, key);
                    match map.get(key) {
                        Some(prop_value) => {
                            result.merge(check_field_def_shape(
                                prop_schema,
                                prop_value,
                                &prop_path,
                            ));
                        }
                        None if prop_schema.required.unwrap_or(false) => {
                            result.add_error(ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                &prop_path,
                                format!("required field '{}' missing on object literal", key),
                            ));
                        }
                        None => {}
                    }
                }
            }
        }
        _ => {
            // Primitive leaf — enum first (closed-string union), then
            // structural type.
            if let Some(values) = &schema.values {
                if let SExpression::String(s) = value {
                    if !s.starts_with('@') && !values.iter().any(|v| v == s) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                path,
                                format!(
                                    "'{}' isn't a legal enum value (allowed: {})",
                                    s,
                                    values.join(", ")
                                ),
                            )
                            .with_valid_values(values.clone()),
                        );
                        return result;
                    }
                }
            }
            let type_tag = match schema.field_type {
                FieldType::String => "string",
                FieldType::Number => "number",
                FieldType::Boolean => "boolean",
                FieldType::Date => "date",
                FieldType::Datetime => "datetime",
                FieldType::Timestamp => "timestamp",
                _ => return result, // Other field types are opaque to the leaf check.
            };
            result.merge(check_primitive_shape(type_tag, value, path));
        }
    }
    result
}

/// Type-tag → value-kind primitive check. SExpression bindings
/// (`@entity.X`, `@config.X`, `@payload.X`) and function calls / nested
/// expressions are late-bound; they pass the static check.
fn check_primitive_shape(type_tag: &str, value: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();
    if let SExpression::String(s) = value {
        if s.starts_with('@') {
            return result;
        }
    }
    let actual = match value {
        SExpression::String(_) => "string",
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        SExpression::Array(_) => "array",
        SExpression::Object(_) => "object",
        SExpression::Null => "null",
    };
    let ok = match type_tag {
        "string" => actual == "string",
        "number" => actual == "number",
        "boolean" => actual == "boolean",
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        _ => true, // Unknown type tags pass — covered by other validators.
    };
    if !ok {
        result.add_error(ValidationError::error(
            ErrorCode::TraitConfigShapeMismatch,
            path,
            format!("expected {}, got {}", type_tag, actual),
        ));
    }
    result
}
