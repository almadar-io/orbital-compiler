//! persist effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::SExpression;
use std::collections::HashSet;

/// Validate persist effect (array form)
pub(crate) fn validate_persist(
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
