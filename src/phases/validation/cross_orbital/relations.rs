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

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{EntityDefinition, EntityRef, Orbital};

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
                uses: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    shared: false,
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
            }],
            metadata: None,
        }
    }

    // =========================================================================
    // COMP-9: validate_relation_targets
    // =========================================================================

    /// Positive case: a relation field whose `relation.entity` is declared in
    /// another orbital in the same schema must not produce a warning.
    #[test]
    fn test_relation_targets_in_schema_ok() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        // Add a second orbital that declares "Patient".
        schema.orbitals.push(Orbital {
            name: "PatientOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                name: "Patient".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        });
        // Point Task.patient at Patient — should resolve cleanly.
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::UnresolvedRelationTarget),
            "Resolvable relation must not warn. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the relation points at an entity that is not declared
    /// anywhere in the schema — expect ORB_X_UNRESOLVED_RELATION_TARGET.
    #[test]
    fn test_relation_targets_dangling_warns() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Dangling relation must produce exactly one ORB_X_UNRESOLVED_RELATION_TARGET. \
             Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("orbitals[0].entity.fields[0]"));
    }

    /// Relations nested inside an array's `items.relation.entity` must also
    /// be validated, proving the recursive walker works.
    #[test]
    fn test_relation_targets_nested_in_array_items() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patientList".to_string()),
                field_type: FieldType::Array,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: Some(Box::new(FieldDefinition {
                    name: None,
                    field_type: FieldType::Relation,
                    required: None,
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: Some(RelationDefinition {
                        entity: "Patient".to_string(),
                        cardinality: None,
                    }),
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                })),
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains(".items.relation.entity"));
    }
}
