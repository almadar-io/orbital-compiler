//! `ORB_X_OPERATOR_ARG_TYPE_MISMATCH` (D-12): an operator argument whose static type contradicts the operator's
//! signature. Both evaluators raise the same typed mismatch for it at run time, so this is the first rung: the
//! program is rejected before it can run. The signature is the installed operator table's, resolved by
//! `orbital_core::signature` — the one resolver the runtime check drives — over the one `OirExpr → OirType`
//! inference (`crate::type_infer`), which binds the type variables of a call (so `array/append : [T], T` rejects a
//! number appended to a `[string]`) and types a lambda's parameters. An argument whose type is not determined (an
//! unresolved binding, a date field) never rejects.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::set::{BindingTypes, EntityFieldMeta, ExprReturnCtx};
use crate::type_infer::{check_oir_expr_under, is_effect, is_value_operator, TypeCtx as _};
use orbital_core_base::ir::OirExpr;
use orbital_core_base::schema::SExpression;
use orbital_core_base::signature::Mismatch;
use std::collections::HashMap;

pub(crate) fn validate_operator_arg_types(
    expr: &SExpression,
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    types: &BindingTypes,
    schema: Option<&orbital_core_base::schema::OrbitalSchema>,
) -> ValidationResult {
    validate_operator_arg_types_under(
        expr,
        None,
        path,
        entity_field_types,
        payload_field_types,
        types,
        schema,
    )
}

/// The same check where `guard` (the transition's or tick's) holds, as it does for the effects it guards.
pub(crate) fn validate_operator_arg_types_under(
    expr: &SExpression,
    guard: Option<&SExpression>,
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    types: &BindingTypes,
    schema: Option<&orbital_core_base::schema::OrbitalSchema>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
        types: Some(types),
        schema,
    };
    let condition = guard.and_then(|guard| orbital_core_eval::lower::lower_sexpr(guard).ok());
    collect(expr, path, &ctx, condition.as_ref(), &mut result, false);
    result
}

/// Walks the program for the maximal value expressions and checks each as one tree. `covered` is true inside
/// a value expression already checked as a whole; only an effect found inside it starts a new check.
fn collect(
    expr: &SExpression,
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    condition: Option<&OirExpr>,
    result: &mut ValidationResult,
    covered: bool,
) {
    match expr {
        SExpression::Object(fields) => {
            for value in fields.values() {
                collect(value, path, ctx, condition, result, covered);
            }
        }
        SExpression::Array(items) => {
            result.merge(required_field_from_missing_result(
                items, path, ctx, condition,
            ));
            result.merge(super::keyed_call::check_key_expression_type(
                items, path, ctx,
            ));
            result.merge(super::keyed_call::check_cancel_call_arg(items, path, ctx));
            result.merge(super::cast::check_cast(items, path, ctx));
            result.merge(super::fetch::check_fetch_nearest(items, path, ctx));
            result.merge(super::option_since::check_option_release(items, path));
            let mut covered = covered;
            let mut children = items.as_slice();
            if let Some(SExpression::String(head)) = items.first() {
                match head.as_str() {
                    // Syntax, not values: a quoted body.
                    "quote" | "quasiquote" => return,
                    // A lambda's parameter names are syntax; its body is a value.
                    "fn" => {
                        if let Some(body) = items.last().filter(|_| items.len() >= 3) {
                            if let Some(effect) = effect_in(body) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::EffectInLambda,
                                        path,
                                        format!(
                                            "`({effect} …)` inside a `fn` body never runs: a lambda is evaluated for its value, and an effect cannot loop over a runtime list"
                                        ),
                                    )
                                    .with_suggestion(
                                        "Emit one event that carries the whole list and let the listener handle it, re-emit an event with the rest of the list as its payload, or write the rows with `persist batch` built by `array/map`",
                                    ),
                                );
                            }
                            collect(body, path, ctx, condition, result, covered);
                        }
                        return;
                    }
                    // An effect is checked as one tree with the value expressions it is handed, so a `let` or lambda
                    // around them scopes their locals.
                    name if !covered && (is_effect(name) || is_value_operator(name)) => {
                        let mut rows = Vec::new();
                        let mut row_locals = Vec::new();
                        let expr = with_fetch_filter_rows(expr, ctx, &mut rows, &mut row_locals);
                        match with_row_locals(ctx, row_locals) {
                            Some(types) => {
                                let scoped = ExprReturnCtx {
                                    entity: ctx.entity,
                                    payload: ctx.payload,
                                    types: Some(&types),
                                    schema: ctx.schema,
                                };
                                report(&expr, path, &scoped, condition, &rows, result);
                            }
                            None => report(&expr, path, ctx, condition, &rows, result),
                        }
                        covered = true;
                    }
                    _ => {}
                }
                children = &items[1..];
            }
            for item in children {
                collect(item, path, ctx, condition, result, covered);
            }
        }
        _ => {}
    }
}

/// `(set @entity.<field> value)` where the field is required and the value is a failable operator's maybe-missing
/// result: the row would be written without the value the field promises.
fn required_field_from_missing_result(
    items: &[SExpression],
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    condition: Option<&OirExpr>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let (Some(SExpression::String(head)), Some(SExpression::String(target)), Some(value)) =
        (items.first(), items.get(1), items.get(2))
    else {
        return result;
    };
    let Some(field) = target
        .strip_prefix("@entity.")
        .filter(|f| head == "set" && !f.contains('.'))
    else {
        return result;
    };
    if !ctx.entity.get(field).is_some_and(|meta| meta.required) {
        return result;
    }
    let Ok(oir) = orbital_core_eval::lower::lower_sexpr(value) else {
        return result;
    };
    if let Some(source) = crate::type_infer::missing_failable_result(&oir, condition, ctx) {
        result.add_error(
            ValidationError::error(
                ErrorCode::OperatorNullOperand,
                path,
                format!("`{target}` is required, but the value set into it is missing when `{source}` fails on its value (it answers null then)"),
            )
            .with_suggestion(format!(
                "Say what a missing value means: default it, `(set {target} (or … default))`, branch on its presence, or make the field optional (drop `!`)"
            )),
        );
    }
    result
}

/// The first effect in a lambda body, not looking into a nested lambda (it reports its own) or a quoted program.
fn effect_in(expr: &SExpression) -> Option<String> {
    match expr {
        SExpression::Object(fields) => fields.values().find_map(effect_in),
        SExpression::Array(items) => {
            if let Some(SExpression::String(head)) = items.first() {
                match head.as_str() {
                    "quote" | "quasiquote" | "fn" => return None,
                    name if is_effect(name) => return Some(name.to_string()),
                    _ => {}
                }
            }
            items.iter().skip(1).find_map(effect_in)
        }
        _ => None,
    }
}

/// `expr` with the row operand of each `(object/get @entity k)` in a `fetch` filter over a declared entity read as
/// that entity's row: the filter's whole `@entity` there is the candidate row it admits, while `@entity.f` stays the
/// issuing trait's (`interpolate_filter_trait_refs`).
/// The row read becomes a local of the checker's own (`locals` collects each one's name and row type; the checker's
/// binding types carry them, see [`with_row_locals`]), so no declaration reaches the program.
/// `rows` collects how each row read prints, so a finding names it `@entity` as written.
fn with_fetch_filter_rows(
    expr: &SExpression,
    ctx: &ExprReturnCtx<'_>,
    rows: &mut Vec<String>,
    locals: &mut Vec<(String, orbital_core_base::ir::OirType)>,
) -> SExpression {
    fn row_reads(expr: &SExpression, row: &SExpression) -> SExpression {
        match expr {
            SExpression::Array(items) => {
                let mut items: Vec<SExpression> =
                    items.iter().map(|item| row_reads(item, row)).collect();
                if matches!(items.as_slice(), [SExpression::String(head), SExpression::String(arg), ..] if head == "object/get" && arg == "@entity")
                {
                    items[1] = row.clone();
                }
                SExpression::Array(items.into())
            }
            SExpression::Object(map) => SExpression::object(
                map.iter()
                    .map(|(k, v)| (k.clone(), row_reads(v, row)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    match expr {
        SExpression::Array(items) => {
            let mut items: Vec<SExpression> = items
                .iter()
                .map(|item| with_fetch_filter_rows(item, ctx, rows, locals))
                .collect();
            if let [SExpression::String(head), SExpression::String(target), SExpression::Object(options), ..] =
                items.as_mut_slice()
            {
                if let (true, Some(row_type)) = (head == "fetch", ctx.entity_type(target)) {
                    if let Some(filter) = options.get_mut("filter") {
                        let name = format!("fetch-row-{}", locals.len());
                        let row = SExpression::string(format!("@{name}"));
                        if let Ok(lowered) = orbital_core_eval::lower::lower_sexpr(&row) {
                            rows.push(orbital_core_base::ir::pretty_print_expr(&lowered));
                        }
                        locals.push((name, row_type));
                        *filter = row_reads(filter, &row);
                    }
                }
            }
            SExpression::Array(items.into())
        }
        SExpression::Object(map) => SExpression::object(
            map.iter()
                .map(|(k, v)| (k.clone(), with_fetch_filter_rows(v, ctx, rows, locals)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The binding types of `ctx` with each fetch row `locals` bound to its row type; `None` when there are none.
fn with_row_locals(
    ctx: &ExprReturnCtx<'_>,
    locals: Vec<(String, orbital_core_base::ir::OirType)>,
) -> Option<BindingTypes> {
    if locals.is_empty() {
        return None;
    }
    let mut types = ctx.types?.clone();
    types.locals.extend(locals);
    Some(types)
}

fn report(
    expr: &SExpression,
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    condition: Option<&OirExpr>,
    rows: &[String],
    result: &mut ValidationResult,
) {
    let Ok(oir) = orbital_core_eval::lower::lower_sexpr(expr) else {
        return;
    };
    for found in check_oir_expr_under(&oir, condition, ctx) {
        let position = found.index + 1;
        let function = &found.function;
        let call = &rows.iter().fold(found.expression.clone(), |call, row| {
            call.replace(row.as_str(), "@entity")
        });
        if let Mismatch::Undetermined { reason } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorArgUndetermined,
                    path,
                    format!(
                        "`({function} …)` argument {position}, `{argument}`, has no determined type ({reason}), so the operator's declared type cannot be checked against it, and the type variable it binds, with the type of the result, is unknown: {call}",
                        argument = found.argument
                    ),
                )
                .with_suggestion(
                    "Narrow a loose value (json, an expression, a loose union, a value of unknown type) to the declared type with `(as value T)`: checked narrowing, null on a misfit, so follow it with `(default (as value T) fallback)` where null is possible. Otherwise give the value a real type: read it from a declared payload or entity shape, declare a named union alias for a list of differently-shaped items, or annotate the binding with the declared type (`(let ((rows : [Shape] [...])) ...)`). There is no untyped escape",
                ),
            );
            continue;
        }
        if let Mismatch::MaybeAbsent { operator, operand } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrderingOptionalOperand,
                    path,
                    format!(
                        "`({operator} …)` argument {position}, `{operand}`, can be missing (an optional field with no default), and a missing value compares below any number in the evaluators but fails the comparison in a store query: {call}"
                    ),
                )
                .with_suggestion(format!(
                    "Say what a missing value means: guard it, `(and (validate/required {operand}) ({operator} {operand} …))`, or put the comparison under an `if`/`when` on its presence, or give the field a default (`= 0`) or make it required (`!`)"
                )),
            );
            continue;
        }
        if let Mismatch::NullOperand { operator, operand } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorNullOperand,
                    path,
                    if operand == "null" {
                        format!("`({operator} …)` argument {position} is the null literal, and `{operator}` raises on a null operand: {call}")
                    } else {
                        format!(
                            "`({operator} …)` argument {position}, `{operand}`, can be missing (an optional value or field with no default), and `{operator}` raises on a null operand: {call}"
                        )
                    },
                )
                .with_suggestion(format!(
                    "Say what a missing value means: default it (`(default {operand} …)`), guard it, `(and (validate/required {operand}) ({operator} {operand} …))`, or put the call under an `if`/`when` on its presence, or give the field a default or make it required (`!`)"
                )),
            );
            continue;
        }
        if let Mismatch::MissingResult {
            operator,
            operand,
            source,
        } = &found.mismatch
        {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorNullOperand,
                    path,
                    format!(
                        "`({operator} …)` argument {position}, `{operand}`, is missing when `{source}` fails on its value (it answers null then), and `{operator}` needs a value: {call}"
                    ),
                )
                .with_suggestion(format!(
                    "Say what a missing value means: default it, `(default {operand} …)`, or bind it with `let` and branch on its presence, `(if (= x null) … …)`"
                )),
            );
            continue;
        }
        if let Mismatch::NoField { path: field, record } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::ObjectPathUndeclared,
                    path,
                    format!("`({function} …)` argument {position} reads `{field}`, which is not declared by the {record} it reads: {call}"),
                )
                .with_suggestion("Name a path the type declares: a record field, a Map key (any non-empty segment), a list or tuple index. A key that contains a dot is written `a\\.b`, or read the map with `object/pick`".to_string()),
            );
            continue;
        }
        let (message, suggestion) = match &found.mismatch {
            Mismatch::Type { expected, actual } if matches!(function.as_str(), "and" | "or") => (
                format!("`({function} …)` argument {position} is a {actual}, but `{function}` combines booleans only: {call}"),
                "Make the operand a boolean (a comparison, `(validate/required x)`); `and`/`or` are not a coalesce, so use `(default x fallback)` to supply a value for a missing one".to_string(),
            ),
            Mismatch::Type { expected, actual } if expected.starts_with("an optional value") => (
                format!("`({function} …)` argument {position} is a {actual}, which is never null, but {expected}: {call}"),
                "Compare a value that can be null (`T?`), or drop the null comparison: a value of a present type needs no presence check".to_string(),
            ),
            Mismatch::Type { expected, actual } if expected == crate::type_infer::NON_NEGATIVE_WHOLE => (
                format!("`({function} …)` argument {position} is `{actual}`, which is not shown to be {expected}, and the kernel raises on a negative one: {call}"),
                "Guard it with `(>= i 0)`, or read with `array/find`".to_string(),
            ),
            Mismatch::Type { expected, actual } => (
                format!("`({function} …)` argument {position} is a {actual}, but the operator declares {expected}: {call}"),
                "Pass a value of the declared type: convert it first (e.g. `time/parse`, `json/parse`, `str/concat`), or use the operator that takes this type".to_string(),
            ),
            Mismatch::Conflict { expected, actual } => (
                format!("`({function} …)` argument {position} is a {actual}, but the arguments of one type variable must share a type and an earlier argument is a {expected}: {call}"),
                "Make the arguments one type: convert one of them, or compare/combine only values of the same type; lists of differently-shaped items are one list only under a declared type (a `set` of a declared field, or `(let ((rows : [Shape] ...)) ...)`)".to_string(),
            ),
            Mismatch::Undetermined { .. }
            | Mismatch::MaybeAbsent { .. }
            | Mismatch::NullOperand { .. }
            | Mismatch::MissingResult { .. } => continue,
            Mismatch::NoField { .. } => continue,
        };
        result.add_error(
            ValidationError::error(ErrorCode::OperatorArgTypeMismatch, path, message)
                .with_suggestion(suggestion),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_types() -> BindingTypes {
        BindingTypes {
            payload: HashMap::new(),
            user: orbital_core_base::ir::OirType::Struct {
                additional: None,
                fields: Default::default(),
            },
            config: HashMap::new(),
            prev_payload: None,
            optional_payload: Default::default(),
            locals: Default::default(),
            type_params: Default::default(),
        }
    }

    fn field(name: &str, ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            EntityFieldMeta {
                field_type: ty.into(),
                shape: orbital_core_base::type_compat::compat_kind_oir(ty),
                required: false,
                defaulted: false,
            },
        );
        m
    }

    fn parse(json: &str) -> SExpression {
        serde_json::from_str(json).unwrap()
    }

    fn mismatches(json: &str, entity: HashMap<String, EntityFieldMeta>) -> Vec<String> {
        validate_operator_arg_types(
            &parse(json),
            "p",
            &entity,
            &HashMap::new(),
            &no_types(),
            None,
        )
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e.code,
                ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared
            )
        })
        .map(|e| e.message.clone())
        .collect()
    }

    #[test]
    fn a_number_field_passed_to_a_string_operator_is_an_error() {
        let got = mismatches(
            r#"["str/upper", "@entity.count"]"#,
            field("count", "number"),
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].contains("`(str/upper …)` argument 1 is a number"),
            "{got:?}"
        );
        assert!(got[0].contains("declares string"), "{got:?}");
    }

    #[test]
    fn control_the_declared_type_passes() {
        assert!(mismatches(r#"["str/upper", "@entity.name"]"#, field("name", "string")).is_empty());
        assert!(mismatches(r#"["+", "@entity.count", 1]"#, field("count", "number")).is_empty());
    }

    #[test]
    fn a_string_literal_in_arithmetic_is_an_error() {
        assert_eq!(mismatches(r#"["+", "a", 1]"#, HashMap::new()).len(), 1);
        assert_eq!(
            mismatches(r#"["+", "5", 1]"#, HashMap::new()).len(),
            1,
            "a numeric string is a string"
        );
    }

    #[test]
    fn a_later_argument_of_a_variadic_names_its_own_position() {
        let got = mismatches(r#"["math/max", 1, 2, "3"]"#, HashMap::new());
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("argument 3 is a string"), "{got:?}");
    }

    #[test]
    fn an_inferred_call_result_is_checked() {
        let got = mismatches(r#"["str/upper", ["+", 1, 2]]"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn a_semantic_string_field_in_a_message_params_map_fits_its_string_member() {
        for ty in ["string", "email", "url", "phone"] {
            let mut fields = field("value", ty);
            fields.get_mut("value").unwrap().required = true;
            let got = mismatches(
                r#"["i18n/t", "invite.sent", {"who": "@entity.value"}]"#,
                fields,
            );
            assert!(got.is_empty(), "{ty}: {got:?}");
        }
        let got = mismatches(
            r#"["i18n/t", "invite.sent", {"who": "@entity.value"}]"#,
            field("value", "object"),
        );
        assert_eq!(
            got.len(),
            1,
            "control: an object is no message value: {got:?}"
        );
    }

    #[test]
    fn a_message_key_is_a_literal_or_a_value_typed_message_key() {
        assert!(mismatches(r#"["i18n/t", "hero.title"]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["i18n/t", "@entity.key"]"#, field("key", "MessageKey")).is_empty());
        assert_eq!(
            mismatches(r#"["i18n/t", "@entity.key"]"#, field("key", "string")).len(),
            1,
            "control: a string field is no catalog key"
        );
        assert_eq!(
            mismatches(r#"["i18n/t", ["str/concat", "hero.", "title"]]"#, HashMap::new()).len(),
            1,
            "a computed key cannot be checked against a catalog"
        );
    }

    #[test]
    fn a_mismatch_inside_an_object_value_is_found() {
        let got = mismatches(r#"{"content": ["str/upper", 5]}"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
    }

    fn spawn_with(config: &str) -> Vec<String> {
        mismatches(
            &format!(
                r#"["os/spawn", "git", ["status"], {config}, {{"emit": {{"success": "S", "failure": "F"}}}}]"#
            ),
            HashMap::new(),
        )
    }

    const SPAWN_OK: &str = r#"{"cwd": null, "env": null, "stdin": "null", "stdout": "pipe", "stderr": "pipe", "detached": false}"#;

    #[test]
    fn a_closed_shape_argument_is_checked_field_by_field() {
        assert!(
            spawn_with(SPAWN_OK).is_empty(),
            "control: every declared field, each of its type"
        );
        let wrong = spawn_with(&SPAWN_OK.replace(r#""detached": false"#, r#""detached": "yes""#));
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert!(
            wrong[0].contains("`detached`") && wrong[0].contains("boolean"),
            "{wrong:?}"
        );
        let stdin = spawn_with(&SPAWN_OK.replace(r#""stdin": "null""#, r#""stdin": 5"#));
        assert_eq!(stdin.len(), 1, "{stdin:?}");
    }

    fn spawn_undetermined(env: &str) -> Vec<String> {
        let config = SPAWN_OK.replace(r#""env": null"#, &format!(r#""env": {env}"#));
        undetermined(
            &format!(
                r#"["os/spawn", "git", ["status"], {config}, {{"emit": {{"success": "S", "failure": "F"}}}}]"#
            ),
            HashMap::new(),
        )
    }

    #[test]
    fn a_null_value_in_a_map_literal_fits_a_map_whose_value_type_admits_null() {
        assert!(
            spawn_undetermined(r#"{"PATH": null}"#).is_empty(),
            "{:?}",
            spawn_undetermined(r#"{"PATH": null}"#)
        );
        assert!(spawn_undetermined(r#"{"PATH": null, "HOME": null}"#).is_empty());
        assert!(
            spawn_undetermined(r#"{"PATH": "x", "HOME": null}"#).is_empty(),
            "control: mixed"
        );
        assert!(spawn_undetermined(r#"{}"#).is_empty(), "control: empty");
        assert!(
            spawn_undetermined(r#"{"PATH": "x"}"#).is_empty(),
            "control: all strings"
        );
        assert!(
            spawn_undetermined("null").is_empty(),
            "control: the nullable map itself null"
        );
        let wrong =
            spawn_with(&SPAWN_OK.replace(r#""env": null"#, r#""env": {"PATH": 5, "HOME": null}"#));
        let baseline = spawn_with(&SPAWN_OK.replace(r#""env": null"#, r#""env": {"PATH": 5}"#));
        assert_eq!(baseline.len(), 1, "baseline: {baseline:?}");
        assert_eq!(
            wrong.len(),
            1,
            "control: a present value of the wrong type is still refused: {wrong:?}"
        );
    }

    #[test]
    fn a_closed_shape_argument_names_a_missing_field_and_an_unknown_one() {
        let missing = spawn_with(&SPAWN_OK.replace(r#", "detached": false"#, ""));
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(
            missing[0].contains("`detached`") && missing[0].contains("missing"),
            "{missing:?}"
        );
        let unknown = spawn_with(&SPAWN_OK.replace(
            r#""detached": false"#,
            r#""detached": false, "shell": true"#,
        ));
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert!(unknown[0].contains("`shell`"), "{unknown:?}");
    }

    #[test]
    fn unknowable_arguments_never_reject() {
        assert!(mismatches(r#"["str/upper", "@entity.unknown"]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", "@payload.x"]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", null]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", "@entity.when"]"#, field("when", "date")).is_empty());
    }

    #[test]
    fn a_date_is_text_so_arithmetic_on_it_is_an_error() {
        let got = mismatches(r#"["+", "@entity.when", 1]"#, field("when", "date"));
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            mismatches(r#"["+", "@entity.count", 1]"#, field("count", "number")).is_empty(),
            "control: a number adds"
        );
    }

    fn undetermined(json: &str, entity: HashMap<String, EntityFieldMeta>) -> Vec<String> {
        validate_operator_arg_types(
            &parse(json),
            "p",
            &entity,
            &HashMap::new(),
            &no_types(),
            None,
        )
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
        .map(|e| e.message.clone())
        .collect()
    }

    #[test]
    fn an_empty_list_in_an_accumulator_object_takes_the_type_its_use_site_gives_it() {
        let read_out = |init: &str, step: &str| {
            let json = format!(
                r#"["array/len", ["object/get", ["array/reduce", ["array/range", 0, 3], {init}, ["fn", ["acc", "i"], {step}]], "out"]]"#
            );
            undetermined(&json, HashMap::new())
        };
        let append = r#"["object/set", "@acc", "out", ["array/append", ["object/get", "@acc", "out"], "@i"]]"#;
        assert!(
            read_out(r#"{"out": []}"#, append).is_empty(),
            "{:?}",
            read_out(r#"{"out": []}"#, append)
        );
        assert!(
            read_out(r#"{"out": [0]}"#, append).is_empty(),
            "control: a non-empty list keeps its element type"
        );
        let unused = read_out(r#"{"out": []}"#, r#""@acc""#);
        assert_eq!(
            unused.len(),
            1,
            "control: no use site gives the empty list a type, so it is never guessed: {unused:?}"
        );
        let alone = undetermined(r#"["array/len", []]"#, HashMap::new());
        assert_eq!(
            alone.len(),
            1,
            "control: a bare empty list stays undetermined: {alone:?}"
        );
    }

    #[test]
    fn an_undetermined_value_handed_to_a_type_variable_is_an_error_naming_it() {
        let got = undetermined(r#"["=", "@entity.nope", 3]"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`(== …)`"), "names the operator: {got:?}");
        assert!(
            got[0].contains("@entity.nope"),
            "names the expression: {got:?}"
        );
        assert!(got[0].contains("argument 1"), "{got:?}");
        let row = undetermined(r#"["object/get", "@entity.row", "name"]"#, HashMap::new());
        assert_eq!(row.len(), 1, "{row:?}");
    }

    #[test]
    fn a_list_literal_with_an_undetermined_item_is_an_error() {
        let got = undetermined(r#"["list", 1, "@entity.nope"]"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`(list …)`"), "{got:?}");
        assert!(got[0].contains("@entity.nope"), "{got:?}");
        assert!(
            undetermined(r#"["list", 1, 2]"#, HashMap::new()).is_empty(),
            "control: every item typed"
        );
        assert!(
            undetermined(r#"["list", 1, null]"#, HashMap::new()).is_empty(),
            "control: null is absent, not undetermined"
        );
    }

    #[test]
    fn controls_a_determined_value_and_null_are_not_undetermined() {
        assert!(undetermined(r#"["=", "@entity.count", 3]"#, field("count", "number")).is_empty());
        assert!(
            undetermined(r#"["=", "@entity.count", null]"#, field("count", "number")).is_empty()
        );
        assert_eq!(
            undetermined(r#"["str/upper", "@entity.nope"]"#, HashMap::new()).len(),
            1,
            "a value of unknown type is undetermined at a concrete position too"
        );
        assert!(
            undetermined(r#"["str/upper", "@entity.name"]"#, field("name", "string")).is_empty(),
            "control: a determined value"
        );
    }

    #[test]
    fn an_effect_is_void_not_undetermined() {
        let e = || field("count", "number");
        for expr in [
            r#"["if", true, ["set", "@entity.count", 1], ["set", "@entity.count", 2]]"#,
            r#"["if", true, ["set", "@entity.count", 1], null]"#,
            r#"["do", ["set", "@entity.count", 1], ["emit", "DONE"]]"#,
            r#"["if", true, ["do", ["set", "@entity.count", 1]], ["render-ui", "main", null]]"#,
            r#"["let", [["n", 3]], ["set", "@entity.count", "@n"]]"#,
            r#"["when", true, ["emit", "DONE"]]"#,
        ] {
            assert!(
                undetermined(expr, e()).is_empty(),
                "{expr}: {:?}",
                undetermined(expr, e())
            );
        }
        assert_eq!(
            mismatches(
                r#"["if", ["==", "@entity.count", 3], ["set", "@entity.count", 1], 5]"#,
                e()
            )
            .len(),
            1,
            "control: an effect and a value are not one type"
        );
        assert_eq!(
            mismatches(r#"["str/upper", ["set", "@entity.count", 1]]"#, e()).len(),
            1,
            "control: an effect is no string"
        );
    }

    #[test]
    fn the_bindings_the_language_defines_are_typed() {
        for expr in [
            r#"[">", "@now", "@now"]"#,
            r#"["==", "@state", "idle"]"#,
            r#"["==", "@fromState", "idle"]"#,
            r#"["==", "@locale", "en"]"#,
            r#"["list", "@trait.Header", "@trait.Footer"]"#,
            r#"["array/includes", "@prevStates", "idle"]"#,
            r#"["==", "@event.event", "GO"]"#,
        ] {
            assert!(
                undetermined(expr, HashMap::new()).is_empty(),
                "{expr}: {:?}",
                undetermined(expr, HashMap::new())
            );
            assert!(
                mismatches(expr, HashMap::new()).is_empty(),
                "{expr}: {:?}",
                mismatches(expr, HashMap::new())
            );
        }
        assert_eq!(
            mismatches(r#"[">", "@now", "x"]"#, HashMap::new()).len(),
            1,
            "control: @now is a point in time, not text"
        );
        assert_eq!(
            mismatches(r#"[">", "@now", 5]"#, HashMap::new()).len(),
            1,
            "control: a number is no point in time"
        );
        assert_eq!(
            mismatches(r#"["==", "@state", 3]"#, HashMap::new()).len(),
            1,
            "control: @state is a string"
        );
        assert_eq!(
            mismatches(r#"["==", "@event.payload", 3]"#, HashMap::new()).len(),
            1,
            "a delivery's payload is the triggering event's record"
        );
    }

    #[test]
    fn a_declared_iso_literal_knob_is_a_point_in_time() {
        let read = |kind: &str, text: &str| {
            let now = if kind == "date" { serde_json::json!(["time/toDate", "@now"]) } else { serde_json::json!("@now") };
            serde_json::json!([
                "time/diff",
                now,
                [
                    "let",
                    [["knob_at", text, format!(r#"{{"kind":"{kind}"}}"#)]],
                    "@knob_at"
                ],
                "s"
            ])
            .to_string()
        };
        for (kind, text) in [
            ("instant", "1970-01-01T00:00:00.000Z"),
            ("datetime", "2026-10-28T10:30:00+02:00"),
            ("date", "2026-10-28"),
        ] {
            let expr = read(kind, text);
            assert!(
                mismatches(&expr, HashMap::new()).is_empty(),
                "{kind} {text}: {:?}",
                mismatches(&expr, HashMap::new())
            );
        }
        for (kind, text) in [
            ("instant", "2026-10-28"),
            ("datetime", "soon"),
            ("date", "2026-10-28T10:30:00Z"),
        ] {
            let expr = read(kind, text);
            assert!(
                !mismatches(&expr, HashMap::new()).is_empty(),
                "{kind} {text}: a literal off the declared shape is refused"
            );
        }
        assert_eq!(
            mismatches(
                r#"["time/diff", "@now", "1970-01-01T00:00:00.000Z", "s"]"#,
                HashMap::new()
            )
            .len(),
            1,
            "control: a bare string value at a date parameter stays refused"
        );
    }

    #[test]
    fn now_is_an_instant_not_a_number() {
        assert!(
            mismatches(r#"["time/diff", "@now", "@now", "s"]"#, HashMap::new()).is_empty(),
            "{:?}",
            mismatches(r#"["time/diff", "@now", "@now", "s"]"#, HashMap::new())
        );
        assert_eq!(
            mismatches(r#"["/", "@now", 1000]"#, HashMap::new()).len(),
            1,
            "@now in arithmetic"
        );
        assert!(
            mismatches(
                r#"["/", ["time/diff", "@now", "@now", "ms"], 1000]"#,
                HashMap::new()
            )
            .is_empty(),
            "control: a time/diff result is a number"
        );
    }

    fn with_payload(json: &str, shapes: &[(&str, orbital_core_base::ir::OirType)]) -> Vec<String> {
        let types = BindingTypes {
            payload: shapes
                .iter()
                .map(|(n, t)| ((*n).to_string(), t.clone()))
                .collect(),
            ..no_types()
        };
        validate_operator_arg_types(
            &parse(json),
            "p",
            &HashMap::new(),
            &HashMap::new(),
            &types,
            None,
        )
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
        .map(|e| e.message.clone())
        .collect()
    }

    #[test]
    fn a_declared_payload_field_has_its_full_type() {
        use orbital_core_base::ir::{OirField, OirType};
        let row = OirType::Struct {
            additional: None,
            fields: vec![OirField::new("name".to_string(), OirType::String)].into(),
        };
        let declared = [("data", OirType::array(row))];
        assert!(with_payload(r#"["array/len", "@payload.data"]"#, &declared).is_empty());
        assert!(
            with_payload(r#"["array/map", "@payload.data", ["fn", "r", ["str/upper", ["object/get", "@r", "name"]]]]"#, &declared)
                .is_empty(),
            "a lambda over the declared list sees its element"
        );
        assert_eq!(
            with_payload(r#"["array/len", "@payload.data"]"#, &[]).len(),
            1,
            "control: a payload field nothing declares is undetermined"
        );
    }

    #[test]
    fn the_viewer_and_the_config_knobs_have_their_declared_types() {
        use orbital_core_base::ir::{OirField, OirType};
        let types = BindingTypes {
            payload: HashMap::new(),
            user: OirType::Struct {
                additional: None,
                fields: vec![OirField::new("role".to_string(), OirType::String)].into(),
            },
            config: [("limit".to_string(), OirType::Number)]
                .into_iter()
                .collect(),
            prev_payload: None,
            optional_payload: Default::default(),
            locals: Default::default(),
            type_params: Default::default(),
        };
        let run = |json: &str| {
            let r = validate_operator_arg_types(
                &parse(json),
                "p",
                &HashMap::new(),
                &HashMap::new(),
                &types,
                None,
            );
            (
                r.errors
                    .iter()
                    .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
                    .count(),
                r.errors
                    .iter()
                    .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared))
                    .count(),
            )
        };
        assert_eq!(run(r#"["==", "@user.role", "admin"]"#), (0, 0));
        assert_eq!(run(r#"[">", "@config.limit", 3]"#), (0, 0));
        assert_eq!(
            run(r#"["==", "@user.role", 3]"#),
            (0, 1),
            "control: role is a string"
        );
        assert_eq!(
            run(r#"["==", "@user.nope", "x"]"#),
            (1, 0),
            "control: a field the roster does not declare"
        );
        assert_eq!(
            run(r#"["==", "@config.other", 3]"#),
            (1, 0),
            "control: an undeclared knob"
        );
    }

    #[test]
    fn an_optional_viewer_member_is_a_possibly_missing_operand_and_the_guaranteed_id_is_not() {
        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "DocOrbital",
                "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs",
                            "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{ "name": "DocView", "linkedEntity": "Doc",
                            "stateMachine": { "events": [], "states": [], "transitions": [] } }],
                "pages": []
            }]
        }))
        .expect("schema fixture");
        let types = BindingTypes {
            user: crate::phases::validation::user_identity::identity_type(&schema, "DocView"),
            ..no_types()
        };
        let nulls = |json: &str| {
            validate_operator_arg_types(
                &parse(json),
                "p",
                &HashMap::new(),
                &HashMap::new(),
                &types,
                None,
            )
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorNullOperand)
            .count()
        };
        assert_eq!(nulls(r#"["str/split", "@user.name", " "]"#), 1);
        assert_eq!(
            nulls(r#"["str/split", ["default", "@user.name", ""], " "]"#),
            0,
            "control: defaulted"
        );
        assert_eq!(
            nulls(r#"["str/split", "@user.id", " "]"#),
            0,
            "control: the viewer's id is guaranteed"
        );
    }

    #[test]
    fn the_anonymous_viewer_id_validates_as_string_and_role_as_optional_string_exec_seed_5() {
        use orbital_core_base::ir::OirType;
        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "DocOrbital",
                "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs",
                            "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{ "name": "DocView", "linkedEntity": "Doc",
                            "stateMachine": { "events": [], "states": [], "transitions": [] } }],
                "pages": []
            }]
        }))
        .expect("schema fixture");
        let OirType::Struct { fields, .. } =
            crate::phases::validation::user_identity::identity_type(&schema, "DocView")
        else {
            panic!("@user is a record");
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("@user.{name} undeclared"))
        };
        assert_eq!(
            field("id").field_type,
            OirType::String,
            "@user.id is string, and the anonymous row carries \"anonymous\""
        );
        assert_eq!(field("role").field_type, OirType::optional(OirType::String), "@user.role is string?: the anonymous row carries \"anonymous\", a bound row may declare no role");
        assert!(
            field("email").field_type.is_optional(),
            "control: a member the anonymous viewer lacks stays optional"
        );
    }

    #[test]
    fn a_substituted_config_list_carrying_its_declared_type_is_typed() {
        let mixed = serde_json::json!(["list", { "anchor": "exe-dir", "path": "tool" }, "tool"]);
        let ty = crate::phases::resolve::config_field_carried_type(
            &serde_json::from_value(serde_json::json!({
                "type": "[CliCommand]",
                "items": { "type": "union", "values": ["string", "AnchoredPath"], "properties": {
                    "string": { "name": "string", "type": "string" },
                    "AnchoredPath": { "name": "AnchoredPath", "type": "object", "properties": {
                        "anchor": { "name": "anchor", "type": "string", "required": true },
                        "path": { "name": "path", "type": "string", "required": true } } }
                } }
            }))
            .expect("knob"),
        )
        .unwrap();
        let typed = serde_json::json!([
            "array/len",
            [
                "let",
                [["knob_commands", mixed, serde_json::to_string(&ty).unwrap()]],
                "@knob_commands"
            ]
        ])
        .to_string();
        assert!(
            undetermined(&typed, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(&typed, HashMap::new())
        );
        let inline = serde_json::json!(["array/len", mixed]).to_string();
        assert_eq!(
            undetermined(&inline, HashMap::new()).len(),
            1,
            "control: a mixed literal written inline is refused"
        );
    }

    #[test]
    fn a_union_knob_read_at_an_os_target_carrying_its_declared_type_raises_no_arg_type_mismatch() {
        let knob: orbital_core_base::schema::ConfigField = serde_json::from_value(serde_json::json!({
            "type": "string",
            "union": true,
            "values": ["string", "Env"],
            "properties": {
                "string": { "name": "string", "type": "string" },
                "Env": { "name": "Env", "type": "object", "properties": {
                    "anchor": { "name": "anchor", "type": "object", "required": true, "properties": {
                        "env": { "name": "env", "type": "string", "required": true } } } } }
            }
        })).expect("knob");
        let ty = crate::phases::resolve::config_field_carried_type(&knob).unwrap();
        let wrap = |literal: &str, ty: &str| {
            serde_json::json!(["os/tcp-connect", ["let", [["knob_host", literal, ty]], "@knob_host"], 80, { "key": "c" }]).to_string()
        };
        let typed = wrap("api.github.com", &serde_json::to_string(&ty).unwrap());
        assert!(
            mismatches(&typed, HashMap::new()).is_empty(),
            "{:?}",
            mismatches(&typed, HashMap::new())
        );
        let plain =
            serde_json::json!(["os/tcp-connect", "api.github.com", 80, { "key": "c" }]).to_string();
        assert!(
            mismatches(&plain, HashMap::new()).is_empty(),
            "control: a literal host is accepted"
        );
        let record_of_optionals = orbital_core_base::ir::OirType::Struct {
            additional: None,
            fields: vec![orbital_core_base::ir::OirField::new(
                "string".to_string(),
                orbital_core_base::ir::OirType::optional(orbital_core_base::ir::OirType::String),
            )]
            .into(),
        };
        let as_struct = wrap(
            "api.github.com",
            &serde_json::to_string(&record_of_optionals).unwrap(),
        );
        assert_eq!(
            mismatches(&as_struct, HashMap::new()).len(),
            1,
            "control: the record-of-optionals reading is the refused shape: {:?}",
            mismatches(&as_struct, HashMap::new())
        );
    }

    #[test]
    fn a_sequential_let_over_an_entity_list_is_typed() {
        use orbital_core_base::ir::{OirField, OirType};
        let band = OirType::Struct {
            additional: None,
            fields: vec![
                OirField::new("upperBound".to_string(), OirType::Number),
                OirField::new("label".to_string(), OirType::String),
            ]
            .into(),
        };
        let entity = || {
            shaped(&[
                ("bands", OirType::array(band.clone())),
                ("dose", OirType::Number),
            ])
        };
        let expr = r#"["let", [
            ["matched", ["array/filter", "@entity.bands", ["fn", "b", ["<=", "@entity.dose", ["object/get", "@b", "upperBound"]]]]],
            ["band", ["if", [">", ["array/len", "@matched"], 0], ["array/first", "@matched"], ["array/nth", "@entity.bands", ["-", ["array/len", "@entity.bands"], 1]]]]
          ], ["object/get", "@band", "label"]]"#;
        assert!(
            undetermined(expr, entity()).is_empty(),
            "{:?}",
            undetermined(expr, entity())
        );
        assert!(mismatches(expr, entity()).is_empty());
    }

    #[test]
    fn a_callback_that_is_handed_an_index_types_it_as_a_number() {
        let tags = || tags();
        for expr in [
            r#"["array/map", "@entity.tags", ["fn", ["t", "i"], ["+", "@i", 1]]]"#,
            r#"["array/map", "@entity.tags", ["+", "@index", 1]]"#,
            r#"["array/some", "@entity.tags", ["==", "@index", 0]]"#,
            r#"["array/filter", "@entity.tags", ["==", "@index", 0]]"#,
        ] {
            assert!(
                undetermined(expr, tags()).is_empty(),
                "{expr}: {:?}",
                undetermined(expr, tags())
            );
            assert!(
                mismatches(expr, tags()).is_empty(),
                "{expr}: {:?}",
                mismatches(expr, tags())
            );
        }
        assert_eq!(
            mismatches(
                r#"["array/map", "@entity.tags", ["str/upper", "@index"]]"#,
                tags()
            )
            .len(),
            1,
            "control: the index is a number, not a string"
        );
        assert_eq!(
            undetermined(
                r#"["array/findIndex", "@entity.tags", ["fn", "t", ["==", "@index", 0]]]"#,
                tags()
            )
            .len(),
            1,
            "control: findIndex hands its callback no index"
        );
    }

    #[test]
    fn a_branch_a_literal_condition_rules_out_is_not_typed() {
        let dead = r#"["if", ["==", "create", "create"], "x", ["str/upper", 5]]"#;
        assert!(
            mismatches(dead, HashMap::new()).is_empty(),
            "{:?}",
            mismatches(dead, HashMap::new())
        );
        let dead_then = r#"["if", ["==", "a", "b"], ["str/upper", 5], "x"]"#;
        assert!(mismatches(dead_then, HashMap::new()).is_empty());
        let live = r#"["if", ["==", "@entity.name", "x"], "x", ["str/upper", 5]]"#;
        assert_eq!(
            mismatches(live, field("name", "string")).len(),
            1,
            "control: an undecided condition types both branches"
        );
        let live_type = r#"["str/upper", ["if", ["==", "a", "a"], 5, "x"]]"#;
        assert_eq!(
            mismatches(live_type, HashMap::new()).len(),
            1,
            "control: the live branch is a number"
        );
    }

    // std-lifecycle under `fromStatus: ""`: `(or (= "" "") (= (object/get @entity statusField) …))` never reads the field.
    #[test]
    fn an_operand_a_decided_and_or_never_evaluates_is_not_typed() {
        assert!(mismatches(
            r#"["or", ["=", "", ""], ["=", ["str/upper", 5], "x"]]"#,
            HashMap::new()
        )
        .is_empty());
        assert!(mismatches(
            r#"["and", ["=", "a", "b"], ["=", ["str/upper", 5], "x"]]"#,
            HashMap::new()
        )
        .is_empty());
    }

    #[test]
    fn control_an_operand_before_or_beside_an_undecided_one_is_typed() {
        assert_eq!(
            mismatches(
                r#"["or", ["=", ["str/upper", 5], "x"], ["=", "", ""]]"#,
                HashMap::new()
            )
            .len(),
            1
        );
        assert_eq!(
            mismatches(
                r#"["and", ["=", "a", "a"], ["=", ["str/upper", 5], "x"]]"#,
                HashMap::new()
            )
            .len(),
            1
        );
        assert_eq!(
            mismatches(
                r#"["or", ["=", "@entity.name", "x"], ["=", ["str/upper", 5], "x"]]"#,
                field("name", "string")
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_reference_to_a_row_is_read_through_its_entity() {
        use orbital_core_base::ir::OirType;
        let schema: orbital_core_base::schema::OrbitalSchema =
            serde_json::from_value(serde_json::json!({
                "name": "App",
                "orbitals": [{
                    "name": "O",
                    "entity": { "name": "Note", "collection": "notes", "fields": [
                        { "name": "title", "type": "string" },
                        { "name": "tags", "type": "array", "items": { "type": "string" } }
                    ] },
                    "traits": [], "pages": []
                }]
            }))
            .unwrap();
        let types = BindingTypes {
            payload: [
                (
                    "row".to_string(),
                    OirType::Reference {
                        entity: "Note".into(),
                    },
                ),
                (
                    "rows".to_string(),
                    OirType::array(OirType::Reference {
                        entity: "Note".into(),
                    }),
                ),
                (
                    "ghost".to_string(),
                    OirType::Reference {
                        entity: "Nope".into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..no_types()
        };
        let run = |json: &str| {
            let r = validate_operator_arg_types(
                &parse(json),
                "p",
                &HashMap::new(),
                &HashMap::new(),
                &types,
                Some(&schema),
            );
            (
                r.errors
                    .iter()
                    .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
                    .count(),
                r.errors
                    .iter()
                    .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared))
                    .count(),
            )
        };
        assert_eq!(run(r#"["str/upper", "@payload.row.title"]"#), (0, 0));
        assert_eq!(
            run(r#"["str/upper", ["object/get", "@payload.row", "title"]]"#),
            (1, 0),
            "a bare relation is the row's id, so a keyed read of it has no determined type"
        );
        assert_eq!(run(r#"["array/len", "@payload.row.tags"]"#), (0, 0));
        assert_eq!(
            run(
                r#"["array/map", "@payload.rows", ["fn", "r", ["str/upper", ["object/get", "@r", "title"]]]]"#
            ),
            (1, 0),
            "likewise for the ids of a list of relations"
        );
        assert_eq!(
            run(r#"["+", "@payload.row.title", 1]"#),
            (0, 1),
            "control: title is a string"
        );
        assert_eq!(
            run(r#"["==", "@payload.ghost.title", "x"]"#),
            (1, 0),
            "control: a row of no declared entity"
        );
    }

    // G-ORB-534: std-lifecycle's delete scan `(fetch @config.targetEntity { filter: (= (object/get @entity @config.statusField) …) })`
    // reads the TARGET row (std-otel: `OtelSpan.state`), not the run-log entity the scheduler trait is bound to.
    fn fetch_filter_mismatches(filter: &str) -> usize {
        fetch_filter_findings(filter).len()
    }

    fn fetch_filter_findings(filter: &str) -> Vec<String> {
        effect_findings(&format!(
            r#"["if", ["=", "@entity.headId", ""], ["fetch", "Span", {{ "filter": {filter} }}], null]"#
        ))
    }

    fn effect_findings(effect: &str) -> Vec<String> {
        let program: orbital_core_base::schema::OrbitalSchema = serde_json::from_str(&format!(
            r#"{{ "name": "App", "version": "1.0.0", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "RunLog", "fields": [ {{ "name": "id", "type": "string" }}, {{ "name": "headId", "type": "string" }} ] }},
              "auxiliaryEntities": [ {{ "name": "Span", "persistence": "persistent", "collection": "spans", "fields": [
                {{ "name": "id", "type": "string" }}, {{ "name": "state", "type": "string" }}, {{ "name": "endTime", "type": "number" }} ] }} ],
              "traits": [{{ "name": "T", "linkedEntity": "RunLog", "stateMachine": {{
                "states": [{{ "name": "idle", "isInitial": true }}],
                "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                  "effects": [{effect}] }}] }} }}],
              "pages": [{{ "name": "P", "path": "/p", "traits": [{{ "ref": "T" }}] }}] }}] }}"#
        ))
        .expect("program");
        crate::phases::validation::validate_comprehensive_without_services(&program)
            .errors
            .iter()
            .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared))
            .map(|e| e.message.clone())
            .collect()
    }

    fn render_ui_with(prop: &str) -> Vec<String> {
        effect_findings(&format!(
            r#"["render-ui", "main", {{ "type": "data-grid", "entity": {prop} }}]"#
        ))
    }

    fn rows_mapped(body: &str) -> String {
        format!(
            r#"["array/map", ["list", {{ "s1": "a", "s2": "b", "f1": true, "f2": false }}], ["fn", "r", {body}]]"#
        )
    }

    #[test]
    fn g_orb_627_a_pattern_prop_lambda_body_is_type_checked() {
        let strings = render_ui_with(&rows_mapped(r#"{ "a": ["or", "@r.s1", "@r.s2"] }"#));
        assert_eq!(
            strings.len(),
            2,
            "or over two strings in a lambda body: {strings:?}"
        );
        assert!(strings[0].contains("or"), "{strings:?}");
        assert_eq!(
            render_ui_with(&rows_mapped(r#"{ "a": ["or", "@r.f1", "@r.f2"] }"#)).len(),
            0,
            "control: booleans"
        );
        assert_eq!(
            render_ui_with(&rows_mapped(r#"{ "a": ["default", "@r.s1", "@r.s2"] }"#)).len(),
            0,
            "control: default on strings"
        );
        let nested = render_ui_with(&rows_mapped(
            r#"{ "a": ["array/map", ["list", 1], ["fn", "q", ["or", "@r.s1", "@r.s2"]]] }"#,
        ));
        assert_eq!(nested.len(), 2, "nested lambda: {nested:?}");
        assert_eq!(
            effect_findings(r#"["set", "@entity.headId", ["or", "x", "y"]]"#).len(),
            1,
            "control: the same call outside a lambda is checked"
        );
    }

    #[test]
    fn a_fetch_filter_reads_the_row_of_the_fetched_entity() {
        assert_eq!(
            fetch_filter_mismatches(
                r#"["and", ["=", ["object/get", "@entity", "state"], "exported"], ["<", ["object/get", "@entity", "endTime"], 5]]"#
            ),
            0
        );
    }

    #[test]
    fn control_a_fetch_filter_still_names_missing_target_fields_and_reads_the_trait_through_entity_fields(
    ) {
        assert_eq!(
            fetch_filter_mismatches(r#"["=", ["object/get", "@entity", "headId"], "x"]"#),
            1,
            "headId is the trait's, not Span's"
        );
        assert_eq!(
            fetch_filter_mismatches(
                r#"["=", ["object/get", "@entity", "state"], ["+", "@entity.headId", 1]]"#
            ),
            1
        );
        assert_eq!(
            fetch_filter_mismatches(
                r#"["=", ["object/get", "@entity", "state"], "@entity.headId"]"#
            ),
            0
        );
        assert_eq!(
            effect_findings(r#"["if", ["=", "delete", "delete"], ["fetch", "Span", { "filter": ["=", ["object/get", "@entity", "state"], "x"] }], ["fetch", "Span", { "filter": ["=", ["object/get", "@entity", "status"], "x"] }]]"#).len(),
            0,
            "a branch the call site's literal config rules out is not typed"
        );
        let found = fetch_filter_findings(r#"["=", ["object/get", "@entity", "headId"], "x"]"#);
        assert!(
            found[0].contains(r#"object/get(@entity, "headId")"#),
            "the finding names the read as written: {found:?}"
        );
    }

    #[test]
    fn a_set_value_reads_a_row_field_under_the_same_check_as_a_guard() {
        use orbital_core_base::ir::OirType;
        let schema: orbital_core_base::schema::OrbitalSchema =
            serde_json::from_value(serde_json::json!({
                "name": "App",
                "orbitals": [{
                    "name": "O",
                    "entity": { "name": "Note", "collection": "notes", "fields": [
                        { "name": "title", "type": "string" }
                    ] },
                    "traits": [], "pages": []
                }]
            }))
            .unwrap();
        let types = BindingTypes {
            payload: [
                ("row".to_string(), OirType::row("Note")),
                (
                    "byKey".to_string(),
                    OirType::map(OirType::String, OirType::row("Note")),
                ),
            ]
            .into_iter()
            .collect(),
            ..no_types()
        };
        let mismatches = |json: &str| {
            validate_operator_arg_types(
                &parse(json),
                "p",
                &HashMap::new(),
                &HashMap::new(),
                &types,
                Some(&schema),
            )
            .errors
            .iter()
            .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared))
            .count()
        };
        assert_eq!(
            mismatches(r#"["==", ["object/get", "@payload.row", "nope"], "x"]"#),
            1,
            "the guard form"
        );
        assert_eq!(
            mismatches(r#"["set", "@entity.label", ["object/get", "@payload.row", "nope"]]"#),
            1
        );
        assert_eq!(
            mismatches(
                r#"["set", "@entity.label", ["object/get", ["object/get", "@payload.byKey", "k"], "nope"]]"#
            ),
            1,
            "through a map of rows"
        );
        assert_eq!(
            mismatches(r#"["set", "@entity.label", ["object/get", "@payload.row", "title"]]"#),
            0,
            "control"
        );
        assert_eq!(
            mismatches(
                r#"["set", "@entity.label", ["object/get", ["object/get", "@payload.byKey", "k"], "title"]]"#
            ),
            0,
            "control through the map"
        );
    }

    #[test]
    fn an_object_literal_is_exactly_the_record_it_writes() {
        assert!(undetermined(
            r#"["str/upper", ["object/get", {"a": "x"}, "a"]]"#,
            HashMap::new()
        )
        .is_empty());
        assert!(mismatches(
            r#"["str/upper", ["object/get", {"a": "x"}, "a"]]"#,
            HashMap::new()
        )
        .is_empty());
        assert_eq!(
            mismatches(
                r#"["+", ["object/get", {"a": "x"}, "a"], 1]"#,
                HashMap::new()
            )
            .len(),
            1
        );
        assert_eq!(
            mismatches(r#"["object/get", {"a": 1}, "b"]"#, HashMap::new()).len(),
            1,
            "no such key"
        );
        assert!(
            mismatches(r#"["object/get", {"a": 1}, "b", 0]"#, HashMap::new()).is_empty(),
            "with a default"
        );
        assert_eq!(
            undetermined(
                r#"["object/get", {"a": "@entity.nope"}, "a"]"#,
                HashMap::new()
            )
            .len(),
            1,
            "control: a field whose value is undetermined leaves the record undetermined"
        );
        let rows = r#"["array/some", ["list", {"from": "a", "to": "b"}, {"from": "b", "to": "c"}], ["fn", "t", ["==", ["object/get", "@t", "from"], "a"]]]"#;
        assert!(
            undetermined(rows, HashMap::new()).is_empty()
                && mismatches(rows, HashMap::new()).is_empty()
        );
    }

    #[test]
    fn the_delivery_is_typed_by_the_event_that_triggers_the_transition() {
        use orbital_core_base::ir::OirType;
        let declared = [("id", OirType::String)];
        for expr in [
            r#"["==", "@event.payload.id", "x"]"#,
            r#"["==", ["object/get", "@event.payload", "id"], "x"]"#,
            r#"["default", "@event.source.trait", ""]"#,
            r#"["==", "@event.event", "GO"]"#,
            r#"["==", "@event.source.fromBridge", true]"#,
        ] {
            assert!(
                with_payload(expr, &declared).is_empty(),
                "{expr}: {:?}",
                with_payload(expr, &declared)
            );
        }
        assert_eq!(
            with_payload(r#"["==", "@event.payload.id", "x"]"#, &[]).len(),
            1,
            "control: no declared payload field"
        );
    }

    #[test]
    fn the_empty_list_is_typed_and_refined_by_a_fold() {
        let fold = r#"["array/reduce", ["list", "a", "b"], [], ["fn", ["acc", "e"], ["if", ["array/includes", "@acc", "@e"], "@acc", ["array/append", "@acc", "@e"]]]]"#;
        assert!(
            undetermined(fold, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(fold, HashMap::new())
        );
        assert!(
            mismatches(fold, HashMap::new()).is_empty(),
            "{:?}",
            mismatches(fold, HashMap::new())
        );
        let used = format!(r#"["array/len", {fold}]"#);
        assert!(undetermined(&used, HashMap::new()).is_empty());
        assert!(
            undetermined(r#"["or", "@entity.nope", []]"#, HashMap::new()).len() == 1,
            "control: the undetermined side still is"
        );
        let record =
            r#"["list", {"samples": [], "label": "a"}, {"samples": [{"x": 1}], "label": "b"}]"#;
        assert!(
            undetermined(record, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(record, HashMap::new())
        );
    }

    #[test]
    fn a_fold_from_the_empty_list_reads_its_accumulator_as_the_list_its_body_builds() {
        // The accumulator starts as `[]` and the body filters it before it appends to it: what types the
        // accumulator is the list the body builds, so the filter's lambda sees the elements.
        let dedupe = r#"["array/reduce", ["list", 1, 2, 2], [], ["fn", ["acc", "g"], ["if", [">", ["array/len", ["array/filter", "@acc", ["fn", "x", ["==", "@x", "@g"]]]], 0], "@acc", ["array/append", "@acc", "@g"]]]]"#;
        assert!(
            undetermined(dedupe, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(dedupe, HashMap::new())
        );
        assert!(
            mismatches(dedupe, HashMap::new()).is_empty(),
            "{:?}",
            mismatches(dedupe, HashMap::new())
        );
        // control: the elements really are numbers, so comparing one with a string is still found.
        let compared = r#"["array/reduce", ["list", 1, 2], [], ["fn", ["acc", "g"], ["if", [">", ["array/len", ["array/filter", "@acc", ["fn", "x", ["==", "@x", "a"]]]], 0], "@acc", ["array/append", "@acc", "@g"]]]]"#;
        assert_eq!(
            mismatches(compared, HashMap::new()).len(),
            1,
            "control: a number is not the string `a`"
        );
        // control: a body that builds no typed list leaves the accumulator untyped.
        let nothing = r#"["array/reduce", ["list", 1, 2], [], ["fn", ["acc", "g"], "@acc"]]"#;
        assert!(
            !undetermined(nothing, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(nothing, HashMap::new())
        );
    }

    #[test]
    fn a_callback_that_is_handed_an_index_binds_item_and_index_for_the_body() {
        // Both evaluators bind `@item` (the element) and `@index` (its position) in such a callback, whatever the
        // lambda names its own parameters.
        let by_index = r#"["array/filter", ["list", 1, 2, 3], ["fn", "b", ["<", "@index", 2]]]"#;
        assert!(
            undetermined(by_index, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(by_index, HashMap::new())
        );
        assert!(mismatches(by_index, HashMap::new()).is_empty());
        let by_item = r#"["array/map", ["list", 1, 2], ["fn", "b", ["+", "@item", "@index"]]]"#;
        assert!(undetermined(by_item, HashMap::new()).is_empty());
        assert!(mismatches(by_item, HashMap::new()).is_empty());
        // control: `@index` is a number, so a string comparison with it is found.
        let wrong = r#"["array/filter", ["list", 1, 2, 3], ["fn", "b", ["==", "@index", "a"]]]"#;
        assert_eq!(
            mismatches(wrong, HashMap::new()).len(),
            1,
            "{:?}",
            mismatches(wrong, HashMap::new())
        );
        // control: outside a lambda `@index` is nothing.
        assert!(!undetermined(r#"["<", "@index", 2]"#, HashMap::new()).is_empty());
        // control: `@item` is the element, here a string.
        assert_eq!(
            mismatches(
                r#"["array/map", ["list", "a"], ["fn", "b", ["+", "@item", 1]]]"#,
                HashMap::new()
            )
            .len(),
            1
        );
    }

    #[test]
    fn an_earlier_delivery_has_the_payload_every_listened_event_declares() {
        use orbital_core_base::ir::{OirField, OirType};
        let with = |prev: Option<OirType>| BindingTypes {
            prev_payload: prev,
            ..no_types()
        };
        let expr = r#"["array/some", "@prevEvents", ["fn", "e", ["==", ["object/get", ["object/get", "@e", "payload"], "id"], "x"]]]"#;
        let run = |types: &BindingTypes| {
            let r = validate_operator_arg_types(
                &parse(expr),
                "p",
                &HashMap::new(),
                &HashMap::new(),
                types,
                None,
            );
            r.errors.len()
        };
        let declared = OirType::Struct {
            additional: None,
            fields: vec![OirField::new("id".to_string(), OirType::String)].into(),
        };
        assert_eq!(run(&with(Some(declared))), 0);
        assert!(
            run(&with(None)) > 0,
            "control: no agreed payload, so reading a field of it is an error"
        );
    }

    #[test]
    fn a_dead_branch_still_types_the_empty_list_the_live_one_is() {
        let folded = r#"["array/map", ["if", ["==", "off", "on"], "@entity.tags", []], ["fn", "t", ["str/upper", "@t"]]]"#;
        assert!(
            undetermined(folded, tags()).is_empty(),
            "{:?}",
            undetermined(folded, tags())
        );
        assert!(mismatches(folded, tags()).is_empty());
        let wrong = r#"["array/map", ["if", ["==", "off", "on"], "@entity.tags", []], ["fn", "t", ["+", "@t", 1]]]"#;
        assert_eq!(
            mismatches(wrong, tags()).len(),
            1,
            "control: the elements are strings"
        );
        let nothing = r#"["array/map", ["if", ["==", "off", "on"], "@entity.nope", []], ["fn", "t", ["object/get", "@t", "x"]]]"#;
        assert!(
            !undetermined(nothing, tags()).is_empty(),
            "control: a dead branch of no type types nothing"
        );
    }

    #[test]
    fn a_let_binding_sees_the_bindings_before_it() {
        let both =
            r#"["let", [["xs", ["list", 1, 2]], ["x", ["array/first", "@xs"]]], ["+", "@x", 1]]"#;
        assert!(
            undetermined(both, HashMap::new()).is_empty(),
            "{:?}",
            undetermined(both, HashMap::new())
        );
        assert!(mismatches(both, HashMap::new()).is_empty());
        let wrong = r#"["let", [["xs", ["list", 1, 2]], ["x", ["array/first", "@xs"]]], ["str/upper", "@x"]]"#;
        assert_eq!(
            mismatches(wrong, HashMap::new()).len(),
            1,
            "control: @x is a number"
        );
        let unbound = r#"["let", [["x", ["array/first", "@nope"]]], ["+", "@x", 1]]"#;
        assert!(
            !undetermined(unbound, HashMap::new()).is_empty(),
            "control: an undetermined list stays undetermined"
        );
    }

    fn required(name: &str, ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = field(name, ty);
        m.get_mut(name).unwrap().required = true;
        m
    }

    fn all_codes(json: &str, entity: HashMap<String, EntityFieldMeta>) -> Vec<String> {
        validate_operator_arg_types(
            &parse(json),
            "p",
            &entity,
            &HashMap::new(),
            &no_types(),
            None,
        )
        .errors
        .iter()
        .map(|e| format!("{:?}: {}", e.code, e.message))
        .collect()
    }

    #[test]
    fn a_parsed_time_is_a_declared_union_that_a_null_test_narrows() {
        let tested = r#"["let", [["tp", ["time/parse", "@entity.s"]]], ["if", ["=", "@tp", null], "", ["time/format", ["time/add", "@tp", 1000, "ms"], "YYYY"]]]"#;
        let got = all_codes(tested, required("s", "string"));
        assert!(got.is_empty(), "{got:?}");
        let bare =
            r#"["let", [["tp", ["time/parse", "@entity.s"]]], ["time/format", "@tp", "YYYY"]]"#;
        assert!(
            !all_codes(bare, required("s", "string")).is_empty(),
            "control: an untested optional is refused"
        );
        let wrong = r#"["let", [["tp", ["time/parse", "@entity.s"]]], ["if", ["=", "@tp", null], "", ["+", "@tp", 1]]]"#;
        assert!(
            !all_codes(wrong, required("s", "string")).is_empty(),
            "control: a date or datetime is not a number"
        );
        let neither = r#"["time/format", "@entity.n", "YYYY"]"#;
        assert!(
            !all_codes(neither, required("n", "number")).is_empty(),
            "control: a number is not a date"
        );
    }

    #[test]
    fn an_error_already_reported_is_not_reported_again_by_the_enclosing_call() {
        let got = undetermined(r#"["=", ["+", "a", 1], 3]"#, HashMap::new());
        assert!(got.is_empty(), "{got:?}");
        assert_eq!(
            mismatches(r#"["=", ["+", "a", 1], 3]"#, HashMap::new()).len(),
            1
        );
    }

    #[test]
    fn any_and_union_positions_stay_permissive() {
        assert!(mismatches(r#"["str/concat", "a", 5, true]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["<", 1, 2]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["<", "a", "b"]"#, HashMap::new()).is_empty());
        assert_eq!(mismatches(r#"["<", true, 1]"#, HashMap::new()).len(), 1);
        assert!(mismatches(r#"["validate/minLength", "abc", 2]"#, HashMap::new()).is_empty());
        assert_eq!(
            mismatches(r#"["validate/minLength", 5, 2]"#, HashMap::new()).len(),
            1
        );
    }

    #[test]
    fn syntax_positions_are_not_values() {
        // Parameter names and a `let` binding named like an operator are syntax, not calls.
        assert!(mismatches(
            r#"["fn", ["min", "max"], ["+", "@min", "@max"]]"#,
            HashMap::new()
        )
        .is_empty());
        assert!(mismatches(
            r#"["let", [["count", 3]], ["+", "@count", 1]]"#,
            HashMap::new()
        )
        .is_empty());
        assert!(mismatches(r#"["quote", "[\"+\",\"a\",1]"]"#, HashMap::new()).is_empty());
    }

    #[test]
    fn a_lambda_body_is_checked() {
        let got = mismatches(
            r#"["array/map", ["list", 1], ["fn", "x", ["str/upper", 5]]]"#,
            HashMap::new(),
        );
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn a_data_array_whose_head_is_not_an_operator_is_data() {
        assert!(mismatches(r#"["alpha", 5, "x"]"#, HashMap::new()).is_empty());
    }

    #[test]
    fn a_retired_alias_is_data_not_an_operator() {
        assert!(mismatches(r#"["upper", 5]"#, HashMap::new()).is_empty());
        assert_eq!(
            mismatches(r#"["str/upper", 5]"#, HashMap::new()).len(),
            1,
            "control: the canonical name is checked"
        );
    }

    fn shaped(
        fields: &[(&str, orbital_core_base::ir::OirType)],
    ) -> HashMap<String, EntityFieldMeta> {
        fields
            .iter()
            .map(|(name, shape)| {
                (
                    (*name).to_string(),
                    EntityFieldMeta {
                        field_type: orbital_core_base::type_compat::oir_type_kind(shape),
                        shape: shape.clone(),
                        required: false,
                        defaulted: false,
                    },
                )
            })
            .collect()
    }

    fn tags() -> HashMap<String, EntityFieldMeta> {
        use orbital_core_base::ir::{OirField, OirType};
        let profile = OirType::Struct {
            additional: None,
            fields: vec![OirField::new("name".to_string(), OirType::String)].into(),
        };
        shaped(&[
            ("tags", OirType::array(OirType::String)),
            ("count", OirType::Number),
            ("profile", profile),
        ])
    }

    #[test]
    fn an_element_of_another_type_appended_to_a_list_is_an_error() {
        let got = mismatches(r#"["array/append", "@entity.tags", 1]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("argument 2 is a int"), "{got:?}");
        assert!(
            got[0].contains("an earlier argument is a string"),
            "{got:?}"
        );
        assert!(mismatches(r#"["array/append", "@entity.tags", "x"]"#, tags()).is_empty());
    }

    #[test]
    fn lists_of_op_tuples_of_unlike_widths_join_into_one_list() {
        let updates =
            r#"["array/map", "@entity.tags", ["fn", "t", ["upd", "E", "@t", { "a": 1 }]]]"#;
        let prepended = format!(r#"["array/prepend", {updates}, ["del", "E", "x"]]"#);
        assert!(
            mismatches(&prepended, tags()).is_empty(),
            "{:?}",
            mismatches(&prepended, tags())
        );
        let deletes = r#"["array/map", "@entity.tags", ["fn", "t", ["del", "E", "@t"]]]"#;
        let concatenated = format!(r#"["array/concat", {deletes}, {updates}]"#);
        assert!(
            mismatches(&concatenated, tags()).is_empty(),
            "{:?}",
            mismatches(&concatenated, tags())
        );
    }

    #[test]
    fn control_lists_of_unlike_scalars_still_do_not_join() {
        assert_eq!(
            mismatches(r#"["array/concat", [1], ["a"]]"#, tags()).len(),
            1
        );
        assert_eq!(
            mismatches(r#"["array/prepend", "@entity.tags", 1]"#, tags()).len(),
            1
        );
    }

    fn effects_in_lambdas(json: &str) -> usize {
        validate_operator_arg_types(
            &parse(json),
            "p",
            &HashMap::new(),
            &HashMap::new(),
            &no_types(),
            None,
        )
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::EffectInLambda)
        .count()
    }

    #[test]
    fn an_effect_inside_a_collection_lambda_is_refused() {
        assert_eq!(
            effects_in_lambdas(
                r#"["array/map", "@payload.items", ["fn", "x", ["emit", "E", { "id": "@x" }]]]"#
            ),
            1
        );
        assert_eq!(
            effects_in_lambdas(
                r#"["array/filter", "@payload.items", ["fn", "x", ["do", ["set", "@entity.n", 1], true]]]"#
            ),
            1
        );
        assert_eq!(
            effects_in_lambdas(
                r#"["array/map", "@payload.items", ["fn", "x", ["if", true, ["persist", "create", "E", {}], null]]]"#
            ),
            1
        );
    }

    #[test]
    fn control_a_pure_lambda_and_an_effect_outside_one_are_accepted() {
        assert_eq!(
            effects_in_lambdas(
                r#"["array/map", "@payload.items", ["fn", "x", ["str/upper", "@x"]]]"#
            ),
            0
        );
        assert_eq!(
            effects_in_lambdas(
                r#"["emit", "E", { "ids": ["array/map", "@payload.items", ["fn", "x", ["object/get", "@x", "id"]]] }]"#
            ),
            0
        );
        assert_eq!(
            effects_in_lambdas(
                r#"["array/map", "@payload.items", ["fn", "x", ["quote", ["emit", "E", {}]]]]"#
            ),
            0,
            "a quoted program is data"
        );
    }

    #[test]
    fn two_operands_of_one_type_variable_must_share_a_type() {
        assert_eq!(
            mismatches(r#"["=", "@entity.count", "x"]"#, tags()).len(),
            1
        );
        assert!(mismatches(r#"["=", "@entity.count", 3]"#, tags()).is_empty());
        assert_eq!(
            mismatches(r#"["if", ["==", "@entity.count", 3], 1, "a"]"#, tags()).len(),
            1
        );
        assert!(mismatches(r#"["if", ["==", "@entity.count", 3], 1, 2]"#, tags()).is_empty());
    }

    #[test]
    fn a_polymorphic_result_is_seen_through_by_the_enclosing_call() {
        // (array/first tags) is a string, so (+ it 1) is a string in arithmetic.
        let got = mismatches(r#"["+", ["array/first", "@entity.tags"], 1]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("argument 1 is a string"), "{got:?}");
        assert!(mismatches(r#"["str/upper", ["array/first", "@entity.tags"]]"#, tags()).is_empty());
    }

    #[test]
    fn a_lambda_parameter_is_typed_by_the_list_it_walks() {
        let got = mismatches(
            r#"["array/map", "@entity.tags", ["fn", "t", ["+", "@t", 1]]]"#,
            tags(),
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(mismatches(
            r#"["array/map", "@entity.tags", ["fn", "t", ["str/upper", "@t"]]]"#,
            tags()
        )
        .is_empty());
    }

    #[test]
    fn a_key_that_names_no_field_of_the_record_is_an_error() {
        let got = mismatches(r#"["object/get", "@entity.profile", "nope"]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`nope`"), "{got:?}");
        assert!(mismatches(r#"["object/get", "@entity.profile", "name"]"#, tags()).is_empty());
        assert_eq!(
            mismatches(
                r#"["+", ["object/get", "@entity.profile", "name"], 1]"#,
                tags()
            )
            .len(),
            1
        );
    }

    #[test]
    fn an_effect_nested_in_a_value_expression_is_still_checked() {
        let got = mismatches(
            r#"["if", true, ["set", "@entity.count", ["str/upper", 5]], null]"#,
            tags(),
        );
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn a_default_makes_an_undeclared_field_a_legitimate_optional_read() {
        assert!(mismatches(r#"["object/get", "@entity.profile", "roles", []]"#, tags()).is_empty());
        assert_eq!(
            mismatches(r#"["object/get", "@entity.profile", "roles"]"#, tags()).len(),
            1
        );
        assert!(mismatches(r#"["object/has", "@entity.profile", "roles"]"#, tags()).is_empty());
        assert!(
            mismatches(r#"["object/get", "@entity", "id"]"#, tags()).is_empty(),
            "the whole entity row also carries system fields, so it is a record of unknown extent"
        );
    }

    fn with_types(json: &str) -> Vec<String> {
        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "collection": "es", "fields": [] },
                "traits": [],
                "pages": [],
                "types": {
                    "Point": [{ "name": "x", "type": "number" }, { "name": "y", "type": "number" }],
                    "Row": [{ "name": "", "type": "Map<string,string>" }],
                    "Rows": [{ "name": "", "type": "[Row]" }]
                }
            }]
        }))
        .unwrap();
        validate_operator_arg_types(
            &parse(json),
            "p",
            &HashMap::new(),
            &HashMap::new(),
            &no_types(),
            Some(&schema),
        )
        .errors
        .iter()
        .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::ObjectPathUndeclared))
        .map(|e| e.message.clone())
        .collect()
    }

    #[test]
    fn json_parse_names_a_declared_type_and_its_fields_are_typed() {
        assert!(with_types(r#"["json/parse", "{}", "Point"]"#).is_empty());
        let unknown = with_types(r#"["json/parse", "{}", "Nope"]"#);
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert!(unknown[0].contains("`Nope`"), "{unknown:?}");
        // The parsed value is a Point: x is a number, so a string operator rejects it, and z is no field.
        let got = with_types(
            r#"["str/upper", ["object/get", ["json/parse", ["str/concat", "{", "}"], "Point"], "x", 0]]"#,
        );
        assert_eq!(got.len(), 1, "{got:?}");
        let missing = with_types(r#"["object/get", ["json/parse", "{}", "Point"], "z"]"#);
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(
            with_types(r#"["+", ["object/get", ["json/parse", "{}", "Point"], "x"], 1]"#)
                .is_empty()
        );
    }

    #[test]
    fn a_string_binding_in_a_boolean_position_is_an_error_for_not_if_and_when() {
        let name = || field("name", "string");
        for (expr, what) in [
            (r#"["not", "@entity.name"]"#, "not"),
            (r#"["if", "@entity.name", 1, 2]"#, "if"),
            (r#"["when", "@entity.name", ["emit", "X"]]"#, "when"),
            (r#"["and", true, "@entity.name"]"#, "and"),
        ] {
            let got = mismatches(expr, name());
            assert_eq!(got.len(), 1, "{what}: {got:?}");
            assert!(got[0].contains("string"), "{what}: {got:?}");
        }
        // control: the explicit comparison is a boolean
        assert!(mismatches(r#"["not", ["=", "@entity.name", ""]]"#, name()).is_empty());
        assert!(mismatches(r#"["if", ["!=", "@entity.name", ""], 1, 2]"#, name()).is_empty());
    }

    #[test]
    fn json_parse_may_name_a_dictionary_or_list_alias() {
        assert!(with_types(
            r#"["str/upper", ["object/get", ["json/parse", "{}", "Row"], "any-key"]]"#
        )
        .is_empty());
        let wrong = with_types(r#"["+", ["object/get", ["json/parse", "{}", "Row"], "k"], 1]"#);
        assert_eq!(wrong.len(), 1, "a Row holds strings: {wrong:?}");
        assert!(with_types(r#"["array/len", ["json/parse", "[]", "Rows"]]"#).is_empty());
        let not_a_list = with_types(r#"["str/upper", ["json/parse", "[]", "Rows"]]"#);
        assert_eq!(not_a_list.len(), 1, "{not_a_list:?}");
    }

    mod ordering_optional_operand {
        use super::*;

        fn entity(required: bool, defaulted: bool) -> HashMap<String, EntityFieldMeta> {
            let mut m = HashMap::new();
            m.insert(
                "p".to_string(),
                EntityFieldMeta {
                    field_type: "number".into(),
                    shape: orbital_core_base::ir::OirType::Number,
                    required,
                    defaulted,
                },
            );
            m
        }

        fn findings(
            json: &str,
            entity: HashMap<String, EntityFieldMeta>,
            optional_payload: &[&str],
        ) -> usize {
            let payload: HashMap<String, orbital_core_base::ir::OirType> =
                [("q".to_string(), orbital_core_base::ir::OirType::Number)]
                    .into_iter()
                    .collect();
            let types = BindingTypes {
                payload,
                optional_payload: optional_payload.iter().map(|n| n.to_string()).collect(),
                locals: Default::default(),
                type_params: Default::default(),
                ..no_types()
            };
            validate_operator_arg_types(&parse(json), "p", &entity, &HashMap::new(), &types, None)
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::OrderingOptionalOperand)
                .count()
        }

        fn null_findings(json: &str, entity: HashMap<String, EntityFieldMeta>) -> usize {
            let types = BindingTypes {
                payload: HashMap::new(),
                ..no_types()
            };
            validate_operator_arg_types(&parse(json), "p", &entity, &HashMap::new(), &types, None)
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::OperatorNullOperand)
                .count()
        }

        #[test]
        fn a_null_test_on_a_let_local_narrows_it_as_an_operator_operand() {
            let local = |body: &str| format!(r#"["let", [["x", ["str/to-number", "a"]]], {body}]"#);
            let divide = r#"["/", 1, "@x"]"#;
            let untested = null_findings(&local(divide), entity(false, false));
            assert!(untested > 0, "control: untested, the divisor may be null");
            assert_eq!(
                null_findings(
                    &local(&format!(r#"["if", ["=", "@x", null], 0, {divide}]"#)),
                    entity(false, false)
                ),
                0
            );
            assert_eq!(
                null_findings(
                    &local(&format!(
                        r#"["if", ["and", true, ["!=", "@x", null]], {divide}, 0]"#
                    )),
                    entity(false, false)
                ),
                0
            );
            assert!(
                null_findings(
                    &local(&format!(r#"["if", ["=", "@x", null], {divide}, 0]"#)),
                    entity(false, false)
                ) > 0,
                "control: the null branch"
            );
        }

        #[test]
        fn std_browse_scope_filter_with_the_scope_knob_substituted_to_null_is_not_flagged() {
            let filter = |scope: &str, search: &str| {
                format!(
                    r#"["and", ["or", ["not", ["validate/required", {scope}]], ["=", ["object/get", "@entity", {scope}], "@user.id"]],
                        ["or", ["=", "@payload.q", ""], ["if", ["=", {search}, null], false, ["str/includes", ["default", ["object/get", "@entity", {search}], ""], "@payload.q"]]]]"#
                )
            };
            assert_eq!(
                null_findings(&filter("null", "null"), entity(false, false)),
                0,
                "both knobs unset"
            );
            assert_eq!(
                null_findings(&filter(r#""""#, "null"), entity(false, false)),
                0,
                "an empty scope is missing too"
            );
            assert!(
                null_findings(&filter(r#""@entity.q""#, "null"), entity(false, false)) == 0,
                "control: a bound scope reads"
            );
            assert!(
                null_findings(
                    r#"["and", ["or", ["not", ["validate/required", "@entity.p"]], ["=", ["object/get", "@entity", null], 1]], true]"#,
                    entity(false, false)
                ) > 0,
                "control: a non-literal presence test decides nothing, so the null read is checked"
            );
        }

        #[test]
        fn a_divisor_that_can_be_missing_makes_the_result_missing_where_a_value_is_needed() {
            for json in [
                r#"["+", ["/", 1, "@entity.p"], 1]"#,
                r#"["+", ["%", 7, "@entity.p"], 1]"#,
                r#"["+", ["math/mod", 7, "@entity.p"], 1]"#,
                r#"["+", ["math/randomInt", 1, "@entity.p"], 1]"#,
            ] {
                assert_eq!(null_findings(json, entity(false, false)), 2, "{json}: the divisor raises on null, and the result is missing when the call fails");
            }
        }

        #[test]
        fn control_a_required_defaulted_guarded_or_tolerant_operand_is_fine() {
            assert_eq!(
                null_findings(r#"["/", 1, "@entity.p"]"#, entity(true, false)),
                0,
                "required"
            );
            assert_eq!(
                null_findings(r#"["/", 1, "@entity.p"]"#, entity(false, true)),
                0,
                "defaulted"
            );
            assert_eq!(
                null_findings(r#"["/", "@entity.p", 2]"#, entity(false, false)),
                1,
                "a missing numerator raises on null"
            );
            assert_eq!(
                null_findings(r#"["/", "@entity.p", 2]"#, entity(true, false)),
                0,
                "control: a required numerator"
            );
            assert_eq!(
                null_findings(
                    r#"["and", ["validate/required", "@entity.p"], ["/", 1, "@entity.p"]]"#,
                    entity(false, false)
                ),
                0,
                "a presence guard narrows the operand"
            );
            assert_eq!(
                null_findings(
                    r#"["when", ["!=", "@entity.p", null], ["/", 1, "@entity.p"]]"#,
                    entity(false, false)
                ),
                0,
                "a null comparison narrows the operand"
            );
            assert_eq!(
                null_findings(r#"["+", 1, "@entity.p"]"#, entity(false, false)),
                1,
                "an operand that is missing raises on null"
            );
            assert_eq!(
                null_findings(r#"["+", 1, "@entity.p"]"#, entity(true, false)),
                0,
                "control: a required operand"
            );
        }

        // G-ORB-548: an emit payload's record literal holds value expressions, checked at any depth.
        #[test]
        fn a_missing_read_inside_a_nested_emit_payload_record_is_flagged_once() {
            let find = r#"["array/find", ["list", {"value": "x"}], ["fn", "v", true]]"#;
            let read = format!(r#"["object/get", {find}, "value"]"#);
            let defaulted =
                format!(r#"["object/get", ["default", {find}, {{"value": ""}}], "value"]"#);
            let nested =
                |value: &str| format!(r#"["emit", "ENV_CHECK", {{"env": {{"a": {value}}}}}]"#);
            assert_eq!(
                null_findings(&nested(&read), HashMap::new()),
                1,
                "a nested record field"
            );
            assert_eq!(
                null_findings(&nested(&defaulted), HashMap::new()),
                0,
                "control: defaulted"
            );
            assert_eq!(
                null_findings(
                    &format!(r#"["emit", "EV", {{"a": {read}}}]"#),
                    HashMap::new()
                ),
                1,
                "a top-level payload field"
            );
            assert_eq!(
                null_findings(
                    &format!(r#"["emit", "EV", {{"xs": ["list", {read}]}}]"#),
                    HashMap::new()
                ),
                1,
                "a list literal item"
            );
            assert_eq!(
                null_findings(&read, HashMap::new()),
                1,
                "control: a bare read is flagged once"
            );
        }

        #[test]
        fn an_optional_field_in_an_ordering_position_is_an_error() {
            for op in ["<", "<=", ">", ">="] {
                assert_eq!(
                    findings(
                        &format!(r#"["{op}", "@entity.p", 3]"#),
                        entity(false, false),
                        &[]
                    ),
                    1,
                    "{op}"
                );
                assert_eq!(
                    findings(
                        &format!(r#"["{op}", 3, "@entity.p"]"#),
                        entity(false, false),
                        &[]
                    ),
                    1,
                    "{op} right"
                );
            }
            assert_eq!(
                findings(
                    r#"["<", ["object/get", "@entity", "p"], 3]"#,
                    entity(false, false),
                    &[]
                ),
                1,
                "object/get"
            );
        }

        #[test]
        fn a_required_or_defaulted_field_is_fine() {
            assert_eq!(
                findings(r#"["<", "@entity.p", 3]"#, entity(true, false), &[]),
                0,
                "required"
            );
            assert_eq!(
                findings(r#"["<", "@entity.p", 3]"#, entity(false, true), &[]),
                0,
                "defaulted"
            );
            assert_eq!(
                findings(r#"["==", "@entity.p", 3]"#, entity(false, false), &[]),
                0,
                "equality is not ordering"
            );
        }

        #[test]
        fn a_presence_guard_narrows_the_operand() {
            let guarded = [
                r#"["and", ["validate/required", "@entity.p"], ["<", "@entity.p", 3]]"#,
                r#"["and", ["!=", "@entity.p", null], ["<", "@entity.p", 3]]"#,
                r#"["and", ["not", ["=", "@entity.p", null]], ["<", "@entity.p", 3]]"#,
                r#"["and", "@entity.p", ["<", "@entity.p", 3]]"#,
                r#"["if", ["validate/required", "@entity.p"], ["<", "@entity.p", 3], false]"#,
                r#"["when", ["validate/required", "@entity.p"], ["<", "@entity.p", 3]]"#,
                r#"["or", ["not", ["validate/required", "@entity.p"]], ["<", "@entity.p", 3]]"#,
                r#"["if", ["validate/required", "@entity.p"], true, ["<", "@entity.p", 3]]"#,
                r#"["and", ["and", true, ["validate/required", "@entity.p"]], ["<", "@entity.p", 3]]"#,
                r#"["and", ["validate/required", ["object/get", "@entity", "p"]], ["<", "@entity.p", 3]]"#,
            ];
            for json in &guarded[..guarded.len() - 3] {
                assert_eq!(findings(json, entity(false, false), &[]), 0, "{json}");
            }
            // The if with the guard in the wrong branch is not narrowed; the others above are.
            assert_eq!(
                findings(guarded[7], entity(false, false), &[]),
                1,
                "{}",
                guarded[7]
            );
            assert_eq!(
                findings(guarded[8], entity(false, false), &[]),
                0,
                "{}",
                guarded[8]
            );
            assert_eq!(
                findings(guarded[9], entity(false, false), &[]),
                0,
                "{}",
                guarded[9]
            );
        }

        #[test]
        fn a_guard_that_does_not_cover_the_operand_does_not_narrow() {
            for json in [
                r#"["and", ["<", "@entity.p", 3], ["validate/required", "@entity.p"]]"#,
                r#"["or", ["validate/required", "@entity.p"], ["<", "@entity.p", 3]]"#,
                r#"["and", ["or", ["validate/required", "@entity.p"], true], ["<", "@entity.p", 3]]"#,
                r#"["and", ["not", ["validate/required", "@entity.p"]], ["<", "@entity.p", 3]]"#,
                r#"["and", ["validate/required", "@entity.other"], ["<", "@entity.p", 3]]"#,
            ] {
                assert_eq!(findings(json, entity(false, false), &[]), 1, "{json}");
            }
        }

        #[test]
        fn an_optional_payload_field_is_checked_like_an_entity_field() {
            assert_eq!(
                findings(r#"["<", "@payload.q", 3]"#, HashMap::new(), &["q"]),
                1
            );
            assert_eq!(
                findings(r#"["<", "@payload.q", 3]"#, HashMap::new(), &[]),
                0,
                "required payload field"
            );
            assert_eq!(
                findings(
                    r#"["and", ["validate/required", "@payload.q"], ["<", "@payload.q", 3]]"#,
                    HashMap::new(),
                    &["q"]
                ),
                0
            );
        }

        #[test]
        fn g_std_290_a_null_test_on_an_event_optional_payload_field_is_a_valid_operand() {
            let null_errors = |optional: bool| {
                let mut types = BindingTypes {
                    payload: [("q".to_string(), orbital_core_base::ir::OirType::String)]
                        .into_iter()
                        .collect(),
                    optional_payload: if optional {
                        ["q".to_string()].into_iter().collect()
                    } else {
                        Default::default()
                    },
                    ..no_types()
                };
                types.type_optional_payload();
                validate_operator_arg_types(
                    &parse(r#"["!=", "@payload.q", null]"#),
                    "p",
                    &HashMap::new(),
                    &HashMap::new(),
                    &types,
                    None,
                )
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch && e.message.contains("never null"))
                .count()
            };
            assert_eq!(null_errors(true), 0, "an optional field may be absent");
            assert_eq!(null_errors(false), 1, "control: a required field is never null");
        }

        #[test]
        fn a_local_bound_to_an_optional_field_stays_optional() {
            assert_eq!(
                findings(
                    r#"["let", [["x", "@entity.p"]], ["<", "@x", 3]]"#,
                    entity(false, false),
                    &[]
                ),
                1
            );
            assert_eq!(
                findings(
                    r#"["let", [["x", "@entity.p"]], ["<", "@x", 3]]"#,
                    entity(true, false),
                    &[]
                ),
                0
            );
            assert_eq!(
                findings(
                    r#"["let", [["x", "@entity.p"]], ["and", ["validate/required", "@x"], ["<", "@x", 3]]]"#,
                    entity(false, false),
                    &[]
                ),
                0
            );
        }
    }

    /// K2 census (P-017): per operator, how many argument positions the static inference proves. Measurement only.
    /// `K2_CENSUS_FILES` comma-separated `.orb`/`.lolo` paths, `K2_CENSUS_OUT` the JSON path.
    mod k2_census {
        use super::super::{with_fetch_filter_rows, with_row_locals, BindingTypes, ExprReturnCtx};
        use crate::type_infer::ProvenKey;
        use crate::type_infer::{is_effect, is_value_operator};
        use orbital_core_base::ir::{OirExpr, OirType};
        use orbital_core_base::schema::SExpression;
        use orbital_core_base::schema::TraitReference;
        use std::collections::BTreeMap;
        use std::collections::HashMap;

        #[derive(Default, Clone, Copy)]
        struct Row {
            positions: usize,
            proven: usize,
            calls: usize,
            all_proven: usize,
        }

        type Tally = BTreeMap<String, Row>;

        fn markable(head: &str) -> bool {
            is_value_operator(head)
                && !is_effect(head)
                && !matches!(
                    head,
                    "let"
                        | "proven"
                        | "fn"
                        | "lambda"
                        | "do"
                        | "list"
                        | "if"
                        | "when"
                        | "unless"
                        | "cond"
                        | "match"
                )
        }

        fn count(expr: &SExpression, proven: &HashMap<ProvenKey, Vec<usize>>, tally: &mut Tally) {
            match expr {
                SExpression::Object(fields) => {
                    fields.values().for_each(|v| count(v, proven, tally))
                }
                SExpression::Array(items) => {
                    let Some(SExpression::String(head)) = items.first() else {
                        items.iter().for_each(|item| count(item, proven, tally));
                        return;
                    };
                    if matches!(head.as_str(), "quote" | "quasiquote") {
                        return;
                    }
                    items[1..]
                        .iter()
                        .for_each(|item| count(item, proven, tally));
                    let arity = items.len() - 1;
                    if arity == 0 || !markable(head) {
                        return;
                    }
                    let n = orbital_core_eval::lower::lower_sexpr(expr)
                        .ok()
                        .and_then(|oir| ProvenKey::of(&oir))
                        .filter(|key| key.arity() == arity)
                        .and_then(|key| proven.get(&key).map(Vec::len))
                        .unwrap_or(0);
                    let row = tally.entry(head.as_str().to_string()).or_default();
                    row.positions += arity;
                    row.proven += n;
                    row.calls += 1;
                    row.all_proven += usize::from(n == arity);
                }
                _ => {}
            }
        }

        fn walk(
            expr: &SExpression,
            ctx: &ExprReturnCtx<'_>,
            condition: Option<&OirExpr>,
            tally: &mut Tally,
        ) {
            match expr {
                SExpression::Object(fields) => {
                    fields.values().for_each(|v| walk(v, ctx, condition, tally))
                }
                SExpression::Array(items) => {
                    if let Some(SExpression::String(head)) = items.first() {
                        match head.as_str() {
                            "quote" | "quasiquote" => return,
                            "fn" | "lambda" => {
                                if items.len() >= 3 {
                                    walk(&items[items.len() - 1], ctx, condition, tally);
                                }
                                return;
                            }
                            name if is_effect(name) || is_value_operator(name) => {
                                let mut rows = Vec::new();
                                let mut row_locals = Vec::new();
                                let read =
                                    with_fetch_filter_rows(expr, ctx, &mut rows, &mut row_locals);
                                let scoped = with_row_locals(ctx, row_locals);
                                let proven = match orbital_core_eval::lower::lower_sexpr(&read) {
                                    Ok(oir) => match &scoped {
                                        Some(types) => crate::type_infer::proven_calls_under(
                                            &oir,
                                            condition,
                                            &ExprReturnCtx {
                                                entity: ctx.entity,
                                                payload: ctx.payload,
                                                types: Some(types),
                                                schema: ctx.schema,
                                            },
                                        ),
                                        None => crate::type_infer::proven_calls_under(
                                            &oir, condition, ctx,
                                        ),
                                    },
                                    Err(_) => HashMap::new(),
                                };
                                count(expr, &proven, tally);
                                return;
                            }
                            _ => {}
                        }
                    }
                    items
                        .iter()
                        .for_each(|item| walk(item, ctx, condition, tally));
                }
                _ => {}
            }
        }

        fn census_schema(
            schema: &orbital_core_base::schema::OrbitalSchema,
            trust_payload: bool,
        ) -> Tally {
            let definitions = crate::phases::validation::entity_definitions_of(schema);
            let no_payload: HashMap<String, String> = HashMap::new();
            let mut tally = Tally::new();
            let guaranteed = crate::phases::validation::effect::GuaranteedPayloads::new();
            for orbital in &schema.orbitals {
                for trait_ref in &orbital.traits {
                    let TraitReference::Inline(trait_def) = trait_ref else {
                        continue;
                    };
                    let scopes = crate::phases::validation::effect::TraitScopes::new(
                        schema,
                        trait_def,
                        &definitions,
                        &guaranteed,
                    );
                    let unproven = |mut types: BindingTypes| {
                        if !trust_payload {
                            types.payload.clear();
                            types.prev_payload = None;
                            types.user = OirType::Struct {
                                additional: None,
                                fields: Default::default(),
                            };
                        }
                        types
                    };
                    let mut run =
                        |expr: &SExpression, guard: Option<&SExpression>, types: &BindingTypes| {
                            let ctx = ExprReturnCtx {
                                entity: &scopes.entity_field_types,
                                payload: &no_payload,
                                types: Some(types),
                                schema: Some(schema),
                            };
                            let condition =
                                guard.and_then(|g| orbital_core_eval::lower::lower_sexpr(g).ok());
                            walk(expr, &ctx, condition.as_ref(), &mut tally);
                        };
                    for transition in &trait_def.state_machine.transitions {
                        let types =
                            unproven(scopes.types_for(schema, trait_def, &transition.event));
                        if let Some(guard) = &transition.guard {
                            run(guard, None, &types);
                        }
                        for effect in transition.effects.iter().flatten() {
                            run(effect, transition.guard.as_ref(), &types);
                        }
                    }
                    if let Some(ticks) = &trait_def.ticks {
                        let types = unproven(scopes.tick_types.clone());
                        for tick in ticks {
                            if let Some(guard) = &tick.guard {
                                run(guard, None, &types);
                            }
                            for effect in &tick.effects {
                                run(effect, tick.guard.as_ref(), &types);
                            }
                        }
                    }
                }
            }
            tally
        }

        fn total(tally: &Tally) -> Row {
            tally.values().fold(Row::default(), |a, r| Row {
                positions: a.positions + r.positions,
                proven: a.proven + r.proven,
                calls: a.calls + r.calls,
                all_proven: a.all_proven + r.all_proven,
            })
        }

        fn merge(into: &mut Tally, from: &Tally) {
            for (op, r) in from {
                let row = into.entry(op.clone()).or_default();
                row.positions += r.positions;
                row.proven += r.proven;
                row.calls += r.calls;
                row.all_proven += r.all_proven;
            }
        }

        fn json(tally: &Tally) -> serde_json::Value {
            let row = |r: &Row| serde_json::json!({ "positions": r.positions, "proven": r.proven, "calls": r.calls, "allProven": r.all_proven });
            let mut map = serde_json::Map::new();
            for (op, r) in tally {
                map.insert(op.clone(), row(r));
            }
            map.insert("_total".to_string(), row(&total(tally)));
            serde_json::Value::Object(map)
        }

        fn resolved(path: &std::path::Path) -> orbital_core_base::schema::OrbitalSchema {
            crate::loader::register_source_lowerer(|path, _, _| {
                orbital_test_support::lower_lolo_file(path)
            });
            let start = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let user_store = orbital_core_base::behavior_packages::user_store_dir()
                .unwrap_or_else(|| start.clone());
            let mut options = crate::CompilerOptions::default();
            options.inline.loader_config.packages =
                orbital_core_base::behavior_packages::discover(&start, &user_store)
                    .expect("package discovery");
            let schema = if path.extension().and_then(|e| e.to_str()) == Some("lolo") {
                orbital_test_support::lower_lolo_file(path)
                    .unwrap_or_else(|e| panic!("lower {}: {e}", path.display()))
            } else {
                crate::phases::parse_file(path)
                    .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
            };
            crate::phases::resolve_compiled_program_with_path(schema, &options, Some(path))
                .unwrap_or_else(|e| panic!("resolve {}: {e}", path.display()))
        }

        #[test]
        #[ignore = "tool: census of provable argument positions over the .lolo files named by K2_CENSUS_FILES"]
        fn k2_census_provable_argument_positions() {
            let files = std::env::var("K2_CENSUS_FILES").expect("K2_CENSUS_FILES");
            let out = std::env::var("K2_CENSUS_OUT").expect("K2_CENSUS_OUT");
            let mut as_marked = Tally::new();
            let mut typed = Tally::new();
            let mut per_file = serde_json::Map::new();
            for file in files.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                let path = std::fs::canonicalize(file).unwrap_or_else(|e| panic!("{file}: {e}"));
                let schema = resolved(&path);
                let a = census_schema(&schema, false);
                let b = census_schema(&schema, true);
                let row = |r: Row| serde_json::json!({ "positions": r.positions, "proven": r.proven, "calls": r.calls, "allProven": r.all_proven });
                per_file.insert(
                    file.to_string(),
                    serde_json::json!({ "untrustedPayloads": row(total(&a)), "typedPayloads": row(total(&b)) }),
                );
                eprintln!(
                    "{file}: untrusted {:?} typed {:?}",
                    total(&a).proven,
                    total(&b).proven
                );
                merge(&mut as_marked, &a);
                merge(&mut typed, &b);
            }
            let mut report = match json(&as_marked) {
                serde_json::Value::Object(m) => m,
                _ => unreachable!(),
            };
            report.insert("_files".to_string(), serde_json::Value::Object(per_file));
            report.insert("_typedPayloads".to_string(), json(&typed));
            std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
        }
    }
}
