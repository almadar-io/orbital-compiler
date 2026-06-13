//! Canonical `OirExpr → OirType` inference — the single derivation of "what
//! type does this expression evaluate to", shared by the validator (the
//! `(set @field <expr>)` type check, Gap G-LOLO-SET-OP-RETURN) and the TS
//! codegen (operator-result / array-element typing, replacing the parallel
//! `infer_oirexpr_element_type_ts`).
//!
//! Operator result types come from the canonical `@almadar/std` metadata
//! (`canonical_operators`): the static `return_type` for fixed-return operators,
//! and the **`return_semantics` lattice** for the input-dependent ones
//! (`element-of-arg<0>?`, `branch-union`, `object-key-lookup`, `lambda-result`,
//! `identity-of-arg<0>`, first/last-truthy). This is a *generic interpreter* of
//! that lattice — no per-operator special-casing. Where a result is genuinely
//! unknowable (an untagged imprecise op, an open object-key lookup), it infers
//! `Json` so downstream compat (`orbital_core::oir_type_satisfies_field`) is
//! conservative and never false-positives. Widening precision is an upstream
//! `@almadar/std` `return_semantics` change, never per-op Rust code.

use crate::phases::validation::canonical_operators::get_canonical_operator;
use orbital_core::ir::{OirBinaryOp, OirBinding, OirExpr, OirType, OirUnaryOp};

/// Binding-resolution context the inferencer needs (and only that). Each
/// consumer supplies its own — the validator from the resolved entity/payload
/// field types, the codegen from its `CodegenContext` catalog — so the
/// inferencer itself is layer-agnostic.
pub trait TypeCtx {
    /// The declared type of a binding (`@entity.f` / `@payload.f` / `@config.f`),
    /// or `None` when it can't be resolved (→ inferred `Json`, conservative).
    fn binding_type(&self, binding: &OirBinding) -> Option<OirType>;
}

/// Map a canonical operator `return_type` string to an `OirType`. Imprecise
/// kinds (`any`/`object`/`json`/`void`/ML types) collapse to `Json` so compat
/// stays conservative.
fn return_type_to_oir(rt: &str) -> OirType {
    match rt {
        "string" => OirType::String,
        "number" | "integer" => OirType::Number,
        "boolean" => OirType::Boolean,
        "date" => OirType::Date,
        "datetime" => OirType::DateTime,
        "array" => OirType::Array { inner: Box::new(OirType::Json) },
        "number[]" => OirType::Array { inner: Box::new(OirType::Number) },
        _ => OirType::Json, // any / object / json / void / function / nn|tensor|train …
    }
}

/// Infer the `OirType` a `(set @field <expr>)`-style value expression evaluates to.
pub fn infer_oir_expr_type<C: TypeCtx>(expr: &OirExpr, ctx: &C) -> OirType {
    match expr {
        OirExpr::Null => OirType::Json,
        OirExpr::Bool { .. } => OirType::Boolean,
        OirExpr::Number { .. } => OirType::Number,
        OirExpr::String { .. } => OirType::String,
        OirExpr::Array { items } => {
            let inner = items
                .first()
                .map(|e| infer_oir_expr_type(e, ctx))
                .unwrap_or(OirType::Json);
            OirType::Array { inner: Box::new(inner) }
        }
        OirExpr::Object { .. } => OirType::Json,
        OirExpr::Binding { binding } => ctx.binding_type(binding).unwrap_or(OirType::Json),
        OirExpr::BinaryOp { op, .. } => match op {
            OirBinaryOp::Add | OirBinaryOp::Sub | OirBinaryOp::Mul | OirBinaryOp::Div
            | OirBinaryOp::Mod => OirType::Number,
            OirBinaryOp::Eq | OirBinaryOp::Ne | OirBinaryOp::Lt | OirBinaryOp::Gt
            | OirBinaryOp::Le | OirBinaryOp::Ge => OirType::Boolean,
            OirBinaryOp::Concat => OirType::String,
            // `and`/`or` return a truthy operand (JS semantics) — union, unknowable.
            OirBinaryOp::And | OirBinaryOp::Or => OirType::Json,
        },
        OirExpr::UnaryOp { op, .. } => match op {
            OirUnaryOp::Not => OirType::Boolean,
            OirUnaryOp::Neg => OirType::Number,
        },
        OirExpr::Conditional { then_branch, else_branch, .. } => {
            let t = infer_oir_expr_type(then_branch, ctx);
            let e = infer_oir_expr_type(else_branch, ctx);
            if t == e { t } else { OirType::Json } // branch-union, conservative
        }
        OirExpr::Call { function, args } => infer_call(function, args, ctx),
        // Map/Reduce produce values whose element/result type needs the bound
        // loop var's type; conservative without over-reaching.
        OirExpr::Map { .. } => OirType::Array { inner: Box::new(OirType::Json) },
        OirExpr::Filter { source, .. } => infer_oir_expr_type(source, ctx), // identity
        OirExpr::Reduce { initial, .. } => infer_oir_expr_type(initial, ctx),
        OirExpr::Lambda { .. } | OirExpr::Effect { .. } => OirType::Json,
    }
}

fn infer_call<C: TypeCtx>(function: &str, args: &[OirExpr], ctx: &C) -> OirType {
    let Some(meta) = get_canonical_operator(function) else {
        return OirType::Json; // unknown operator — validator rejects separately
    };
    match meta.return_semantics {
        None => return_type_to_oir(meta.return_type),
        Some(sem) => apply_return_semantics(sem, args, ctx),
    }
}

/// The `return_semantics` lattice — a generic interpreter, no per-operator code.
fn apply_return_semantics<C: TypeCtx>(sem: &str, args: &[OirExpr], ctx: &C) -> OirType {
    let arg0 = || args.first().map(|a| infer_oir_expr_type(a, ctx)).unwrap_or(OirType::Json);
    match sem {
        // element of the arg<0> array (may be undefined)
        "element-of-arg<0>?" => {
            let inner = match arg0() {
                OirType::Array { inner } => *inner,
                _ => OirType::Json,
            };
            OirType::Optional { inner: Box::new(inner) }
        }
        // the arg<0> array passes through unchanged (filter/reject/sort/…)
        "identity-of-arg<0>" => arg0(),
        // union of the branches/args, or an open object-key lookup — unknowable.
        "branch-union" | "first-truthy-of-args" | "last-truthy-of-args"
        | "object-key-lookup" | "lambda-result" => OirType::Json,
        _ => OirType::Json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoCtx;
    impl TypeCtx for NoCtx {
        fn binding_type(&self, _: &OirBinding) -> Option<OirType> {
            None
        }
    }

    fn call(name: &str, args: Vec<OirExpr>) -> OirExpr {
        OirExpr::Call { function: name.to_string(), args }
    }

    #[test]
    fn literals_and_binops() {
        assert_eq!(infer_oir_expr_type(&OirExpr::Number { value: 1.0 }, &NoCtx), OirType::Number);
        assert_eq!(
            infer_oir_expr_type(
                &OirExpr::BinaryOp {
                    op: OirBinaryOp::Concat,
                    left: Box::new(OirExpr::String { value: "a".into() }),
                    right: Box::new(OirExpr::String { value: "b".into() }),
                },
                &NoCtx
            ),
            OirType::String
        );
    }

    #[test]
    fn static_return_type_op() {
        // str/concat → string (static returnType)
        assert_eq!(
            infer_oir_expr_type(
                &call("str/concat", vec![OirExpr::String { value: "a".into() }]),
                &NoCtx
            ),
            OirType::String
        );
    }

    #[test]
    fn input_dependent_element_of_arg() {
        // array/first over a [Number] array → Optional<Number>
        let arr = OirExpr::Array { items: vec![OirExpr::Number { value: 1.0 }] };
        let got = infer_oir_expr_type(&call("array/first", vec![arr]), &NoCtx);
        assert_eq!(got, OirType::Optional { inner: Box::new(OirType::Number) });
    }

    #[test]
    fn unknowable_falls_to_json() {
        // object/get (object-key-lookup) → Json (conservative)
        assert_eq!(
            infer_oir_expr_type(&call("object/get", vec![]), &NoCtx),
            OirType::Json
        );
        // unknown operator → Json
        assert_eq!(infer_oir_expr_type(&call("totally/unknown", vec![]), &NoCtx), OirType::Json);
    }
}
