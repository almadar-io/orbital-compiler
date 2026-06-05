//! Pattern validation: prop names, required props, prop value types, enum
//! values, field-ref markers, nested children recursion.

use super::field_path::{validate_field_paths, validate_pattern_field_ref};
use super::item_actions::validate_item_actions;
use super::sync::patterns::{
    get_pattern_props, get_pattern_valid_props, get_prop_enum_values, get_prop_types,
    is_valid_pattern, requires_action,
};
use super::sync::ExtraPatterns;
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EntityDefinition, SExpression};
use std::collections::HashMap;

/// Validate a pattern
pub(super) fn validate_pattern(
    pattern: &SExpression,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    extra_patterns: Option<&ExtraPatterns>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match pattern {
        SExpression::Object(obj) => {
            // Check for type property
            if let Some(SExpression::String(pattern_type)) = obj.get("type") {
                // Check core patterns first, then extra patterns
                let in_core = is_valid_pattern(pattern_type);
                let in_extra = extra_patterns
                    .map(|ep| ep.contains_pattern(pattern_type))
                    .unwrap_or(false);

                if !in_core && !in_extra {
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::RenderUIInvalidPattern,
                            path,
                            format!("Unknown pattern type: '{}'", pattern_type),
                        )
                        .with_suggestion(
                            "Use a known pattern type or ensure custom pattern is registered",
                        ),
                    );
                }

                // Validate pattern props against registry schema
                // Check core props first, then fall back to extra patterns
                let core_props = get_pattern_valid_props(pattern_type);
                let extra_props = extra_patterns.and_then(|ep| ep.get_valid_props(pattern_type));

                if core_props.is_some() || extra_props.is_some() {
                    for prop_name in obj.keys() {
                        // Skip 'type' - it's the pattern identifier, not a prop
                        if prop_name == "type" {
                            continue;
                        }
                        let in_core_props = core_props
                            .map(|p| p.contains(prop_name.as_str()))
                            .unwrap_or(false);
                        let in_extra_props = extra_props
                            .map(|p| p.contains(prop_name.as_str()))
                            .unwrap_or(false);

                        if !in_core_props && !in_extra_props {
                            // Collect valid values from whichever set has them
                            let valid_values: Vec<String> = if let Some(cp) = core_props {
                                cp.iter().map(|s| s.to_string()).collect()
                            } else if let Some(ep) = extra_props {
                                ep.iter().cloned().collect()
                            } else {
                                vec![]
                            };

                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIInvalidProp,
                                    path,
                                    format!(
                                        "Unknown prop '{}' for pattern '{}'",
                                        prop_name, pattern_type
                                    ),
                                )
                                .with_valid_values(valid_values)
                                .with_suggestion(format!(
                                    "Check pattern registry for valid props for '{}'",
                                    pattern_type
                                )),
                            );
                        }
                    }
                }

                // Check for missing required props
                if let Some(required) = get_pattern_props(pattern_type) {
                    for req_prop in required {
                        if !obj.contains_key(*req_prop) {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIMissingRequiredProp,
                                    path,
                                    format!(
                                        "Pattern '{}' requires prop '{}' but it is not provided",
                                        pattern_type, req_prop
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Add '\"{}\"' to the render-ui config for '{}'",
                                    req_prop, pattern_type
                                )),
                            );
                        }
                    }
                }

                // Validate prop value types against registry
                for (prop_name, prop_value) in obj.iter() {
                    if prop_name == "type" || prop_name == "children" {
                        continue;
                    }
                    if let Some(expected_types) = get_prop_types(pattern_type, prop_name) {
                        let actual_type = classify_prop_type(prop_value);
                        // Skip bindings (@entity.x, @payload.y) - runtime resolves these
                        if actual_type == "binding" {
                            continue;
                        }
                        let expects_array = expected_types.iter().any(|t| *t == "array");
                        let expects_object = expected_types.iter().any(|t| *t == "object");
                        let accepts_string = expected_types.iter().any(|t| *t == "string");
                        // V-2 hard rule: an array-typed prop (e.g. `data-grid.entity`,
                        // `data-grid.fields`) MUST be either an array literal or a
                        // binding expression. A bare string literal like
                        // `entity: "Tenant"` cannot iterate at runtime and silently
                        // renders zero rows — error here so authors see it at validate
                        // time. The `expects_object` co-presence (entity is
                        // `["object", "array"]`) doesn't excuse a string value because
                        // a string can satisfy neither side. We DO exempt patterns
                        // that explicitly list `"string"` in expected_types (e.g.
                        // entity is `["string", "object", "array"]`) — those legitimately
                        // accept an entity-name reference.
                        if expects_array && actual_type == "string" && !accepts_string {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIPropTypeMismatch,
                                    path,
                                    format!(
                                        "Prop '{}' on '{}' was a string literal but expected one of: {} — a bare string cannot iterate as an array, use `@payload.X` / `@entity.X` (binding) or an array literal",
                                        prop_name,
                                        pattern_type,
                                        expected_types.join(", ")
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Change '{}' to a binding expression like '@payload.data' or an array literal",
                                    prop_name,
                                )),
                            );
                            continue;
                        }
                        // L2.4 — Element-shape check: when the prop is an
                        // array of objects with per-element required fields,
                        // assert each element of an inline array literal
                        // carries every required field. Catches the
                        // landing-page/StepCard-missing-id class of bugs
                        // before codegen. Pure structural check; runtime
                        // bindings (`@payload.X`) are exempt because their
                        // resolved shape only exists at runtime. Lives BEFORE
                        // the `expects_object` early-out so it still runs on
                        // props whose types are `["object","array"]` (e.g.
                        // `entity` on EntityDisplayProps<T>-derived patterns).
                        if let SExpression::Array(elements) = prop_value {
                            let registry_prop = crate::embedded::get_pattern(pattern_type)
                                .and_then(|p| p.props_schema.get(prop_name));
                            if let Some(rp) = registry_prop {
                                if let Some(items) = rp.items.as_ref() {
                                    let required_from_array = items.required.as_names();
                                    let required_from_props: Vec<String> = items
                                        .properties
                                        .as_ref()
                                        .map(|p| {
                                            p.iter()
                                                .filter(|(_, v)| v.required.as_bool())
                                                .map(|(k, _)| k.clone())
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let mut required_fields: Vec<&str> =
                                        required_from_array.iter().map(|s| s.as_str()).collect();
                                    for r in &required_from_props {
                                        if !required_fields.contains(&r.as_str()) {
                                            required_fields.push(r.as_str());
                                        }
                                    }
                                    if !required_fields.is_empty() {
                                        for (idx, elem) in elements.iter().enumerate() {
                                            if let SExpression::Object(elem_obj) = elem {
                                                for field_name in &required_fields {
                                                    if !elem_obj.contains_key(*field_name) {
                                                        result.add_error(
                                                            ValidationError::error(
                                                                ErrorCode::RenderUIElementShapeMismatch,
                                                                &format!("{}.{}[{}]", path, prop_name, idx),
                                                                format!(
                                                                    "Element [{}] of prop '{}' on '{}' is missing required field '{}' — the pattern's element shape requires it",
                                                                    idx, prop_name, pattern_type, field_name
                                                                ),
                                                            )
                                                            .with_suggestion(format!(
                                                                "Add '{}' to every element of '{}' (e.g. `{{ {}: \"...\", ... }}`)",
                                                                field_name, prop_name, field_name
                                                            )),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Skip if expected is "object" - too broad, custom types resolve
                        // to "object" but often accept strings (e.g., StackDirection).
                        // The array-vs-string mismatch above runs first so this skip
                        // doesn't paper over the V-2 case where `entity` is `["object",
                        // "array"]` and the value is a bare string.
                        if expects_object {
                            continue;
                        }
                        let type_matches = expected_types.iter().any(|t| {
                            *t == actual_type
                                || *t == "node"  // ReactNode accepts any rendered value
                                || *t == "function" // functions can't appear in .orb
                                || *t == "icon" // icon names are strings
                                    && actual_type == "string"
                        });
                        if !type_matches {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::RenderUIPropTypeMismatch,
                                    path,
                                    format!(
                                        "Prop '{}' on '{}' has type '{}' but expected: {}",
                                        prop_name,
                                        pattern_type,
                                        actual_type,
                                        expected_types.join(", ")
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Change '{}' value to match type: {}",
                                    prop_name,
                                    expected_types.join(" or ")
                                )),
                            );
                        }

                    }

                    // Enum-value check: when the prop's TS type was a bounded
                    // string-literal union (e.g. `StackJustify`, `AlertVariant`),
                    // the registry carries the allowed values. Reject any string
                    // literal that isn't in the set. Bindings (`@entity.x`)
                    // resolve at runtime and are skipped.
                    if let Some(allowed) = get_prop_enum_values(pattern_type, prop_name) {
                        if let SExpression::String(s) = prop_value {
                            if !s.starts_with('@') && !allowed.contains(&s.as_str()) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::RenderUIPropTypeMismatch,
                                        path,
                                        format!(
                                            "Prop '{}' on '{}' has value '{}' but must be one of: {}",
                                            prop_name,
                                            pattern_type,
                                            s,
                                            allowed.join(", ")
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Use one of the allowed values for '{}': {}",
                                        prop_name,
                                        allowed.join(", ")
                                    )),
                                );
                            }
                        }
                    }
                }

                // Validate itemActions if present
                if let Some(item_actions) = obj.get("itemActions") {
                    result.merge(validate_item_actions(item_actions, path));
                }

                // Validate nested field paths in 'fields' prop
                if let Some(fields) = obj.get("fields") {
                    if let Some(SExpression::String(entity_name)) = obj.get("entity") {
                        result.merge(validate_field_paths(
                            fields,
                            entity_name,
                            path,
                            entity_definitions,
                        ));
                    }
                }

                // Validate field-ref markers on pattern props.
                // Scalar props (`kind: 'field-ref'`) and array element props
                // (`itemKind: 'field-ref'`) are resolved against the linked
                // entity's fields. See Phase 3.2/3.3 in
                // docs/Almadar_Agent_Behaviors_Plan.md.
                if let Some(SExpression::String(entity_name)) = obj.get("entity") {
                    if let Some(embedded_pattern) = crate::embedded::get_pattern(pattern_type) {
                        for (prop_name, prop_value) in obj.iter() {
                            if prop_name == "type" || prop_name == "entity" {
                                continue;
                            }
                            let Some(prop_schema) =
                                embedded_pattern.props_schema.get(prop_name)
                            else {
                                continue;
                            };

                            let scalar_is_field_ref = prop_schema
                                .kind
                                .as_deref()
                                .map(|k| k == "field-ref")
                                .unwrap_or(false);
                            let array_is_field_ref = prop_schema
                                .item_kind
                                .as_deref()
                                .map(|k| k == "field-ref")
                                .unwrap_or(false);

                            if scalar_is_field_ref {
                                if let SExpression::String(value) = prop_value {
                                    // Skip bindings (@entity.x, @payload.y,
                                    // @config.x) - the runtime/inline phase
                                    // resolves these.
                                    if value.starts_with('@') {
                                        continue;
                                    }
                                    result.merge(validate_pattern_field_ref(
                                        value,
                                        entity_name,
                                        pattern_type,
                                        prop_name,
                                        path,
                                        entity_definitions,
                                    ));
                                }
                            }

                            if array_is_field_ref {
                                if let SExpression::Array(elements) = prop_value {
                                    for element in elements {
                                        // Only validate string elements; skip
                                        // Column-object forms, objects,
                                        // numbers, and bindings.
                                        let SExpression::String(value) = element else {
                                            continue;
                                        };
                                        if value.starts_with('@') {
                                            continue;
                                        }
                                        result.merge(validate_pattern_field_ref(
                                            value,
                                            entity_name,
                                            pattern_type,
                                            prop_name,
                                            path,
                                            entity_definitions,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }

                // Check for action on button/link patterns using synced definition
                if requires_action(pattern_type)
                    && !obj.contains_key("action")
                    && !obj.contains_key("onClick")
                    && !obj.contains_key("href")
                    && !obj.contains_key("event")
                {
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::RenderUIMissingAction,
                            path,
                            format!(
                                "{} pattern should have 'action', 'onClick', 'event', or 'href'",
                                pattern_type
                            ),
                        )
                        .with_suggestion("Add action: 'EVENT_NAME' to emit an event on click"),
                    );
                }

                // Recurse into `children` so nested pattern configs
                // (stack-in-dashboard-in-modal chains) get their own prop
                // type + enum-value checks. Children can be a single pattern
                // object or an array of patterns; handle both.
                if let Some(children) = obj.get("children") {
                    match children {
                        SExpression::Array(items) => {
                            for (i, child) in items.iter().enumerate() {
                                let child_path = format!("{}.children[{}]", path, i);
                                result.merge(validate_pattern(
                                    child,
                                    &child_path,
                                    entity_definitions,
                                    extra_patterns,
                                ));
                            }
                        }
                        SExpression::Object(_) => {
                            let child_path = format!("{}.children", path);
                            result.merge(validate_pattern(
                                children,
                                &child_path,
                                entity_definitions,
                                extra_patterns,
                            ));
                        }
                        _ => {}
                    }
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::RenderUIMissingPatternType,
                        path,
                        "Pattern must have a 'type' property",
                    )
                    .with_suggestion("Add type: 'pattern-name' to define the pattern type"),
                );
            }
        }
        SExpression::String(pattern_type) => {
            // `@trait.X` / `@payload.X` / `@entity.X` etc. are binding tokens,
            // not pattern-type names — a separate positional validator
            // (`walk_pattern_for_trait_positions`) enforces where they may
            // appear. Skip the pattern-type check for any binding literal
            // so it does not masquerade as an unknown pattern.
            if pattern_type.starts_with('@') {
                return result;
            }
            // Simple string form for pattern type
            let in_core = is_valid_pattern(pattern_type);
            let in_extra = extra_patterns
                .map(|ep| ep.contains_pattern(pattern_type))
                .unwrap_or(false);
            if !in_core && !in_extra {
                result.add_error(ValidationError::warning(
                    ErrorCode::RenderUIInvalidPattern,
                    path,
                    format!("Unknown pattern type: '{}'", pattern_type),
                ));
            }
        }
        SExpression::Null => {
            // null is valid - it clears/closes the slot
            // e.g., ["render-ui", "modal", null] closes the modal
        }
        _ => {
            result.add_error(ValidationError::error(
                ErrorCode::RenderUIInvalidPattern,
                path,
                "Pattern must be an object, string, or null (to clear slot)",
            ));
        }
    }

    result
}

// =============================================================================
// Prop Type Classification
// =============================================================================

/// Classify an S-expression value into a simple type string for prop type validation.
pub(super) fn classify_prop_type(value: &SExpression) -> &'static str {
    match value {
        SExpression::String(s) if s.starts_with('@') => "binding",
        SExpression::String(_) => "string",
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        SExpression::Array(_) => "array",
        SExpression::Object(_) => "object",
        SExpression::Null => "null",
    }
}
