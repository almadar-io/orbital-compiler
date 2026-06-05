//! set effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::SExpression;

/// Validate set effect (array form)
pub(crate) fn validate_set(
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
pub(crate) fn is_path_expression(arr: &[SExpression]) -> bool {
    if arr.is_empty() {
        return false;
    }
    matches!(&arr[0], SExpression::String(op) if op == "path")
}

/// Validate set effect (object form)
pub(crate) fn validate_set_object(target: &SExpression, path: &str) -> ValidationResult {
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
