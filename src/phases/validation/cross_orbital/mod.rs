//! Cross-Orbital Validation
//!
//! Validates cross-orbital concerns: entity uniqueness, event wiring, circular dependencies.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, TraitDefinition, TraitReference};

mod cycles;
mod entity;
mod event_wiring;
mod filter_field_written;
mod listen_redundant;
mod listen_starvation;
mod listen_unkeyed;
mod listen_trigger;
mod listens;
mod payload;
mod relations;
mod scope;

pub(crate) use cycles::validate_circular_dependencies;
pub(crate) use entity::{validate_entity_uniqueness, validate_orbital_name_uniqueness};
pub(crate) use event_wiring::{
    collect_async_result_events, collect_effect_produced_events, collect_failure_events,
    extract_emit_event, transition_can_propagate, validate_event_wiring,
};
pub(crate) use filter_field_written::validate_filter_field_never_written;
pub(crate) use listen_redundant::validate_listen_redundant_embed_route;
pub(crate) use listen_starvation::validate_listen_route_starves_required_field;
pub(crate) use listen_unkeyed::validate_wildcard_listen_keyed;
pub(crate) use listen_trigger::validate_listen_triggers;
pub(crate) use listens::{validate_listen_source_never_produces, validate_listens_integrity};
pub(crate) use payload::validate_external_emit_payloads;
pub(crate) use relations::validate_relation_targets;
pub(crate) use scope::validate_scope_boundary;

/// Validate cross-orbital concerns
pub fn validate_cross_orbital(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    // Validate schema-level requirements
    if schema.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaMissingName,
                "name",
                "Schema name is required",
            )
            .with_suggestion("Add a name for your schema"),
        );
    }

    if schema.orbitals.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaNoOrbitals,
                "orbitals",
                "Schema must have at least one orbital",
            )
            .with_suggestion("Add at least one orbital to define application features"),
        );
    }

    // Check version if present
    if let Some(version) = &schema.version {
        if version.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::SchemaEmptyVersion,
                    "version",
                    "Schema version should not be empty",
                )
                .with_suggestion("Use semantic versioning (e.g., '1.0.0')"),
            );
        }
    }

    // Validate entity uniqueness
    result.merge(validate_entity_uniqueness(schema));
    result.merge(validate_orbital_name_uniqueness(schema));

    // Validate event wiring
    result.merge(validate_event_wiring(schema));

    // Validate circular dependencies
    result.merge(validate_circular_dependencies(schema));

    // Validate external emit payloads
    result.merge(validate_external_emit_payloads(schema));

    // COMP-7: validate that cross-trait override wires only touch emits
    // declared `scope: 'external'`. Error severity.
    result.merge(validate_scope_boundary(schema, &ctx.packages));

    // COMP-9: validate that every `type: 'relation'` field's target entity
    // resolves to an entity declared in the same schema. Error severity; the
    // entity-scope validator also emits `ORB_E_INVALID_RELATION` for the
    // same condition.
    result.merge(validate_relation_targets(schema));

    // Cross-orbital trait isolation (gap #13). Listens integrity (error):
    // cross-trait/cross-orbital listens must resolve to a real emit on the
    // named source trait. Backstop for the L1 (`orbital-lolo`) check;
    // covers `use`-imported behaviors that inline new listens after
    // lowering. NOTE: a transition-event collision warning was considered
    // and rejected — once Phase 4 codegen emits qualified bus keys per
    // trait (`UI:ContactCreate.EDIT` vs `UI:DealCreate.EDIT`), two
    // orbitals sharing the bare identifier `EDIT` is the expected
    // consequence of CRUD being a uniform vocabulary, not a smell.
    result.merge(validate_listens_integrity(schema));

    // Second half of the listens-source resolution: the CONTRACT resolves
    // (declared in `emits[]`) but nothing live currently backs it. Error
    // severity, whatever the emit's declared scope.
    result.merge(validate_listen_source_never_produces(schema));

    // A resolved `listens` route (trait-sourced) whose trigger declares a
    // required leaf payload field the route can never actually supply —
    // neither an explicit `with{}` mapping nor (pass-through) the source's
    // declared emit contract produces it. Error severity.
    result.merge(validate_listen_route_starves_required_field(schema));

    // A same-name route from a trait the host embeds re-delivers an event the host already
    // hears on its own embed-chain scope. Error severity.
    result.merge(validate_listen_redundant_embed_route(schema));

    // A wildcard-source route on a call-keyed event whose guard never reads the key hears every caller.
    result.merge(validate_wildcard_listen_keyed(schema));

    // Ported wiring-lint kind (sub-wave L1): a fetch filtered on a field no
    // persist anywhere ever writes can never match a real row. Error
    // severity, client-bound traits only.
    result.merge(validate_filter_field_never_written(schema));

    // Validate orbital names
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.name.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalMissingName,
                    format!("orbitals[{}].name", i),
                    "Orbital name is required",
                )
                .with_suggestion("Add a descriptive name for the orbital"),
            );
        }
    }

    ctx.merge(result);
}

/// Collect all trait definitions from schema, each paired with its owning
/// orbital's name.
/// Note: Traits are now only defined inline within orbitals, not at schema level.
pub(crate) fn collect_all_traits(schema: &OrbitalSchema) -> Vec<(&str, &TraitDefinition)> {
    let mut traits: Vec<(&str, &TraitDefinition)> = Vec::new();

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                traits.push((orbital.name.as_str(), trait_def));
            }
        }
    }

    traits
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{EntityDefinition, EntityRef, Orbital};

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            theme_definitions: None,
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
    fn test_missing_schema_name() {
        let mut schema = create_minimal_schema();
        schema.name = String::new();

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaMissingName));
    }

    #[test]
    fn test_empty_orbitals() {
        let mut schema = create_minimal_schema();
        schema.orbitals = vec![];

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaNoOrbitals));
    }
}
