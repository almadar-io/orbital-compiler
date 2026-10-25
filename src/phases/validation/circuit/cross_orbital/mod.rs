//! Cross-Orbital Validation
//!
//! Validates cross-orbital concerns: event wiring, circular dependencies.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

use super::{ValidationContext, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, TraitDefinition, TraitReference};

mod cycles;
mod event_wiring;
mod filter_field_written;
mod listen_starvation;
mod listens;
mod payload;
mod scope;

pub(crate) use cycles::validate_circular_dependencies;
pub(crate) use event_wiring::{
    collect_async_result_events, collect_effect_produced_events, collect_failure_events,
    extract_emit_event, transition_can_propagate, validate_event_wiring,
};
pub(crate) use filter_field_written::validate_filter_field_never_written;
pub(crate) use listen_starvation::validate_listen_route_starves_required_field;
pub(crate) use listens::{validate_listen_source_never_produces, validate_listens_integrity};
pub(crate) use payload::validate_external_emit_payloads;
pub(crate) use scope::validate_scope_boundary;

/// Validate cross-orbital concerns
pub fn validate_cross_orbital(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    // Validate event wiring
    result.merge(validate_event_wiring(schema));

    // Validate circular dependencies
    result.merge(validate_circular_dependencies(schema));

    // Validate external emit payloads
    result.merge(validate_external_emit_payloads(schema));

    // COMP-7: validate that cross-trait override wires only touch emits
    // declared `scope: 'external'`. Error severity.
    result.merge(validate_scope_boundary(schema, &ctx.packages));


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

    // A wildcard-source route on a call-keyed event whose guard never reads the key hears every caller.

    // Ported wiring-lint kind (sub-wave L1): a fetch filtered on a field no
    // persist anywhere ever writes can never match a real row. Error
    // severity, client-bound traits only.
    result.merge(validate_filter_field_never_written(schema));

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
