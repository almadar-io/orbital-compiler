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

/// The field names that EVERY element of an array-of-objects prop must carry,
/// for the `ORB_RUI_ELEMENT_SHAPE_MISMATCH` element-shape check. Two sources,
/// unioned:
/// - `items.required` (the `Names([...])` at the element level — fields required
///   on each element), read via `as_names()`; and
/// - each per-element property that is itself required ON the element
///   (`required_on_parent()` = the `Bool(true)` form ONLY).
///
/// A property whose `required` is a `Names([...])` lists *its own* nested
/// required sub-keys and is NOT itself required on the element — so e.g. an
/// optional `relation: { …, required: ["entity"] }` must NOT be demanded on
/// every `form-section` field (the CG-FORM-SECTION-RELATION bug).
fn element_required_fields(items: &crate::embedded::EmbeddedPatternProp) -> Vec<String> {
    let mut out: Vec<String> = items.required.as_names().to_vec();
    if let Some(props) = items.properties.as_ref() {
        for (k, v) in props {
            if v.required.required_on_parent() && !out.contains(k) {
                out.push(k.clone());
            }
        }
    }
    out
}

/// Validate a pattern
pub(super) fn validate_pattern(
    pattern: &SExpression,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    extra_patterns: Option<&ExtraPatterns>,
    linked_entity: Option<&str>,
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
                        let expects_array = expected_types.contains(&"array");
                        let expects_object = expected_types.contains(&"object");
                        let accepts_string = expected_types.contains(&"string");
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
                                    let required_fields = element_required_fields(items);
                                    if !required_fields.is_empty() {
                                        for (idx, elem) in elements.iter().enumerate() {
                                            if let SExpression::Object(elem_obj) = elem {
                                                for field_name in &required_fields {
                                                    if !elem_obj.contains_key(field_name.as_str()) {
                                                        result.add_error(
                                                            ValidationError::error(
                                                                ErrorCode::RenderUIElementShapeMismatch,
                                                                format!("{}.{}[{}]", path, prop_name, idx),
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
                                || (*t == "icon" // icon names are strings
                                    && actual_type == "string")
                                // A `date`/`datetime`/`timestamp` prop accepts an ISO
                                // string or an epoch number — consistent with
                                // `orbital_core::type_compat` (date_family accepts
                                // string|number). A literal date default lowers to a
                                // string/number, so rejecting it here would contradict
                                // the type system (a config knob couldn't carry a date).
                                || (matches!(*t, "date" | "datetime" | "timestamp")
                                    && matches!(actual_type, "string" | "number"))
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
                            let Some(prop_schema) = embedded_pattern.props_schema.get(prop_name)
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

                // Entity-row clean slate: enforce the `EntityWith<K>`
                // contract — every prop whose registry schema declares
                // `requiredFields` must have those fields provided by the
                // bound entity or a remap/field-map. Remap-aware: runs
                // post-inline, so `@entity.<old>` was already rewritten to
                // the new name and `@config.X` substituted.
                result.merge(validate_entity_prop_contract(
                    pattern_type,
                    obj,
                    path,
                    entity_definitions,
                    linked_entity,
                ));

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
                                    linked_entity,
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
                                linked_entity,
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
// Entity Prop Contract (ORB_X_ENTITY_PROP_CONTRACT)
// =============================================================================

/// Enforce the `EntityWith<K>` clean-slate contract: for each prop whose
/// registry schema declares `requiredFields`, the bound entity (or a remap
/// field-map on the resolved pattern object) MUST provide every required
/// field. Remap-aware — runs post-inline, so `@entity.<old>` references were
/// already rewritten to the new field name and `@config.X` substituted.
///
/// Permissive on provision (false negatives are preferred to spurious
/// errors): a field counts as provided if the entity declares it, a
/// `columns`/`fields` map remaps it, a prop slot literally named for it is
/// bound, or any `@entity.<field>` reference for it appears in the resolved
/// object.
fn validate_entity_prop_contract(
    pattern_type: &str,
    obj: &HashMap<String, SExpression>,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    linked_entity: Option<&str>,
) -> ValidationResult {
    let Some(embedded_pattern) = crate::embedded::get_pattern(pattern_type) else {
        return ValidationResult::new();
    };
    validate_entity_prop_contract_inner(
        pattern_type,
        embedded_pattern,
        obj,
        path,
        entity_definitions,
        linked_entity,
    )
}

/// Registry-injectable core of `validate_entity_prop_contract` — takes the
/// resolved `EmbeddedPattern` so unit tests can supply a pattern carrying
/// `requiredFields` (the baked registry won't carry it until a later re-bake).
fn validate_entity_prop_contract_inner(
    pattern_type: &str,
    embedded_pattern: &crate::embedded::EmbeddedPattern,
    obj: &HashMap<String, SExpression>,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    linked_entity: Option<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Resolve the bound entity name. A literal `entity: "Name"` names it
    // directly. A WHOLE-entity binding (`@entity`, `@payload[.data]`, `@data`)
    // or an absent `entity` prop maps to the trait's linked entity. A SUB-FIELD
    // binding (`entity: @entity.items`, `@config.team`) binds an element
    // collection whose element type is not recoverable here (`FieldType::Array`
    // carries no inner type) — the prop's `requiredFields` apply to that opaque
    // element type, NOT the parent entity — so we skip rather than false-positive
    // against the parent. (The element rows are already validated by the
    // component contract on the producing side.)
    let bound_entity: Option<&str> = match obj.get("entity") {
        Some(SExpression::String(s)) if !s.starts_with('@') => Some(s.as_str()),
        Some(SExpression::String(s)) => {
            if is_whole_entity_binding(s) {
                linked_entity
            } else {
                return result;
            }
        }
        None => linked_entity,
        // Any non-string literal value (array/object) — element type opaque.
        Some(_) => return result,
    };

    // SKIP when the bound entity is unresolvable — exactly like the existing
    // field-ref checks. Better to miss than to emit a spurious error.
    let Some(entity_name) = bound_entity else {
        return result;
    };
    let entity_def = entity_definitions.get(entity_name);

    for (prop_name, prop_schema) in embedded_pattern.props_schema.iter() {
        let Some(required_fields) = prop_schema.required_fields.as_ref() else {
            continue;
        };
        if required_fields.is_empty() {
            continue;
        }
        // Only enforce on props that are actually bound at this call site.
        if !obj.contains_key(prop_name) {
            continue;
        }

        for field in required_fields {
            if !is_field_provided(field, obj, entity_def) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityPropContract,
                        path,
                        format!(
                            "Component '{}' requires entity field '{}', but the bound entity '{}' does not provide it (declare the field, or remap one with a field map)",
                            pattern_type, field, entity_name
                        ),
                    )
                    .with_suggestion(format!(
                        "Add field '{}' to entity '{}', or remap an existing field onto '{}' via the pattern's columns/fields map",
                        field, entity_name, field
                    )),
                );
            }
        }
    }

    result
}

/// True when an `@`-binding refers to the entity as a whole — the linked
/// entity record (`@entity`) or its loaded collection (`@payload`,
/// `@payload.data`, `@data`) — rather than a sub-field. Only whole-entity
/// bindings let a prop's `requiredFields` be checked against the linked
/// entity; a sub-field binding (`@entity.items`, `@config.team`) binds an
/// element collection whose type is opaque here.
fn is_whole_entity_binding(binding: &str) -> bool {
    let mut segs = binding.trim_start_matches('@').split('.');
    match segs.next() {
        // The loaded entity collection.
        Some("payload") | Some("data") => true,
        // The entity record itself — only with no sub-path.
        Some("entity") => segs.next().is_none(),
        _ => false,
    }
}

/// A required field `f` is PROVIDED if ANY of:
/// (i) the bound entity declares a field named `f`;
/// (ii) the resolved pattern obj supplies it via a remap/field-map —
///      a `columns` array element with `field == f`, a `fields` array
///      containing `f`, or a prop slot literally named `f`;
/// (iii) any `@entity.f` reference appears anywhere in the resolved obj.
fn is_field_provided(
    field: &str,
    obj: &HashMap<String, SExpression>,
    entity_def: Option<&EntityDefinition>,
) -> bool {
    // (i) entity declares the field
    if let Some(ed) = entity_def {
        if ed.fields.iter().any(|f| f.name.as_deref() == Some(field)) {
            return true;
        }
    }

    // (ii) field-map provision
    if let Some(SExpression::Array(cols)) = obj.get("columns") {
        for col in cols {
            if let SExpression::Object(col_obj) = col {
                if let Some(SExpression::String(f)) = col_obj.get("field") {
                    if f == field {
                        return true;
                    }
                }
            }
        }
    }
    if let Some(SExpression::Array(fields)) = obj.get("fields") {
        for f in fields {
            if let SExpression::String(s) = f {
                if s == field {
                    return true;
                }
            }
        }
    }
    if obj.contains_key(field) {
        return true;
    }

    // (iii) any `@entity.<field>` reference in the resolved obj
    let needle = format!("@entity.{}", field);
    if sexpr_references_binding(&SExpression::Object(obj.clone()), &needle) {
        return true;
    }

    false
}

/// Recursively scan an S-expression for a string whose value equals or
/// path-prefixes `needle` (so `@entity.title` matches `@entity.title.x`).
fn sexpr_references_binding(node: &SExpression, needle: &str) -> bool {
    match node {
        SExpression::String(s) => s == needle || s.starts_with(&format!("{}.", needle)),
        SExpression::Array(arr) => arr.iter().any(|n| sexpr_references_binding(n, needle)),
        SExpression::Object(obj) => obj.values().any(|n| sexpr_references_binding(n, needle)),
        _ => false,
    }
}

#[cfg(test)]
mod element_shape_tests {
    use super::element_required_fields;
    use crate::embedded::EmbeddedPatternProp;

    fn items_prop(json: serde_json::Value) -> EmbeddedPatternProp {
        serde_json::from_value(json).expect("valid items prop fixture")
    }

    #[test]
    fn optional_nested_object_prop_is_not_required_on_parent() {
        // The `form-section` `fields[]` element shape: `name`/`label`/`type` are
        // plain optional props, `relation` is an OBJECT prop whose OWN `entity`
        // sub-key is required (`required: ["entity"]`). None of these is required
        // ON the element, so the element-required set must be EMPTY — a field
        // literal `{ name, label, type }` (no relation) must not be rejected.
        let items = items_prop(serde_json::json!({
            "types": ["object"],
            "properties": {
                "name":  { "types": ["string"] },
                "label": { "types": ["string"] },
                "type":  { "types": ["string"] },
                "relation": {
                    "types": ["object"],
                    "properties": { "entity": { "types": ["string"] } },
                    "required": ["entity"]
                }
            }
        }));
        assert!(
            element_required_fields(&items).is_empty(),
            "optional relation (required:[\"entity\"]) must not be parent-required"
        );
    }

    #[test]
    fn bool_true_and_item_level_names_are_required_on_parent() {
        // `items.required: ["title"]` (element-level) + a property `id` marked
        // `required: true` (Bool) → both required on every element.
        let items = items_prop(serde_json::json!({
            "types": ["object"],
            "required": ["title"],
            "properties": {
                "id":    { "types": ["string"], "required": true },
                "title": { "types": ["string"] }
            }
        }));
        let mut got = element_required_fields(&items);
        got.sort();
        assert_eq!(got, vec!["id".to_string(), "title".to_string()]);
    }
}

#[cfg(test)]
mod entity_prop_contract_tests {
    use super::*;
    use crate::embedded::EmbeddedPattern;
    use orbital_core::schema::{EntityDefinition, FieldDefinition, FieldType};

    /// A pattern whose `entity` prop is typed `EntityWith<"title" | "subtitle">`:
    /// pattern-sync writes `requiredFields: ["title","subtitle"]` onto it. The
    /// baked registry won't carry this until a re-bake, so we build it here.
    fn pattern_with_required_fields() -> EmbeddedPattern {
        serde_json::from_value(serde_json::json!({
            "type": "entity-cards",
            "category": "data",
            "propsSchema": {
                "entity": {
                    "types": ["string", "object", "array"],
                    "requiredFields": ["title", "subtitle"]
                }
            },
            "entityAware": true
        }))
        .expect("valid embedded pattern fixture")
    }

    fn entity_with_fields(name: &str, field_names: &[&str]) -> EntityDefinition {
        EntityDefinition {
            name: name.to_string(),
            fields: field_names
                .iter()
                .map(|fname| FieldDefinition {
                    name: Some(fname.to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    relation: None,
                    items: None,
                    values: None,
                    default: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                })
                .collect(),
            collection: None,
            persistence: None,
            instances: None,
        }
    }

    fn pattern_obj(entries: Vec<(&str, SExpression)>) -> HashMap<String, SExpression> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn rejects_entity_missing_required_field() {
        // Entity has `title` but NOT `subtitle`; pattern requires both.
        let mut defs = HashMap::new();
        defs.insert(
            "Article".to_string(),
            entity_with_fields("Article", &["title"]),
        );

        let obj = pattern_obj(vec![("entity", SExpression::String("Article".to_string()))]);

        let result = validate_entity_prop_contract_inner(
            "entity-cards",
            &pattern_with_required_fields(),
            &obj,
            "test",
            &defs,
            None,
        );

        let contract_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::EntityPropContract)
            .collect();
        assert_eq!(
            contract_errors.len(),
            1,
            "Expected exactly one ORB_X_ENTITY_PROP_CONTRACT for missing 'subtitle'. Errors: {:?}",
            result.errors
        );
        assert!(
            contract_errors[0].message.contains("subtitle"),
            "Error should name the missing field 'subtitle'. Got: {}",
            contract_errors[0].message
        );
    }

    #[test]
    fn accepts_entity_providing_all_required_fields() {
        let mut defs = HashMap::new();
        defs.insert(
            "Article".to_string(),
            entity_with_fields("Article", &["title", "subtitle"]),
        );

        let obj = pattern_obj(vec![("entity", SExpression::String("Article".to_string()))]);

        let result = validate_entity_prop_contract_inner(
            "entity-cards",
            &pattern_with_required_fields(),
            &obj,
            "test",
            &defs,
            None,
        );

        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityPropContract),
            "Entity provides all required fields; no contract error expected. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn accepts_when_column_field_map_remaps_missing_field() {
        // Entity lacks `subtitle`, but a `columns` field-map remaps a column
        // onto `subtitle` — provision (ii). No error expected.
        let mut defs = HashMap::new();
        defs.insert(
            "Article".to_string(),
            entity_with_fields("Article", &["title"]),
        );

        let columns = SExpression::Array(vec![SExpression::Object(pattern_obj(vec![(
            "field",
            SExpression::String("subtitle".to_string()),
        )]))]);
        let obj = pattern_obj(vec![
            ("entity", SExpression::String("Article".to_string())),
            ("columns", columns),
        ]);

        let result = validate_entity_prop_contract_inner(
            "entity-cards",
            &pattern_with_required_fields(),
            &obj,
            "test",
            &defs,
            None,
        );

        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityPropContract),
            "A columns field-map remaps 'subtitle'; no contract error expected. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn resolves_bound_entity_from_linked_entity_for_binding() {
        // `entity: "@payload.data"` (binding) → resolve to the trait's
        // linked entity (remap-aware coverage). Linked entity lacks
        // 'subtitle' → error.
        let mut defs = HashMap::new();
        defs.insert(
            "Article".to_string(),
            entity_with_fields("Article", &["title"]),
        );

        let obj = pattern_obj(vec![(
            "entity",
            SExpression::String("@payload.data".to_string()),
        )]);

        let result = validate_entity_prop_contract_inner(
            "entity-cards",
            &pattern_with_required_fields(),
            &obj,
            "test",
            &defs,
            Some("Article"),
        );

        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityPropContract
                    && e.message.contains("subtitle")),
            "Binding entity should resolve to linked 'Article' and flag missing 'subtitle'. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn skips_when_entity_unresolvable() {
        // Binding entity, no linked_entity → unresolvable → SKIP (no error).
        let defs: HashMap<String, EntityDefinition> = HashMap::new();
        let obj = pattern_obj(vec![(
            "entity",
            SExpression::String("@payload.data".to_string()),
        )]);

        let result = validate_entity_prop_contract_inner(
            "entity-cards",
            &pattern_with_required_fields(),
            &obj,
            "test",
            &defs,
            None,
        );

        assert!(
            result.is_valid(),
            "Unresolvable bound entity must be skipped, not errored. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn skips_sub_field_binding_against_parent_entity() {
        // `entity: @entity.items` binds an element collection whose type is
        // opaque here. The parent (linked) entity `Section` lacks 'subtitle',
        // but the requiredFields apply to the element type — must NOT
        // false-positive against the parent.
        let mut defs = HashMap::new();
        defs.insert(
            "Section".to_string(),
            entity_with_fields("Section", &["id", "items"]),
        );

        for binding in ["@entity.items", "@config.team", "@config.steps"] {
            let obj = pattern_obj(vec![("entity", SExpression::String(binding.to_string()))]);
            let result = validate_entity_prop_contract_inner(
                "entity-cards",
                &pattern_with_required_fields(),
                &obj,
                "test",
                &defs,
                Some("Section"),
            );
            assert!(
                result.is_valid(),
                "Sub-field binding {binding} must skip the parent-entity contract. Errors: {:?}",
                result.errors
            );
        }
    }

    #[test]
    fn whole_entity_binding_classification() {
        for b in ["@entity", "@payload", "@payload.data", "@data"] {
            assert!(is_whole_entity_binding(b), "{b} should be whole-entity");
        }
        for b in ["@entity.items", "@config.team", "@row.id", "@entity.x.y"] {
            assert!(!is_whole_entity_binding(b), "{b} should be sub-field");
        }
    }
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
