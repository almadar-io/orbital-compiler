//! Nested field-path validation and flat field-ref resolution against entity
//! definitions.

use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EntityDefinition, FieldType, SExpression};
use std::collections::HashMap;

/// Validate nested field paths in a pattern's `fields` prop
///
/// For field paths like `["company.name", "inspector.fullName"]`:
/// - Validates that the first segment is a relation field on the entity
/// - Validates that subsequent segments exist on the related entity
pub(super) fn validate_field_paths(
    fields: &SExpression,
    entity_name: &str,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Get the base entity definition
    let Some(base_entity) = entity_definitions.get(entity_name) else {
        // Entity not found - this will be caught by entity validation
        return result;
    };

    // Extract field strings from array
    let field_paths = match fields {
        SExpression::Array(arr) => arr
            .iter()
            .filter_map(|f| {
                if let SExpression::String(s) = f {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>(),
        _ => return result,
    };

    for field_path in field_paths {
        // Skip non-nested paths (no validation needed for simple fields)
        if !field_path.contains('.') {
            continue;
        }

        let segments: Vec<&str> = field_path.split('.').collect();
        if segments.is_empty() {
            continue;
        }

        // First segment should be a relation field on the base entity
        let first_segment = segments[0];

        // Find the relation field (check both "fieldName" and "fieldNameId" forms)
        let relation_field = base_entity.fields.iter().find(|f| {
            if f.field_type != FieldType::Relation {
                return false;
            }
            if let Some(name) = &f.name {
                // Match "company" against "company" or "companyId"
                name == first_segment
                    || name.strip_suffix("Id") == Some(first_segment)
                    || format!("{}Id", first_segment) == *name
            } else {
                false
            }
        });

        let Some(relation_field) = relation_field else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::RenderUIInvalidFieldPath,
                    path,
                    format!(
                        "Field path '{}' - '{}' has no relation field '{}' to traverse",
                        field_path, entity_name, first_segment
                    ),
                )
                .with_suggestion(format!(
                    "Add a relation field for '{}' or use a direct field reference",
                    first_segment
                )),
            );
            continue;
        };

        // Get the target entity of the relation
        let target_entity_name = relation_field.relation.as_ref().map(|r| r.entity.as_str());

        let Some(target_entity_name) = target_entity_name else {
            // Relation doesn't specify target entity - skip further validation
            continue;
        };

        // For remaining segments (e.g., "name" in "company.name"), validate against target entity
        if segments.len() > 1 {
            let Some(target_entity) = entity_definitions.get(target_entity_name) else {
                // Target entity not found - will be caught by entity validation
                continue;
            };

            // Validate the second segment exists on the target entity
            let second_segment = segments[1];
            let field_exists = target_entity.fields.iter().any(|f| {
                f.name
                    .as_ref()
                    .map(|n| n == second_segment)
                    .unwrap_or(false)
            });

            if !field_exists {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::RenderUIInvalidFieldPath,
                        path,
                        format!(
                            "Field path '{}' - '{}' has no field '{}' on related entity '{}'",
                            field_path, entity_name, second_segment, target_entity_name
                        ),
                    )
                    .with_suggestion(format!(
                        "Check that '{}' entity has a '{}' field",
                        target_entity_name, second_segment
                    )),
                );
            }

            // Note: We only validate 2 levels deep. Deeper nesting (e.g., "company.address.city")
            // would require recursive validation, which is intentionally not implemented
            // to encourage simpler data structures.
            if segments.len() > 2 {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::RenderUIInvalidFieldPath,
                        path,
                        format!(
                            "Field path '{}' has more than 2 levels of nesting (max recommended: 2)",
                            field_path
                        ),
                    )
                    .with_suggestion(
                        "Consider flattening the data structure or using a custom pattern",
                    ),
                );
            }
        }
    }

    result
}

/// Resolve a single field-ref value against the linked entity's fields.
///
/// Emits an `UnresolvedPatternFieldRef` error if `field_name` does not match
/// any field on `entity_name`. Dotted paths (e.g. `"company.name"`) are left
/// for the existing `validate_field_paths` helper — this function only checks
/// flat field names used by `kind: 'field-ref'` / `itemKind: 'field-ref'`
/// markers.
///
/// Severity is `Warning` per the Phase 3 staging guidance; will graduate to
/// `Error` once scope declarations in Phase 4 land.
pub(super) fn validate_pattern_field_ref(
    field_name: &str,
    entity_name: &str,
    pattern_type: &str,
    prop_name: &str,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Dotted paths fall through to the existing nested-field-path validator
    // (invoked separately on the `fields` prop); skip them here.
    if field_name.contains('.') {
        return result;
    }

    // Entity not found -> entity validation will report it; no further work.
    let Some(entity) = entity_definitions.get(entity_name) else {
        return result;
    };

    // Matches when the entity declares a field with the exact name, OR when
    // the entity declares `<name>Id` for a relation (caller can refer to the
    // relation by its logical name without the `Id` suffix, matching the
    // existing convention used in `validate_field_paths`).
    let exists = entity.fields.iter().any(|f| {
        let Some(name) = &f.name else {
            return false;
        };
        name == field_name
            || name.strip_suffix("Id") == Some(field_name)
            || format!("{}Id", field_name) == *name
    });

    if !exists {
        let valid: Vec<String> = entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect();

        result.add_error(
            ValidationError::error(
                ErrorCode::UnresolvedPatternFieldRef,
                path,
                format!(
                    "Pattern '{}' prop '{}' references field '{}' which does not exist on entity '{}'",
                    pattern_type, prop_name, field_name, entity_name
                ),
            )
            .with_suggestion(format!(
                "Use a field declared on '{}' or add the field to the entity schema",
                entity_name
            ))
            .with_valid_values(valid),
        );
    }

    result
}
