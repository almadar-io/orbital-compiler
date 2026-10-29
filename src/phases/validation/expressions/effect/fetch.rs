//! fetch effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{EntityDefinition, FieldType, SExpression};

/// The types of a fetch `nearest`'s `vector` (a `[number]`) and `k` (a number), over the one `OirExpr` inference.
/// An expression whose type is not determined never rejects.
pub(super) fn check_fetch_nearest(
    arr: &[SExpression],
    path: &str,
    ctx: &super::set::ExprReturnCtx<'_>,
) -> ValidationResult {
    use orbital_core_base::ir::OirType;
    let mut result = ValidationResult::new();
    if arr.first().and_then(SExpression::as_str) != Some("fetch") {
        return result;
    }
    let Some(SExpression::Object(options)) = arr.get(2) else {
        return result;
    };
    let Some(SExpression::Object(nearest)) = options.get("nearest") else {
        return result;
    };
    for (member, expected) in [
        ("vector", OirType::Array { inner: Box::new(OirType::Number) }),
        ("k", OirType::Number),
    ] {
        let Some(expr) = nearest.get(member) else {
            continue;
        };
        let Ok(resolved) = crate::phases::resolve::resolve_sexpr_standalone(expr) else {
            continue;
        };
        let Some(inferred) = crate::type_infer::infer_oir_expr_type(&resolved, ctx) else {
            continue;
        };
        if !orbital_core_base::type_compat::oir_type_satisfies(&inferred, &expected) {
            result.add_error(ValidationError::error(
                ErrorCode::OperatorArgTypeMismatch,
                path,
                format!(
                    "fetch `nearest.{member}` must be a {}, got {}",
                    orbital_core_base::type_compat::oir_type_kind(&expected),
                    orbital_core_base::type_compat::oir_type_kind(&inferred)
                ),
            ));
        }
    }
    result
}

/// Parse a fetch `include` option's `SExpression` into the literal field
/// names it names. `None` when `include_expr` isn't an array of strings (the
/// caller, `validate_fetch_include`, reports that shape error itself). The
/// single reader of this shape — shared with the compiler's field-usage
/// collector (`phases/inline/entity.rs`) so a fetch-include reference counts
/// toward "this trait references the field" the same way validation reads it.
pub(crate) fn parse_fetch_include_literals(include_expr: &SExpression) -> Option<Vec<String>> {
    match include_expr {
        SExpression::Array(arr) => Some(
            crate::phases::inline::rewrite::strip_literal_list_marker(arr)
                .iter()
                .filter_map(|item| {
                    if let SExpression::String(s) = item {
                        Some(s.to_string())
                    } else {
                        None
                    }
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Resolve a fetch `include` field literal to the entity FIELD name it
/// designates: either a relation field's own name, or that field's base name
/// with the `Id` suffix stripped (`"company"` -> `"companyId"`). `None` when
/// it matches no relation field on `entity_def` — `validate_fetch_include`
/// reports that as an error; the field-usage collector simply doesn't count
/// it as a reference. The single matcher for this rule (no second copy).
pub(crate) fn resolve_fetch_include_field(
    entity_def: &EntityDefinition,
    include_field: &str,
) -> Option<String> {
    entity_def
        .fields
        .iter()
        .find(|f| f.field_type == FieldType::Relation && f.name.as_deref() == Some(include_field))
        .and_then(|f| f.name.clone())
        .or_else(|| {
            entity_def
                .fields
                .iter()
                .filter(|f| f.field_type == FieldType::Relation)
                .find_map(|f| {
                    let field_name = f.name.as_ref()?;
                    let base_name = field_name.strip_suffix("Id").unwrap_or(field_name);
                    (base_name == include_field).then(|| field_name.clone())
                })
        })
}
