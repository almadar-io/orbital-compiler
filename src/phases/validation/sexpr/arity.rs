//! Operator arity rules.

use super::operators::EFFECT_OPERATORS;

/// Operator arity rules
pub struct ArityRule {
    pub min: usize,
    pub max: Option<usize>, // None means unlimited
}

/// Get arity rules for an operator.
///
/// The canonical operator registry (`canonical-operators.json`, surfaced via
/// `embedded::get_operator`) is the source of truth for arity wherever the
/// operator is declared there. The match below is a fallback for operators not
/// yet carried in the registry (legacy aliases, ML/tensor/graph/contract/data
/// modules); every arm here is for an operator the registry does NOT define, so
/// the registry lookup always wins when both have it.
pub(crate) fn get_arity_rule(operator: &str) -> Option<ArityRule> {
    // Substrate ops (llm/*, workspace/*, …) are async EFFECTS — like
    // fetch/persist (EFFECT_OPERATORS, below) they carry a trailing options
    // object and have their own validation, so no static arity rule applies.
    // Consistent with the max-arity check in resolve_sexpr_call (resolve.rs).
    if crate::phases::resolve::is_substrate_op(operator) {
        return None;
    }
    // browser/* device effects are async client-host calls that carry a
    // trailing `{ emit: { success, failure } }` envelope beyond their real
    // positional arity (options / text). They have dedicated validation in
    // emit_declaration.rs, so no static arity rule applies — consistent with
    // the substrate guard above and the resolve_sexpr_call routing.
    if operator.starts_with("browser/") {
        return None;
    }
    if let Some(meta) = crate::embedded::get_operator(operator) {
        return Some(ArityRule {
            min: meta.min_arity as usize,
            max: meta.max_arity.map(|m| m as usize),
        });
    }
    match operator {
        // Unary
        "abs" | "floor" | "ceil" | "round" | "upper" | "lower" | "trim" | "length" | "first"
        | "last" | "rest" | "count" | "keys" | "values" | "entries" | "type" | "is-null"
        | "is-empty" | "is-string" | "is-number" | "is-boolean" | "is-array" | "is-object"
        | "reverse" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // Binary
        "eq" | "neq" | "lt" | "gt" | "lte" | "gte" | "sub" | "div" | "mod" | "get" | "nth"
        | "date-diff" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Binary or more
        "add" | "mul" | "concat" | "merge" | "min" | "max" => Some(ArityRule { min: 2, max: None }),

        // Ternary
        "substring" | "replace" | "slice" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // Variable arity
        "cond" => Some(ArityRule { min: 1, max: None }),

        // Effects have their own validation
        _ if EFFECT_OPERATORS.contains(&operator) => None,

        _ => None,
    }
}
