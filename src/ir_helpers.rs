//! IR Resolution Helpers
//!
//! Helper functions for resolving S-expressions to OIR.

use crate::phases::resolve::{ResolveError, Resolver};
use orbital_core_base::ir::OirExpr;
use orbital_core_base::schema::SExpression;

/// Helper for binary operations
pub fn binary_op<F>(
    resolver: &Resolver,
    args: &[SExpression],
    f: F,
) -> Result<OirExpr, ResolveError>
where
    F: FnOnce(OirExpr, OirExpr) -> OirExpr,
{
    if args.len() < 2 {
        return Err(ResolveError::InvalidSExpr(
            "Binary operation requires 2 arguments".to_string(),
        ));
    }
    let left = resolver.resolve_sexpr(&args[0])?;
    let right = resolver.resolve_sexpr(&args[1])?;
    Ok(f(left, right))
}

/// Helper for unary operations
pub fn unary_op<F>(resolver: &Resolver, args: &[SExpression], f: F) -> Result<OirExpr, ResolveError>
where
    F: FnOnce(OirExpr) -> OirExpr,
{
    if args.is_empty() {
        return Err(ResolveError::InvalidSExpr(
            "Unary operation requires 1 argument".to_string(),
        ));
    }
    let operand = resolver.resolve_sexpr(&args[0])?;
    Ok(f(operand))
}
