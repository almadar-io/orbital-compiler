//! Cross-Orbital Validation
//!
//! Validates cross-orbital concerns: event wiring.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

use super::{ValidationContext, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;

mod event_wiring;

pub(crate) use event_wiring::{
    collect_async_result_events, collect_failure_events, validate_event_wiring,
};

/// Validate cross-orbital concerns
pub fn validate_cross_orbital(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    result.merge(validate_event_wiring(schema));

    ctx.merge(result);
}
