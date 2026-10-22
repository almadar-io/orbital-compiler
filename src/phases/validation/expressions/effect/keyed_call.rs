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
    ["success", "failure", "onMessage", "cancelled"];

/// Operator heads that start a keyed call.
pub(super) const KEYED_CALL_OPERATORS: [&str; 3] = ["call-service", "call_service", "http"];

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
            Some(opts)
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
        if !orbital_core_base::type_compat::oir_type_satisfies(
            &inferred,
            &orbital_core_base::ir::OirType::String,
        ) {
            result.add_error(ValidationError::error(
                ErrorCode::CallServiceKeyType,
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
                        Some(SExpression::Object(o)) => Some(o),
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

/// How a keyed call names its key, for the dead-key rule: a literal text, or something only run time knows.
enum KeyText {
    Literal(String),
    Dynamic,
}

fn key_text(key: &SExpression) -> KeyText {
    match key {
        SExpression::String(text) if !text.starts_with('@') && !text.starts_with('?') => {
            KeyText::Literal(text.clone())
        }
        SExpression::Array(items) => match (
            items.first().and_then(SExpression::as_str),
            items.get(1),
            items.get(2),
        ) {
            (Some("as"), Some(SExpression::String(text)), Some(target))
                if target.as_str() == Some("CallKey")
                    && !text.starts_with('@')
                    && !text.starts_with('?') =>
            {
                KeyText::Literal(text.clone())
            }
            _ => KeyText::Dynamic,
        },
        _ => KeyText::Dynamic,
    }
}

/// `ORB_CALL_KEY_UNUSED`: a literal `(as "k" CallKey)` in a program whose every keyed call is keyed by a literal,
/// none of them `"k"`, names a key nothing runs under. A call keyed by a computed value could carry any text,
/// so the rule stays silent then.
pub(crate) fn validate_call_key_literals_are_used(schema: &OrbitalSchema) -> ValidationResult {
    struct Scan {
        casts: Vec<(String, String)>,
        keys: Vec<KeyText>,
    }
    fn scan(expr: &SExpression, path: &str, out: &mut Scan) {
        match expr {
            SExpression::Array(arr) => {
                if let Some(SExpression::String(head)) = arr.first() {
                    if head == "as" {
                        if let (Some(SExpression::String(text)), Some(target)) =
                            (arr.get(1), arr.get(2))
                        {
                            if target.as_str() == Some("CallKey")
                                && !text.starts_with('@')
                                && !text.starts_with('?')
                            {
                                out.casts.push((path.to_string(), text.clone()));
                            }
                        }
                    }
                    let keyed = if KEYED_CALL_OPERATORS.contains(&head.as_str()) {
                        trailing_options(arr).and_then(declared_key)
                    } else {
                        on_device_or_tab_key(head, arr).or_else(|| substrate_key(head, arr))
                    };
                    if let Some(key) = keyed {
                        out.keys.push(key_text(key));
                    }
                }
                for item in arr {
                    scan(item, path, out);
                }
            }
            SExpression::Object(obj) => {
                for v in obj.values() {
                    scan(v, path, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Scan {
        casts: Vec::new(),
        keys: Vec::new(),
    };
    for (trait_def, path) in super::super::inline_traits(schema) {
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            if let Some(guard) = &transition.guard {
                scan(
                    guard,
                    &format!("{path}.stateMachine.transitions[{i}].guard"),
                    &mut out,
                );
            }
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                scan(
                    effect,
                    &format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
                    &mut out,
                );
            }
        }
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                scan(effect, &format!("{path}.ticks[{i}].effects[{j}]"), &mut out);
            }
        }
    }
    let mut result = ValidationResult::new();
    if out.keys.iter().any(|k| matches!(k, KeyText::Dynamic)) {
        return result;
    }
    for (path, text) in out.casts {
        if out
            .keys
            .iter()
            .any(|k| matches!(k, KeyText::Literal(l) if *l == text))
        {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::CallKeyUnused,
                path,
                format!("`(as \"{text}\" CallKey)` names a key, but no call in this program runs under \"{text}\""),
            )
            .with_suggestion(format!(
                "Key the call it should reach with `key: (as \"{text}\" CallKey)`, or use the key the call declares"
            )),
        );
    }
    result
}

/// Effects whose options take `emit` but carry no `key`: a key written there never reaches the completion.
const UNKEYED_EMIT_OPERATORS: [&str; 2] = ["persist", "fetch"];

/// `ORB_EFFECT_OPTION_DROPPED`: an options object declaring `emit` and `key` on an effect that carries no key
/// (G-ORB-425). The completion would arrive without `key`, so a `?key` guard on its arm could never match.
pub(crate) fn validate_effect_options_carry_their_key(schema: &OrbitalSchema) -> ValidationResult {
    fn scan(expr: &SExpression, path: &str, result: &mut ValidationResult) {
        match expr {
            SExpression::Array(arr) => {
                if let Some(head) = arr.first().and_then(SExpression::as_str) {
                    if UNKEYED_EMIT_OPERATORS.contains(&head) {
                        let dropped = arr[1..].iter().any(|arg| {
                            matches!(arg, SExpression::Object(o) if o.contains_key("emit") && o.contains_key("key"))
                        });
                        if dropped {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::EffectOptionDropped,
                                    path,
                                    format!("`{head}` carries no `key`, so its completion would never echo the `key` its options declare"),
                                )
                                .with_suggestion(format!("Remove `key` from the `{head}` options, or tell the arm apart by its event")),
                            );
                        }
                    }
                }
                for item in arr {
                    scan(item, path, result);
                }
            }
            SExpression::Object(obj) => {
                for v in obj.values() {
                    scan(v, path, result);
                }
            }
            _ => {}
        }
    }

    let mut result = ValidationResult::new();
    for (trait_def, path) in super::super::inline_traits(schema) {
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                scan(effect, &format!("{path}.stateMachine.transitions[{i}].effects[{j}]"), &mut result);
            }
        }
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                scan(effect, &format!("{path}.ticks[{i}].effects[{j}]"), &mut result);
            }
        }
    }
    result
}

/// `ORB_CALL_SERVICE_KEY_DUPLICATE`: a transition runs its effects as one step, and a step refuses a second
/// `call-service` under a key already used in it (the second fails with the step's visible error). Two such calls
/// in the transition's own effect list, with the same literal `key`, can never both run.
pub(crate) fn validate_call_service_keys_are_unique_per_transition(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in super::super::inline_traits(schema) {
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            let mut seen: Vec<&str> = Vec::new();
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                let SExpression::Array(arr) = effect else { continue };
                if !matches!(arr.first().and_then(SExpression::as_str), Some("call-service" | "call_service")) {
                    continue;
                }
                let Some(SExpression::String(key)) = trailing_options(arr).and_then(declared_key) else { continue };
                if key.starts_with('@') || key.starts_with('?') {
                    continue;
                }
                if seen.contains(&key.as_str()) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CallServiceKeyDuplicate,
                            format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
                            format!("call-service key '{key}' is already used by another call in this transition"),
                        )
                        .with_suggestion("Give each call-service of a transition its own key"),
                    );
                } else {
                    seen.push(key);
                }
            }
        }
    }
    result
}
