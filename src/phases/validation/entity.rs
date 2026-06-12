//! Entity Validation
//!
//! Validates entity structure, fields, types, and relations.
//! Ported from `builder/packages/shared/src/orbitals/validation/entity-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_FIELD_TYPES};
use orbital_core::schema::{
    EntityDefinition, FieldDefinition, FieldType, OrbitalSchema, Persistence,
};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Regex for valid field names (camelCase)
static FIELD_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-zA-Z0-9]*$").unwrap());

/// Regex for valid entity names (PascalCase)
static ENTITY_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// UI component names that collide with entity names in compiled TypeScript output.
/// Sourced from the single canonical `crate::pattern_registry` embed (no longer a
/// per-phase copy of component-mapping.json).
static RESERVED_COMPONENT_NAMES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    crate::pattern_registry::component_names()
        .map(|s| s.to_string())
        .collect()
});

/// Validate all entities in a schema
pub fn validate_all_entities(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let check_naming = ctx.schema_language.should_check_ascii_naming();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let path = format!("orbitals[{}].entity", i);
        // Only validate inline entities, not references (references are validated during resolution)
        if let Some(entity) = orbital.entity.as_inline() {
            let result = validate_entity(entity, &path, &ctx.entity_names, check_naming);
            ctx.merge(result);

            // Defense-in-depth pair with the lolo lowering check
            // `LowerError::EntityNameShadowsTraitUnion`. The lolo rule
            // catches hand-authored `.lolo`; this validator rule catches
            // `.orb` mutated directly (factory edits, agent rewrites,
            // raw JSON patches) that bypass the lolo parse path. Both
            // emit the same diagnostic so authors and agents see one
            // consistent error message regardless of which path they
            // took. The TS codegen would otherwise produce TS2440
            // "Import declaration conflicts with local declaration".
            for t in &orbital.traits {
                let Some(trait_name) = t.trait_name() else { continue };
                let state_shadow = format!("{}State", trait_name);
                let event_shadow = format!("{}Event", trait_name);
                if entity.name == state_shadow {
                    ctx.add_error(
                        ValidationError::error(
                            ErrorCode::EntityNameShadowsTraitUnion,
                            &path,
                            format!(
                                "Entity '{}' name collides with trait '{}'s generated state union — TS codegen would emit both `import type {{ {} }}` and `type {} = '...'` in every handler/hook/view file (TS2440). Rename the entity (e.g. '{}Data').",
                                entity.name, trait_name, entity.name, entity.name, trait_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Rename entity '{}' to '{}Data' (or any name that doesn't match a trait's <Name>State / <Name>Event union)",
                            entity.name, trait_name
                        )),
                    );
                }
                if entity.name == event_shadow {
                    ctx.add_error(
                        ValidationError::error(
                            ErrorCode::EntityNameShadowsTraitUnion,
                            &path,
                            format!(
                                "Entity '{}' name collides with trait '{}'s generated event union — TS codegen would emit both `import type {{ {} }}` and `type {} = '...'` in every handler/hook/view file (TS2440). Rename the entity (e.g. '{}Data').",
                                entity.name, trait_name, entity.name, entity.name, trait_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Rename entity '{}' to '{}Data' (or any name that doesn't match a trait's <Name>State / <Name>Event union)",
                            entity.name, trait_name
                        )),
                    );
                }
            }
        }
    }
}

/// Validate a single entity definition
///
/// When `check_naming` is false (non-Latin schemas), ASCII PascalCase/camelCase
/// checks are skipped (GAP-V001).
pub fn validate_entity(
    entity: &EntityDefinition,
    path: &str,
    all_entity_names: &HashSet<String>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Entity must have a name
    if entity.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityMissingName,
                format!("{}.name", path),
                "Entity name is required",
            )
            .with_suggestion("Add a name for your entity using PascalCase (e.g., 'Task', 'User')"),
        );
    } else if check_naming && !ENTITY_NAME_REGEX.is_match(&entity.name) {
        // GAP-V001: Only check ASCII naming for English/Latin schemas
        result.add_error(
            ValidationError::warning(
                ErrorCode::EntityInvalidFieldName,
                format!("{}.name", path),
                format!("Entity name '{}' should be PascalCase", entity.name),
            )
            .with_suggestion("Use PascalCase for entity names (e.g., 'Task', 'UserProfile')"),
        );
    }

    // Entity name must not collide with a UI component name
    if RESERVED_COMPONENT_NAMES.contains(&entity.name) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityInvalidFieldName, // reuse closest error code
                format!("{}.name", path),
                format!(
                    "Entity name '{}' collides with a UI component name. This causes duplicate identifier errors in compiled output.",
                    entity.name
                ),
            )
            .with_suggestion(format!(
                "Rename the entity to avoid the collision (e.g., '{}Entry', '{}Record', '{}Data')",
                entity.name, entity.name, entity.name
            )),
        );
    }

    // Entity must have at least one field
    if entity.fields.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityNoFields,
                format!("{}.fields", path),
                "Entity must have at least one field",
            )
            .with_suggestion("Add at least one field to define the entity's data structure"),
        );
    }

    // Validate each field
    let mut field_names: HashSet<String> = HashSet::new();
    for (i, field) in entity.fields.iter().enumerate() {
        let field_path = format!("{}.fields[{}]", path, i);
        let field_result = validate_field(
            field,
            &field_path,
            all_entity_names,
            &mut field_names,
            check_naming,
        );
        result.merge(field_result);
    }

    // Check for persistent entity collection
    if let Some(persistence) = &entity.persistence {
        if *persistence == Persistence::Persistent && entity.collection.is_none() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityMissingCollection,
                    format!("{}.collection", path),
                    "Persistent entity requires a collection name",
                )
                .with_suggestion("Add a 'collection' property for database storage"),
            );
        }
    }

    result
}

/// Validate a single field definition
///
/// When `check_naming` is false (non-Latin schemas), ASCII camelCase
/// checks are skipped (GAP-V001).
pub fn validate_field(
    field: &FieldDefinition,
    path: &str,
    all_entity_names: &HashSet<String>,
    seen_names: &mut HashSet<String>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Field must have a name
    if let Some(name) = &field.name {
        if name.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidFieldName,
                    format!("{}.name", path),
                    "Field name cannot be empty",
                )
                .with_suggestion("Provide a valid camelCase field name"),
            );
        } else {
            // Check for duplicate field names
            if !seen_names.insert(name.clone()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityDuplicateField,
                        format!("{}.name", path),
                        format!("Duplicate field name: '{}'", name),
                    )
                    .with_suggestion("Each field must have a unique name within the entity"),
                );
            }

            // Check field name format (camelCase) — skip for non-Latin schemas (GAP-V001)
            if check_naming && !FIELD_NAME_REGEX.is_match(name) {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::EntityInvalidFieldName,
                        format!("{}.name", path),
                        format!("Field name '{}' should be camelCase", name),
                    )
                    .with_suggestion(
                        "Use camelCase for field names (e.g., 'firstName', 'createdAt')",
                    ),
                );
            }
        }
    }

    // Validate field type
    let type_str = field_type_to_string(&field.field_type);
    if !VALID_FIELD_TYPES.contains(&type_str.as_str()) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityInvalidFieldType,
                format!("{}.type", path),
                format!("Invalid field type: '{}'", type_str),
            )
            .with_suggestion("Use a valid field type")
            .with_valid_values(VALID_FIELD_TYPES.iter().map(|s| s.to_string()).collect()),
        );
    }

    // Validate enum fields have values
    if field.field_type == FieldType::Enum {
        if let Some(values) = &field.values {
            if values.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityEmptyEnumValues,
                        format!("{}.values", path),
                        "Enum field must have at least one value",
                    )
                    .with_suggestion("Add values array with possible enum values"),
                );
            }
        } else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityEmptyEnumValues,
                    format!("{}.values", path),
                    "Enum field requires a 'values' array",
                )
                .with_suggestion("Add values: [\"option1\", \"option2\"] for enum fields"),
            );
        }
    }

    // Validate relation fields reference existing entities
    if field.field_type == FieldType::Relation {
        if let Some(relation) = &field.relation {
            if !all_entity_names.contains(&relation.entity) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityInvalidRelation,
                        format!("{}.relation.entity", path),
                        format!(
                            "Relation target entity '{}' does not exist",
                            relation.entity
                        ),
                    )
                    .with_suggestion("Ensure the related entity is defined in the schema")
                    .with_valid_values(all_entity_names.iter().cloned().collect()),
                );
            }
        } else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidRelation,
                    format!("{}.relation", path),
                    "Relation field requires a 'relation' configuration",
                )
                .with_suggestion("Add relation: { entity: 'EntityName', cardinality: 'one' }"),
            );
        }
    }

    result
}

/// Convert FieldType to string for validation
fn field_type_to_string(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Date => "date".to_string(),
        FieldType::Datetime => "datetime".to_string(),
        FieldType::Timestamp => "timestamp".to_string(),
        FieldType::Array => "array".to_string(),
        FieldType::Object => "object".to_string(),
        FieldType::Enum => "enum".to_string(),
        FieldType::Relation => "relation".to_string(),
        FieldType::Trait => "trait".to_string(),
        FieldType::Slot => "slot".to_string(),
        FieldType::Pattern => "pattern".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::RelationDefinition;

    fn create_valid_entity() -> EntityDefinition {
        EntityDefinition {
            name: "Task".to_string(),
            collection: Some("tasks".to_string()),
            persistence: Some(Persistence::Persistent),
            fields: vec![FieldDefinition {
                name: Some("title".to_string()),
                field_type: FieldType::String,
                required: Some(true),
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            }],
            instances: None,
        }
    }

    fn create_field(name: &str, field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            name: Some(name.to_string()),
            field_type,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    #[test]
    fn test_valid_entity() {
        let entity = create_valid_entity();
        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_entity_missing_name() {
        let mut entity = create_valid_entity();
        entity.name = String::new();

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityMissingName));
    }

    #[test]
    fn test_entity_no_fields() {
        let mut entity = create_valid_entity();
        entity.fields = vec![];

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityNoFields));
    }

    #[test]
    fn test_entity_duplicate_field() {
        let mut entity = create_valid_entity();
        entity.fields.push(create_field("title", FieldType::String)); // Duplicate

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityDuplicateField));
    }

    #[test]
    fn test_entity_enum_no_values() {
        let mut entity = create_valid_entity();
        entity.fields.push(create_field("status", FieldType::Enum));

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityEmptyEnumValues));
    }

    #[test]
    fn test_entity_invalid_relation() {
        let mut entity = create_valid_entity();
        let mut field = create_field("owner", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "NonExistent".to_string(),
            cardinality: None,
        });
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityInvalidRelation));
    }

    #[test]
    fn test_entity_valid_relation() {
        let mut entity = create_valid_entity();
        let mut field = create_field("owner", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "User".to_string(),
            cardinality: None,
        });
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string(), "User".to_string()]
            .into_iter()
            .collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(result.is_valid());
    }

    #[test]
    fn test_persistent_entity_missing_collection() {
        let mut entity = create_valid_entity();
        entity.collection = None;

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityMissingCollection));
    }
}
