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
use crate::embedded::is_known_operator;

/// True when `value` is a computed S-expression (a binding reference or an
/// operator call) rather than a literal — its shape only materializes at
/// runtime, so structural checks against the declared type must defer to
/// the runtime/Layer 2 obligation instead of firing statically. Reuses the
/// same call-detection predicate as `sexpr::validate` (`is_known_operator`
/// or a namespaced `/` operator name) so "is this a call" has one
/// definition across the validator.
fn is_deferred_computed(value: &SExpression) -> bool {
    if value.is_binding() {
        return true;
    }
    if let SExpression::Array(arr) = value {
        if let Some(SExpression::String(first)) = arr.first() {
            return is_known_operator(first) || first.contains('/');
        }
    }
    false
}

/// Validate all `slot` and `pattern` config fields in the schema.
pub fn validate_all_config_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    result.merge(validate_dangling_config_refs(schema));

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let path_prefix = super::trait_path(oi, &orbital.name, ti, trait_ref);
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
    // Binding-ref defaults (`@config.*` forwards, `@entity.*`) resolve at
    // inline time against the composing scope — their shape can't be
    // checked statically against the declared type here. (Degenerate
    // same-key self-reference is guarded at L1, where declarations and
    // rebind override blocks are still syntactically distinct.)
    if let SExpression::String(s) = default {
        if s.starts_with('@') {
            return result;
        }
    }
    // Computed S-expression default (e.g. `(array/map @entity.tiles (fn
    // [t] {...}))`) — an operator call, not a literal. It lowers to the
    // same `SExpression::Array` variant as a literal array, so it must be
    // distinguished before the array/object literal checks below run, or
    // the call's operator/argument slots get misread as literal elements.
    // Its shape only materializes at runtime — defer to Layer 2.
    if is_deferred_computed(default) {
        return result;
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
    // Same computed-value deferral as the top-level default: a per-element
    // or per-property value can itself be a binding or an operator call
    // (e.g. an object property whose value is `(array/map ...)`).
    if is_deferred_computed(value) {
        return result;
    }
    match schema.field_type {
        FieldType::Array => {
            let SExpression::Array(items) = value else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "expected an array, got {}",
                            super::sexpr::render_sexpr_brief(value)
                        ),
                    )
                    .with_suggestion("wrap the value(s) in `[ … ]`".to_string()),
                );
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
                // Name what was FOUND and the expected member keys — the bare
                // "expected an object" left a run-22 agent with nothing to
                // act on (no line resolves for config(…) paths).
                let expected_fields = schema.properties.as_ref().map(|p| {
                    let mut keys: Vec<&str> = p.keys().map(String::as_str).collect();
                    keys.sort_unstable();
                    keys.join(", ")
                });
                let mut err = ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "expected an object, got {}",
                        super::sexpr::render_sexpr_brief(value)
                    ),
                );
                err = err.with_suggestion(match &expected_fields {
                    Some(keys) => {
                        format!("write this entry as an object literal `{{ … }}` with fields: {keys}")
                    }
                    None => "write this entry as an object literal `{ … }`".to_string(),
                });
                result.add_error(err);
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
    if is_deferred_computed(value) {
        return result;
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
        let mut err = ValidationError::error(
            ErrorCode::TraitConfigShapeMismatch,
            path,
            format!("expected {}, got {}", type_tag, actual),
        );
        // An object supplied where a scalar element was expected is the
        // `[json]`/`[string]`-carries-objects trap: name the typed-alias form
        // as the precise way to declare an array of object records.
        if actual == "object" {
            err = err.with_suggestion(TYPED_ALIAS_ARRAY_HINT);
        }
        result.add_error(err);
    }
    result
}

/// The precise remedy for an object-array config knob: declare a named type
/// alias and annotate the knob with it, so the object element shape is checked.
const TYPED_ALIAS_ARRAY_HINT: &str = "to declare an array of object records, \
define a named type alias — `type Item = { field : T, … }` — then annotate the \
knob `name : [Item]`. A bare `[json]` array lowers to unconstrained object \
elements; `[string]`/scalar element types cannot carry objects.";

/// C-CONFIG-DANGLING-REF-NO-VALIDATE: a literal `@config.<key>` in a resolved
/// trait's body (config defaults, transition effects, tick effects) whose key
/// is declared by NEITHER the owning trait NOR any transitive `@trait.X`
/// referrer chain is a dangling reference — at runtime the forward has no
/// schema to chain through and the component receives the literal string
/// (silent empty subtree / `.map` crash class). Referrer-declared keys are
/// LEGIT: `buildResolvedTraitConfigs` chains a child's `@config.fields`
/// through its embedder, so only keys unreachable from the whole chain flag.
fn validate_dangling_config_refs(schema: &OrbitalSchema) -> ValidationResult {
    use std::collections::{HashMap, HashSet};

    fn config_key_of(s: &str) -> Option<&str> {
        let rest = s.strip_prefix("@config.")?;
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        Some(key)
    }

    fn collect_config_keys(expr: &SExpression, out: &mut HashSet<String>) {
        collect_config_keys_inner(expr, None, out);
    }

    /// `prop_context` is Some((prop_key, pattern_has_prop)) when `expr` is the
    /// direct value of a prop on a registry-known pattern node. A SELF-NAMED
    /// token there (`amount: "@config.amount"` on a stat-display) is the
    /// pattern layer's declaration-form default for an unset prop — enrich
    /// materializes it and the component resolves/nulls it — not a forward
    /// that needs a declaring trait. Everything else collects.
    fn collect_config_keys_inner(
        expr: &SExpression,
        prop_context: Option<(&str, bool)>,
        out: &mut HashSet<String>,
    ) {
        match expr {
            SExpression::String(s) => {
                if let Some(key) = config_key_of(s) {
                    let is_declaration_default = matches!(
                        prop_context,
                        Some((prop_key, true)) if prop_key == key
                    );
                    if !is_declaration_default {
                        out.insert(key.to_string());
                    }
                }
            }
            SExpression::Array(items) => {
                for item in items {
                    collect_config_keys_inner(item, None, out);
                }
            }
            SExpression::Object(map) => {
                let pattern_props = match map.get("type") {
                    Some(SExpression::String(ty)) => {
                        crate::pattern_registry::get_props_schema(ty.as_str())
                    }
                    _ => None,
                };
                for (k, v) in map {
                    let ctx = pattern_props
                        .map(|props| (k.as_str(), props.contains_key(k.as_str())));
                    collect_config_keys_inner(v, ctx, out);
                }
            }
            _ => {}
        }
    }

    fn collect_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
        match expr {
            SExpression::String(s) => {
                if let Some(rest) = s.strip_prefix("@trait.") {
                    let end = rest.find(['.', '[']).unwrap_or(rest.len());
                    if end > 0 {
                        out.insert(rest[..end].to_string());
                    }
                }
            }
            SExpression::Array(items) => {
                for item in items {
                    collect_trait_refs(item, out);
                }
            }
            SExpression::Object(map) => {
                for v in map.values() {
                    collect_trait_refs(v, out);
                }
            }
            _ => {}
        }
    }

    let mut result = ValidationResult::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Declared knob keys per trait + child -> referrers adjacency (the
        // same config-OR-state-machine scan core's embed walkers use).
        let mut declared: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut referrers: HashMap<String, Vec<&str>> = HashMap::new();
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let keys: HashSet<&str> = td
                .config
                .iter()
                .flat_map(|c| c.keys())
                .map(|k| k.as_str())
                .collect();
            declared.insert(td.name.as_str(), keys);
            let mut children: HashSet<String> = HashSet::new();
            for cf in td.config.iter().flat_map(|c| c.values()) {
                if let Some(default) = &cf.default {
                    collect_trait_refs(default, &mut children);
                }
            }
            for transition in &td.state_machine.transitions {
                for effect in transition.effects.iter().flatten() {
                    collect_trait_refs(effect, &mut children);
                }
            }
            for tick in td.ticks.iter().flatten() {
                for effect in &tick.effects {
                    collect_trait_refs(effect, &mut children);
                }
            }
            for child in children {
                if child != td.name {
                    referrers.entry(child).or_default().push(td.name.as_str());
                }
            }
        }
        // Undirected embed adjacency: forwards chain UP through referrers
        // (`buildResolvedTraitConfigs`), and a parent's effect tree also
        // inherits its CHILDREN's materialized pattern props whose defaults
        // are self-forward tokens (`amount: "@config.amount"` declared on the
        // embedded stat-display) — so a key declared anywhere in the embed
        // component is reachable; only component-wide undeclared keys dangle.
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for (child, parents) in &referrers {
            for parent in parents {
                adjacency.entry(child.as_str()).or_default().push(parent);
                adjacency.entry(parent).or_default().push(child.as_str());
            }
        }

        let key_reachable = |owner: &str, key: &str| -> bool {
            let mut queue = vec![owner];
            let mut seen: HashSet<&str> = HashSet::new();
            while let Some(name) = queue.pop() {
                if !seen.insert(name) {
                    continue;
                }
                if declared.get(name).is_some_and(|keys| keys.contains(key)) {
                    return true;
                }
                if let Some(neighbors) = adjacency.get(name) {
                    queue.extend(neighbors.iter().copied());
                }
            }
            false
        };

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let mut used: HashSet<String> = HashSet::new();
            for cf in td.config.iter().flat_map(|c| c.values()) {
                if let Some(default) = &cf.default {
                    collect_config_keys(default, &mut used);
                }
            }
            for transition in &td.state_machine.transitions {
                for effect in transition.effects.iter().flatten() {
                    collect_config_keys(effect, &mut used);
                }
            }
            for tick in td.ticks.iter().flatten() {
                for effect in &tick.effects {
                    collect_config_keys(effect, &mut used);
                }
            }
            let mut dangling: Vec<&String> = used
                .iter()
                .filter(|key| !key_reachable(&td.name, key))
                .collect();
            dangling.sort();
            for key in dangling {
                let path = format!(
                    "orbitals[{} ({})].traits[{} ({})]",
                    oi, orbital.name, ti, td.name
                );
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ConfigDanglingRef,
                        path,
                        format!(
                            "`@config.{}` — no trait in {}'s referrer chain declares config knob '{}'; \
                             the forward cannot resolve and renders as the literal string",
                            key, td.name, key
                        ),
                    )
                    .with_suggestion(format!(
                        "Declare `{}` on the owning trait (or its embedder) or remove the reference",
                        key
                    )),
                );
            }
        }
    }

    result
}

#[cfg(test)]
mod dangling_config_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    const TRUE_DANGLE: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Panel",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "typography", "content": "@config.missingTitle" }]]
            }]
          }
        }]
      }]
    }"#;

    const DECLARATION_DEFAULT: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Panel",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "stat-display", "label": "@config.label" }]]
            }]
          }
        }]
      }]
    }"#;

    const REFERRER_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          {
            "name": "Child",
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "typography", "content": "@config.fields" }]]
              }]
            }
          },
          {
            "name": "Parent",
            "config": { "fields": { "type": "string", "default": "x" } },
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "stack", "children": ["@trait.Child"] }]]
              }]
            }
          }
        ]
      }]
    }"#;

    fn dangles(json: &str) -> usize {
        validate_dangling_config_refs(&schema_from_json(json))
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
            .count()
    }

    #[test]
    fn flags_component_wide_undeclared_key() {
        assert_eq!(dangles(TRUE_DANGLE), 1, "undeclared key must flag");
    }

    #[test]
    fn accepts_self_named_pattern_declaration_default() {
        assert_eq!(
            dangles(DECLARATION_DEFAULT),
            0,
            "self-named pattern-prop declaration default is the enrich idiom, not a dangle"
        );
    }

    #[test]
    fn accepts_key_declared_by_embed_component() {
        assert_eq!(
            dangles(REFERRER_DECLARED),
            0,
            "a key declared anywhere in the undirected embed component is reachable"
        );
    }
}
