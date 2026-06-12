//! Emit declaration collection + effect dispatch.

use super::super::{ErrorCode, PageInitPayload, ValidationError, ValidationResult};
use super::{
    validate_call_service, validate_call_service_object, validate_emit, validate_fetch, validate_fetch_object, validate_navigate,
    validate_os_watch, validate_persist, validate_ref, validate_set, validate_set_object,
    effect_operators, validate_set_value_type, DeclaredEmit,
};
use orbital_core::schema::{EntityDefinition, SExpression, TraitDefinition};
use std::collections::{HashMap, HashSet};

/// Collect all declared emits from a trait definition
pub(crate) fn collect_declared_emits(trait_def: &TraitDefinition) -> Vec<DeclaredEmit> {
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
pub(crate) fn validate_emit_against_declaration(
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
pub(crate) fn validate_required_emit_success(
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_effect(
    effect: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_actions: &HashMap<String, HashSet<String>>,
    declared_emits: Option<&[DeclaredEmit]>,
    // SET-EMITS-PAYLOAD backstop inputs: `{ name -> type }` maps for the
    // linked entity's fields and the trait's declared payload fields.
    entity_field_types: &HashMap<String, String>,
    payload_field_types: &HashMap<String, String>,
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
                        result.merge(validate_set_value_type(
                            arr,
                            path,
                            entity_field_types,
                            payload_field_types,
                        ));
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
                    op if !effect_operators().contains(&op) => {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::EffectUnknownType,
                                path,
                                format!("Unknown effect operator: '{}'", op),
                            )
                            .with_valid_values(
                                effect_operators().iter().map(|s| s.to_string()).collect(),
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
