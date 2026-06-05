//! Enrich Phase
//!
//! Adds derived information and defaults to the schema.

use orbital_core::schema::{
    EntityDefinition, EntityRef, Orbital, OrbitalSchema, PageRef, TraitDefinition, TraitReference,
};
use std::collections::HashMap;

/// Enrichment configuration
#[derive(Debug, Clone, Default)]
pub struct EnrichmentConfig {
    /// Add default collection names for entities
    pub add_default_collections: bool,
    /// Add id field if missing
    pub add_id_field: bool,
    /// Add timestamp fields (createdAt, updatedAt)
    pub add_timestamps: bool,
    /// Infer initial state from first state
    pub infer_initial_state: bool,
}

impl EnrichmentConfig {
    /// Create config with all enrichments enabled
    pub fn all() -> Self {
        Self {
            add_default_collections: true,
            add_id_field: true,
            add_timestamps: true,
            infer_initial_state: true,
        }
    }
}

/// Enrich context for tracking cross-references
#[derive(Debug, Default)]
pub struct EnrichmentContext {
    /// Entity usage: entity_name -> list of trait names that use it
    pub entity_used_by_traits: HashMap<String, Vec<String>>,
    /// Entity usage: entity_name -> list of page names that use it
    pub entity_used_by_pages: HashMap<String, Vec<String>>,
    /// Trait usage: trait_name -> list of page names that use it
    pub trait_used_by_pages: HashMap<String, Vec<String>>,
}

/// Enrich a schema with derived information
pub fn enrich_schema(schema: &mut OrbitalSchema, config: &EnrichmentConfig) {
    let mut ctx = EnrichmentContext::default();

    // First pass: enrich individual orbitals
    for orbital in &mut schema.orbitals {
        enrich_orbital(orbital, config);
    }

    // Second pass: build cross-references
    build_cross_references(schema, &mut ctx);
}

fn enrich_orbital(orbital: &mut Orbital, config: &EnrichmentConfig) {
    // Enrich entity only if it's inline (not a reference)
    if let EntityRef::Inline(ref mut entity) = orbital.entity {
        enrich_entity(entity, config);
    }

    // Enrich inline traits
    for trait_ref in &mut orbital.traits {
        if let TraitReference::Inline(trait_def) = trait_ref {
            enrich_trait(trait_def, config);
        }
    }
}

fn enrich_entity(entity: &mut EntityDefinition, config: &EnrichmentConfig) {
    // Add default collection name
    if config.add_default_collections && entity.collection.is_none() {
        entity.collection = Some(to_snake_case(&entity.name));
    }

    // Note: Adding fields would require knowing the FieldDefinition structure
    // which uses FieldType enum. For now, we just set collection names.
}

fn enrich_trait(trait_def: &mut TraitDefinition, config: &EnrichmentConfig) {
    // Infer initial state from first state if none is marked as initial
    if config.infer_initial_state && !trait_def.state_machine.states.is_empty() {
        let has_initial = trait_def
            .state_machine
            .states
            .iter()
            .any(|s| s.is_initial == Some(true));

        if !has_initial {
            // Mark the first state as initial
            if let Some(first_state) = trait_def.state_machine.states.first_mut() {
                first_state.is_initial = Some(true);
            }
        }
    }
}

fn build_cross_references(schema: &OrbitalSchema, ctx: &mut EnrichmentContext) {
    for orbital in &schema.orbitals {
        // Track trait -> entity references
        for trait_ref in &orbital.traits {
            match trait_ref {
                TraitReference::Inline(trait_def) => {
                    if let Some(linked) = &trait_def.linked_entity {
                        ctx.entity_used_by_traits
                            .entry(linked.clone())
                            .or_default()
                            .push(trait_def.name.clone());
                    }
                }
                TraitReference::Reference {
                    linked_entity,
                    reference,
                    name,
                    ..
                } => {
                    if let Some(linked) = linked_entity {
                        let trait_name = reference
                            .as_ref()
                            .or(name.as_ref())
                            .map(|s| s.clone())
                            .unwrap_or_default();
                        ctx.entity_used_by_traits
                            .entry(linked.clone())
                            .or_default()
                            .push(trait_name);
                    }
                }
            }
        }

        // Track page -> trait and page -> entity references
        if let Some(pages) = &orbital.pages {
            for page_ref in pages {
                // Only process inline pages (not references)
                if let PageRef::Inline(page) = page_ref {
                    // Track page -> trait references
                    if let Some(traits) = &page.traits {
                        for trait_ref in traits {
                            if let Some(trait_name) = trait_ref.trait_name() {
                                ctx.trait_used_by_pages
                                    .entry(trait_name.to_string())
                                    .or_default()
                                    .push(page.name.clone());
                            }
                        }
                    }

                    // Track page -> entity references from sections
                    if let Some(sections) = &page.sections {
                        for section in sections {
                            // Check props for entity references
                            if let Some(props) = &section.props {
                                if let Some(entity_value) = props.get("entity") {
                                    if let orbital_core::schema::SExpression::String(entity) =
                                        entity_value
                                    {
                                        ctx.entity_used_by_pages
                                            .entry(entity.clone())
                                            .or_default()
                                            .push(page.name.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Convert PascalCase to snake_case
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("Task"), "task");
        assert_eq!(to_snake_case("TaskItem"), "task_item");
        assert_eq!(to_snake_case("HTTPServer"), "h_t_t_p_server");
    }
}
