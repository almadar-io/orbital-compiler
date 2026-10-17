//! The key / emit contract shared by every keyed call (`call-service`, `http`).
//!
//! A keyed call is an async one-shot whose trailing options object carries
//! `key` and/or `emit { success, failure, cancelled, onMessage }`; a
//! `(cancel-call <key>)` aborts it by key.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::call_service::literal_kind;
use super::extract_emit_object_ref;
use super::set::ExprReturnCtx;
use orbital_core_base::schema::{OrbitalSchema, SExpression};

/// The `emit` slots a keyed call may declare (`onMessage` is accepted as the
/// camelCase spelling of `on_message`).
pub(super) const KEYED_CALL_EMIT_KEYS: [&str; 4] =
    ["success", "failure", "on_message", "cancelled"];

/// Operator heads that start a keyed call.
pub(super) const KEYED_CALL_OPERATORS: [&str; 3] = ["call-service", "call_service", "http"];

/// The on-device model effects: keyed like a call, with their options as the last argument.
pub(super) const ON_DEVICE_OPERATORS: [&str; 2] = ["browser/prompt", "browser/embed"];

/// The trailing options object of a keyed-call array (head included): the
/// last element past arity 4, an object carrying `emit` and/or `key`. Mirrors
/// the resolver's split — a plain trailing object is the args payload, not
/// options.
pub(super) fn trailing_options(arr: &[SExpression]) -> Option<&orbital_core_base::schema::SObject> {
    if arr.len() <= 4 {
        return None;
    }
    match arr.last() {
        Some(SExpression::Object(opts))
            if opts.contains_key("emit") || opts.contains_key("key") =>
        {
            Some(opts)
        }
        _ => None,
    }
}

pub(super) fn declared_key(options: &orbital_core_base::schema::SObject) -> Option<&SExpression> {
    options.get("key")
}

/// `key` must be string-typed: a non-string literal can never name a call;
/// expressions (bindings, calls) are not decidable here and pass.
pub(super) fn check_key_and_cancelled(
    options: &orbital_core_base::schema::SObject,
    path: &str,
    operator: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let key = declared_key(options);
    if matches!(key, Some(SExpression::Null))
        || matches!(key, Some(SExpression::String(text)) if text.is_empty())
    {
        result.add_error(ValidationError::error(
            ErrorCode::CallServiceKeyType,
            path,
            format!("{operator} `key` must be a non-empty string"),
        ));
    }
    if let Some(kind) = key.and_then(literal_kind) {
        if kind != "string" {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CallServiceKeyType,
                    path,
                    format!("{operator} `key` must be a string, got a {kind} literal"),
                )
                .with_suggestion(
                    "Use a string literal or a string-typed expression such as @entity.batchId",
                ),
            );
        }
    }
    let declares_cancelled = matches!(
        extract_emit_object_ref(options).and_then(|e| e.get("cancelled")),
        Some(v) if !matches!(v, SExpression::Null)
    );
    if declares_cancelled && matches!(key, None | Some(SExpression::Null)) {
        result.add_error(
            ValidationError::error(
                ErrorCode::CallServiceCancelledWithoutKey,
                path,
                "emit.cancelled is declared but the call has no `key` — nothing can cancel an unkeyed call",
            )
            .with_suggestion(format!(
                "Add `key: <string expression>` to the {operator} options, and cancel it with (cancel-call <key>)"
            )),
        );
    }
    result
}

pub(super) fn check_key_expression_type(
    arr: &[SExpression],
    path: &str,
    ctx: &ExprReturnCtx<'_>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(operator) = arr.first().and_then(SExpression::as_str) else {
        return result;
    };
    if !KEYED_CALL_OPERATORS.contains(&operator) {
        return result;
    }
    let Some(key) = trailing_options(arr).and_then(declared_key) else {
        return result;
    };
    if literal_kind(key).is_some() || matches!(key, SExpression::Null) {
        return result;
    }
    let Ok(expr) = crate::phases::resolve::resolve_sexpr_standalone(key) else {
        return result;
    };
    if let Some(inferred) = crate::type_infer::infer_oir_expr_type(&expr, ctx) {
        if !orbital_core_base::type_compat::oir_type_satisfies(&inferred, &orbital_core_base::ir::OirType::String) {
            result.add_error(ValidationError::error(
                ErrorCode::CallServiceKeyType,
                path,
                format!("{operator} `key` must evaluate to a string, got {}", orbital_core_base::type_compat::oir_type_kind(&inferred)),
            ));
        }
    }
    result
}

/// `ORB_CANCEL_CALL_NO_KEYED_CALL`: a `cancel-call` aborts a call by key, so an
/// app that runs one while no keyed call (`call-service` or `http`) anywhere
/// carries a `key` has a cancel that can never reach anything.
pub(crate) fn validate_cancel_calls_have_a_keyed_call(schema: &OrbitalSchema) -> ValidationResult {
    fn scan(expr: &SExpression, path: &str, keyed: &mut bool, cancels: &mut Vec<String>) {
        match expr {
            SExpression::Array(arr) => {
                if let Some(SExpression::String(head)) = arr.first() {
                    if KEYED_CALL_OPERATORS.contains(&head.as_str()) {
                        if trailing_options(arr).is_some_and(|o| declared_key(o).is_some()) {
                            *keyed = true;
                        }
                    } else if ON_DEVICE_OPERATORS.contains(&head.as_str()) {
                        if matches!(arr.last(), Some(SExpression::Object(o)) if declared_key(o).is_some()) {
                            *keyed = true;
                        }
                    } else if head == "cancel-call" {
                        cancels.push(path.to_string());
                    }
                }
                for item in arr {
                    scan(item, path, keyed, cancels);
                }
            }
            SExpression::Object(obj) => {
                for v in obj.values() {
                    scan(v, path, keyed, cancels);
                }
            }
            _ => {}
        }
    }

    let mut keyed = false;
    let mut cancels: Vec<String> = Vec::new();
    for (trait_def, path) in super::super::inline_traits(schema) {
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                scan(effect, &effect_path, &mut keyed, &mut cancels);
            }
        }
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                scan(effect, &effect_path, &mut keyed, &mut cancels);
            }
        }
    }
    let mut result = ValidationResult::new();
    if keyed {
        return result;
    }
    for path in cancels {
        result.add_error(
            ValidationError::error(
                ErrorCode::CancelCallNoKeyedCall,
                path,
                "cancel-call has nothing to cancel: no call-service or http call in this app carries a `key`",
            )
            .with_suggestion(
                "Add `key: <string expression>` to the options of the call to cancel",
            ),
        );
    }
    result
}
