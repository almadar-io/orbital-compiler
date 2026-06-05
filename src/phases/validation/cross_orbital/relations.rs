use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{FieldDefinition, FieldType, OrbitalSchema};
use std::collections::HashSet;

// =============================================================================
// COMP-9: validate_relation_targets
// =============================================================================

/// Validate that every `type: 'relation'` field's `relation.entity` resolves
/// to an entity declared in the same schema.
///
/// Runs alongside the existing entity-scope check (`EntityInvalidRelation`,
/// `ORB_E_INVALID_RELATION`) which reports the same condition at error
/// severity. The cross-orbital pass is intentionally staged as a warning so
/// tooling that consumes warnings-only streams still sees unresolved relations
/// while we gate the error-side behavior behind Phase 4 (when the plan calls
/// for graduating these to errors and dropping the duplicate).
pub(crate) fn validate_relation_targets(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect declared entity names (inline entities only — references are
    // resolved by the inline phase; if a reference somehow reaches this point
    // we don't know its final name and would produce a spurious warning, so
    // we only assert on inline declarations).
    let mut entity_names: HashSet<&str> = HashSet::new();
    for orbital in &schema.orbitals {
        if let Some(entity) = orbital.entity.as_inline() {
            entity_names.insert(entity.name.as_str());
        }
    }

    // Walk every inline entity's fields and recurse into array `items` and
    // object `properties` so nested relations are caught too.
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let Some(entity) = orbital.entity.as_inline() else {
            continue;
        };
        let base = format!("orbitals[{}].entity", oi);
        for (fi, field) in entity.fields.iter().enumerate() {
            let field_path = format!("{}.fields[{}]", base, fi);
            check_field_relation(&entity.name, field, &field_path, &entity_names, &mut result);
        }
    }

    result
}

/// Recursively walk a `FieldDefinition`, emitting an `UnresolvedRelationTarget`
/// warning for any `type: 'relation'` whose `relation.entity` is not in
/// `declared_entities`.
pub(crate) fn check_field_relation(
    owning_entity: &str,
    field: &FieldDefinition,
    path: &str,
    declared_entities: &HashSet<&str>,
    result: &mut ValidationResult,
) {
    if field.field_type == FieldType::Relation {
        if let Some(relation) = &field.relation {
            if !declared_entities.contains(relation.entity.as_str()) {
                let field_label = field.name.as_deref().unwrap_or("<anonymous>");
                let mut declared: Vec<&&str> = declared_entities.iter().collect();
                declared.sort();
                let declared_list = declared
                    .iter()
                    .map(|s| (**s).to_string())
                    .collect::<Vec<_>>();
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::UnresolvedRelationTarget,
                        format!("{}.relation.entity", path),
                        format!(
                            "Relation field '{}' on '{}' points at '{}', which is not declared in this schema",
                            field_label, owning_entity, relation.entity
                        ),
                    )
                    .with_suggestion(
                        "Declare the target entity in one of the schema's orbitals, or correct the relation.entity name.",
                    )
                    .with_valid_values(declared_list),
                );
            }
        }
    }

    // Recurse into array item type — e.g. `fields[].items.relation.entity`.
    if let Some(item) = &field.items {
        let item_path = format!("{}.items", path);
        check_field_relation(owning_entity, item, &item_path, declared_entities, result);
    }

    // Recurse into object property types — e.g. `fields[].properties.foo.relation.entity`.
    if let Some(props) = &field.properties {
        for (key, sub) in props {
            let sub_path = format!("{}.properties.{}", path, key);
            check_field_relation(owning_entity, sub, &sub_path, declared_entities, result);
        }
    }
}
