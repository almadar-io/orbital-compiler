//! Effect Validation
//!
//! Validates effects: call_service, set, fetch, emit, persist, etc.
//! Ported from `builder/packages/shared/src/orbitals/validation/effect-validator.ts`.

use super::{ErrorCode, PageInitPayload, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, FieldType, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Known effect operators
pub const EFFECT_OPERATORS: &[&str] = &[
    "set",
    "emit",
    "fetch", // Runtime effect (EffectExecutor handles it)
    "persist",
    "delete", // Note: typically a persist sub-action, kept for compat
    "navigate",
    "render-ui",
    "render_ui",
    "call-service",
    "call_service",
    "notify",
    "log", // Runtime effect (EffectExecutor handles it)
    "spawn",
    "destroy", // Legacy alias — canonical name is "despawn"
    "despawn", // Canonical (GAP-E001): used by TS evaluator + runtime
    "update",  // Note: typically a persist sub-action, kept for compat
    // Conditional and array mutation effects
    "if",            // Conditional effect execution
    "array/append",  // Append item to array field
    "array/prepend", // Prepend item to array field
    "array/remove",  // Remove item from array field
    "increment",     // Increment numeric field
    "decrement",     // Decrement numeric field
    // Functional array operators (registered in almadar-operators/operators.json)
    "array/filter", // Filter elements by predicate
    "array/map",    // Transform elements
    "array/reduce", // Reduce array to single value
    // GAP-V002: Control-flow wrappers that can contain effects
    "do",   // Sequential effect execution
    "let",  // Local variable binding in effects
    "when", // Conditional effect (like if but for effects)
    "fn",   // Lambda wrapping effects
    // OS trigger operators
    "os/watch-files",
    "os/watch-process",
    "os/watch-port",
    "os/watch-http",
    "os/watch-cron",
    "os/watch-signal",
    "os/watch-env",
    "os/debounce",
    // Resource operators (ref/deref/swap!/watch/atomic)
    "ref",    // Reactive entity subscription
    "deref",  // Snapshot read
    "swap!",  // Atomic CAS read-modify-write
    "watch",  // Entity change callback
    "atomic", // Transaction group
    // ML effect operators (Gap 4: forward/train/evaluate/checkpoint)
    "forward",
    "train",
    "evaluate",
    "checkpoint/save",
    "checkpoint/load",
    // Agent operators (Phase 1: agent/* namespace)
    "agent/memorize",
    "agent/forget",
    "agent/pin",
    "agent/reinforce",
    "agent/decay",
    "agent/generate",
    "agent/switch-provider",
    "agent/invoke",
    "agent/compact",
    "agent/fork",
    "agent/label",
    "agent/search-code",
    // Agent pure operators (valid in effects too, return values)
    "agent/recall",
    "agent/memories",
    "agent/memory-strength",
    "agent/is-pinned",
    "agent/provider",
    "agent/model",
    "agent/tools",
    "agent/token-count",
    "agent/context-usage",
    "agent/session-id",
];

/// Validate all effects in a schema
pub fn validate_all_effects(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate effects in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_trait_effects(
                    trait_def,
                    &path,
                    &ctx.entity_names,
                    &ctx.entity_definitions,
                    &ctx.service_names,
                    &ctx.page_init_payloads,
                    &ctx.service_actions,
                );
                ctx.merge(result);
            }
        }
    }
}

/// Validate all effects in a trait definition
pub fn validate_trait_effects(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_actions: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect declared emits for this trait
    let declared_emits = collect_declared_emits(trait_def);

    // Validate effects in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_actions,
                    Some(&declared_emits),
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    // Validate effects in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_actions,
                    Some(&declared_emits),
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    result
}

/// Information about a declared emit
#[derive(Clone)]
pub(crate) struct DeclaredEmit {
    event: String,
    is_external: bool,
    payload_fields: Vec<String>,
    required_fields: Vec<String>,
}

/// Collect all declared emits from a trait definition
fn collect_declared_emits(trait_def: &TraitDefinition) -> Vec<DeclaredEmit> {
    let mut declared = Vec::new();

    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            let payload_fields: Vec<String> = emit
                .payload_schema
                .as_ref()
                .map(|fields| fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            let required_fields: Vec<String> = emit
                .payload_schema
                .as_ref()
                .map(|fields| {
                    fields
                        .iter()
                        .filter(|f| f.required.unwrap_or(false))
                        .map(|f| f.name.clone())
                        .collect()
                })
                .unwrap_or_default();

            let is_external = emit
                .scope
                .as_ref()
                .map(|s| *s == orbital_core::schema::EventScope::External)
                .unwrap_or(false);

            declared.push(DeclaredEmit {
                event: emit.event.clone(),
                is_external,
                payload_fields,
                required_fields,
            });
        }
    }

    declared
}

/// Validate an emit effect against declared emits
fn validate_emit_against_declaration(
    effect: &SExpression,
    path: &str,
    declared_emits: &[DeclaredEmit],
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Only validate emit effects
    if let SExpression::Array(arr) = effect {
        if arr.is_empty() {
            return result;
        }
        if let SExpression::String(operator) = &arr[0] {
            if operator != "emit" {
                return result;
            }

            // Get the event name
            if arr.len() < 2 {
                return result; // Already validated in validate_emit
            }

            if let SExpression::String(event_name) = &arr[1] {
                // Check if the event is declared
                let declared = declared_emits.iter().find(|e| &e.event == event_name);

                if declared.is_none() {
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::TraitEventUndeclared,
                            path,
                            format!(
                                "Event '{}' is emitted but not declared in trait's emits array",
                                event_name
                            ),
                        )
                        .with_suggestion(
                            "Add the event to the trait's 'emits' array with proper payload schema",
                        )
                        .with_expected_shape(
                            r#"{
  "emits": [
    {
      "event": "EVENT_NAME",
      "scope": "internal" | "external",
      "description": "What this event means",
      "payload": [
        { "name": "fieldName", "type": "string", "required": true }
      ]
    }
  ]
}"#,
                        ),
                    );
                    return result;
                }

                let declared = declared.unwrap();

                // For external events, validate payload fields
                if declared.is_external && arr.len() >= 3 {
                    if let SExpression::Object(payload_obj) = &arr[2] {
                        // Check for required fields
                        for required_field in &declared.required_fields {
                            if !payload_obj.contains_key(required_field) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::CrossOrbitalPayloadMismatch,
                                        path,
                                        format!(
                                            "External emit '{}' is missing required payload field '{}'",
                                            event_name, required_field
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Add the '{}' field to the emit payload",
                                        required_field
                                    )),
                                );
                            }
                        }
                    } else if !declared.required_fields.is_empty() {
                        // Payload is missing or not an object but there are required fields
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::CrossOrbitalPayloadMismatch,
                                path,
                                format!(
                                    "External emit '{}' requires payload with fields: {:?}",
                                    event_name, declared.required_fields
                                ),
                            )
                            .with_suggestion("Add a payload object with the required fields"),
                        );
                    }
                }
            }
        }
    }

    result
}

/// Generic constraint: when the operator's canonical metadata declares
/// `requiresEmitSuccess: true`, the effect call site MUST carry a trailing
/// options object with `emit.success` set to a non-empty event key. Driven
/// entirely by `canonical-operators.json` so future "this op requires X"
/// constraints land as one-line JSON flips without new validator passes.
///
/// Iteration shape mirrors `collectServerEmittedEvents` in verify's planner:
/// scan from arg index 2 to the end, find the first object that has an
/// `emit` property, and check that property for a non-empty `success` string.
fn validate_required_emit_success(
    operator: &str,
    arr: &[SExpression],
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let meta = match crate::embedded::get_operator(operator) {
        Some(m) => m,
        None => return result,
    };
    if !meta.requires_emit_success {
        return result;
    }

    // Find the first options-object arg (anything from index 2 onward that's
    // a plain object — not an array, not null).
    let mut found_emit_success: Option<String> = None;
    let mut saw_options_object = false;
    for arg in arr.iter().skip(2) {
        let SExpression::Object(obj) = arg else { continue };
        saw_options_object = true;
        let Some(emit) = obj.get("emit") else { continue };
        let SExpression::Object(emit_obj) = emit else { continue };
        let Some(success) = emit_obj.get("success") else { continue };
        if let SExpression::String(s) = success {
            if !s.is_empty() {
                found_emit_success = Some(s.clone());
                break;
            }
        }
    }

    if found_emit_success.is_none() {
        let detail = if !saw_options_object {
            format!(
                "Operator '{}' requires a trailing options object with `emit: {{ success: \"<EVENT>\" }}`. None found.",
                operator
            )
        } else {
            format!(
                "Operator '{}' requires `emit.success` to be a non-empty event key in the trailing options object.",
                operator
            )
        };
        result.add_error(
            ValidationError::error(ErrorCode::EffectMissingRequiredEmit, path, detail)
                .with_suggestion(format!(
                    "Add `{{ emit: {{ success: \"<EVENT_KEY>\" }} }}` as the last arg of the {} effect (declare the success event in the trait's `emits` array too).",
                    operator
                )),
        );
    }

    result
}

/// Validate a single effect
pub(crate) fn validate_effect(
    effect: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_actions: &HashMap<String, HashSet<String>>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match effect {
        // S-expression array form: ["operator", ...args]
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(operator) = &arr[0] {
                // Generic constraint check driven by canonical-operators.json:
                // when an operator's `requiresEmitSuccess: true`, the call site
                // MUST carry a trailing options object with `emit.success = "X"`.
                // Future "this op requires X" constraints can be added by
                // extending `EmbeddedOperator` with new boolean flags + matching
                // checks here — no per-operator special cases needed.
                result.merge(validate_required_emit_success(operator, arr, path));

                match operator.as_str() {
                    "call-service" | "call_service" => {
                        result.merge(validate_call_service(
                            arr,
                            path,
                            known_services,
                            service_actions,
                            declared_emits,
                        ));
                    }
                    "set" => {
                        result.merge(validate_set(arr, path, declared_emits));
                    }
                    "fetch" => {
                        result.merge(validate_fetch(
                            arr,
                            path,
                            known_entities,
                            entity_definitions,
                            declared_emits,
                        ));
                    }
                    "persist" => {
                        result.merge(validate_persist(
                            arr,
                            path,
                            known_entities,
                            declared_emits,
                        ));
                    }
                    "ref" => {
                        result.merge(validate_ref(arr, path, declared_emits));
                    }
                    "emit" => {
                        result.merge(validate_emit(arr, path));
                    }
                    "render-ui" | "render_ui" => {
                        // render_ui is validated separately in render_ui.rs
                    }
                    "navigate" => {
                        result.merge(validate_navigate(arr, path, page_init_payloads));
                    }
                    os_op if os_op.starts_with("os/watch-") => {
                        result.merge(validate_os_watch(arr, path, declared_emits));
                    }
                    op if !EFFECT_OPERATORS.contains(&op) => {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::EffectUnknownType,
                                path,
                                format!("Unknown effect operator: '{}'", op),
                            )
                            .with_valid_values(
                                EFFECT_OPERATORS.iter().map(|s| s.to_string()).collect(),
                            ),
                        );
                    }
                    _ => {
                        // Known operator, no additional validation needed
                    }
                }
            }
        }
        // Object form: { set: { ... } } or { call_service: { ... } }
        SExpression::Object(obj) => {
            if let Some(set_target) = obj.get("set") {
                result.merge(validate_set_object(set_target, path));
            }
            if obj.contains_key("call_service") || obj.contains_key("call-service") {
                result.merge(validate_call_service_object(
                    obj,
                    path,
                    known_services,
                    service_actions,
                ));
            }
            if let Some(fetch_entity) = obj.get("fetch") {
                result.merge(validate_fetch_object(fetch_entity, path, known_entities));
            }
        }
        _ => {
            // Other forms are not effect-like, skip
        }
    }

    result
}

/// Validate call_service effect (array form)
fn validate_call_service(
    arr: &[SExpression],
    path: &str,
    known_services: &HashSet<String>,
    service_actions: &HashMap<String, HashSet<String>>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["call-service", "serviceName", "action", { onSuccess: [...], onError: [...] }]
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "call_service requires service name and action",
            )
            .with_suggestion(
                "Use: [\"call-service\", \"serviceName\", \"action\", { onSuccess: [...] }]",
            ),
        );
        return result;
    }

    // Check service name
    if let SExpression::String(service_name) = &arr[1] {
        if !known_services.is_empty() && !known_services.contains(service_name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectCallServiceMissingService,
                    path,
                    format!("Service '{}' is not defined", service_name),
                )
                .with_suggestion("Define the service in schema.services")
                .with_valid_values(known_services.iter().cloned().collect()),
            );
        }

        // Validate action against services registry (if loaded)
        if let Some(SExpression::String(action)) = arr.get(2) {
            if let Some(valid_actions) = service_actions.get(service_name) {
                if !valid_actions.contains(action) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EffectCallServiceMissingAction,
                            path,
                            format!(
                                "Action '{}' is not a known action on service '{}'",
                                action, service_name
                            ),
                        )
                        .with_suggestion("Check the service contract for valid action names")
                        .with_valid_values(valid_actions.iter().cloned().collect()),
                    );
                }
            }
        }
    } else {
        result.add_error(ValidationError::error(
            ErrorCode::EffectCallServiceMissingService,
            path,
            "Service name must be a string",
        ));
    }

    // Check action
    if let Some(SExpression::String(_action)) = arr.get(2) {
        // Action is present and is a string - valid
    } else if arr.len() >= 3 {
        result.add_error(ValidationError::error(
            ErrorCode::EffectCallServiceMissingAction,
            path,
            "Action must be a string",
        ));
    }

    // Optional trailing options carrying `emit:` (call-service is async one-shot).
    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Validate call_service effect (object form)
fn validate_call_service_object(
    obj: &std::collections::HashMap<String, SExpression>,
    path: &str,
    known_services: &HashSet<String>,
    service_actions: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Get the call_service value
    let call_service = obj.get("call_service").or_else(|| obj.get("call-service"));

    if let Some(SExpression::Object(cs_obj)) = call_service {
        // Check service
        if let Some(SExpression::String(service)) = cs_obj.get("service") {
            if !known_services.is_empty() && !known_services.contains(service) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectCallServiceMissingService,
                        path,
                        format!("Service '{}' is not defined", service),
                    )
                    .with_valid_values(known_services.iter().cloned().collect()),
                );
            }

            // Validate action against services registry (if loaded)
            if let Some(SExpression::String(action)) = cs_obj.get("action") {
                if let Some(valid_actions) = service_actions.get(service) {
                    if !valid_actions.contains(action) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectCallServiceMissingAction,
                                path,
                                format!(
                                    "Action '{}' is not a known action on service '{}'",
                                    action, service
                                ),
                            )
                            .with_suggestion("Check the service contract for valid action names")
                            .with_valid_values(valid_actions.iter().cloned().collect()),
                        );
                    }
                }
            }
        } else {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "call_service requires 'service' property",
            ));
        }

        // Check action
        if !cs_obj.contains_key("action") {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingAction,
                path,
                "call_service requires 'action' property",
            ));
        }
    }

    result
}

/// Validate set effect (array form)
fn validate_set(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["set", "@entity.field", value] or ["set", ["path", ...], value]
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectSetInvalidBinding,
                path,
                "set requires binding and value",
            )
            .with_suggestion("Use: [\"set\", \"@entity.field\", value]"),
        );
        return result;
    }

    // Check binding - can be string OR path expression
    match &arr[1] {
        SExpression::String(binding) => {
            if !binding.starts_with('@') {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetInvalidBinding,
                        path,
                        format!("set binding must start with @: '{}'", binding),
                    )
                    .with_suggestion("Use @entity.field, @EntityName.field, or @config.field"),
                );
            } else if binding.starts_with("@payload") {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetPayloadNotAllowed,
                        path,
                        "set cannot target @payload (payload is read-only)",
                    )
                    .with_suggestion("Use @entity to update the entity state")
                    .with_fix_guidance("Replace @payload with @entity in your set binding"),
                );
            } else if binding == "@entity" || binding == "@state" || binding == "@now" {
                // Missing field path
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingSetTargetMissingPath,
                        path,
                        format!("set binding '{}' requires a field path", binding),
                    )
                    .with_suggestion("Add a field path: @entity.fieldName"),
                );
            }
        }
        SExpression::Array(path_expr) if is_path_expression(path_expr) => {
            // ["path", "segment1", "@payload.segment2"] is valid for dynamic field paths
            // Validate that path expression has at least one segment
            if path_expr.len() < 2 {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetInvalidBinding,
                        path,
                        "path expression requires at least one segment",
                    )
                    .with_suggestion("Use: [\"path\", \"fieldName\", \"@payload.subField\"]"),
                );
            }
        }
        _ => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectSetInvalidBinding,
                    path,
                    "set binding must be a string starting with @ or a path expression",
                )
                .with_suggestion("Use: [\"set\", \"@entity.field\", value] or [\"set\", [\"path\", \"field\", \"@payload.subField\"], value]"),
            );
        }
    }

    // Optional trailing options carrying `emit:` at position 3. `set` is
    // synchronous — only `success` is meaningful (fires after the write).
    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[3] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Check if an S-expression is a path expression ["path", ...]
fn is_path_expression(arr: &[SExpression]) -> bool {
    if arr.is_empty() {
        return false;
    }
    matches!(&arr[0], SExpression::String(op) if op == "path")
}

/// Validate set effect (object form)
fn validate_set_object(target: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::String(binding) = target {
        if binding.starts_with("@payload") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectSetPayloadNotAllowed,
                    path,
                    "set cannot target @payload (payload is read-only)",
                )
                .with_suggestion("Use @entity to update the entity state"),
            );
        }
    }

    result
}

/// Validate fetch effect (array form)
fn validate_fetch(
    arr: &[SExpression],
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["fetch", "EntityName", { filter: ..., include: [...] }]
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectFetchInvalidEntity,
                path,
                "fetch requires entity name",
            )
            .with_suggestion("Use: [\"fetch\", \"EntityName\"]"),
        );
        return result;
    }

    // Check entity name
    let entity_name = if let SExpression::String(name) = &arr[1] {
        if !known_entities.is_empty() && !known_entities.contains(name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectFetchInvalidEntity,
                    path,
                    format!("Entity '{}' does not exist", name),
                )
                .with_valid_values(known_entities.iter().cloned().collect()),
            );
        }
        Some(name.clone())
    } else {
        result.add_error(ValidationError::error(
            ErrorCode::EffectFetchInvalidEntity,
            path,
            "Entity name must be a string",
        ));
        None
    };

    // Validate third argument (options or legacy positional)
    if let Some(entity_name) = &entity_name {
        if let Some(config) = arr.get(2) {
            match config {
                SExpression::Object(obj) => {
                    // Valid: { id: ..., filter: ..., include: [...] }
                    if let Some(include_expr) = obj.get("include") {
                        result.merge(validate_fetch_include(
                            include_expr,
                            path,
                            entity_name,
                            entity_definitions,
                        ));
                    }
                    // fetch is one-shot async — success + failure allowed.
                    if let Some(emit_obj) = extract_emit_object_ref(obj) {
                        result.merge(validate_emit_config(
                            emit_obj,
                            &["success", "failure"],
                            declared_emits,
                            path,
                        ));
                    }
                }
                SExpression::String(s) if s.starts_with('@') && s.contains('.') => {
                    // A raw binding like "@payload.id" as the 3rd arg is almost certainly
                    // a fetch-by-ID that should use the options object syntax.
                    // The resolve phase handles this as legacy compat, but warn the author.
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::EffectFetchInvalidEntity,
                            path,
                            format!(
                                "fetch with raw binding '{}' as third argument. Use options object for fetch-by-ID",
                                s
                            ),
                        )
                        .with_suggestion(format!(
                            "Use: [\"fetch\", \"{}\", {{ \"id\": \"{}\" }}]",
                            entity_name, s
                        )),
                    );
                }
                _ => {
                    // Other types (arrays for filter expressions, null, etc.) are valid legacy
                }
            }

            // When using options object syntax, no extra positional args allowed
            if matches!(config, SExpression::Object(_)) && arr.len() > 3 {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::EffectFetchInvalidEntity,
                        path,
                        format!(
                            "fetch has {} arguments but options object should be the last argument",
                            arr.len() - 1
                        ),
                    )
                    .with_suggestion("Put all options in the object: { \"id\": ..., \"filter\": ..., \"include\": [...] }"),
                );
            }
        }
    }

    // Legacy positional: ["fetch", Entity, filter, orderBy, limit] — max 5 elements
    if arr.len() > 5 {
        result.add_error(
            ValidationError::warning(
                ErrorCode::EffectFetchInvalidEntity,
                path,
                format!(
                    "fetch has {} arguments, maximum is 5: [\"fetch\", Entity, filter, orderBy, limit]",
                    arr.len()
                ),
            )
            .with_suggestion("Extra arguments are ignored. Use the options object syntax instead."),
        );
    }

    result
}

/// Validate fetch include option references valid relation fields
fn validate_fetch_include(
    include_expr: &SExpression,
    path: &str,
    entity_name: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Get the entity definition to check relation fields
    let entity_def = entity_definitions.get(entity_name);

    // Parse include array
    let include_fields: Vec<String> = match include_expr {
        SExpression::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                if let SExpression::String(s) = item {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect(),
        _ => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectFetchInvalidInclude,
                    path,
                    "fetch include must be an array of strings",
                )
                .with_suggestion("Use: [\"fetch\", \"Entity\", { \"include\": [\"fieldName\"] }]"),
            );
            return result;
        }
    };

    // Validate each include field is a relation field on the entity
    if let Some(entity_def) = entity_def {
        // Get all relation fields on the entity (only those with names)
        let relation_fields: Vec<String> = entity_def
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::Relation && f.name.is_some())
            .filter_map(|f| f.name.clone())
            .collect();

        // Also create a map of relation field name (without "Id" suffix) to field name
        // e.g., "companyId" -> "company" for convenience
        let relation_base_names: HashMap<String, String> = entity_def
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::Relation && f.name.is_some())
            .filter_map(|f| {
                let field_name = f.name.as_ref()?;
                // Strip "Id" suffix if present for the base name
                let base_name = field_name
                    .strip_suffix("Id")
                    .unwrap_or(field_name)
                    .to_string();
                Some((base_name, field_name.clone()))
            })
            .collect();

        for include_field in &include_fields {
            // Check if it matches a relation field directly or by base name
            let is_valid_relation = relation_fields.contains(include_field)
                || relation_base_names.contains_key(include_field);

            if !is_valid_relation {
                let valid_options: Vec<String> = relation_base_names.keys().cloned().collect();
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectFetchInvalidInclude,
                        path,
                        format!(
                            "Cannot include '{}' - {} has no relation field for it",
                            include_field, entity_name
                        ),
                    )
                    .with_suggestion(format!(
                        "Available relations: {}",
                        if valid_options.is_empty() {
                            "none".to_string()
                        } else {
                            valid_options.join(", ")
                        }
                    ))
                    .with_valid_values(valid_options),
                );
            }
        }
    }
    // If entity definition is not available, skip validation (entity validation will catch it)

    result
}

/// Validate fetch effect (object form)
fn validate_fetch_object(
    entity: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::String(entity_name) = entity {
        if !known_entities.is_empty() && !known_entities.contains(entity_name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectFetchInvalidEntity,
                    path,
                    format!("Entity '{}' does not exist", entity_name),
                )
                .with_valid_values(known_entities.iter().cloned().collect()),
            );
        }
    }

    result
}

/// Validate persist effect (array form)
fn validate_persist(
    arr: &[SExpression],
    path: &str,
    known_entities: &HashSet<String>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected formats:
    // 4-element: ["persist", "create"|"update"|"delete", "EntityName", data]
    // 3-element: ["persist", "EntityName", data] (legacy)
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectFetchInvalidEntity,
                path,
                "persist requires operation and entity name",
            )
            .with_suggestion("Use: [\"persist\", \"create\", \"EntityName\", data]"),
        );
        return result;
    }

    // Determine entity position based on format
    // If arr[1] is an operation (create/update/delete), entity is at arr[2]
    // Otherwise, entity is at arr[1] (legacy format)
    let entity_index = if let SExpression::String(op) = &arr[1] {
        if op == "create" || op == "update" || op == "delete" {
            2 // 4-element format: ["persist", "op", "Entity", data]
        } else {
            1 // 3-element format: ["persist", "Entity", data]
        }
    } else {
        1
    };

    // Check entity name
    if arr.len() > entity_index {
        if let SExpression::String(entity_name) = &arr[entity_index] {
            if !known_entities.is_empty() && !known_entities.contains(entity_name) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectFetchInvalidEntity,
                        path,
                        format!("Entity '{}' does not exist", entity_name),
                    )
                    .with_valid_values(known_entities.iter().cloned().collect()),
                );
            }
        }
    }

    // Trailing options object containing `emit:` is detected by the resolver
    // only when it actually has an `emit` key — match that here to avoid
    // treating an inline data payload as an options bag.
    if arr.len() > entity_index + 1 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Validate `ref` effect. Ref is a reactive subscription — `on_change` fires
/// on each update; `failure` fires on initial subscription error.
fn validate_ref(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["ref", "EntityName"] or ["ref", "EntityName", { ...options }]
    if arr.len() < 2 {
        return result;
    }

    if let Some(SExpression::Object(obj)) = arr.get(2) {
        if let Some(emit_obj) = extract_emit_object_ref(obj) {
            result.merge(validate_emit_config(
                emit_obj,
                &["on_change", "failure"],
                declared_emits,
                path,
            ));
        }
    }

    result
}

/// Validate any `os/watch-*` effect. Each is a streaming subscription —
/// `on_message` fires per event, `failure` fires if the subscription breaks.
fn validate_os_watch(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Walk the tail looking for an options object carrying `emit:`. The
    // operator-specific arg positions vary but `emit:` is always inside a
    // trailing options object, detected by having an `emit` key.
    for arg in arr.iter().skip(1) {
        if let SExpression::Object(options) = arg {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["on_message", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Validate emit effect
fn validate_emit(arr: &[SExpression], path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["emit", "EVENT_NAME", payload?]
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitOrphanEvent,
                path,
                "emit requires event name",
            )
            .with_suggestion("Use: [\"emit\", \"EVENT_NAME\"]"),
        );
        return result;
    }

    // Post-Phase 2.5: event names are nominal identifiers declared via
    // `type X = Event<T>`. Any valid identifier (MixedCase or UPPER_SNAKE)
    // is accepted; the type system checks resolvability.
    if let SExpression::String(event_name) = &arr[1] {
        if !is_valid_event_identifier(event_name) {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::TraitEventInvalidName,
                    path,
                    format!(
                        "Event name '{}' is not a valid identifier",
                        event_name
                    ),
                )
                .with_suggestion(
                    "Declare the event via `type EventName = Event<Payload>` and reference it by identifier",
                ),
            );
        }
    }

    result
}

/// Validate an `emit:` config block on an async/reactive data operator.
///
/// `allowed_keys` restricts which lifecycle hooks apply to the operator
/// (e.g. `fetch` only supports `success` + `failure`; `set` only `success`;
/// `os/watch-*` only `on_message` + `failure`). Unknown keys get hard errors.
///
/// Event names must be UPPER_SNAKE_CASE. Events not present in the trait's
/// declared `emits` array produce a warning (forward-compatibility hint) —
/// the language-level rule is still enforced in `validate_emit_against_declaration`
/// once the runtime actually fires the event.
fn validate_emit_config(
    emit_obj: &std::collections::HashMap<String, SExpression>,
    allowed_keys: &[&str],
    declared_emits: Option<&[DeclaredEmit]>,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Normalize both camelCase and snake_case forms that the resolver
    // accepts so the validator sees the same keys users typed.
    let key_aliases: &[(&str, &str)] = &[
        ("onChange", "on_change"),
        ("onMessage", "on_message"),
    ];

    for (key, value) in emit_obj {
        // Canonical name (as it appears in allowed_keys).
        let canonical = key_aliases
            .iter()
            .find(|(alt, _)| alt == key)
            .map(|(_, canon)| *canon)
            .unwrap_or(key.as_str());

        if !allowed_keys.iter().any(|k| *k == canonical) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectEmitKeyInvalid,
                    path,
                    format!(
                        "emit.{} is not supported on this operator (allowed: {})",
                        key,
                        allowed_keys.join(", ")
                    ),
                )
                .with_valid_values(allowed_keys.iter().map(|s| s.to_string()).collect())
                .with_suggestion(
                    "Remove the invalid key or use a different operator that supports it",
                ),
            );
            continue;
        }

        // Post-Phase 2.5: emit event values are nominal identifiers (declared
        // via `type X = Event<T>`). Any valid identifier is accepted; the
        // type registry verifies resolvability. Mixed-case and UPPER_SNAKE
        // both pass. Empty strings and malformed identifiers are still hard
        // errors.
        match value {
            SExpression::String(event_name) => {
                if !is_valid_event_identifier(event_name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EffectEmitInvalidName,
                            path,
                            format!(
                                "emit.{} = '{}' is not a valid identifier",
                                key, event_name
                            ),
                        )
                        .with_suggestion(
                            "Declare an `Event<T>` type alias and reference its name (e.g. 'PatientLoaded' or 'PATIENT_LOADED')",
                        ),
                    );
                } else if let Some(declared) = declared_emits {
                    if !declared.iter().any(|d| d.event == *event_name) {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::EffectEmitUndeclaredEvent,
                                path,
                                format!(
                                    "emit.{} = '{}' is not declared in the trait's emits array",
                                    key, event_name
                                ),
                            )
                            .with_suggestion(
                                "Declare the event in the trait's `emits` block with its payload shape",
                            ),
                        );
                    }
                }
            }
            _ => {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectEmitInvalidName,
                        path,
                        format!("emit.{} must be a string event name", key),
                    )
                    .with_suggestion("e.g. emit.success = 'PATIENT_LOADED'"),
                );
            }
        }
    }

    result
}

/// Extract the `emit:` object from a trailing options SExpression::Object.
/// Returns None if the map has no `emit` key or if `emit` is not an object.
fn extract_emit_object_ref<'a>(
    options: &'a std::collections::HashMap<String, SExpression>,
) -> Option<&'a std::collections::HashMap<String, SExpression>> {
    match options.get("emit")? {
        SExpression::Object(obj) => Some(obj),
        _ => None,
    }
}

/// Validate navigate effect
fn validate_navigate(
    arr: &[SExpression],
    path: &str,
    page_init_payloads: &HashMap<String, PageInitPayload>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["navigate", "/path"] or ["navigate", "/path", { payload }]
    // or ["navigate", ["str/concat", ...], { payload }]
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectUnknownType,
                path,
                "navigate requires a path",
            )
            .with_suggestion("Use: [\"navigate\", \"/path\"]"),
        );
        return result;
    }

    // Extract the navigate path (may be static string or dynamic expression)
    let nav_path_str: Option<String> = match &arr[1] {
        SExpression::String(s) => Some(s.clone()),
        SExpression::Array(_) => {
            // Dynamic path like ["str/concat", "/inspection/", "@entity.id"]
            // We can't statically validate the exact path, but we can try to
            // extract a pattern if the first segment is a literal
            extract_static_path_prefix(&arr[1])
        }
        _ => None,
    };

    // Check path format
    if let Some(ref nav_path) = nav_path_str {
        if !nav_path.starts_with('/') && !nav_path.starts_with('@') {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::PageInvalidPath,
                    path,
                    format!("Navigate path should start with '/': '{}'", nav_path),
                )
                .with_suggestion("Use absolute paths starting with /"),
            );
        }
    }

    // Check payload requirements for the target page
    if let Some(ref nav_path) = nav_path_str {
        // Try to find a matching page path pattern
        let matching_page = find_matching_page_path(nav_path, page_init_payloads);

        // L2.4 backstop (gap #19A author-side detection): when no
        // declared page matches, error. Catches navigate calls injected
        // by molecule-level `effects: { ... }` overrides AND navigate
        // targets that resolved to imported pages from `uses` refs that
        // L1's pre-inline check couldn't see.
        //
        // Skip warning the dynamic-prefix case where the path was
        // synthesised via `extract_static_path_prefix` from a
        // `["str/concat", ...]` expression. Those are best-effort
        // partial paths; the page_init_payloads side does prefix
        // matching for them already, so no match here genuinely means
        // no page begins with that prefix.
        if matching_page.is_none() && !page_init_payloads.is_empty() {
            // Skip when the schema is genuinely page-less (early-stage
            // scaffolding). The check fires only when the schema
            // declares pages but none match this navigate target.
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectNavigateTargetUnreachable,
                    path,
                    format!(
                        "Navigate target '{}' doesn't resolve to any declared page",
                        nav_path
                    ),
                )
                .with_suggestion(format!(
                    "Declare a matching `pages[].path` (e.g. `page \"{}\" -> ...`) or correct the navigate path",
                    nav_path
                )),
            );
        }

        if let Some((page_path, init_payload)) = matching_page {
            // Check if navigate provides required payload
            if !init_payload.required_fields.is_empty() {
                let provided_fields = extract_payload_fields(&arr);

                // Check each required field
                for required_field in &init_payload.required_fields {
                    if !provided_fields.contains(required_field) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectNavigateMissingPayload,
                                path,
                                format!(
                                    "Navigation to '{}' requires payload field '{}' for INIT event",
                                    page_path, required_field
                                ),
                            )
                            .with_suggestion(format!(
                                "Add payload to navigate effect: [\"navigate\", \"{}\", {{ \"{}\": \"@entity.{}\" }}]",
                                nav_path, required_field, required_field
                            ))
                            .with_expected_shape(&format!(
                                r#"["navigate", "{}", {{ {} }}]"#,
                                nav_path,
                                init_payload
                                    .required_fields
                                    .iter()
                                    .map(|f| format!("\"{}\": \"...\"", f))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )),
                        );
                    }
                }
            }
        }
    }

    result
}

/// Extract a static path prefix from a dynamic expression like ["str/concat", "/inspection/", "@entity.id"]
fn extract_static_path_prefix(expr: &SExpression) -> Option<String> {
    if let SExpression::Array(arr) = expr {
        if arr.len() >= 2 {
            if let SExpression::String(op) = &arr[0] {
                if op == "str/concat" || op == "concat" {
                    // First argument might be a static string
                    if let SExpression::String(first) = &arr[1] {
                        // Return the first segment - this helps match against path patterns
                        // For "/inspection/" + dynamic, we get "/inspection/"
                        return Some(first.clone());
                    }
                }
            }
        }
    }
    None
}

/// Find a matching page path pattern for a given navigation path
fn find_matching_page_path<'a>(
    nav_path: &str,
    page_init_payloads: &'a HashMap<String, PageInitPayload>,
) -> Option<(&'a String, &'a PageInitPayload)> {
    // First try exact match
    if let Some(payload) = page_init_payloads.get(nav_path) {
        return Some((
            page_init_payloads.keys().find(|k| *k == nav_path).unwrap(),
            payload,
        ));
    }

    // Try pattern matching (e.g., "/inspection/123" matches "/inspection/:id")
    for (page_path, payload) in page_init_payloads {
        if path_matches_pattern(nav_path, page_path) {
            return Some((page_path, payload));
        }
    }

    // For partial matches (from dynamic paths like "/inspection/"),
    // try to find a pattern that starts with the same prefix
    for (page_path, payload) in page_init_payloads {
        if page_path.starts_with(nav_path.trim_end_matches('/')) {
            return Some((page_path, payload));
        }
    }

    None
}

/// Check if a concrete path matches a pattern with :param placeholders
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();

    if path_parts.len() != pattern_parts.len() {
        return false;
    }

    for (path_part, pattern_part) in path_parts.iter().zip(pattern_parts.iter()) {
        if pattern_part.starts_with(':') {
            // This is a param, it matches anything
            continue;
        }
        if path_part != pattern_part {
            return false;
        }
    }

    true
}

/// Extract payload field names from a navigate effect
fn extract_payload_fields(arr: &[SExpression]) -> HashSet<String> {
    let mut fields = HashSet::new();

    // Payload is typically the third argument: ["navigate", "/path", { ... }]
    if arr.len() >= 3 {
        if let SExpression::Object(payload) = &arr[2] {
            for key in payload.keys() {
                fields.insert(key.clone());
            }
        }
    }

    fields
}

/// Check if a string is UPPER_SNAKE_CASE
fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

/// Accept a `.lolo`-shaped identifier: starts with a letter, contains only
/// letters / digits / underscores. Used by the emit validator post-Phase 2.5
/// since event names are now nominal identifiers in the type system (declared
/// as `type X = Event<T>`) rather than string literals tagged by convention.
/// Both UPPER_SNAKE_CASE and MixedCase shapes are valid identifiers.
fn is_valid_event_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_set_effect() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.title".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_set_effect_payload_not_allowed() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@payload.title".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_set_effect_missing_path() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity".to_string()),
            SExpression::String("value".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingSetTargetMissingPath));
    }

    #[test]
    fn test_fetch_effect_invalid_entity() {
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("NonExistent".to_string()),
        ]);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectFetchInvalidEntity));
    }

    #[test]
    fn test_call_service_missing_service() {
        let effect = SExpression::Array(vec![
            SExpression::String("call-service".to_string()),
            SExpression::String("unknownService".to_string()),
            SExpression::String("action".to_string()),
        ]);

        let known_services: HashSet<String> = ["api".to_string()].into_iter().collect();
        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &known_services,
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectCallServiceMissingService));
    }

    #[test]
    fn test_navigate_missing_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate without payload
        let effect = SExpression::Array(vec![
            SExpression::String("navigate".to_string()),
            SExpression::String("/inspection/123".to_string()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNavigateMissingPayload));
    }

    #[test]
    fn test_navigate_with_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate with payload
        let effect = SExpression::Array(vec![
            SExpression::String("navigate".to_string()),
            SExpression::String("/inspection/123".to_string()),
            SExpression::Object(
                vec![(
                    "id".to_string(),
                    SExpression::String("@entity.id".to_string()),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &HashMap::new(),
            None,
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("/inspection/123", "/inspection/:id"));
        assert!(path_matches_pattern(
            "/users/42/posts/7",
            "/users/:userId/posts/:postId"
        ));
        assert!(path_matches_pattern("/about", "/about"));
        assert!(!path_matches_pattern("/about", "/contact"));
        assert!(!path_matches_pattern("/inspection", "/inspection/:id"));
    }

    #[test]
    fn test_is_upper_snake_case() {
        assert!(is_upper_snake_case("TASK_CREATED"));
        assert!(is_upper_snake_case("START"));
        assert!(is_upper_snake_case("ITEM_1_SELECTED"));
        assert!(!is_upper_snake_case("taskCreated"));
        assert!(!is_upper_snake_case("task_created"));
        assert!(!is_upper_snake_case("_INVALID"));
        assert!(!is_upper_snake_case("INVALID_"));
    }

    #[test]
    fn test_emit_declared_passes() {
        use orbital_core::schema::{EmitDefinition, EventScope, PayloadField};

        let declared_emits = vec![DeclaredEmit {
            event: "TASK_CREATED".to_string(),
            is_external: true,
            payload_fields: vec!["taskId".to_string(), "title".to_string()],
            required_fields: vec!["taskId".to_string()],
        }];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("TASK_CREATED".to_string()),
            SExpression::Object(
                vec![("taskId".to_string(), SExpression::String("123".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_emit_undeclared_warns() {
        let declared_emits: Vec<DeclaredEmit> = vec![];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("UNKNOWN_EVENT".to_string()),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventUndeclared));
    }

    #[test]
    fn test_emit_missing_required_payload_field() {
        let declared_emits = vec![DeclaredEmit {
            event: "ORDER_CONFIRMED".to_string(),
            is_external: true,
            payload_fields: vec![
                "orderId".to_string(),
                "customerId".to_string(),
                "total".to_string(),
            ],
            required_fields: vec!["orderId".to_string(), "customerId".to_string()],
        }];

        // Only provide orderId, missing customerId
        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("ORDER_CONFIRMED".to_string()),
            SExpression::Object(
                vec![(
                    "orderId".to_string(),
                    SExpression::String("123".to_string()),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalPayloadMismatch));
    }

    #[test]
    fn test_internal_emit_no_payload_required() {
        let declared_emits = vec![DeclaredEmit {
            event: "STATUS_CHANGED".to_string(),
            is_external: false, // Internal event
            payload_fields: vec![],
            required_fields: vec![],
        }];

        let effect = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("STATUS_CHANGED".to_string()),
            // No payload needed for internal events
        ]);

        let result = validate_emit_against_declaration(&effect, "test", &declared_emits);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_fetch_include_valid_relation() {
        use orbital_core::schema::{FieldDefinition, RelationDefinition};

        // Create an entity with a relation field
        let mut entity_defs: HashMap<String, EntityDefinition> = HashMap::new();
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                collection: None,
                persistence: None,
                fields: vec![
                    FieldDefinition {
                        name: Some("id".to_string()),
                        field_type: FieldType::String,
                        required: Some(true),
                        primary_key: Some(true),
                        unique: None,
                        default: None,
                        values: None,
                        relation: None,
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                    FieldDefinition {
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: Some(true),
                        primary_key: None,
                        unique: None,
                        default: None,
                        values: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                        }),
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                ],
                instances: None,
            },
        );

        let known_entities: HashSet<String> = ["Inspection".to_string()].into_iter().collect();
        // Declare the success event so the `emit.success` requirement
        // (added across emit-type ops in commit 222109fa3) is satisfied.
        let declared_emits = vec![DeclaredEmit {
            event: "InspectionLoaded".to_string(),
            is_external: false,
            payload_fields: vec![],
            required_fields: vec![],
        }];

        // Fetch with valid include + required emit.success
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("Inspection".to_string()),
            SExpression::Object(
                vec![
                    (
                        "include".to_string(),
                        SExpression::Array(vec![SExpression::String("company".to_string())]),
                    ),
                    (
                        "emit".to_string(),
                        SExpression::Object(
                            vec![(
                                "success".to_string(),
                                SExpression::String("InspectionLoaded".to_string()),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &entity_defs,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            Some(&declared_emits),
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_fetch_include_invalid_relation() {
        use orbital_core::schema::{FieldDefinition, RelationDefinition};

        // Create an entity with a relation field
        let mut entity_defs: HashMap<String, EntityDefinition> = HashMap::new();
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                collection: None,
                persistence: None,
                fields: vec![
                    FieldDefinition {
                        name: Some("id".to_string()),
                        field_type: FieldType::String,
                        required: Some(true),
                        primary_key: Some(true),
                        unique: None,
                        default: None,
                        values: None,
                        relation: None,
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                    FieldDefinition {
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: Some(true),
                        primary_key: None,
                        unique: None,
                        default: None,
                        values: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                        }),
                        items: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                ],
                instances: None,
            },
        );

        let known_entities: HashSet<String> = ["Inspection".to_string()].into_iter().collect();

        // Fetch with invalid include (nonExistent is not a relation)
        let effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("Inspection".to_string()),
            SExpression::Object(
                vec![(
                    "include".to_string(),
                    SExpression::Array(vec![SExpression::String("nonExistent".to_string())]),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &entity_defs,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectFetchInvalidInclude));
    }
}
