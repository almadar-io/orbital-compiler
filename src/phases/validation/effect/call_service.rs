//! call_service effect validation.
//!
//! One shared core (`check_service_call`) validates service name, action, and
//! per-action params for BOTH spellings (array + object form) and for every
//! occurrence depth — top-level effects and nested forms found by
//! `walk_nested_call_services` (I-8). Param checks (I-13) run against the
//! services registry's declared param contracts.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use crate::embedded::ServiceRegistryData;
use orbital_core::schema::SExpression;
use std::collections::HashSet;

/// Envelope keys the call-service form itself owns; never param-checked.
const RESERVED_OPTION_KEYS: [&str; 4] = ["emit", "onSuccess", "onError", "timeout"];

/// Validate the service name against inline `schema.services` ∪ the services
/// registry (the union gate — an inline declaration must never mask registry
/// services). Unknown name: hard error when the schema declares inline
/// services (it opted into decidability), else the staged Phase-F warning.
fn check_service_name(
    service_name: &str,
    path: &str,
    known_services: &HashSet<String>,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let known = known_services.contains(service_name) || registry.actions.contains_key(service_name);
    if known {
        return result;
    }
    let mut valid: Vec<String> = known_services
        .iter()
        .cloned()
        .chain(registry.actions.keys().cloned())
        .collect();
    valid.sort();
    if !known_services.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                format!("Service '{}' is not defined", service_name),
            )
            .with_suggestion("Define the service in schema.services or check the name for a typo")
            .with_valid_values(valid),
        );
    } else if !registry.actions.is_empty() {
        // No inline schema.services (every .lolo lowers to services: None),
        // so gate against the services registry. Staged as a WARNING —
        // hard-error flip criteria recorded in the I-1 ledger row.
        result.add_error(
            ValidationError::phase_f_warning(
                ErrorCode::EffectCallServiceUnknownService,
                path,
                format!(
                    "Service '{}' is not a registered integrator or declared service",
                    service_name
                ),
            )
            .with_suggestion(
                "Register the integrator (almadar-sync services) or add it to DECLARED_SERVICES, or check the name for a typo",
            )
            .with_valid_values(valid),
        );
    }
    result
}

/// Validate the action against the registry contract. A declared-only
/// service carries an EMPTY action set: name known, actions unchecked.
fn check_action(
    service_name: &str,
    action: &str,
    path: &str,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if let Some(valid_actions) = registry.actions.get(service_name) {
        if !valid_actions.is_empty() && !valid_actions.contains(action) {
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
    result
}

/// Registry param types are TS-flavored strings. Map the checkable subset to
/// the SExpression literal kinds; anything else (named TS types, unions of
/// non-literals) is deliberately unchecked.
fn checkable_type(declared: &str) -> Option<&'static str> {
    let t = declared.trim();
    match t {
        "string" => Some("string"),
        "number" => Some("number"),
        "boolean" => Some("boolean"),
        "object" => Some("object"),
        "array" => Some("array"),
        _ => {
            if t.ends_with("[]") {
                Some("array")
            } else if t.starts_with('\'') {
                // 'a' | 'b' string-literal union
                Some("string")
            } else {
                None
            }
        }
    }
}

/// The literal kind of a param value, or None when the value is dynamic
/// (bindings, computed s-expressions, null) and must not be type-checked.
fn literal_kind(value: &SExpression) -> Option<&'static str> {
    match value {
        SExpression::String(s) => {
            if s.starts_with('@') || s.starts_with('?') || s.starts_with('$') {
                None
            } else {
                Some("string")
            }
        }
        SExpression::Number(_) => Some("number"),
        SExpression::Bool(_) => Some("boolean"),
        SExpression::Object(_) => Some("object"),
        // An array literal is only a literal when it isn't an s-expression
        // call — a leading string makes it a computed form.
        SExpression::Array(items) => match items.first() {
            Some(SExpression::String(_)) => None,
            _ => Some("array"),
        },
        SExpression::Null => None,
    }
}

/// Param checks (I-13) for one call site: unknown key, missing required,
/// literal type mismatch. Runs only when the registry declares params for
/// this service+action; `params` = the expression the resolver would pass as
/// the call's args payload (None = no payload → empty object).
fn check_params(
    service_name: &str,
    action: &str,
    params: Option<&SExpression>,
    path: &str,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(declared) = registry
        .params
        .get(service_name)
        .and_then(|actions| actions.get(action))
    else {
        return result;
    };

    static EMPTY: std::sync::OnceLock<std::collections::HashMap<String, SExpression>> =
        std::sync::OnceLock::new();
    let obj = match params {
        None => EMPTY.get_or_init(std::collections::HashMap::new),
        Some(SExpression::Object(map)) => map,
        // Dynamic params expression — keys unknowable, skip all param checks.
        Some(_) => return result,
    };

    let declared_names: HashSet<&str> = declared.iter().map(|p| p.name.as_str()).collect();
    let mut sorted_declared: Vec<String> = declared.iter().map(|p| p.name.clone()).collect();
    sorted_declared.sort();

    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        if RESERVED_OPTION_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !declared_names.contains(key.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectCallServiceUnknownParam,
                    path,
                    format!(
                        "Param '{}' is not declared by action '{}' on service '{}'",
                        key, action, service_name
                    ),
                )
                .with_suggestion("Check the service contract for the declared param names")
                .with_valid_values(sorted_declared.clone()),
            );
        }
    }

    for p in declared {
        if p.required && !obj.contains_key(&p.name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectCallServiceMissingParam,
                    path,
                    format!(
                        "Required param '{}' is missing on '{}' of service '{}'",
                        p.name, action, service_name
                    ),
                )
                .with_suggestion("Supply the required param in the call's args object"),
            );
        }
        if let (Some(expected), Some(value)) = (checkable_type(&p.param_type), obj.get(&p.name)) {
            if let Some(actual) = literal_kind(value) {
                if actual != expected {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EffectCallServiceParamType,
                            path,
                            format!(
                                "Param '{}' on '{}' of service '{}' expects {}, got a {} literal",
                                p.name, action, service_name, p.param_type, actual
                            ),
                        )
                        .with_suggestion("Match the declared param type from the service contract"),
                    );
                }
            }
        }
    }

    result
}

/// The shared name/action/param core for an array-form call site. `arr`
/// includes the operator head. Mirrors the resolver's arg reading exactly:
/// params = the last non-envelope arg (an object carrying `emit` in trailing
/// position past arity 4 is the envelope, not params).
fn check_array_call(
    arr: &[SExpression],
    path: &str,
    known_services: &HashSet<String>,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let service_name = match &arr[1] {
        SExpression::String(s) => s,
        _ => {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "Service name must be a string",
            ));
            return result;
        }
    };
    result.merge(check_service_name(service_name, path, known_services, registry));

    let Some(SExpression::String(action)) = arr.get(2) else {
        result.add_error(ValidationError::error(
            ErrorCode::EffectCallServiceMissingAction,
            path,
            "Action must be a string",
        ));
        return result;
    };
    result.merge(check_action(service_name, action, path, registry));

    // Resolver truth (resolve_call_service_effect): args_end walks back over
    // a trailing `{ emit: … }` object only past 4 total args; params is the
    // arg just before args_end, absent → empty payload.
    let mut args_end = arr.len();
    if arr.len() > 4 {
        if let Some(SExpression::Object(last)) = arr.last() {
            if last.contains_key("emit") {
                args_end -= 1;
            }
        }
    }
    let params = if args_end > 3 { arr.get(args_end - 1) } else { None };
    result.merge(check_params(service_name, action, params, path, registry));

    result
}

/// Validate call_service effect (array form, top-level effect position).
pub(crate) fn validate_call_service(
    arr: &[SExpression],
    path: &str,
    known_services: &HashSet<String>,
    registry: &ServiceRegistryData,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["call-service", "serviceName", "action", { ...params }, { emit: {...} }]
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

    result.merge(check_array_call(arr, path, known_services, registry));

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
pub(crate) fn validate_call_service_object(
    obj: &std::collections::HashMap<String, SExpression>,
    path: &str,
    known_services: &HashSet<String>,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let call_service = obj.get("call_service").or_else(|| obj.get("call-service"));

    if let Some(SExpression::Object(cs_obj)) = call_service {
        if let Some(SExpression::String(service)) = cs_obj.get("service") {
            result.merge(check_service_name(service, path, known_services, registry));
            if let Some(SExpression::String(action)) = cs_obj.get("action") {
                result.merge(check_action(service, action, path, registry));
                if let Some(params @ SExpression::Object(_)) = cs_obj.get("params") {
                    result.merge(check_params(service, action, Some(params), path, registry));
                }
            }
        } else {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "call_service requires 'service' property",
            ));
        }

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

/// Walk an expression tree for nested call-service forms (I-8): occurrences
/// inside effect arguments — `(set x (call-service …))`, the `llm embed`
/// class — previously bypassed validation entirely. Nested occurrences get
/// the full name/action/param checks; the emit-envelope check stays
/// top-level-only (its declared-emits scope belongs to the owning effect).
pub(crate) fn walk_nested_call_services(
    expr: &SExpression,
    path: &str,
    known_services: &HashSet<String>,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if (head == "call-service" || head == "call_service") && arr.len() >= 3 {
                    result.merge(check_array_call(arr, path, known_services, registry));
                }
            }
            for item in arr.iter().skip(1) {
                result.merge(walk_nested_call_services(item, path, known_services, registry));
            }
            // A non-call head array (a plain list literal) still nests: walk
            // the head position too when it is not an operator string.
            if let Some(first) = arr.first() {
                if !matches!(first, SExpression::String(_)) {
                    result.merge(walk_nested_call_services(first, path, known_services, registry));
                }
            }
        }
        SExpression::Object(obj) => {
            if obj.contains_key("call_service") || obj.contains_key("call-service") {
                result.merge(validate_call_service_object(obj, path, known_services, registry));
            }
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                result.merge(walk_nested_call_services(&obj[key], path, known_services, registry));
            }
        }
        _ => {}
    }
    result
}
