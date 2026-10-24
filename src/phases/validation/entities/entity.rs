//! Entity Validation
//!
//! Validates what the std `orb lint` entity rules do not own: the trait-union name shadow (field defaults are
//! std-orb-lint-entity-semantic-default's, relation targets std-orb-lint-relation-targets').

use super::{ErrorCode, ValidationContext, ValidationError};
use orbital_core_base::schema::{EntityDefinition, OrbitalSchema};

/// Validate all entities in a schema
pub fn validate_all_entities(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.is_spec() {
            continue;
        }
        // The primary entity AND every auxiliary entity (multi-entity
        // orbitals, e.g. a relation target declared inline alongside the
        // trait-bound entity) get the SAME per-entity validation — every
        // entity-field rule, not only the primary's. An auxiliary entity was
        // previously invisible to this whole function (`orbital.entity` was
        // the only thing walked), so every entity-field rule was blind to
        // it, not just this one.
        let mut entities: Vec<(&EntityDefinition, String)> = Vec::new();
        if let Some(entity) = orbital.entity.as_inline() {
            entities.push((entity, format!("orbitals[{}].entity", i)));
        }
        for (j, aux) in orbital.auxiliary_entities.iter().flatten().enumerate() {
            if let Some(entity) = aux.as_inline() {
                entities.push((entity, format!("orbitals[{}].auxiliaryEntities[{}]", i, j)));
            }
        }

        for (entity, path) in entities {
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
                let Some(trait_name) = t.trait_name() else {
                    continue;
                };
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
