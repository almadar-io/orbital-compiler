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
