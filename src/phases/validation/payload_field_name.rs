//! G6 Rule 1: payload-schema field-name identifier check.
//!
//! Every `payloadSchema` field name on an emit or event declaration must be a
//! valid TypeScript identifier (`^[A-Za-z_][A-Za-z0-9_]*$`). Codegen turns each
//! field into a struct member, so a hand-authored name like `row.category`
//! (dotted) or `2nd` (leading digit) emits broken TS. This is the validate-layer
//! defense-in-depth for the lowering pass that flattens nested payload names.
//!
//! Severity is hard error: an un-emittable field name is a typing bug.

use orbital_core::schema::{OrbitalSchema, PayloadField, TraitReference};

use super::error::{ErrorCode, ValidationError};
use super::{trait_path, ValidationContext};

/// Run payload-field-name validation on every trait's emit and event schemas.
pub fn validate_payload_field_names(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let path = trait_path(oi, &orbital.name, ti, trait_ref);

            if let Some(emits) = &trait_def.emits {
                for emit in emits {
                    if let Some(fields) = &emit.payload_schema {
                        let where_ = format!("{}.emits({})", path, emit.event);
                        check_fields(fields, &where_, ctx);
                    }
                }
            }

            if let Some(events) = &trait_def.state_machine.events {
                for event in events {
                    if let Some(fields) = &event.payload_schema {
                        let where_ = format!("{}.events({})", path, event.key);
                        check_fields(fields, &where_, ctx);
                    }
                }
            }
        }
    }
}

fn check_fields(fields: &[PayloadField], where_: &str, ctx: &mut ValidationContext) {
    for field in fields {
        if !is_valid_ts_identifier(&field.name) {
            ctx.add_error(
                ValidationError::error(
                    ErrorCode::PayloadFieldNameInvalid,
                    format!("{}.{}", where_, field.name),
                    format!(
                        "Payload field name '{}' is not a valid TypeScript identifier",
                        field.name
                    ),
                )
                .with_suggestion(
                    "Use a single identifier (letters, digits, underscores; no dot, no space, no leading digit). For a nested shape, declare an object field with nested properties instead of a dotted name.",
                ),
            );
        }
        if let Some(nested) = &field.properties {
            let nested_where = format!("{}.{}", where_, field.name);
            check_fields(nested, &nested_where, ctx);
        }
    }
}

/// A field name is a valid TS identifier iff it matches `^[A-Za-z_][A-Za-z0-9_]*$`.
/// Underscore-leading is allowed; the event-name helper in `effect::utils`
/// requires an alphabetic first char, so it can't be reused here.
fn is_valid_ts_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::is_valid_ts_identifier;

    #[test]
    fn accepts_plain_identifiers() {
        assert!(is_valid_ts_identifier("category"));
        assert!(is_valid_ts_identifier("_private"));
        assert!(is_valid_ts_identifier("field2"));
        assert!(is_valid_ts_identifier("rowCategory"));
    }

    #[test]
    fn rejects_dotted_spaced_and_leading_digit() {
        assert!(!is_valid_ts_identifier("row.category"));
        assert!(!is_valid_ts_identifier("first name"));
        assert!(!is_valid_ts_identifier("2nd"));
        assert!(!is_valid_ts_identifier(""));
    }
}
