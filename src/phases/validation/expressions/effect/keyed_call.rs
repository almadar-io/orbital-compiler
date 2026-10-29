//! The key / emit contract shared by every keyed call (`call-service`, `http`).
//!
//! A keyed call is an async one-shot whose trailing options object carries
//! `key` and/or `emit { success, failure, cancelled, onMessage }`; a
//! `(cancel-call <key>)` aborts it by key.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::set::ExprReturnCtx;
use orbital_core_base::schema::{OrbitalSchema, SExpression};

/// The `emit` slots a keyed call may declare (`onMessage` is accepted as the
/// camelCase spelling of `on_message`).
pub(super) const KEYED_CALL_EMIT_KEYS: [&str; 4] =
    ["success", "failure", "onMessage", "cancelled"];

/// Operator heads that start a keyed call.
pub(super) const KEYED_CALL_OPERATORS: [&str; 1] = ["call-service"];

/// The on-device model effects: keyed like a call, with their options as the last argument.
pub(super) const ON_DEVICE_OPERATORS: [&str; 2] = ["browser/prompt", "browser/embed"];

/// The `key` an on-device model or tab call declares in its options object: the last argument of a model call, the
/// third element of a `browser/tab-*` call (its request is the second).
pub(super) fn on_device_or_tab_key<'a>(head: &str, arr: &'a [SExpression]) -> Option<&'a SExpression> {
    let options = if ON_DEVICE_OPERATORS.contains(&head) {
        arr.last()
    } else if orbital_core_base::tab::TabOp::is_tab_effect(head) {
        arr.get(2)
    } else {
        None
    };
    match options {
        Some(SExpression::Object(o)) => declared_key(o),
        _ => None,
    }
}

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
            Some(&**opts)
        }
        _ => None,
    }
}

/// The `key` a substrate or program op declares beside its `emit` routes (EXEC-QUEUE-13): the trailing options object
/// the kernel splits off, which carries both. A keyed substrate op is a call `cancel-call` reaches.
pub(super) fn substrate_key<'a>(head: &str, arr: &'a [SExpression]) -> Option<&'a SExpression> {
    if !orbital_core_base::ir::is_substrate_operator(head) {
        return None;
    }
    match arr.last() {
        Some(SExpression::Object(o)) if arr.len() > 1 && o.contains_key("emit") => declared_key(o),
        _ => None,
    }
}

pub(super) fn declared_key(options: &orbital_core_base::schema::SObject) -> Option<&SExpression> {
    options.get("key")
}

/// Whether any `call-service`, `http` or on-device call in the program carries a `key`.
pub(crate) fn program_has_keyed_call(schema: &OrbitalSchema) -> bool {
    fn scan(expr: &SExpression) -> bool {
        match expr {
            SExpression::Array(arr) => {
                if let Some(SExpression::String(head)) = arr.first() {
                    if KEYED_CALL_OPERATORS.contains(&head.as_str())
                        && trailing_options(arr).is_some_and(|o| declared_key(o).is_some())
                    {
                        return true;
                    }
                    if on_device_or_tab_key(head, arr).is_some() || substrate_key(head, arr).is_some() {
                        return true;
                    }
                }
                arr.iter().any(scan)
            }
            SExpression::Object(obj) => obj.values().any(scan),
            _ => false,
        }
    }
    super::super::inline_traits(schema).any(|(trait_def, _)| {
        let transitions = trait_def
            .state_machine
            .transitions
            .iter()
            .flat_map(|t| t.effects.iter().flatten());
        let ticks = trait_def
            .ticks
            .iter()
            .flatten()
            .flat_map(|t| t.effects.iter());
        transitions.chain(ticks).any(scan)
    })
}

/// Whether `expr` is typed `CallKey` or a list of them: made deliberately with `as`, not any string.
fn is_call_key_typed(expr: &SExpression, ctx: &ExprReturnCtx<'_>) -> bool {
    use orbital_core_base::ir::OirType;
    let Ok(resolved) = crate::phases::resolve::resolve_sexpr_standalone(expr) else {
        return false;
    };
    match crate::type_infer::infer_oir_expr_type(&resolved, ctx) {
        Some(OirType::CallKey) => true,
        Some(OirType::Array { inner }) => matches!(*inner, OirType::CallKey),
        _ => false,
    }
}

/// What one finished program does with `CallKey`s: the sites that cancel one, and how many keyed calls run under a
/// `CallKey`-typed `key:`.
#[derive(Default)]
pub(crate) struct CallKeyFacts {
    pub(crate) typed_cancels: Vec<String>,
    pub(crate) call_key_calls: usize,
}

/// Records into `facts` every typed `cancel-call` and every `CallKey`-keyed call inside `expr`, at any depth.
pub(super) fn collect_call_key_facts(
    expr: &SExpression,
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    facts: &mut CallKeyFacts,
) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if head == "cancel-call" && arr.get(1).is_some_and(|arg| is_call_key_typed(arg, ctx)) {
                    facts.typed_cancels.push(path.to_string());
                }
                let options = if KEYED_CALL_OPERATORS.contains(&head.as_str()) {
                    trailing_options(arr)
                } else if ON_DEVICE_OPERATORS.contains(&head.as_str()) {
                    match arr.last() {
                        Some(SExpression::Object(o)) => Some(&**o),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(key) = options.and_then(declared_key) {
                    if is_call_key_typed(key, ctx) && !matches!(key, SExpression::Null) {
                        facts.call_key_calls += 1;
                    }
                }
            }
            for item in arr {
                collect_call_key_facts(item, path, ctx, facts);
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                collect_call_key_facts(value, path, ctx, facts);
            }
        }
        _ => {}
    }
}

/// `ORB_CANCEL_CALL_NO_KEYED_CALL`: a `cancel-call` aborts a call by key. A `CallKey`-typed argument, or a list of them, is exempt:
/// the type says the key was made deliberately and is supplied by whoever composes the program. A string
/// argument needs some call in the program to carry a `key`, or the cancel can never reach anything.
pub(super) fn check_cancel_call_arg(
    arr: &[SExpression],
    path: &str,
    ctx: &ExprReturnCtx<'_>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if arr.first().and_then(SExpression::as_str) != Some("cancel-call") {
        return result;
    }
    let Some(schema) = ctx.schema else {
        return result;
    };
    if arr.get(1).is_some_and(|arg| is_call_key_typed(arg, ctx)) {
        return result;
    }
    if program_has_keyed_call(schema) {
        return result;
    }
    result.add_error(
        ValidationError::error(
            ErrorCode::CancelCallNoKeyedCall,
            path,
            "cancel-call has nothing to cancel: no call-service, http or substrate call in this app carries a `key`",
        )
        .with_suggestion(
            "Add `key: <string expression>` to the options of the call to cancel, or cancel a `CallKey` (`(as <string> CallKey)`) the composing program supplies",
        ),
    );
    result
}

/// The `key` of a keyed call that is a typed expression (not a literal) must evaluate to a string; the literal keys
/// (null, empty, non-string) are the std lint `keyed-call-keys`.
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
        if !orbital_core_base::type_compat::oir_type_satisfies(
            &inferred,
            &orbital_core_base::ir::OirType::String,
        ) {
            result.add_error(ValidationError::error(
                ErrorCode::OperatorArgTypeMismatch,
                path,
                format!(
                    "{operator} `key` must evaluate to a string, got {}",
                    orbital_core_base::type_compat::oir_type_kind(&inferred)
                ),
            ));
        }
    }
    result
}

/// The literal kind of a param value, or None when the value is dynamic
/// (bindings, computed s-expressions, null) and must not be type-checked.
pub(super) fn literal_kind(value: &SExpression) -> Option<&'static str> {
    match value {
        SExpression::String(s) => {
            if s.starts_with('@') || s.starts_with('?') || s.starts_with('$') {
                None
            } else {
                Some("string")
            }
        }
        SExpression::Number(_) => Some("number"),
        SExpression::Bool(_) => Some("boolean"),
        SExpression::Object(_) => Some("object"),
        // An array literal is only a literal when it isn't an s-expression
        // call — a leading string makes it a computed form.
        SExpression::Array(items) => match items.first() {
            Some(SExpression::String(_)) => None,
            _ => Some("array"),
        },
        SExpression::Null => None,
    }
}
