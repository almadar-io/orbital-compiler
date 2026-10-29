//! A record literal written into an entity row: the data of a `persist` and the object `set` into a relation field.
//! Each key must be a field of the entity, each value must be admitted by that field's declared type (an enum field by
//! its members), and a `persist create` must carry every column the kernel requires of it.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::set::{expr_return_incompatible_in_ctx, ExprReturnCtx};
use orbital_core_base::ir::types::field_shape;
use orbital_core_base::schema::types::FRAMEWORK_STAMPED_FIELDS;
use orbital_core_base::schema::{EntityDefinition, FieldType, SExpression};
use std::collections::HashMap;

/// What a row literal is checked against: the entities the program declares, and the bindings its values read.
pub(crate) struct RowSite<'a> {
    pub(crate) entities: &'a HashMap<String, EntityDefinition>,
    pub(crate) ctx: &'a ExprReturnCtx<'a>,
}

/// Whether the literal is the whole row of a new record, so its required columns must all be present.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowWrite {
    Create,
    Partial,
}

pub(crate) fn check_row_literal(
    entity: &EntityDefinition,
    data: &SExpression,
    write: RowWrite,
    what: &str,
    path: &str,
    site: &RowSite<'_>,
    result: &mut ValidationResult,
) {
    let SExpression::Object(literal) = data else {
        return;
    };
    let declared: Vec<String> = entity
        .fields
        .iter()
        .filter_map(|f| f.name.clone())
        .collect();
    for (key, value) in literal.iter() {
        let Some(field) = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(key.as_str()))
        else {
            if !FRAMEWORK_STAMPED_FIELDS.contains(&key.as_str()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::RowLiteralUnknownField,
                        path,
                        format!("{what}: '{key}' is not a field of '{}'", entity.name),
                    )
                    .with_valid_values(declared.clone()),
                );
            }
            continue;
        };
        if let (FieldType::Enum, Some(members), SExpression::String(text)) =
            (&field.field_type, &field.values, value)
        {
            let text = text.as_str();
            if !text.starts_with(['@', '?']) && !members.iter().any(|m| m == text) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EnumValueNotMember,
                        path,
                        format!(
                            "{what}: '{text}' is not a member of the enum '{}.{key}'",
                            entity.name
                        ),
                    )
                    .with_valid_values(members.clone()),
                );
            }
            continue;
        }
        let whole_number = matches!((&field.field_type, value), (FieldType::Int, SExpression::Number(n)) if n.fract() == 0.0);
        let Ok(declared) = field_shape(field) else {
            continue;
        };
        if let Some(slot) = super::set::null_written_into_never_null_slot(value, &declared, site.ctx) {
            result.add_error(ValidationError::error(
                ErrorCode::RowLiteralFieldTypeMismatch,
                path,
                format!(
                    "{what}: '{}.{key}' builds a null where {} is declared; the declared type admits no null",
                    entity.name,
                    orbital_core_base::signature::describe_type(&slot)
                ),
            ));
            continue;
        }
        if matches!(value, SExpression::Null) || whole_number {
            continue;
        }
        if let Some(inferred) = expr_return_incompatible_in_ctx(value, &declared, site.ctx) {
            result.add_error(ValidationError::error(
                ErrorCode::RowLiteralFieldTypeMismatch,
                path,
                format!(
                    "{what}: '{}.{key}' declares {}, the value is {}",
                    entity.name,
                    orbital_core_base::signature::describe_type(&declared),
                    orbital_core_base::signature::describe_type(&inferred)
                ),
            ));
        }
    }
    if write == RowWrite::Create {
        let missing: Vec<String> = entity
            .required_create_field_names()
            .into_iter()
            .filter(|name| !literal.contains_key(name.as_str()))
            .collect();
        if !missing.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::RowLiteralMissingRequired,
                    path,
                    format!(
                        "{what}: required column(s) {} of '{}' missing",
                        missing.join(", "),
                        entity.name
                    ),
                )
                .with_valid_values(missing),
            );
        }
    }
}
