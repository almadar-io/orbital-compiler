//! call_service effect validation.
//!
//! The keyed-call checks for a call-service site, at every occurrence depth —
//! top-level effects and nested forms found by `walk_nested_call_services`
//! (I-8). Service name, action and param checks are owned by std lints.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::keyed_call::{
    check_key_and_cancelled, declared_key, trailing_options, KEYED_CALL_EMIT_KEYS,
};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use crate::embedded::ServiceRegistryData;
use orbital_core_base::schema::SExpression;

/// The literal kind of a param value, or None when the value is dynamic
/// (bindings, computed s-expressions, null) and must not be type-checked.
pub(super) fn literal_kind(value: &SExpression) -> Option<&'static str> {
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

/// The keyed-call core for an array-form call site. `arr` includes the
/// operator head.
fn check_array_call(
    arr: &[SExpression],
    path: &str,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let (SExpression::String(service_name), Some(SExpression::String(action))) =
        (&arr[1], arr.get(2))
    else {
        return result;
    };

    let options = trailing_options(arr);
    if let Some(options) = options {
        result.merge(check_key_and_cancelled(options, path, "call-service"));
        if declared_key(options).is_some()
            && registry
                .response_shape(service_name, action)
                .is_some_and(|fields| fields.iter().any(|f| f.name == "key"))
        {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CallServiceResultHasKey,
                    path,
                    format!("keyed call-service {service_name}.{action} returns a record that already has a `key` field; the call's `key` is merged beside the result and would collide"),
                )
                .with_suggestion("Drop the call's `key`, or use an action whose result has no `key` field"),
            );
        }
    }

    result
}

/// Validate call_service effect (array form, top-level effect position).
pub(crate) fn validate_call_service(
    arr: &[SExpression],
    path: &str,
    registry: &ServiceRegistryData,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["call-service", "serviceName", "action", { ...params }, { emit: {...} }]
    if arr.len() < 3 {
        return result;
    }

    result.merge(check_array_call(arr, path, registry));

    // Optional trailing options carrying `emit:` (call-service is async one-shot).
    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &KEYED_CALL_EMIT_KEYS,
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Walk an expression tree for nested call-service forms (I-8): occurrences
/// inside effect arguments — `(set x (call-service …))`, the `llm embed`
/// class — previously bypassed validation entirely. Nested occurrences get
/// the keyed-call checks; the emit-envelope check stays
/// top-level-only (its declared-emits scope belongs to the owning effect).
pub(crate) fn walk_nested_call_services(
    expr: &SExpression,
    path: &str,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if head == "call-service" && arr.len() >= 3 {
                    result.merge(check_array_call(arr, path, registry));
                }
            }
            for item in arr.iter().skip(1) {
                result.merge(walk_nested_call_services(item, path, registry));
            }
            // A non-call head array (a plain list literal) still nests: walk
            // the head position too when it is not an operator string.
            if let Some(first) = arr.first() {
                if !matches!(first, SExpression::String(_)) {
                    result.merge(walk_nested_call_services(first, path, registry));
                }
            }
        }
        SExpression::Object(obj) => {
            let mut keys: Vec<&orbital_core_base::value::Shared<String>> = obj.keys().collect();
            keys.sort();
            for key in keys {
                result.merge(walk_nested_call_services(&obj[key], path, registry));
            }
        }
        _ => {}
    }
    result
}
