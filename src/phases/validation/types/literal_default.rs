//! Phase F.3: literal-union default-value check.
//!
//! Every entity field that declares `values: ["a", "b", ...]` (a literal
//! union) AND `default: <X>` must have X be one of the declared values.
//! Mismatches produced silent drift in seeded mocks: the default in the
//! seed schema didn't satisfy the union, so the runtime's first read of
//! the field returned a value the type system claimed was impossible.
//!
//! Severity is hard error: a default that can't be the value is a typing
//! bug, not a stylistic suggestion.

use orbital_core_base::schema::{EntityRef, OrbitalSchema, SExpression};

use super::error::{ErrorCode, ValidationError};
use super::ValidationContext;

/// Run literal-default validation on every entity in the schema.
pub fn validate_literal_defaults(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let EntityRef::Inline(entity) = &orbital.entity else {
            continue;
        };
        for (fi, field) in entity.fields.iter().enumerate() {
            let Some(values) = field.values.as_ref() else {
                continue;
            };
            if values.is_empty() {
                continue;
            }
            let Some(default) = field.default.as_ref() else {
                continue;
            };
            let Some(default_str) = sexpr_as_string(default) else {
                // Non-string defaults (numbers, booleans, objects) on a
                // string-literal-union field are a different error class
                // already caught by the entity validator.
                continue;
            };
            if values.iter().any(|v| v == default_str) {
                continue;
            }
            let field_label = field
                .name
                .clone()
                .unwrap_or_else(|| format!("[index {}]", fi));
            let path = format!(
                "orbitals[{}].entity({}).fields[{}]({})",
                oi, entity.name, fi, field_label
            );
            ctx.add_error(
                ValidationError::error(
                    ErrorCode::LiteralUnionDefaultMismatch,
                    path,
                    format!(
                        "Field '{}' default '{}' is not one of the declared values [{}]",
                        field_label,
                        default_str,
                        values.join(", ")
                    ),
                )
                .with_suggestion(format!(
                    "Pick a default from [{}] or add '{}' to the values list",
                    values.join(", "),
                    default_str
                )),
            );
        }
    }
}

fn sexpr_as_string(expr: &SExpression) -> Option<&str> {
    match expr {
        SExpression::String(s) => Some(s),
        _ => None,
    }
}
