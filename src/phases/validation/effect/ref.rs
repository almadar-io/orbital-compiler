//! ref effect validation.

use super::super::ValidationResult;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::schema::SExpression;

/// Validate `ref` effect. Ref is a reactive subscription — `on_change` fires
/// on each update; `failure` fires on initial subscription error.
pub(crate) fn validate_ref(
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
