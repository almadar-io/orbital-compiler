use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::OrbitalSchema;
use std::collections::HashMap;

/// Validate entity names are unique across all orbitals
pub(crate) fn validate_entity_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut entity_locations: HashMap<&str, Vec<usize>> = HashMap::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        // Only check inline entities, not references
        if let Some(entity) = orbital.entity.as_inline() {
            entity_locations.entry(&entity.name).or_default().push(i);
        }
    }

    for (entity_name, locations) in entity_locations {
        if locations.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalDuplicateEntity,
                    "orbitals",
                    format!(
                        "Entity '{}' is defined in multiple orbitals: {:?}",
                        entity_name,
                        locations
                            .iter()
                            .map(|i| &schema.orbitals[*i].name)
                            .collect::<Vec<_>>()
                    ),
                )
                .with_suggestion("Each entity name must be unique across all orbitals"),
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{EntityDefinition, EntityRef, Orbital, OrbitalSchema};

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
                    identity: false,
                    read_policy: None,
                    create_policy: None,
                    update_policy: None,
                    delete_policy: None,
                    access_waivers: None,
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                    id: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
                id: None,
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
        }
    }

    #[test]
    fn test_valid_schema() {
        let schema = create_minimal_schema();
        let result = validate_entity_uniqueness(&schema);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_duplicate_entity() {
        let mut schema = create_minimal_schema();
        schema.orbitals.push(Orbital {
            name: "AnotherOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Task".to_string(), // Duplicate
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        });

        let result = validate_entity_uniqueness(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalDuplicateEntity));
    }
}
