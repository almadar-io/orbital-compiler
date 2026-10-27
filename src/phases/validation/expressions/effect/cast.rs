//! `(as value TypeName)`: the explicit cast. The conversion relation is `type_compat::sub`; a cast is legal
//! wherever it returns any coercion, including `Explicit`; a declared type is a checked narrowing.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::set::ExprReturnCtx;
use orbital_core_base::schema::SExpression;
use orbital_core_base::type_compat::{castable, is_unknowable, oir_type_kind};

pub(super) fn check_cast(
    arr: &[SExpression],
    path: &str,
    ctx: &ExprReturnCtx<'_>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if arr.first().and_then(SExpression::as_str) != Some("as") || arr.len() != 3 {
        return result;
    }
    let Some(name) = arr[2].as_str() else {
        return result;
    };
    // A declared type is a checked narrowing (null when the value does not fit); the shape check names an undeclared one.
    let Some(target) = orbital_core_base::signature::nominal_type(name) else {
        return result;
    };
    let Ok(expr) = crate::phases::resolve::resolve_sexpr_standalone(&arr[1]) else {
        return result;
    };
    let Some(source) = crate::type_infer::infer_oir_expr_type(&expr, ctx) else {
        return result;
    };
    if is_unknowable(&source) || castable(&source, &target) {
        return result;
    }
    result.add_error(
        ValidationError::error(
            ErrorCode::OperatorArgTypeMismatch,
            path,
            format!(
                "`(as …)` argument 1 is a {}, but a {name} converts only from a string",
                oir_type_kind(&source)
            ),
        )
        .with_suggestion("Cast a string, e.g. a string field or literal"),
    );
    result
}
