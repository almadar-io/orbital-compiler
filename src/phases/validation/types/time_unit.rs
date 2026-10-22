//! Time-unit literal validation.
//!
//! Every `time/*` operator that takes a unit resolves it through
//! `orbital_core::evaluator::TimeUnit::parse`, which errors on anything outside
//! the vocabulary. That error arrives at *evaluation* time, and an evaluation
//! error inside a read predicate drops the row fail-closed — so
//! `std-calendar.lolo` shipped a date-range filter whose `month`/`year` branches
//! threw on every row, the calendar silently showed nothing, and the run still
//! reported PASS (`S-CALENDAR-RANGE-FILTER-USES-UNSUPPORTED-TIME-UNITS`).
//!
//! A unit that is a *literal* in the source is knowable at validate time, so it
//! is checked here instead. A unit computed at runtime (a binding, a nested
//! expression) is not knowable and is left to the evaluator.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_eval::evaluator::TimeUnit;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition};

/// `time/*` operators whose unit argument sits at a known position. `time/diff`
/// and `time/isSame` take an OPTIONAL unit, which is why the index is checked
/// against the actual argument count rather than assumed present.
const UNIT_ARG_INDEX: &[(&str, usize)] = &[
    ("time/add", 3),
    ("time/subtract", 3),
    ("time/diff", 3),
    ("time/isSame", 3),
    ("time/startOf", 2),
    ("time/endOf", 2),
];

pub fn validate_all_time_units(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        ctx.merge(validate_trait_time_units(trait_def, &path));
    }
}

pub fn validate_trait_time_units(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        if let Some(guard) = &transition.guard {
            walk(guard, &format!("{}.guard", trans_path), &mut result);
        }
        if let Some(effects) = &transition.effects {
            for (k, effect) in effects.iter().enumerate() {
                walk(
                    effect,
                    &format!("{}.effects[{}]", trans_path, k),
                    &mut result,
                );
            }
        }
    }

    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (k, effect) in tick.effects.iter().enumerate() {
                walk(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, k),
                    &mut result,
                );
            }
        }
    }

    result
}

/// Recurse through arrays AND objects — the reported instance lived in a
/// `filter:` prop of a `render-ui` component, i.e. inside an object nested in an
/// effect, which an array-only walk would never reach.
fn walk(expr: &SExpression, path: &str, result: &mut ValidationResult) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(op)) = arr.first() {
                if let Some((_, idx)) = UNIT_ARG_INDEX.iter().find(|(name, _)| name == op) {
                    // Only a literal spelling is knowable here; a computed unit
                    // is the evaluator's problem.
                    if let Some(SExpression::String(unit)) = arr.get(*idx) {
                        if TimeUnit::parse(unit).is_err() {
                            result.add_error(ValidationError::error(
                                ErrorCode::OperatorInvalidTimeUnit,
                                format!("{}[{}]", path, idx),
                                format!(
                                    "'{}' is not a time unit. `{}` accepts: {}. \
                                     An unknown unit throws at evaluation time, and inside a read \
                                     predicate that silently drops every row.",
                                    unit,
                                    op,
                                    orbital_core_eval::evaluator::TIME_UNIT_VOCABULARY
                                ),
                            ));
                        }
                    }
                }
            }
            for (i, child) in arr.iter().enumerate() {
                walk(child, &format!("{}[{}]", path, i), result);
            }
        }
        SExpression::Object(map) => {
            for (key, child) in map {
                walk(child, &format!("{}.{}", path, key), result);
            }
        }
        _ => {}
    }
}
