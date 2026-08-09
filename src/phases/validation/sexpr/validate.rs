//! S-expression traversal and validation entry points.

use super::arity::get_arity_rule;
use super::binding::validate_binding;
use super::operators::{is_known_operator, EFFECT_OPERATORS};
use crate::phases::validation::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
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

    result
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
                // or looks like an operator (contains '/' for namespaced ops)
                is_known_operator(first) || first.contains('/')
            } else {
                // Non-string first element (array, object, etc) - treat as data
                false
            };

            if is_sexpr_call {
                if let SExpression::String(operator) = &arr[0] {
                    // Detect lambda expressions: ["fn", "param", body] or ["lambda", ["p1", "p2"], body]
                    if (operator == "fn" || operator == "lambda") && arr.len() == 3 {
                        let mut extended_locals = lambda_locals.clone();
                        if let SExpression::Array(params) = &arr[1] {
                            for param in params {
                                if let SExpression::String(name) = param {
                                    extended_locals.insert(name.clone());
                                }
                            }
                        } else if let SExpression::String(param_name) = &arr[1] {
                            extended_locals.insert(param_name.clone());
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
                                            extended_locals.insert(name.clone());
                                        }
                                    }
                                }
                            }
                            SExpression::Object(bindings) => {
                                // Object-form bindings are unordered — credit all
                                // names first (permissive). lolo lowers `let` to the
                                // ordered array form above.
                                for name in bindings.keys() {
                                    extended_locals.insert(name.clone());
                                }
                                for (name, value_expr) in bindings {
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
                        && EFFECT_OPERATORS.contains(&operator.as_str())
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

                    // Check operator validity (only warn for namespaced operators that aren't known)
                    if !is_known_operator(operator) && operator.contains('/') {
                        let suggestion = super::operators::suggest_operator(operator)
                            .unwrap_or_else(|| {
                                "Use a known operator or ensure custom operator is registered"
                                    .to_string()
                            });
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::SExprInvalidOperator,
                                path,
                                format!("Unknown operator: '{}'", operator),
                            )
                            .with_suggestion(suggestion),
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
                    // Only validate bindings (@...) within data arrays
                    if let SExpression::String(s) = item {
                        if s.starts_with('@') {
                            let binding_result = validate_binding(
                                s,
                                &format!("{}[{}]", path, i),
                                context,
                                known_entities,
                                lambda_locals,
                            );
                            result.merge(binding_result);
                        }
                    } else if matches!(item, SExpression::Array(_) | SExpression::Object(_)) {
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
        SExpression::String(s) if s.starts_with('@') => {
            // Validate binding
            let binding_result = validate_binding(s, path, context, known_entities, lambda_locals);
            result.merge(binding_result);
        }
        SExpression::Object(obj) => {
            // Recursively validate object values
            for (key, value) in obj {
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
        SExpression::String(s) => s.clone(),
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
    fn test_valid_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String("=".to_string()),
            SExpression::String("@entity.status".to_string()),
            SExpression::String("active".to_string()),
        ]);

        let result = validate_sexpr(&guard, "test", SExprContext::Guard, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_effect_in_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.status".to_string()),
            SExpression::String("active".to_string()),
        ]);

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
            SExpression::String("not".to_string()),
            // not takes exactly 1 argument, but we give 2
            SExpression::Bool(true),
            SExpression::Bool(false),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprArityMismatch));
    }

    #[test]
    fn arity_mismatch_message_renders_the_offending_call() {
        // Run 19 (C-LOWERED-OPERATOR-LEAK, refined): the failing expression
        // lived inside an inline-spliced config default, so the resolved line
        // pointed at the reference site. The message must carry the call
        // itself so the author can find it by searching.
        let expr = SExpression::Array(vec![
            SExpression::String("object/get".to_string()),
            SExpression::String("@item".to_string()),
            SExpression::String("reviewCount".to_string()),
            SExpression::String(">".to_string()),
            SExpression::Number(0.0),
        ]);

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
            SExpression::String("not".to_string()),
            SExpression::Bool(true),
            SExpression::Bool(false),
        ]);

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
    fn test_payload_in_tick() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.value".to_string()),
            SExpression::String("@payload.data".to_string()),
        ]);

        let result = validate_sexpr(&effect, "test", SExprContext::TickEffect, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn test_valid_entity_binding() {
        let expr = SExpression::String("@Task.title".to_string());

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &known_entities);
        assert!(result.is_valid());
    }

    #[test]
    fn test_unknown_entity_binding() {
        let expr = SExpression::String("@NonExistent.title".to_string());

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &known_entities);
        // This is a warning, not an error
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprUnknownEntity));
    }

    #[test]
    fn test_nn_forward_valid() {
        // ["nn/forward", "@entity.architecture", "@entity.sensors"]
        let expr = SExpression::Array(vec![
            SExpression::String("nn/forward".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            SExpression::String("@entity.sensors".to_string()),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_nn_sequential_valid() {
        // ["nn/sequential", ["nn/linear", 16, 64], ["nn/relu"], ["nn/linear", 64, 4]]
        let expr = SExpression::Array(vec![
            SExpression::String("nn/sequential".to_string()),
            SExpression::Array(vec![
                SExpression::String("nn/linear".to_string()),
                SExpression::Number(16.0),
                SExpression::Number(64.0),
            ]),
            SExpression::Array(vec![SExpression::String("nn/relu".to_string())]),
            SExpression::Array(vec![
                SExpression::String("nn/linear".to_string()),
                SExpression::Number(64.0),
                SExpression::Number(4.0),
            ]),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_tensor_clamp_valid() {
        // ["tensor/clamp", "@entity.output", 0.0, 1.0]
        let expr = SExpression::Array(vec![
            SExpression::String("tensor/clamp".to_string()),
            SExpression::String("@entity.output".to_string()),
            SExpression::Number(0.0),
            SExpression::Number(1.0),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_train_loop_valid() {
        // ["train/loop", "@entity.architecture", "@entity.buffer", "@entity.config"]
        let expr = SExpression::Array(vec![
            SExpression::String("train/loop".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            SExpression::String("@entity.buffer".to_string()),
            SExpression::String("@entity.config".to_string()),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_nn_forward_arity_error() {
        // nn/forward requires exactly 2 arguments
        let expr = SExpression::Array(vec![
            SExpression::String("nn/forward".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            // Missing second argument
        ]);

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

        // NN operators
        assert!(is_known_operator("nn/forward"));
        assert!(is_known_operator("nn/sequential"));
        assert!(is_known_operator("nn/linear"));

        // Tensor operators
        assert!(is_known_operator("tensor/from"));
        assert!(is_known_operator("tensor/clamp"));

        // Train operators
        assert!(is_known_operator("train/loop"));
        assert!(is_known_operator("train/mse"));

        // Unknown
        assert!(!is_known_operator("unknown/op"));
        assert!(!is_known_operator("foo"));
    }
}
