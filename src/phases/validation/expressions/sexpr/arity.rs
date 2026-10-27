//! Operator arity rules.

/// Operator arity rules
pub struct ArityRule {
    pub min: usize,
    pub max: Option<usize>, // None means unlimited
}

/// Get arity rules for an operator.
///
/// The canonical operator table (`embedded::get_operator`) is the one source of
/// arity; a name outside it has no rule (it is not an operator).
pub(crate) fn get_arity_rule(operator: &str) -> Option<ArityRule> {
    // Substrate ops (llm/*, workspace/*, …) are async EFFECTS — like
    // fetch/persist they carry a trailing options
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
    crate::embedded::get_operator(operator).map(|meta| ArityRule {
        min: meta.min_arity as usize,
        max: meta.max_arity.map(|m| m as usize),
    })
}
