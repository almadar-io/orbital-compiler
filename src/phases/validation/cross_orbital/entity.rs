use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
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

/// Orbital names key the runtime and the import splice (trait prefixes, `Orbital.Trait`
/// listens sources), so a second orbital of the same name shadows or merges with the first.
pub(crate) fn validate_orbital_name_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut locations: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        locations.entry(orbital.name.as_str()).or_default().push(i);
    }
    let mut duplicated: Vec<(&str, Vec<usize>)> = locations
        .into_iter()
        .filter(|(_, at)| at.len() > 1)
        .collect();
    duplicated.sort_by_key(|(_, at)| at[0]);
    for (name, at) in duplicated {
        result.add_error(
            ValidationError::error(
                ErrorCode::OrbitalDuplicateName,
                format!("orbitals[{}]", at[1]),
                format!(
                    "Orbital '{}' is declared {} times (orbitals {:?})",
                    name,
                    at.len(),
                    at
                ),
            )
            .with_suggestion("Give each orbital its own name"),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{EntityDefinition, EntityRef, Orbital, OrbitalSchema};

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                spec: false,
                types: None,
                name: "TaskOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    timestamps: None,
                    soft_delete: None,
                    description: None,
                    visual_prompt: None,
                    asset_ref: None,
                    store: None,
                    shared: false,
                    identity: false,
                    local: false,
                    seed_mock: false,
                    locale_instances: None,
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
                    invariants: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                os_grants: None,
                design: None,
                id: None,
                reference: None,
                config: None,
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
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
            spec: false,
            types: None,
            name: "AnotherOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        });

        let result = validate_entity_uniqueness(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalDuplicateEntity));
    }

    fn second_orbital(schema: &OrbitalSchema, name: &str, entity: &str) -> Orbital {
        let mut orbital = schema.orbitals[0].clone();
        orbital.name = name.to_string();
        if let EntityRef::Inline(def) = &mut orbital.entity {
            def.name = entity.to_string();
        }
        orbital
    }

    #[test]
    fn two_orbitals_sharing_a_name_are_refused_even_when_their_contents_differ() {
        let mut schema = create_minimal_schema();
        let twin = second_orbital(&schema, "TaskOrbital", "Appointment");
        schema.orbitals.push(twin);
        let result = validate_orbital_name_uniqueness(&schema);
        let error = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::OrbitalDuplicateName)
            .expect("duplicate orbital name reported");
        assert!(error.message.contains("TaskOrbital"), "{}", error.message);
    }

    #[test]
    fn control_distinct_orbital_names_pass() {
        let mut schema = create_minimal_schema();
        let other = second_orbital(&schema, "AppointmentOrbital", "Appointment");
        schema.orbitals.push(other);
        assert!(validate_orbital_name_uniqueness(&schema).is_valid());
    }
}
