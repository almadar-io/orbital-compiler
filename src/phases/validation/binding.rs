//! Binding Validation
//!
//! Validates binding references (@entity, @payload, @state, etc.)
//! Ported from `builder/packages/shared/src/orbitals/validation/binding-validator.ts`.

use super::canonical_operators::get_canonical_operator;
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, OrbitalSchema, PayloadField, SExpression, TraitDefinition, TraitReference,
};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Core binding roots
pub const CORE_BINDING_ROOTS: &[&str] = &[
    "entity",  // Current entity data
    "payload", // Event payload
    "state",   // Current state name
    "now",     // Current timestamp
    "config",  // Trait configuration
    "trait",   // Trait context
    "user",    // User context
];

/// Regex for valid binding format
static BINDING_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Support Unicode characters (e.g. Arabic, Cyrillic, etc.) in binding names
    // Structure: @root.field.nested or @root[index]
    Regex::new(r"^@[\p{L}_][\p{L}0-9_]*(\.[\p{L}0-9_]+|\[\d+\])*$").unwrap()
});

/// Regex for PascalCase entity names
static PASCAL_CASE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Binding context for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingContext {
    /// In a transition effect
    Effect,
    /// In a guard condition
    Guard,
    /// In a tick effect
    Tick,
    /// In a render_ui pattern prop
    RenderUI,
}

/// Validate all bindings in a schema
pub fn validate_all_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Collect trait names per orbital: `@trait.X` is orbital-scoped, so each
    // orbital gets its own known-trait set. Native inline traits and
    // reference-form traits both expose a local trait name via `trait_name()`
    // on the TraitReference enum.
    let orbital_trait_names: Vec<HashSet<String>> = schema
        .orbitals
        .iter()
        .map(|orbital| {
            orbital
                .traits
                .iter()
                .filter_map(|t| t.trait_name().map(|s| s.to_string()))
                .collect()
        })
        .collect();

    // Validate bindings in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let known_traits = &orbital_trait_names[i];
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_trait_bindings(
                    trait_def,
                    &path,
                    &ctx.entity_names,
                    &ctx.entity_definitions,
                );
                ctx.merge(result);

                // `@trait.X` bindings (client-only, render-time) need their
                // own pass with orbital-scoped context: known traits in this
                // orbital + the current trait's own name (to catch self-ref).
                let trait_result = validate_trait_binding_usage(
                    trait_def,
                    &path,
                    known_traits,
                );
                ctx.merge(trait_result);
            }
        }

        // Detect cycles in the orbital's `@trait.X` reference graph after
        // all per-trait validation so the error is reported once per orbital
        // rather than once per cycle participant.
        let cycle_result = detect_trait_binding_cycles(
            &orbital.traits,
            &format!("orbitals[{}]", i),
        );
        ctx.merge(cycle_result);
    }
}

/// Validate all bindings in a trait
pub fn validate_trait_bindings(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Resolve the trait's linked entity for @entity.field validation
    let linked_entity_def = trait_def
        .linked_entity
        .as_ref()
        .and_then(|name| entity_definitions.get(name));

    // Build event -> payload fields map
    let event_payloads: HashMap<&str, &[PayloadField]> = trait_def
        .state_machine
        .events
        .as_ref()
        .map(|events| {
            events
                .iter()
                .filter_map(|e| e.payload_schema.as_ref().map(|p| (e.key.as_str(), p.as_slice())))
                .collect()
        })
        .unwrap_or_default();

    // Validate bindings in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        // Get payload fields for this transition's event
        let payload_fields = event_payloads.get(transition.event.as_str()).copied();

        // Validate guard bindings
        if let Some(guard) = &transition.guard {
            result.merge(validate_expr_bindings(
                guard,
                &format!("{}.guard", trans_path),
                BindingContext::Guard,
                known_entities,
            ));
            // Validate payload field references in guards
            result.merge(validate_payload_field_references(
                guard,
                &format!("{}.guard", trans_path),
                payload_fields,
                &transition.event,
            ));
            // Validate entity field references in guards
            result.merge(validate_entity_field_references(
                guard,
                &format!("{}.guard", trans_path),
                linked_entity_def,
                entity_definitions,
            ));
        }

        // Validate effect bindings
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path = format!("{}.effects[{}]", trans_path, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &effect_path,
                    BindingContext::Effect,
                    known_entities,
                ));
                // Validate payload field references in effects
                result.merge(validate_payload_field_references(
                    effect,
                    &effect_path,
                    payload_fields,
                    &transition.event,
                ));
                // Validate entity field references in effects
                result.merge(validate_entity_field_references(
                    effect,
                    &effect_path,
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
    }

    // Validate bindings in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let tick_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &tick_path,
                    BindingContext::Tick,
                    known_entities,
                ));
                // Validate entity field references in tick effects
                result.merge(validate_entity_field_references(
                    effect,
                    &tick_path,
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
    }

    // Explicit-binding rule: every @entity.<field> read must be backed by
    // an explicit (set @entity.<field> ...) somewhere in the trait. Skipped
    // when the trait has no linkedEntity. `@intrinsic` fields on the resolved
    // linked entity are exempt (runtime-managed widget state) — keyed on the
    // resolved entity so the exemption survives a `linkedEntity` rebind.
    let intrinsic_fields: HashSet<String> = linked_entity_def
        .map(|e| {
            e.fields
                .iter()
                .filter(|f| f.intrinsic == Some(true))
                .filter_map(|f| f.name.clone())
                .collect()
        })
        .unwrap_or_default();
    result.merge(validate_entity_fields_have_setters(trait_def, path, &intrinsic_fields));

    result
}

/// Validate bindings in an expression
fn validate_expr_bindings(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    validate_expr_bindings_with_locals(expr, path, context, known_entities, &HashSet::new())
}

/// Validate bindings in an expression, with lambda-local variable tracking.
/// `lambda_locals` contains parameter names bound by enclosing lambdas (e.g., "item", "f", "o").
/// Bindings like `@item.field` are valid when "item" is in `lambda_locals`.
fn validate_expr_bindings_with_locals(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with('@') => {
            // Check if this binding root is a lambda-local variable
            let root = s[1..].split('.').next().unwrap_or("");
            let root = root.split('[').next().unwrap_or(root);
            if lambda_locals.contains(root) {
                // Valid lambda-bound variable — skip standard binding validation
            } else {
                result.merge(validate_binding(s, path, context, known_entities));
            }
        }
        SExpression::Array(arr) => {
            // Detect lambda expressions: ["lambda", [params...], body]
            if arr.len() == 3 {
                if let SExpression::String(op) = &arr[0] {
                    if op == "lambda" || op == "fn" {
                        if let SExpression::Array(params) = &arr[1] {
                            // Extract lambda parameter names and add to locals
                            // Format: ["fn", ["param1", "param2"], body]
                            let mut extended_locals = lambda_locals.clone();
                            for param in params {
                                if let SExpression::String(name) = param {
                                    extended_locals.insert(name.clone());
                                }
                            }
                            // Validate the lambda body with extended locals
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                known_entities,
                                &extended_locals,
                            );
                        } else if let SExpression::String(param_name) = &arr[1] {
                            // Single string parameter shorthand
                            // Format: ["fn", "param", body]
                            let mut extended_locals = lambda_locals.clone();
                            extended_locals.insert(param_name.clone());
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                known_entities,
                                &extended_locals,
                            );
                        }
                    }
                }
            }
            // Detect collection operators (map, filter, find, etc.) that introduce
            // implicit @item/@index lambda scope on their body argument (args[2+]).
            // Uses the canonical operator registry to identify collection/array operators
            // that accept a callback (arity >= 2).
            if arr.len() >= 3 {
                if let SExpression::String(op) = &arr[0] {
                    let is_collection_op = is_lambda_scope_operator(op);
                    if is_collection_op {
                        // Validate the source (args[1]) without implicit locals
                        result.merge(validate_expr_bindings_with_locals(
                            &arr[1],
                            &format!("{}[1]", path),
                            context,
                            known_entities,
                            lambda_locals,
                        ));
                        // Validate the body (args[2+]) with @item/@index in scope
                        let mut extended_locals = lambda_locals.clone();
                        extended_locals.insert("item".to_string());
                        extended_locals.insert("index".to_string());
                        for (i, item) in arr[2..].iter().enumerate() {
                            result.merge(validate_expr_bindings_with_locals(
                                item,
                                &format!("{}[{}]", path, i + 2),
                                context,
                                known_entities,
                                &extended_locals,
                            ));
                        }
                        // Validate the operator name itself (skip, it's not a binding)
                        return result;
                    }
                }
            }
            // Normal array — recurse into all elements
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_expr_bindings_with_locals(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    known_entities,
                    lambda_locals,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_expr_bindings_with_locals(
                    value,
                    &format!("{}.{}", path, key),
                    context,
                    known_entities,
                    lambda_locals,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate a single binding
pub fn validate_binding(
    binding: &str,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check binding format
    if !BINDING_REGEX.is_match(binding) {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidFormat,
                path,
                format!("Invalid binding format: '{}'", binding),
            )
            .with_suggestion("Use @root.field.nested format"),
        );
        return result;
    }

    // Parse binding parts
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() {
        return result;
    }

    let root = parts[0];
    // Handle array index in first part (e.g., @items[0])
    let root = root.split('[').next().unwrap_or(root);

    // Validate binding root
    if CORE_BINDING_ROOTS.contains(&root) {
        // Core binding
        match root {
            "payload" if context == BindingContext::Tick => {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingPayloadInTick,
                        path,
                        "@payload is not available in tick context",
                    )
                    .with_suggestion(
                        "Tick effects don't have event payloads - use @entity instead",
                    ),
                );
            }
            "state" if parts.len() > 1 => {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::BindingStateNoPath,
                        path,
                        "@state should not have a path - it returns the state name directly",
                    )
                    .with_suggestion("Use @state without a path"),
                );
            }
            "now" if parts.len() > 1 => {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::BindingNowNoPath,
                        path,
                        "@now should not have a path - it returns the current timestamp directly",
                    )
                    .with_suggestion("Use @now without a path"),
                );
            }
            _ => {}
        }
    } else if PASCAL_CASE_REGEX.is_match(root) {
        // Entity binding (e.g., @Task, @User)
        if !known_entities.is_empty() && !known_entities.contains(root) {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::BindingUnknownEntity,
                    path,
                    format!("Entity '{}' in binding may not exist", root),
                )
                .with_suggestion("Ensure the entity is defined in the schema")
                .with_valid_values(known_entities.iter().cloned().collect()),
            );
        }
    } else {
        result.add_error(
            ValidationError::warning(
                ErrorCode::BindingUnknownRoot,
                path,
                format!("Unknown binding root: '@{}'", root),
            )
            .with_suggestion("Use @entity, @payload, @state, @now, @config, or @EntityName"),
        );
    }

    result
}

/// Validate a set effect target binding
pub fn validate_set_target(binding: &str, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !binding.starts_with('@') {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidFormat,
                path,
                format!(
                    "set target must be a binding starting with @: '{}'",
                    binding
                ),
            )
            .with_suggestion("Use @entity.field or @EntityName.field"),
        );
        return result;
    }

    // Parse binding
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() {
        return result;
    }

    let root = parts[0];

    // Cannot set @payload (read-only)
    if root == "payload" {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectSetPayloadNotAllowed,
                path,
                "@payload is read-only and cannot be a set target",
            )
            .with_suggestion("Use @entity to update entity state"),
        );
    }

    // Cannot set @state or @now directly
    if root == "state" || root == "now" {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidPath,
                path,
                format!("@{} cannot be a set target", root),
            )
            .with_suggestion("Use @entity.field to update entity data"),
        );
    }

    // Must have a field path (can't set root directly)
    if parts.len() == 1 && (root == "entity" || root == "config") {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingSetTargetMissingPath,
                path,
                format!("set target @{} requires a field path", root),
            )
            .with_suggestion(format!("Use @{}.fieldName", root)),
        );
    }

    result
}

/// Validate that @payload.field references are declared in the event's payload schema
fn validate_payload_field_references(
    expr: &SExpression,
    path: &str,
    payload_fields: Option<&[PayloadField]>,
    event_name: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with("@payload.") => {
            // Extract the field name from @payload.fieldName or @payload.fieldName.nested
            let binding_path = &s[9..]; // Skip "@payload."
            let field_name = binding_path.split('.').next().unwrap_or("");

            if field_name.is_empty() {
                return result;
            }

            match payload_fields {
                Some(fields) => {
                    // Check if the field is declared
                    let field_exists = fields.iter().any(|f| f.name == field_name);
                    if !field_exists {
                        let declared_fields: Vec<&str> =
                            fields.iter().map(|f| f.name.as_str()).collect();
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::BindingPayloadFieldUndeclared,
                                path,
                                format!(
                                    "Payload field '{}' is not declared in event '{}' payload schema",
                                    field_name, event_name
                                ),
                            )
                            .with_suggestion(format!(
                                "Add {{ \"name\": \"{}\", \"type\": \"...\" }} to the event's payload array",
                                field_name
                            ))
                            .with_valid_values(declared_fields.iter().map(|s| s.to_string()).collect()),
                        );
                    }
                }
                None => {
                    // No payload schema defined for this event
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingPayloadFieldUndeclared,
                            path,
                            format!(
                                "Event '{}' uses @payload.{} but has no payload schema defined",
                                event_name, field_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Add a payload schema to the '{}' event definition: {{ \"key\": \"{}\", \"name\": \"...\", \"payload\": [{{ \"name\": \"{}\", \"type\": \"...\" }}] }}",
                            event_name, event_name, field_name
                        )),
                    );
                }
            }
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_payload_field_references(
                    item,
                    &format!("{}[{}]", path, i),
                    payload_fields,
                    event_name,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_payload_field_references(
                    value,
                    &format!("{}.{}", path, key),
                    payload_fields,
                    event_name,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate that @entity.field references exist on the linked entity definition.
/// Also validates @EntityName.field references against the named entity.
fn validate_entity_field_references(
    expr: &SExpression,
    path: &str,
    linked_entity_def: Option<&EntityDefinition>,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with("@entity.") => {
            // Extract the first field name from @entity.fieldName or @entity.fieldName.nested
            let binding_path = &s[8..]; // Skip "@entity."
            let field_name = binding_path.split('.').next().unwrap_or("");
            // Strip array index (e.g., "items[0]" -> "items")
            let field_name = field_name.split('[').next().unwrap_or(field_name);

            if field_name.is_empty() {
                return result;
            }

            // Skip common meta-fields that exist at runtime but not in schema
            if field_name == "id" || field_name == "createdAt" || field_name == "updatedAt" {
                return result;
            }

            if let Some(entity_def) = linked_entity_def {
                let field_exists = entity_def
                    .fields
                    .iter()
                    .any(|f| f.name.as_ref().map(|n| n == field_name).unwrap_or(false));

                if !field_exists {
                    let available_fields: Vec<String> = entity_def
                        .fields
                        .iter()
                        .filter_map(|f| f.name.clone())
                        .collect();
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingEntityFieldNotFound,
                            path,
                            format!(
                                "@entity.{} — field '{}' not found on entity '{}'",
                                field_name, field_name, entity_def.name
                            ),
                        )
                        .with_suggestion(format!(
                            "Add field '{}' to entity '{}' or fix the binding path",
                            field_name, entity_def.name
                        ))
                        .with_valid_values(available_fields),
                    );
                }
            }
        }
        SExpression::String(s) if s.starts_with('@') => {
            // Check @EntityName.field bindings
            let without_at = &s[1..];
            let parts: Vec<&str> = without_at.split('.').collect();
            if parts.len() >= 2 {
                let root = parts[0].split('[').next().unwrap_or(parts[0]);
                if PASCAL_CASE_REGEX.is_match(root) {
                    if let Some(entity_def) = entity_definitions.get(root) {
                        let field_name = parts[1].split('[').next().unwrap_or(parts[1]);
                        // Skip common meta-fields
                        if field_name != "id"
                            && field_name != "createdAt"
                            && field_name != "updatedAt"
                        {
                            let field_exists = entity_def
                                .fields
                                .iter()
                                .any(|f| f.name.as_ref().map(|n| n == field_name).unwrap_or(false));
                            if !field_exists {
                                let available_fields: Vec<String> = entity_def
                                    .fields
                                    .iter()
                                    .filter_map(|f| f.name.clone())
                                    .collect();
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::BindingEntityFieldNotFound,
                                        path,
                                        format!(
                                            "@{}.{} — field '{}' not found on entity '{}'",
                                            root, field_name, field_name, root
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Add field '{}' to entity '{}' or fix the binding path",
                                        field_name, root
                                    ))
                                    .with_valid_values(available_fields),
                                );
                            }
                        }
                    }
                }
            }
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_entity_field_references(
                    item,
                    &format!("{}[{}]", path, i),
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_entity_field_references(
                    value,
                    &format!("{}.{}", path, key),
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Walk an SExpression and collect every `@entity.<field>` reference into
/// `referenced_fields`, plus every `(set @entity.<field> ...)` setter call
/// into `set_fields`. The path arg is unused at the leaf — kept to mirror
/// other walkers in this file. Recursive over arrays + object values.
fn collect_entity_field_refs(
    expr: &SExpression,
    referenced_fields: &mut HashMap<String, String>,
    set_fields: &mut HashSet<String>,
    transition_event: &str,
) {
    match expr {
        SExpression::String(s) if s.starts_with("@entity.") => {
            let binding_path = &s[8..];
            let field_name = binding_path.split('.').next().unwrap_or("");
            let field_name = field_name.split('[').next().unwrap_or(field_name);
            if !field_name.is_empty()
                && field_name != "id"
                && field_name != "createdAt"
                && field_name != "updatedAt"
            {
                referenced_fields
                    .entry(field_name.to_string())
                    .or_insert_with(|| transition_event.to_string());
            }
        }
        SExpression::Array(arr) => {
            // Detect `["set", "@entity.<field>", ...]` setter form. The 4-elem
            // form `["set", entityId, field, value]` (back-compat) doesn't
            // bind through `@entity.path`, so the field-presence check still
            // catches it conservatively as "no path-form set".
            if arr.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(path)) = (&arr[0], &arr[1]) {
                    if op == "set" && path.starts_with("@entity.") {
                        let setter_path = &path[8..];
                        let setter_field = setter_path.split('.').next().unwrap_or("");
                        let setter_field = setter_field.split('[').next().unwrap_or(setter_field);
                        if !setter_field.is_empty() {
                            set_fields.insert(setter_field.to_string());
                        }
                    }
                }
            }
            for item in arr {
                collect_entity_field_refs(item, referenced_fields, set_fields, transition_event);
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                collect_entity_field_refs(value, referenced_fields, set_fields, transition_event);
            }
        }
        _ => {}
    }
}

/// Validate that every `@entity.<field>` referenced in a trait's
/// transitions/guards/effects/ticks has at least one explicit
/// `(set @entity.<field> <value>)` somewhere in the trait.
///
/// Per the explicit-binding rule, `@entity.X` is populated EXCLUSIVELY by
/// an explicit `(set @entity.X Y)` effect. If the schema author binds
/// `@entity.X` in render-ui / a guard / an effect but no transition runs
/// `(set)` for that field, the value is undefined at runtime and the
/// component renders empty. This validator surfaces that defect at parse
/// time so authors see a clear error instead of debugging an empty UI.
///
/// `@intrinsic` fields are exempt: they are runtime-managed widget state the
/// atom computes itself (e.g. a calendar's `selectedRange`), never authored
/// data, so a read with no `(set)` is correct, not a defect. The exemption is
/// keyed on the resolved linked entity's `intrinsic` flag, so it holds through a
/// `linkedEntity` rebind (the intrinsic field rides along via `extends`) — not
/// only for a directly-declared inline entity.
///
/// Skipped when the trait has no `linkedEntity` (no entity to bind to).
fn validate_entity_fields_have_setters(
    trait_def: &TraitDefinition,
    path: &str,
    intrinsic_fields: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if trait_def.linked_entity.is_none() {
        return result;
    }
    let trait_linked_entity = trait_def
        .linked_entity
        .clone()
        .unwrap_or_else(|| "?".to_string());

    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();

    for transition in trait_def.state_machine.transitions.iter() {
        let event = transition.event.as_str();
        if let Some(guard) = &transition.guard {
            collect_entity_field_refs(guard, &mut referenced_fields, &mut set_fields, event);
        }
        if let Some(effects) = &transition.effects {
            for effect in effects {
                collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, event);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, "TICK");
            }
        }
    }

    for (field, first_event) in referenced_fields.iter() {
        if intrinsic_fields.contains(field) {
            continue;
        }
        if !set_fields.contains(field) {
            // Staged as a warning during the rollout so existing atoms in
            // `@almadar/std` keep loading. Graduates to an error after the
            // L4 std audit completes (every render-ui that binds `@entity.X`
            // gets a corresponding `(set @entity.X <value>)` in INIT or a
            // success-listener transition). The L1 lolo
            // `LowerError::EntityFieldNeverSet` is already a hard error for
            // new authoring.
            result.add_error(
                ValidationError::phase_f_warning(
                    ErrorCode::BindingEntityFieldNeverSet,
                    path,
                    format!(
                        "trait {:?} reads @entity.{} (first seen on event {:?}) but no \
                         transition runs (set @entity.{} <value>); per the explicit-binding \
                         rule, every @entity.X must be set explicitly or it stays undefined",
                        trait_def.name, field, first_event, field
                    ),
                )
                .with_suggestion(format!(
                    "Add (set @entity.{} <default-or-payload>) in the trait's INIT \
                     transition or in a transition that runs before the read on entity {:?}",
                    field, trait_linked_entity
                )),
            );
        }
    }

    result
}

/// Check if an operator introduces an implicit @item/@index lambda scope.
/// Uses the embedded operator registry (Phase 3 of Lolo v3): collection and
/// std-array operators with arity >= 2 (i.e., they accept a callback/predicate argument).
/// Also handles unnamespaced aliases (e.g., "map" checks "array/map" too).
fn is_lambda_scope_operator(op: &str) -> bool {
    // Replaced canonical_operators lookup with embedded::get_operator() in Phase 3 of Lolo v3 plan.
    if let Some(meta) = crate::embedded::get_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Legacy fallback for operators not yet in operators.json
    if let Some(meta) = get_canonical_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Check with array/ prefix for unnamespaced names (e.g., "map" -> "array/map")
    if !op.contains('/') {
        let namespaced = format!("array/{}", op);
        if let Some(meta) = crate::embedded::get_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
        if let Some(meta) = get_canonical_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
    }
    false
}

// ============================================================================
// `@trait.X` binding validation (client-only render-time binding)
// ============================================================================

/// Run a dedicated pass over a trait's expressions looking for `@trait.X`
/// bindings and validating each against the orbital-scoped trait registry.
///
/// The binding is strictly single-segment — `@trait.X`. Any additional
/// segment (historically a `.slot` suffix) is rejected as invalid format.
///
/// Checks:
/// - `@trait` bare (no name) → [`ErrorCode::BindingTraitMissingName`].
/// - `@trait.foo` (non-PascalCase) → [`ErrorCode::BindingTraitInvalidFormat`].
/// - `@trait.Foo.anything` (extra segment) → [`ErrorCode::BindingTraitInvalidFormat`].
/// - `@trait.Foo` where `Foo` is not in `known_traits` →
///   [`ErrorCode::BindingTraitUnknown`].
/// - `@trait.<self>` (current trait references itself) →
///   [`ErrorCode::BindingTraitSelfReference`].
/// - `@trait.Foo.<slot>` where `<slot>` is not a known UI slot →
///   [`ErrorCode::BindingTraitInvalidSlot`].
pub fn validate_trait_binding_usage(
    trait_def: &TraitDefinition,
    path: &str,
    known_traits: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Walk transitions (guards + effects) and ticks, collecting every
    // `@trait.*` binding string. Deep-walk into object/array SExpressions so
    // bindings embedded in pattern trees are picked up.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);
        if let Some(guard) = &transition.guard {
            collect_and_validate_trait_bindings(
                guard,
                &format!("{}.guard", trans_path),
                &trait_def.name,
                known_traits,
                &mut result,
            );
        }
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                collect_and_validate_trait_bindings(
                    effect,
                    &format!("{}.effects[{}]", trans_path, j),
                    &trait_def.name,
                    known_traits,
                    &mut result,
                );
            }
        }
    }

    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                collect_and_validate_trait_bindings(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, j),
                    &trait_def.name,
                    known_traits,
                    &mut result,
                );
            }
        }
    }

    result
}

/// Walk an SExpression tree and validate every `@trait.*` string binding.
/// Non-binding strings and non-`@trait` bindings are skipped (they're
/// handled by the general `validate_binding` pass).
fn collect_and_validate_trait_bindings(
    expr: &SExpression,
    path: &str,
    current_trait_name: &str,
    known_traits: &HashSet<String>,
    result: &mut ValidationResult,
) {
    match expr {
        SExpression::String(s) if s.starts_with("@trait") && s.len() >= 6 => {
            // Accept the literal `@trait` or `@trait.X[.slot]`; other strings
            // starting with `@trait...` that don't match the binding regex
            // fall through to the format error below.
            result.merge(validate_trait_binding(
                s,
                path,
                current_trait_name,
                known_traits,
            ));
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                collect_and_validate_trait_bindings(
                    item,
                    &format!("{}[{}]", path, i),
                    current_trait_name,
                    known_traits,
                    result,
                );
            }
        }
        SExpression::Object(obj) => {
            for (k, v) in obj {
                collect_and_validate_trait_bindings(
                    v,
                    &format!("{}.{}", path, k),
                    current_trait_name,
                    known_traits,
                    result,
                );
            }
        }
        _ => {}
    }
}

/// Validate a single `@trait.X[.slot]` binding string.
fn validate_trait_binding(
    binding: &str,
    path: &str,
    current_trait_name: &str,
    known_traits: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !BINDING_REGEX.is_match(binding) {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitInvalidFormat,
            path,
            format!("Invalid @trait binding format: '{}'", binding),
        )
        .with_suggestion("Use @trait.<PascalCaseTraitName>[.<slot>]"));
        return result;
    }

    // strip leading `@` then split on `.`
    let parts: Vec<&str> = binding[1..].split('.').collect();

    // parts[0] must be "trait" — validator is only called for those.
    if parts.len() == 1 {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitMissingName,
            path,
            "@trait binding missing trait name",
        )
        .with_suggestion("Use @trait.<TraitName> to reference another trait's current frame"));
        return result;
    }

    let trait_name = parts[1];

    if !PASCAL_CASE_REGEX.is_match(trait_name) {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitInvalidFormat,
            path,
            format!(
                "@trait binding trait name '{}' is not PascalCase",
                trait_name
            ),
        )
        .with_suggestion("Trait names must be PascalCase (e.g., @trait.ItemSearch)"));
        return result;
    }

    if trait_name == current_trait_name {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitSelfReference,
            path,
            format!("Trait '{}' references itself via @trait", trait_name),
        )
        .with_suggestion(
            "A trait's render-ui cannot embed its own frame; restructure to avoid self-reference",
        ));
        return result;
    }

    if !known_traits.is_empty() && !known_traits.contains(trait_name) {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitUnknown,
            path,
            format!(
                "@trait.{} references a trait not declared in this orbital",
                trait_name
            ),
        )
        .with_suggestion("Declare the trait in the orbital, or check spelling")
        .with_valid_values(known_traits.iter().cloned().collect()));
        return result;
    }

    // The binding is single-segment — `@trait.<Name>`. Any additional
    // segment (historically a `.slot` suffix) is rejected here. The
    // decision to drop slot-specific routing is documented in
    // `docs/Almadar_Std_Gaps.md` §3.8 — cross-slot pulling would force
    // compiled apps to extract per-slot pattern trees, lift hooks into
    // a shared context, and rewrite event dispatch, for a niche case.
    if parts.len() > 2 {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitInvalidFormat,
            path,
            format!(
                "@trait binding has too many segments: '{}' (expected @trait.<Name>)",
                binding
            ),
        ));
    }

    result
}

// ============================================================================
// Cycle detection in the `@trait.X` reference graph
// ============================================================================

/// Detect cycles among `@trait.X` references within a single orbital. Each
/// trait's render-ui effects are scanned for outgoing references; a DFS
/// cycle detector reports the first cycle found. Scope is per-orbital —
/// cross-orbital references are not currently supported by the binding.
pub fn detect_trait_binding_cycles(
    traits: &[TraitReference],
    orbital_path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build adjacency list: trait name -> set of trait names referenced via
    // `@trait.X` inside that trait's effects.
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    let mut trait_indices: HashMap<String, usize> = HashMap::new();
    for (i, t) in traits.iter().enumerate() {
        if let TraitReference::Inline(def) = t {
            adjacency.insert(def.name.clone(), HashSet::new());
            trait_indices.insert(def.name.clone(), i);
        }
    }

    for t in traits.iter() {
        if let TraitReference::Inline(def) = t {
            let mut refs: HashSet<String> = HashSet::new();
            for transition in &def.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        collect_trait_refs(effect, &mut refs);
                    }
                }
            }
            if let Some(ticks) = &def.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        collect_trait_refs(effect, &mut refs);
                    }
                }
            }
            if let Some(entry) = adjacency.get_mut(&def.name) {
                *entry = refs;
            }
        }
    }

    // DFS cycle detection. White=unvisited, Gray=on-stack, Black=finished.
    #[derive(PartialEq, Copy, Clone)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color: HashMap<&str, Color> = adjacency
        .keys()
        .map(|k| (k.as_str(), Color::White))
        .collect();
    let mut parent: HashMap<&str, &str> = HashMap::new();

    let nodes: Vec<&str> = adjacency.keys().map(|k| k.as_str()).collect();

    for start in nodes {
        if color.get(start).copied() != Some(Color::White) {
            continue;
        }
        let mut stack: Vec<(&str, std::collections::hash_set::Iter<String>)> =
            vec![(start, adjacency[start].iter())];
        color.insert(start, Color::Gray);

        while let Some((node, mut children_iter)) = stack.pop() {
            if let Some(child) = children_iter.next() {
                // push the iterator back before descending so we can resume
                stack.push((node, children_iter));
                let child_str = child.as_str();
                match color.get(child_str).copied() {
                    Some(Color::White) => {
                        parent.insert(child_str, node);
                        color.insert(child_str, Color::Gray);
                        if let Some(edges) = adjacency.get(child_str) {
                            stack.push((child_str, edges.iter()));
                        }
                    }
                    Some(Color::Gray) => {
                        // Cycle found — reconstruct path child → ... → child.
                        let mut cycle_path: Vec<&str> = vec![child_str];
                        let mut cur = node;
                        while cur != child_str {
                            cycle_path.push(cur);
                            match parent.get(cur).copied() {
                                Some(p) => cur = p,
                                None => break,
                            }
                        }
                        cycle_path.push(child_str);
                        cycle_path.reverse();
                        let idx = trait_indices
                            .get(child_str)
                            .copied()
                            .unwrap_or(0);
                        result.add_error(ValidationError::error(
                            ErrorCode::BindingTraitCycle,
                            format!("{}.traits[{}]", orbital_path, idx),
                            format!(
                                "@trait reference cycle detected: {}",
                                cycle_path.join(" -> ")
                            ),
                        )
                        .with_suggestion(
                            "Break the cycle by removing one of the @trait.X references",
                        ));
                        return result;
                    }
                    _ => {}
                }
            } else {
                color.insert(node, Color::Black);
            }
        }
    }

    result
}

/// Recursively collect trait names from every `@trait.X` string appearing
/// anywhere in an SExpression tree.
fn collect_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) if s.starts_with("@trait.") => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                if let Some(name) = rest.split('.').next() {
                    if !name.is_empty() {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        SExpression::Array(arr) => {
            for item in arr {
                collect_trait_refs(item, out);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                collect_trait_refs(v, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_bindings() {
        let known_entities: HashSet<String> =
            ["Task", "User"].iter().map(|s| s.to_string()).collect();

        let result = validate_binding(
            "@entity.title",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@Task.title",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn test_payload_in_tick() {
        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Tick,
            &HashSet::new(),
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn test_state_with_path() {
        let result = validate_binding(
            "@state.name",
            "test",
            BindingContext::Effect,
            &HashSet::new(),
        );
        // Warning, not error
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingStateNoPath));
    }

    #[test]
    fn test_unknown_entity() {
        let known_entities: HashSet<String> = ["Task"].iter().map(|s| s.to_string()).collect();
        let result = validate_binding(
            "@NonExistent.field",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingUnknownEntity));
    }

    #[test]
    fn test_set_target_payload() {
        let result = validate_set_target("@payload.field", "test");
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_set_target_missing_path() {
        let result = validate_set_target("@entity", "test");
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingSetTargetMissingPath));
    }

    #[test]
    fn test_payload_field_undeclared_no_schema() {
        let expr = SExpression::String("@payload.rawData".to_string());
        let result = validate_payload_field_references(&expr, "test", None, "READ");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared));
    }

    #[test]
    fn test_payload_field_undeclared_missing_field() {
        let expr = SExpression::String("@payload.unknownField".to_string());
        let fields = vec![PayloadField {
            name: "rawData".to_string(),
            field_type: "array".to_string(),
            required: Some(true),
            description: None,
            properties: None,
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "READ");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared));
    }

    #[test]
    fn test_payload_field_declared() {
        let expr = SExpression::String("@payload.rawData".to_string());
        let fields = vec![PayloadField {
            name: "rawData".to_string(),
            field_type: "array".to_string(),
            required: Some(true),
            description: None,
            properties: None,
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "READ");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_lambda_bound_item_is_valid() {
        let known_entities: HashSet<String> = HashSet::new();
        // ["lambda", ["item"], ["=", "@item.status", "pending"]]
        let expr = SExpression::Array(vec![
            SExpression::String("lambda".to_string()),
            SExpression::Array(vec![SExpression::String("item".to_string())]),
            SExpression::Array(vec![
                SExpression::String("=".to_string()),
                SExpression::String("@item.status".to_string()),
                SExpression::String("pending".to_string()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        // @item inside lambda should NOT produce BindingUnknownRoot
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @item should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_fn_string_param_lambda_is_valid() {
        let known_entities: HashSet<String> = HashSet::new();
        // ["fn", "h", ["=", "@h.owner", "player"]]
        // String param shorthand (not wrapped in array)
        let expr = SExpression::Array(vec![
            SExpression::String("fn".to_string()),
            SExpression::String("h".to_string()),
            SExpression::Array(vec![
                SExpression::String("=".to_string()),
                SExpression::String("@h.owner".to_string()),
                SExpression::String("player".to_string()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @h with string param should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_entity_field_not_found() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            name: "Task".to_string(),
            fields: vec![
                FieldDefinition {
                    name: Some("title".to_string()),
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
                },
                FieldDefinition {
                    name: Some("status".to_string()),
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
                },
            ],
            collection: None,
            persistence: None,
            instances: None,
        };

        let entity_defs = HashMap::new();

        // @entity.title should pass (field exists)
        let expr = SExpression::String("@entity.title".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(
            result.errors.is_empty(),
            "Valid field should not produce warnings: {:?}",
            result.errors
        );

        // @entity.nonExistent should error
        let expr = SExpression::String("@entity.nonExistent".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNotFound),
            "Missing field should produce BindingEntityFieldNotFound error"
        );
    }

    #[test]
    fn test_entity_field_id_skipped() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            name: "Task".to_string(),
            fields: vec![FieldDefinition {
                name: Some("title".to_string()),
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
            }],
            collection: None,
            persistence: None,
            instances: None,
        };

        let entity_defs = HashMap::new();

        // @entity.id should pass (meta-field, always present at runtime)
        let expr = SExpression::String("@entity.id".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(result.errors.is_empty(), "id should be skipped");
    }

    #[test]
    fn test_named_entity_field_validation() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            name: "User".to_string(),
            fields: vec![FieldDefinition {
                name: Some("email".to_string()),
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
            }],
            collection: None,
            persistence: None,
            instances: None,
        };

        let mut entity_defs = HashMap::new();
        entity_defs.insert("User".to_string(), entity_def);

        // @User.email should pass
        let expr = SExpression::String("@User.email".to_string());
        let result = validate_entity_field_references(&expr, "test", None, &entity_defs);
        assert!(result.errors.is_empty());

        // @User.phone should error
        let expr = SExpression::String("@User.phone".to_string());
        let result = validate_entity_field_references(&expr, "test", None, &entity_defs);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingEntityFieldNotFound),);
    }

    #[test]
    fn test_non_lambda_item_is_invalid() {
        let known_entities: HashSet<String> = HashSet::new();
        // @item.status outside a lambda should still warn
        let expr = SExpression::String("@item.status".to_string());
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Non-lambda @item should trigger BindingUnknownRoot"
        );
    }

    // ==========================================================================
    // Explicit-binding rule tests (BindingEntityFieldNeverSet)
    // ==========================================================================

    /// Build a minimal trait fixture for the entity-fields-have-setters tests.
    /// Each transition is `(idle -> idle on EVENT, [effects])`. Linked entity
    /// is required for the validator to run.
    fn build_trait_fixture(
        name: &str,
        linked_entity: Option<&str>,
        transitions: Vec<(&str, Vec<SExpression>)>,
    ) -> orbital_core::schema::TraitDefinition {
        use orbital_core::schema::{
            StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let trans = transitions
            .into_iter()
            .map(|(event, effects)| TransitionDefinition {
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: event.to_string(),
                guard: None,
                effects: if effects.is_empty() { None } else { Some(effects) },
            })
            .collect();
        orbital_core::schema::TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: linked_entity.map(|s| s.to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                }],
                events: None,
                transitions: trans,
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        }
    }

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }
    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }

    #[test]
    fn test_entity_field_never_set_render_ui_no_setter() {
        // (render-ui main { currentPage: "@entity.currentPage" })
        // No `(set @entity.currentPage ...)` anywhere — must fail.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "BadPagination",
            Some("PagedItem"),
            vec![("INIT", vec![render_ui])],
        );
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Reading @entity.currentPage with no (set) must produce BindingEntityFieldNeverSet — got {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_intrinsic_entity_field_read_with_no_setter_is_exempt() {
        // (render-ui main { range: "@entity.selectedRange" }) with no setter —
        // would normally warn, but `selectedRange` is intrinsic (runtime-managed
        // widget state), so it must be exempt. Mirrors a calendar atom rebound
        // onto a domain entity that carries `selectedRange` as an intrinsic field.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("range".to_string(), s("@entity.selectedRange"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "CalendarView",
            Some("Appointment"),
            vec![("INIT", vec![render_ui])],
        );
        let intrinsic: HashSet<String> = ["selectedRange".to_string()].into_iter().collect();
        let result = validate_entity_fields_have_setters(&trait_def, "test", &intrinsic);
        assert!(
            result.warnings.is_empty() && result.errors.is_empty(),
            "Intrinsic @entity.selectedRange with no (set) must be exempt — got {:?}",
            result.warnings,
        );
    }

    #[test]
    fn test_entity_field_set_explicitly_passes() {
        // INIT runs (set @entity.currentPage 1) then render-ui binds it. Pass.
        let set_effect = arr(vec![s("set"), s("@entity.currentPage"), SExpression::Number(1.into())]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "GoodPagination",
            Some("PagedItem"),
            vec![("INIT", vec![set_effect, render_ui])],
        );
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result.errors.is_empty(),
            "Field with explicit (set) must pass — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_set_in_different_transition_passes() {
        // INIT (set @entity.currentPage 1); render-ui happens in PAGE transition.
        // The setter doesn't have to be in the same transition as the read.
        let set_effect = arr(vec![s("set"), s("@entity.currentPage"), SExpression::Number(1.into())]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "CrossTransitionPagination",
            Some("PagedItem"),
            vec![
                ("INIT", vec![set_effect]),
                ("PAGE", vec![render_ui]),
            ],
        );
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result.errors.is_empty(),
            "Setter in different transition still satisfies the rule — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_id_meta_skipped() {
        // @entity.id is a runtime-meta field. No (set @entity.id) needed.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("entityId".to_string(), s("@entity.id"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "MetaIdReader",
            Some("Item"),
            vec![("INIT", vec![render_ui])],
        );
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result.errors.is_empty(),
            "@entity.id is a runtime-meta field; no setter required",
        );
    }

    #[test]
    fn test_entity_field_skipped_when_no_linked_entity() {
        // Trait without linkedEntity — the rule is skipped (the @entity.* reads
        // are someone else's problem; could be a render-only trait).
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("anything".to_string(), s("@entity.anything"));
                m
            }),
        ]);
        let trait_def =
            build_trait_fixture("LinkedEntityless", None, vec![("INIT", vec![render_ui])]);
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result.errors.is_empty(),
            "Trait without linkedEntity should skip the rule",
        );
    }

    #[test]
    fn test_entity_field_set_via_payload_passes() {
        // (set @entity.currentPage @payload.page) — the value source is a
        // payload reference, not a literal, but the (set) call still counts
        // as the explicit setter.
        let set_effect = arr(vec![s("set"), s("@entity.currentPage"), s("@payload.page")]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "PayloadDrivenPagination",
            Some("PagedItem"),
            vec![("PAGE", vec![set_effect, render_ui])],
        );
        let result = validate_entity_fields_have_setters(&trait_def, "test", &HashSet::new());
        assert!(
            result.errors.is_empty(),
            "(set @entity.X @payload.X) is a valid setter — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_only_in_guard_no_setter_fails() {
        // Guard reads @entity.locked but no transition (set)s it. Must fail.
        let guard_only_trait = {
            use orbital_core::schema::{
                StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
            };
            orbital_core::schema::TraitDefinition {
                name: "GuardOnlyReader".to_string(),
                entity_rebindable: None,
                entity_contract: None,
                category: None,
                capabilities: vec![],
                description: None,
                linked_entity: Some("Item".to_string()),
                emits: None,
                listens: None,
                state_machine: StateMachine {
                    states: vec![StateDefinition {
                        name: "idle".to_string(),
                        is_initial: Some(true),
                        is_terminal: None,
                        description: None,
                    }],
                    events: None,
                    transitions: vec![TransitionDefinition {
                        from: StringOrArray::Single("idle".to_string()),
                        to: "idle".to_string(),
                        event: "TRY".to_string(),
                        guard: Some(s("@entity.locked")),
                        effects: None,
                    }],
                },
                ticks: None,
                config: None,
                data_entities: None,
                scope: None,
                source_behavior: None,
                source_entity_definition: None,
            }
        };
        let result = validate_entity_fields_have_setters(&guard_only_trait, "test", &HashSet::new());
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Guard-only @entity.X read with no setter must fail — errors: {:?}",
            result.errors,
        );
    }
}
