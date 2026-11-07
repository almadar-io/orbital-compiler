//! S-expression traversal and validation entry points.

use super::arity::get_arity_rule;
use super::operators::{has_namespace_hit, is_effect_category_operator, is_known_operator};
use crate::phases::validation::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{
    EntityRef, FieldDefinition, OrbitalSchema, SExpression, TraitDefinition,
};
use std::collections::HashSet;

/// Validation context for S-expressions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SExprContext {
    Guard,
    Effect,
    TickEffect,
}

/// Validate all S-expressions in a schema
pub fn validate_all_sexprs(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (trait_def, path) in super::super::inline_traits(schema) {
        let result = validate_trait_sexprs(trait_def, &path, &ctx.entity_names);
        ctx.merge(result);
    }

    // Entity-field `= default` and `derived` expressions are SExpressions too,
    // but were not covered by the trait walk above. Validate them uniformly so a
    // bad operator/arity/binding in a default or derived expression is caught at
    // the same place as transition effects (Phase 1.2 uniform SExpr coverage).
    let entity_names = ctx.entity_names.clone();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let fields = match &orbital.entity {
            EntityRef::Inline(ed) => Some(&ed.fields),
            EntityRef::Extends(ere) => ere.fields.as_ref(),
            EntityRef::Reference(_) => None,
        };
        if let Some(fields) = fields {
            let base = format!("orbitals[{}].entity.fields", oi);
            for (fi, f) in fields.iter().enumerate() {
                let r =
                    validate_field_default_sexprs(f, &format!("{}[{}]", base, fi), &entity_names);
                ctx.merge(r);
            }
        }
    }
}

/// Validate the `default`/`derived` SExpression of an entity field (recursing
/// into array `items` and object `properties` shapes). Plain literal defaults
/// are no-ops here (handled by the literal-default validator); SExpr-shaped
/// defaults (operator calls, bindings, `{ derived: … }`) get full arity +
/// binding validation.
fn validate_field_default_sexprs(
    f: &FieldDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if let Some(def) = &f.default {
        result.merge(validate_sexpr(
            def,
            &format!("{}.default", path),
            SExprContext::Effect,
            known_entities,
        ));
    }
    if let Some(items) = &f.items {
        result.merge(validate_field_default_sexprs(
            items,
            &format!("{}.items", path),
            known_entities,
        ));
    }
    if let Some(props) = &f.properties {
        for (k, p) in props {
            result.merge(validate_field_default_sexprs(
                p,
                &format!("{}.properties.{}", path, k),
                known_entities,
            ));
        }
    }
    result
}

/// Validate S-expressions in a trait
pub fn validate_trait_sexprs(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Validate guards and effects in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        // Validate guard
        if let Some(guard) = &transition.guard {
            let guard_result = validate_sexpr(
                guard,
                &format!("{}.guard", trans_path),
                SExprContext::Guard,
                known_entities,
            );
            result.merge(guard_result);
        }

        // Validate effects
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_result = validate_sexpr(
                    effect,
                    &format!("{}.effects[{}]", trans_path, j),
                    SExprContext::Effect,
                    known_entities,
                );
                result.merge(effect_result);
            }
        }
    }

    // Validate tick effects
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_result = validate_sexpr(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, j),
                    SExprContext::TickEffect,
                    known_entities,
                );
                result.merge(effect_result);
            }
        }
    }

    // Propositions are guard expressions: `@holds` of a state, `@ensures` of a transition.
    for (k, state) in trait_def.state_machine.states.iter().enumerate() {
        if let Some(holds) = &state.holds {
            let p = format!("{}.stateMachine.states[{}].holds", path, k);
            result.merge(validate_sexpr(
                holds,
                &p,
                SExprContext::Guard,
                known_entities,
            ));
        }
    }
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        for (k, ensures) in transition.ensures.iter().flatten().enumerate() {
            let p = format!("{}.stateMachine.transitions[{}].ensures[{}]", path, i, k);
            result.merge(validate_sexpr(
                ensures,
                &p,
                SExprContext::Guard,
                known_entities,
            ));
        }
    }

    result
}

/// Whether any of `args` is itself an obvious nested call (an array headed
/// by a string that is a known operator or namespaced) — see the
/// bare-namespace-hit branch of `is_sexpr_call`'s doc above. A genuine call
/// built from a namespace-dropped operator (`(join (array/map …) ", ")`)
/// carries real argument depth; a literal enum-options list of plain
/// identifiers (`(email sms push mixed)`) never does.
fn has_nested_call_arg(args: &[SExpression]) -> bool {
    args.iter().any(|a| match a {
        SExpression::Array(inner) => matches!(
            inner.first(),
            Some(SExpression::String(h)) if is_known_operator(h) || h.contains('/')
        ),
        _ => false,
    })
}

/// Validate a single S-expression
pub fn validate_sexpr(
    expr: &SExpression,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    validate_sexpr_with_locals(expr, path, context, known_entities, &HashSet::new())
}

/// The finding for a `let` binding or `fn` parameter named after a binding root.
fn shadowed_binding_root(name: &str, path: &str) -> Option<ValidationError> {
    let is_root = crate::embedded::binding_context_rules()
        .values()
        .any(|allowed| allowed.iter().any(|root| root == name));
    is_root.then(|| {
        ValidationError::error(
            ErrorCode::BindingRootShadowed,
            path,
            format!("'{name}' names a binding root, so `@{name}` in this scope reads the local and the root is unreachable"),
        )
        .with_suggestion("Rename the local")
    })
}

/// Validate a single S-expression with lambda-local variable tracking.
pub(crate) fn validate_sexpr_with_locals(
    expr: &SExpression,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            // Check if this looks like an S-expression call (first element is a known operator)
            // vs a data array (first element is a field name or literal value)
            let is_sexpr_call = if let SExpression::String(first) = &arr[0] {
                // It's an S-expression call if the first element is a known operator
                // or looks like an operator (contains '/' for namespaced ops).
                // "concat" is a special case: no longer a known operator (removed as
                // an ambiguous alias, R-EVAL-CONCAT-PATH-DIVERGENCE), but still forced
                // through the call path so it hits the unknown-operator error below
                // instead of silently passing as a literal data array.
                //
                // A BARE (non-namespaced) head that exactly matches a real
                // operator's basename under some OTHER namespace (e.g. "join"
                // vs. the canonical "str/join") is forced through the call
                // path too — L-26 (`Almadar_LOLO_Gaps.md`): before this, a
                // dropped/misremembered namespace prefix on a bare operator
                // name matched none of the conditions above, so the whole
                // expression silently passed as "literal array data" and the
                // interpreter's own unknown-operator fallback let the raw
                // operator NAME leak into the evaluated result — 0
                // errors/0 warnings on a call that could never do what it
                // was written to do.
                //
                // Gated on `has_nested_call_arg`, not just "more than one
                // element": a full corpus sweep (1234 registries) surfaced a
                // real false positive with only the element-count gate —
                // `std-marketing-campaign.lolo`'s `options: (email sms push
                // mixed)`, a genuine literal enum-options list whose head
                // "email" happens to collide with `validate/email`'s
                // basename. A real call built from a namespace-dropped
                // operator almost always carries a NESTED call as one of its
                // arguments (`(join (array/map …) ", ")`); a bare
                // parenthesized list of plain identifiers, this language's
                // established way to write literal enum data, never does.
                is_known_operator(first)
                    || first.contains('/')
                    || first == "concat"
                    || (has_namespace_hit(first) && has_nested_call_arg(&arr[1..]))
            } else {
                // Non-string first element (array, object, etc) - treat as data
                false
            };

            if is_sexpr_call {
                if let SExpression::String(operator) = &arr[0] {
                    // Detect lambda expressions: ["fn", "param", body] or ["fn", ["p1", "p2"], body]
                    if operator == "fn" && arr.len() == 3 {
                        let mut extended_locals = lambda_locals.clone();
                        if let SExpression::Array(params) = &arr[1] {
                            for param in params {
                                if let SExpression::String(name) = param {
                                    result.add_errors(shadowed_binding_root(name, &format!("{}[1]", path)));
                                    extended_locals.insert(name.clone().into_inner());
                                }
                            }
                        } else if let SExpression::String(param_name) = &arr[1] {
                            result.add_errors(shadowed_binding_root(param_name, &format!("{}[1]", path)));
                            extended_locals.insert(param_name.clone().into_inner());
                        }
                        // Validate the body with extended locals
                        result.merge(validate_sexpr_with_locals(
                            &arr[2],
                            &format!("{}[2]", path),
                            context,
                            known_entities,
                            &extended_locals,
                        ));
                        return result;
                    }
                    // Detect `let` bindings: ["let", [[name, valueExpr], ...], body].
                    // SEQUENTIAL `let`: each value-expression sees the names bound
                    // by EARLIER siblings (and the outer scope); the body sees them
                    // all. Matches both runtimes (Rust `LetOp`, JS `EffectExecutor`
                    // `let`). The name is inserted AFTER its own value is validated,
                    // so a self-reference still errors.
                    if operator == "let" && arr.len() == 3 {
                        let mut extended_locals = lambda_locals.clone();
                        match &arr[1] {
                            SExpression::Array(bindings) => {
                                for (bi, binding) in bindings.iter().enumerate() {
                                    if let SExpression::Array(pair) = binding {
                                        if pair.len() >= 2 {
                                            result.merge(validate_sexpr_with_locals(
                                                &pair[1],
                                                &format!("{}[1][{}][1]", path, bi),
                                                context,
                                                known_entities,
                                                &extended_locals,
                                            ));
                                        }
                                        if let Some(SExpression::String(name)) = pair.first() {
                                            result.add_errors(shadowed_binding_root(name, &format!("{}[1][{}][0]", path, bi)));
                                            extended_locals.insert(name.clone().into_inner());
                                        }
                                    }
                                }
                            }
                            SExpression::Object(bindings) => {
                                // Object-form bindings are unordered — credit all
                                // names first (permissive). lolo lowers `let` to the
                                // ordered array form above.
                                for name in bindings.keys() {
                                    result.add_errors(shadowed_binding_root(&name.to_string(), &format!("{}[1].{}", path, name)));
                                    extended_locals.insert(name.to_string());
                                }
                                for (name, value_expr) in bindings.iter() {
                                    result.merge(validate_sexpr_with_locals(
                                        value_expr,
                                        &format!("{}[1].{}", path, name),
                                        context,
                                        known_entities,
                                        &extended_locals,
                                    ));
                                }
                            }
                            _ => {}
                        }
                        result.merge(validate_sexpr_with_locals(
                            &arr[2],
                            &format!("{}[2]", path),
                            context,
                            known_entities,
                            &extended_locals,
                        ));
                        return result;
                    }
                    // Check if effect operator used in guard context
                    if context == SExprContext::Guard
                        && is_effect_category_operator(operator.as_str())
                    {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::SExprEffectInGuard,
                                path,
                                format!(
                                    "Effect operator '{}' cannot be used in guard context",
                                    operator
                                ),
                            )
                            .with_suggestion(
                                "Guards should only use pure operators for conditions",
                            ),
                        );
                    }

                    // Check arity. The message carries the offending call
                    // rendered as source so it stays actionable even when the
                    // resolved line is approximate (inline-spliced subtrees —
                    // config defaults, trait refs — have no spans of their
                    // own, so the line points at the reference site).
                    if let Some(arity) = get_arity_rule(operator) {
                        let arg_count = arr.len() - 1; // Exclude operator
                        if arg_count < arity.min {
                            result.add_error(ValidationError::error(
                                ErrorCode::SExprArityMismatch,
                                path,
                                format!(
                                    "Operator '{}' requires at least {} argument(s), got {} — in {}",
                                    operator, arity.min, arg_count, render_sexpr_brief(expr)
                                ),
                            ));
                        }
                        if let Some(max) = arity.max {
                            if arg_count > max {
                                let mut err = ValidationError::error(
                                    ErrorCode::SExprArityMismatch,
                                    path,
                                    format!(
                                        "Operator '{}' takes at most {} argument(s), got {} — in {}",
                                        operator, max, arg_count, render_sexpr_brief(expr)
                                    ),
                                );
                                err = err.with_suggestion(
                                    infix_in_args_suggestion(operator, &arr[1..], max)
                                        .unwrap_or_else(|| format!(
                                            "Remove the extra argument(s) — '{operator}' takes at most {max}"
                                        )),
                                );
                                result.add_error(err);
                            }
                        }
                    }

                    // A lambda belongs only in the operator's declared callback
                    // slot (registry `lambdaArgPosition`); a `(fn …)` form in any
                    // other argument position is decidable wrong argument order —
                    // both evaluators would silently consume the lambda as a value
                    // (the corpus-wide `(array/reduce arr (fn …) init)` class).
                    if operator != "fn" {
                        if let Some(lambda_pos) = crate::embedded::get_operator(operator)
                            .and_then(|m| m.lambda_arg_position)
                        {
                            for (i, arg) in arr.iter().skip(1).enumerate() {
                                if i as u32 == lambda_pos {
                                    continue;
                                }
                                if let SExpression::Array(inner) = arg {
                                    if matches!(inner.first(), Some(SExpression::String(h)) if h == "fn")
                                    {
                                        result.add_error(
                                            ValidationError::error(
                                                ErrorCode::SExprLambdaArgPosition,
                                                path,
                                                format!(
                                                    "Operator '{}' takes its lambda at argument {}, but a (fn …) form is at argument {} — wrong argument order, in {}",
                                                    operator,
                                                    lambda_pos + 1,
                                                    i + 1,
                                                    render_sexpr_brief(expr)
                                                ),
                                            )
                                            .with_suggestion(format!(
                                                "Move the lambda to argument {} — e.g. array/reduce is (array/reduce arr init (fn (acc x) …))",
                                                lambda_pos + 1
                                            )),
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // Recursively validate arguments
                    for (i, arg) in arr.iter().skip(1).enumerate() {
                        let arg_result = validate_sexpr_with_locals(
                            arg,
                            &format!("{}[{}]", path, i + 1),
                            context,
                            known_entities,
                            lambda_locals,
                        );
                        result.merge(arg_result);
                    }
                }
            } else {
                // Data array - just validate bindings within elements, don't check operators
                for (i, item) in arr.iter().enumerate() {
                    if matches!(item, SExpression::Array(_) | SExpression::Object(_)) {
                        // Recursively check nested structures for bindings
                        let item_result = validate_sexpr_with_locals(
                            item,
                            &format!("{}[{}]", path, i),
                            context,
                            known_entities,
                            lambda_locals,
                        );
                        result.merge(item_result);
                    }
                }
            }
        }
        SExpression::Object(obj) => {
            // Recursively validate object values
            for (key, value) in obj.iter() {
                let value_result = validate_sexpr_with_locals(
                    value,
                    &format!("{}.{}", path, key),
                    context,
                    known_entities,
                    lambda_locals,
                );
                result.merge(value_result);
            }
        }
        SExpression::Array(arr) => {
            // Empty array or array literal
            for (i, item) in arr.iter().enumerate() {
                let item_result = validate_sexpr_with_locals(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    known_entities,
                    lambda_locals,
                );
                result.merge(item_result);
            }
        }
        _ => {
            // Null, Bool, Number, plain String - all valid
        }
    }

    result
}

/// Render a failing s-expression as `.lolo`-flavoured source for diagnostics:
/// atoms in full, nested calls elided to `(head …)`, objects to `{…}` — the
/// top-level call stays greppable without dumping whole subtrees.
pub(crate) fn render_sexpr_brief(expr: &SExpression) -> String {
    match expr {
        SExpression::String(s) => s.clone().into_inner(),
        SExpression::Number(n) => n.to_string(),
        SExpression::Bool(b) => b.to_string(),
        SExpression::Null => "null".to_string(),
        SExpression::Object(_) => "{…}".to_string(),
        SExpression::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|it| match it {
                    SExpression::Array(inner) => match inner.first() {
                        Some(SExpression::String(h)) => format!("({h} …)"),
                        _ => "(…)".to_string(),
                    },
                    other => render_sexpr_brief(other),
                })
                .collect();
            format!("({})", parts.join(" "))
        }
    }
}

/// Detect an infix operator written INSIDE a prefix call's argument list —
/// `(object/get @item reviewCount > 0)` for `(> (object/get @item
/// reviewCount) 0)` — and suggest the prefix rewrite built from the actual
/// pieces. Only consulted once the call has ALREADY failed max-arity (the
/// call is wrong regardless; this only guesses the intent), and never for a
/// token in first position (the rewrite needs a non-empty left side).
fn infix_in_args_suggestion(operator: &str, args: &[SExpression], _max: usize) -> Option<String> {
    const INFIX_TOKENS: [&str; 12] = [
        ">", "<", ">=", "<=", "=", "==", "!=", "+", "-", "*", "/", "%",
    ];
    let (idx, token) = args.iter().enumerate().skip(1).find_map(|(i, a)| match a {
        SExpression::String(s) if INFIX_TOKENS.contains(&s.as_str()) => Some((i, s.clone())),
        _ => None,
    })?;
    let left: Vec<String> = args[..idx].iter().map(render_sexpr_brief).collect();
    let right: Vec<String> = args[idx + 1..].iter().map(render_sexpr_brief).collect();
    Some(format!(
        "s-expressions are prefix — '{token}' cannot appear inside the argument list. Wrap the call instead: ({token} ({operator} {}) {})",
        left.join(" "),
        right.join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proposition_is_checked_like_a_guard() {
        let program = |ensures: serde_json::Value, holds: serde_json::Value| -> TraitDefinition {
            serde_json::from_value(serde_json::json!({
                "name": "T",
                "stateMachine": {
                    "states": [{ "name": "s", "isInitial": true, "holds": holds }],
                    "transitions": [{ "from": "s", "to": "s", "event": "E", "ensures": [ensures] }]
                }
            }))
            .unwrap()
        };
        let codes = |t: &TraitDefinition| -> Vec<ErrorCode> {
            validate_trait_sexprs(t, "t", &HashSet::new())
                .errors
                .into_iter()
                .map(|e| e.code)
                .collect()
        };
        let ok = serde_json::json!([">=", "@entity.n", 0]);
        assert!(
            codes(&program(ok.clone(), ok.clone())).is_empty(),
            "control: known operators"
        );
        let effect = serde_json::json!(["set", "@entity.n", 0]);
        assert!(
            codes(&program(effect.clone(), ok.clone())).contains(&ErrorCode::SExprEffectInGuard),
            "@ensures"
        );
        assert!(
            codes(&program(ok, effect)).contains(&ErrorCode::SExprEffectInGuard),
            "@holds"
        );
    }

    #[test]
    fn g_orb_434_a_let_or_fn_named_after_a_binding_root_is_refused() {
        let codes = |expr: serde_json::Value| -> Vec<ErrorCode> {
            let expr: SExpression = serde_json::from_value(expr).unwrap();
            validate_sexpr(&expr, "p", SExprContext::Effect, &HashSet::new())
                .errors
                .into_iter()
                .map(|e| e.code)
                .collect()
        };
        for root in ["entity", "payload", "pages"] {
            let shadowing = serde_json::json!(["let", [[root, 1, "{}"]], ["+", 1, 2]]);
            assert!(codes(shadowing).contains(&ErrorCode::BindingRootShadowed), "let {root}");
            let lambda = serde_json::json!(["array/map", "@entity.xs", ["fn", [root], ["+", 1, 2]]]);
            assert!(codes(lambda).contains(&ErrorCode::BindingRootShadowed), "fn {root}");
        }
        let plain = serde_json::json!(["let", [["total", 1, "{}"]], ["+", "@total", 2]]);
        assert!(!codes(plain).contains(&ErrorCode::BindingRootShadowed), "control: an ordinary local name");
    }

    #[test]
    fn test_valid_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String(("=".to_string()).into()),
            SExpression::String(("@entity.status".to_string()).into()),
            SExpression::String(("active".to_string()).into()),
        ].into());

        let result = validate_sexpr(&guard, "test", SExprContext::Guard, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_effect_in_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String(("set".to_string()).into()),
            SExpression::String(("@entity.status".to_string()).into()),
            SExpression::String(("active".to_string()).into()),
        ].into());

        let result = validate_sexpr(&guard, "test", SExprContext::Guard, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprEffectInGuard));
    }

    #[test]
    fn test_arity_mismatch() {
        let expr = SExpression::Array(vec![
            SExpression::String(("not".to_string()).into()),
            // not takes exactly 1 argument, but we give 2
            SExpression::Bool(true),
            SExpression::Bool(false),
        ].into());

        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprArityMismatch));
    }

    fn reduce_lambda() -> SExpression {
        SExpression::Array(vec![
            SExpression::String(("fn".to_string()).into()),
            SExpression::Array(vec![
                SExpression::String(("acc".to_string()).into()),
                SExpression::String(("x".to_string()).into()),
            ].into()),
            SExpression::Array(vec![
                SExpression::String(("+".to_string()).into()),
                SExpression::String(("@acc".to_string()).into()),
                SExpression::String(("@x".to_string()).into()),
            ].into()),
        ].into())
    }

    #[test]
    fn reduce_lambda_in_init_slot_is_rejected() {
        // The silent corpus-wide class: (array/reduce arr (fn …) init).
        let expr = SExpression::Array(vec![
            SExpression::String(("array/reduce".to_string()).into()),
            SExpression::String(("@entity.items".to_string()).into()),
            reduce_lambda(),
            SExpression::Number(0.0),
        ].into());
        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprLambdaArgPosition));
    }

    #[test]
    fn reduce_spec_order_is_clean() {
        let expr = SExpression::Array(vec![
            SExpression::String(("array/reduce".to_string()).into()),
            SExpression::String(("@entity.items".to_string()).into()),
            SExpression::Number(0.0),
            reduce_lambda(),
        ].into());
        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SExprLambdaArgPosition),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn reduce_legacy_raw_body_reducer_is_not_flagged() {
        // JS runtime accepts a raw body expression (binds @acc/@item) at the
        // reducer slot; the position check must not require a (fn …) there.
        let expr = SExpression::Array(vec![
            SExpression::String(("array/reduce".to_string()).into()),
            SExpression::String(("@entity.items".to_string()).into()),
            SExpression::Number(0.0),
            SExpression::Array(vec![
                SExpression::String(("+".to_string()).into()),
                SExpression::String(("@acc".to_string()).into()),
                SExpression::String(("@item".to_string()).into()),
            ].into()),
        ].into());
        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SExprLambdaArgPosition),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn map_lambda_in_array_slot_is_rejected() {
        let expr = SExpression::Array(vec![
            SExpression::String(("array/map".to_string()).into()),
            reduce_lambda(),
            SExpression::String(("@entity.items".to_string()).into()),
        ].into());
        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprLambdaArgPosition));
    }

    #[test]
    fn arity_mismatch_message_renders_the_offending_call() {
        // Run 19 (C-LOWERED-OPERATOR-LEAK, refined): the failing expression
        // lived inside an inline-spliced config default, so the resolved line
        // pointed at the reference site. The message must carry the call
        // itself so the author can find it by searching.
        let expr = SExpression::Array(vec![
            SExpression::String(("object/get".to_string()).into()),
            SExpression::String(("@item".to_string()).into()),
            SExpression::String(("reviewCount".to_string()).into()),
            SExpression::String((">".to_string()).into()),
            SExpression::Number(0.0),
        ].into());

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::SExprArityMismatch)
            .expect("arity error expected");
        assert!(
            err.message.contains("(object/get @item reviewCount > 0)"),
            "message should render the call, got: {}",
            err.message
        );
        // The infix `>` sits past object/get's max arity — the suggestion
        // must show the prefix rewrite built from the actual pieces.
        let suggestion = err.suggestion.as_deref().expect("suggestion expected");
        assert!(
            suggestion.contains("(> (object/get @item reviewCount) 0)"),
            "suggestion should show the prefix rewrite, got: {suggestion}"
        );
    }

    #[test]
    fn arity_mismatch_without_infix_token_gets_generic_suggestion() {
        let expr = SExpression::Array(vec![
            SExpression::String(("not".to_string()).into()),
            SExpression::Bool(true),
            SExpression::Bool(false),
        ].into());

        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::SExprArityMismatch)
            .expect("arity error expected");
        let suggestion = err.suggestion.as_deref().expect("suggestion expected");
        assert!(
            suggestion.contains("Remove the extra argument"),
            "got: {suggestion}"
        );
    }


    #[test]
    fn test_valid_entity_binding() {
        let expr = SExpression::String(("@Task.title".to_string()).into());

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &known_entities);
        assert!(result.is_valid());
    }

    #[test]
    fn test_math_clamp_valid() {
        // ["math/clamp", "@entity.output", 0.0, 1.0]
        let expr = SExpression::Array(vec![
            SExpression::String(("math/clamp".to_string()).into()),
            SExpression::String(("@entity.output".to_string()).into()),
            SExpression::Number(0.0),
            SExpression::Number(1.0),
        ].into());

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_math_pow_arity_error() {
        // math/pow requires exactly 2 arguments
        let expr = SExpression::Array(vec![
            SExpression::String(("math/pow".to_string()).into()),
            SExpression::String(("@entity.base".to_string()).into()),
            // Missing second argument
        ].into());

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprArityMismatch));
    }

    #[test]
    fn test_is_known_operator() {
        // Core operators
        assert!(is_known_operator("="));
        assert!(is_known_operator("and"));

        // Effect operators
        assert!(is_known_operator("set"));
        assert!(is_known_operator("fetch"));

        // Math operators
        assert!(is_known_operator("math/clamp"));
        assert!(is_known_operator("math/pow"));

        // Unknown
        assert!(!is_known_operator("unknown/op"));
        assert!(!is_known_operator("foo"));
    }
}
