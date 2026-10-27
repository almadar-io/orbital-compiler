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
    validate_operator_arg_types_under(expr, None, path, entity_field_types, payload_field_types, types, schema)
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
    let condition = guard.and_then(|guard| orbital_core_eval::lower::lower_sexpr_keeping_typed(guard).ok());
    collect(expr, path, &ctx, condition.as_ref(), &mut result, false);
    result
}

/// `expr` with each value-operator call whose argument types the static check proved wrapped as
/// `["proven", [positions], op, ...args]` (G-CROSS-335; flat, so marking adds no depth), so the kernel skips the run-time check at those positions. Each
/// maximal value or effect expression is inferred as one tree where `guard` holds, as the check reads it.
pub(crate) fn mark_proven_operator_args(
    expr: &SExpression,
    guard: Option<&SExpression>,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    types: &BindingTypes,
    schema: Option<&orbital_core_base::schema::OrbitalSchema>,
) -> SExpression {
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
        types: Some(types),
        schema,
    };
    let condition = guard.and_then(|guard| orbital_core_eval::lower::lower_sexpr_keeping_typed(guard).ok());
    prove(expr, &ctx, condition.as_ref())
}

/// The walk of [`collect`]: each maximal value or effect expression is inferred as one tree and marked.
fn prove(expr: &SExpression, ctx: &ExprReturnCtx<'_>, condition: Option<&OirExpr>) -> SExpression {
    match expr {
        SExpression::Object(fields) => {
            SExpression::Object(fields.iter().map(|(k, v)| (k.clone(), prove(v, ctx, condition))).collect())
        }
        SExpression::Array(items) => {
            if let Some(SExpression::String(head)) = items.first() {
                match head.as_str() {
                    "quote" | "quasiquote" => return expr.clone(),
                    "fn" | "lambda" => {
                        let mut items = items.clone();
                        if let Some(body) = items.last_mut().filter(|_| expr_len(expr) >= 3) {
                            *body = prove(body, ctx, condition);
                        }
                        return SExpression::Array(items);
                    }
                    name if is_effect(name) || is_value_operator(name) => {
                        let mut rows = Vec::new();
                        let read = with_fetch_filter_rows(expr, ctx, &mut rows);
                        let Ok(oir) = orbital_core_eval::lower::lower_sexpr_keeping_typed(&read) else {
                            return expr.clone();
                        };
                        let proven = crate::type_infer::proven_calls_under(&oir, condition, ctx);
                        return if proven.is_empty() { expr.clone() } else { wrap(expr, &proven) };
                    }
                    _ => {}
                }
            }
            SExpression::Array(items.iter().map(|item| prove(item, ctx, condition)).collect())
        }
        other => other.clone(),
    }
}

fn expr_len(expr: &SExpression) -> usize {
    match expr {
        SExpression::Array(items) => items.len(),
        _ => 0,
    }
}

/// `expr` with every value-operator call `proven` names wrapped at its proven positions. A call is matched by the key
/// of what it lowers to, and only where the call and its lowering agree on arity (a variadic `+` lowered to nested
/// operations does not).
fn wrap(
    expr: &SExpression,
    proven: &HashMap<crate::type_infer::ProvenKey, Vec<usize>>,
) -> SExpression {
    match expr {
        SExpression::Object(fields) => {
            SExpression::Object(fields.iter().map(|(k, v)| (k.clone(), wrap(v, proven))).collect())
        }
        SExpression::Array(items) => {
            let Some(SExpression::String(head)) = items.first() else {
                return SExpression::Array(items.iter().map(|item| wrap(item, proven)).collect());
            };
            if matches!(head.as_str(), "quote" | "quasiquote") {
                return expr.clone();
            }
            let call: Vec<SExpression> =
                std::iter::once(items[0].clone()).chain(items[1..].iter().map(|item| wrap(item, proven))).collect();
            let markable = is_value_operator(head)
                && !is_effect(head)
                // Control forms can hold effects, which the kernel runs only under their own head.
                && !matches!(
                    head.as_str(),
                    "let" | "typed" | "proven" | "fn" | "lambda" | "do" | "list" | "if" | "when" | "unless" | "cond" | "match"
                );
            let positions = markable
                .then(|| orbital_core_eval::lower::lower_sexpr_keeping_typed(expr).ok())
                .flatten()
                .and_then(|oir| crate::type_infer::ProvenKey::of(&oir))
                .filter(|key| key.arity() == items.len() - 1)
                .and_then(|key| proven.get(&key));
            match positions {
                Some(positions) => {
                    let mut flat = vec![
                        SExpression::string("proven"),
                        SExpression::Array(positions.iter().map(|&p| SExpression::Number(p as f64)).collect()),
                    ];
                    flat.extend(call);
                    SExpression::Array(flat)
                }
                None => SExpression::Array(call),
            }
        }
        other => other.clone(),
    }
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
            result.merge(required_field_from_missing_result(items, path, ctx, condition));
            result.merge(super::keyed_call::check_key_expression_type(items, path, ctx));
            result.merge(super::keyed_call::check_cancel_call_arg(items, path, ctx));
            result.merge(super::cast::check_cast(items, path, ctx));
            result.merge(super::fetch::check_fetch_nearest(items, path, ctx));
            result.merge(super::http::check_http_operand_types(items, path, ctx));
            let mut covered = covered;
            let mut children = items.as_slice();
            if let Some(SExpression::String(head)) = items.first() {
                match head.as_str() {
                    // Syntax, not values: a quoted body.
                    "quote" | "quasiquote" => return,
                    // A lambda's parameter names are syntax; its body is a value.
                    "fn" | "lambda" => {
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
                        let expr = with_fetch_filter_rows(expr, ctx, &mut rows);
                        report(&expr, path, ctx, condition, &rows, result);
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
    let (Some(SExpression::String(head)), Some(SExpression::String(target)), Some(value)) = (items.first(), items.get(1), items.get(2))
    else {
        return result;
    };
    let Some(field) = target.strip_prefix("@entity.").filter(|f| head == "set" && !f.contains('.')) else {
        return result;
    };
    if !ctx.entity.get(field).is_some_and(|meta| meta.required) {
        return result;
    }
    let Ok(oir) = orbital_core_eval::lower::lower_sexpr_keeping_typed(value) else {
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
                    "quote" | "quasiquote" | "fn" | "lambda" => return None,
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
/// `rows` collects how each row read prints, so a finding names it `@entity` as written.
fn with_fetch_filter_rows(expr: &SExpression, ctx: &ExprReturnCtx<'_>, rows: &mut Vec<String>) -> SExpression {
    fn row_reads(expr: &SExpression, row: &SExpression) -> SExpression {
        match expr {
            SExpression::Array(items) => {
                let mut items: Vec<SExpression> = items.iter().map(|item| row_reads(item, row)).collect();
                if matches!(items.as_slice(), [SExpression::String(head), SExpression::String(arg), ..] if head == "object/get" && arg == "@entity") {
                    items[1] = row.clone();
                }
                SExpression::Array(items)
            }
            SExpression::Object(map) => {
                SExpression::Object(map.iter().map(|(k, v)| (k.clone(), row_reads(v, row))).collect())
            }
            other => other.clone(),
        }
    }
    match expr {
        SExpression::Array(items) => {
            let mut items: Vec<SExpression> = items.iter().map(|item| with_fetch_filter_rows(item, ctx, rows)).collect();
            if let [SExpression::String(head), SExpression::String(target), SExpression::Object(options), ..] = items.as_mut_slice() {
                if let (true, Some(row_type)) = (head == "fetch", ctx.entity_type(target)) {
                    if let Some(filter) = options.get_mut("filter") {
                        let shape = serde_json::to_string(&row_type).unwrap_or_default();
                        let row = SExpression::Array(vec![
                            SExpression::string("typed"),
                            SExpression::string("@entity"),
                            SExpression::string(shape),
                        ]);
                        if let Ok(lowered) = orbital_core_eval::lower::lower_sexpr_keeping_typed(&row) {
                            rows.push(orbital_core_base::ir::pretty_print_expr(&lowered));
                        }
                        *filter = row_reads(filter, &row);
                    }
                }
            }
            SExpression::Array(items)
        }
        SExpression::Object(map) => {
            SExpression::Object(map.iter().map(|(k, v)| (k.clone(), with_fetch_filter_rows(v, ctx, rows))).collect())
        }
        other => other.clone(),
    }
}

fn report(
    expr: &SExpression,
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    condition: Option<&OirExpr>,
    rows: &[String],
    result: &mut ValidationResult,
) {
    // A `typed` value is read as the type the program declared for it.
    let Ok(oir) = orbital_core_eval::lower::lower_sexpr_keeping_typed(expr) else {
        return;
    };
    for found in check_oir_expr_under(&oir, condition, ctx) {
        let position = found.index + 1;
        let function = &found.function;
        let call = &rows.iter().fold(found.expression.clone(), |call, row| call.replace(row.as_str(), "@entity"));
        if let Mismatch::Undetermined { reason } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorArgUndetermined,
                    path,
                    format!(
                        "`({function} …)` argument {position}, `{argument}`, has no determined type ({reason}), so the type variable it binds, and the type of the result, are unknown: {call}",
                        argument = found.argument
                    ),
                )
                .with_suggestion(
                    "Give the value a real type: read it from a declared payload or entity shape, declare a named union alias for a list of differently-shaped items, or annotate the binding with the declared type (`(let ((rows : [Shape] [...])) ...)`). There is no untyped escape",
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
                    format!(
                        "`({operator} …)` argument {position}, `{operand}`, can be missing (an optional field with no default), and `{operator}` raises on a null operand: {call}"
                    ),
                )
                .with_suggestion(format!(
                    "Say what a missing value means: default it (`(math/default {operand} …)`), guard it, `(and (validate/required {operand}) ({operator} {operand} …))`, or put the call under an `if`/`when` on its presence, or give the field a default or make it required (`!`)"
                )),
            );
            continue;
        }
        if let Mismatch::MissingResult { operator, operand, source } = &found.mismatch {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorNullOperand,
                    path,
                    format!(
                        "`({operator} …)` argument {position}, `{operand}`, is missing when `{source}` fails on its value (it answers null then), and `{operator}` needs a value: {call}"
                    ),
                )
                .with_suggestion(format!(
                    "Say what a missing value means: default it, `(or {operand} …)` or `(str/default {operand} …)`, or bind it with `let` and branch on its presence, `(if (= x null) … …)`"
                )),
            );
            continue;
        }
        let (message, suggestion) = match &found.mismatch {
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
            Mismatch::NoField { path: field, record } => (
                format!("`({function} …)` argument {position} names `{field}`, which is not a field of the {record} it reads: {call}"),
                "Name a field the record declares, or read a record that has it".to_string(),
            ),
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
            user: orbital_core_base::ir::OirType::Struct { additional: None, fields: Vec::new() },
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
        validate_operator_arg_types(&parse(json), "p", &entity, &HashMap::new(), &no_types(), None)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch)
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
    fn a_mismatch_inside_an_object_value_is_found() {
        let got = mismatches(r#"{"content": ["str/upper", 5]}"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
    }

    fn spawn_with(config: &str) -> Vec<String> {
        mismatches(
            &format!(r#"["os/spawn", "git", ["status"], {config}, {{"emit": {{"success": "S", "failure": "F"}}}}]"#),
            HashMap::new(),
        )
    }

    const SPAWN_OK: &str = r#"{"cwd": null, "env": null, "stdin": "null", "stdout": "pipe", "stderr": "pipe", "detached": false}"#;

    #[test]
    fn a_closed_shape_argument_is_checked_field_by_field() {
        assert!(spawn_with(SPAWN_OK).is_empty(), "control: every declared field, each of its type");
        let wrong = spawn_with(&SPAWN_OK.replace(r#""detached": false"#, r#""detached": "yes""#));
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert!(wrong[0].contains("`detached`") && wrong[0].contains("boolean"), "{wrong:?}");
        let stdin = spawn_with(&SPAWN_OK.replace(r#""stdin": "null""#, r#""stdin": 5"#));
        assert_eq!(stdin.len(), 1, "{stdin:?}");
    }

    #[test]
    fn a_closed_shape_argument_names_a_missing_field_and_an_unknown_one() {
        let missing = spawn_with(&SPAWN_OK.replace(r#", "detached": false"#, ""));
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("`detached`") && missing[0].contains("missing"), "{missing:?}");
        let unknown = spawn_with(&SPAWN_OK.replace(r#""detached": false"#, r#""detached": false, "shell": true"#));
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert!(unknown[0].contains("`shell`"), "{unknown:?}");
    }

    #[test]
    fn unknowable_arguments_never_reject() {
        assert!(mismatches(r#"["str/upper", "@entity.unknown"]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", "@payload.x"]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", null]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["+", "@entity.when", 1]"#, field("when", "date")).is_empty());
    }

    fn undetermined(json: &str, entity: HashMap<String, EntityFieldMeta>) -> Vec<String> {
        validate_operator_arg_types(&parse(json), "p", &entity, &HashMap::new(), &no_types(), None)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
            .map(|e| e.message.clone())
            .collect()
    }

    #[test]
    fn an_undetermined_value_handed_to_a_type_variable_is_an_error_naming_it() {
        let got = undetermined(r#"["=", "@entity.nope", 3]"#, HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`(== …)`"), "names the operator: {got:?}");
        assert!(got[0].contains("@entity.nope"), "names the expression: {got:?}");
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
        assert!(undetermined(r#"["list", 1, 2]"#, HashMap::new()).is_empty(), "control: every item typed");
        assert!(undetermined(r#"["list", 1, null]"#, HashMap::new()).is_empty(), "control: null is absent, not undetermined");
    }

    #[test]
    fn controls_a_determined_value_and_null_are_not_undetermined() {
        assert!(undetermined(r#"["=", "@entity.count", 3]"#, field("count", "number")).is_empty());
        assert!(undetermined(r#"["=", "@entity.count", null]"#, field("count", "number")).is_empty());
        assert!(undetermined(r#"["str/upper", "@entity.nope"]"#, HashMap::new()).is_empty(), "a concrete position binds no variable");
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
            assert!(undetermined(expr, e()).is_empty(), "{expr}: {:?}", undetermined(expr, e()));
        }
        assert_eq!(
            mismatches(r#"["if", ["==", "@entity.count", 3], ["set", "@entity.count", 1], 5]"#, e()).len(),
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
            r#"[">", "@now", 5]"#,
            r#"["==", "@state", "idle"]"#,
            r#"["==", "@fromState", "idle"]"#,
            r#"["==", "@locale", "en"]"#,
            r#"["list", "@trait.Header", "@trait.Footer"]"#,
            r#"["array/includes", "@prevStates", "idle"]"#,
            r#"["==", "@event.event", "GO"]"#,
        ] {
            assert!(undetermined(expr, HashMap::new()).is_empty(), "{expr}: {:?}", undetermined(expr, HashMap::new()));
            assert!(mismatches(expr, HashMap::new()).is_empty(), "{expr}: {:?}", mismatches(expr, HashMap::new()));
        }
        assert_eq!(mismatches(r#"[">", "@now", "x"]"#, HashMap::new()).len(), 1, "control: @now is a number");
        assert_eq!(mismatches(r#"["==", "@state", 3]"#, HashMap::new()).len(), 1, "control: @state is a string");
        assert_eq!(mismatches(r#"["==", "@event.payload", 3]"#, HashMap::new()).len(), 1, "a delivery's payload is the triggering event's record");
    }

    fn with_payload(json: &str, shapes: &[(&str, orbital_core_base::ir::OirType)]) -> Vec<String> {
        let types = BindingTypes {
            payload: shapes.iter().map(|(n, t)| ((*n).to_string(), t.clone())).collect(),
            ..no_types()
        };
        validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &types, None)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorArgUndetermined)
            .map(|e| e.message.clone())
            .collect()
    }

    #[test]
    fn a_declared_payload_field_has_its_full_type() {
        use orbital_core_base::ir::{OirField, OirType};
        let row = OirType::Struct { additional: None, fields: vec![OirField::new("name".to_string(), OirType::String)] };
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
            user: OirType::Struct { additional: None, fields: vec![OirField::new("role".to_string(), OirType::String)] },
            config: [("limit".to_string(), OirType::Number)].into_iter().collect(),
            prev_payload: None,
            optional_payload: Default::default(),
            locals: Default::default(),
            type_params: Default::default(),
        };
        let run = |json: &str| {
            let r = validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &types, None);
            (
                r.errors.iter().filter(|e| e.code == ErrorCode::OperatorArgUndetermined).count(),
                r.errors.iter().filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch).count(),
            )
        };
        assert_eq!(run(r#"["==", "@user.role", "admin"]"#), (0, 0));
        assert_eq!(run(r#"[">", "@config.limit", 3]"#), (0, 0));
        assert_eq!(run(r#"["==", "@user.role", 3]"#), (0, 1), "control: role is a string");
        assert_eq!(run(r#"["==", "@user.nope", "x"]"#), (1, 0), "control: a field the roster does not declare");
        assert_eq!(run(r#"["==", "@config.other", 3]"#), (1, 0), "control: an undeclared knob");
    }

    #[test]
    fn a_substituted_config_list_carrying_its_declared_type_is_typed() {
        let mixed = serde_json::json!(["list", { "anchor": "exe-dir", "path": "tool" }, "tool"]);
        let ty = crate::phases::resolve::config_field_carried_type(&serde_json::from_value(serde_json::json!({
            "type": "[CliCommand]",
            "items": { "type": "union", "values": ["string", "AnchoredPath"], "properties": {
                "string": { "name": "string", "type": "string" },
                "AnchoredPath": { "name": "AnchoredPath", "type": "object", "properties": {
                    "anchor": { "name": "anchor", "type": "string", "required": true },
                    "path": { "name": "path", "type": "string", "required": true } } }
            } }
        })).expect("knob"));
        let typed = serde_json::json!(["array/len", ["typed", mixed, serde_json::to_string(&ty).unwrap()]]).to_string();
        assert!(undetermined(&typed, HashMap::new()).is_empty(), "{:?}", undetermined(&typed, HashMap::new()));
        let inline = serde_json::json!(["array/len", mixed]).to_string();
        assert_eq!(undetermined(&inline, HashMap::new()).len(), 1, "control: a mixed literal written inline is refused");
    }

    #[test]
    fn a_sequential_let_over_an_entity_list_is_typed() {
        use orbital_core_base::ir::{OirField, OirType};
        let band = OirType::Struct {
            additional: None,
            fields: vec![
                OirField::new("upperBound".to_string(), OirType::Number),
                OirField::new("label".to_string(), OirType::String),
            ],
        };
        let entity = || shaped(&[("bands", OirType::array(band.clone())), ("dose", OirType::Number)]);
        let expr = r#"["let", [
            ["matched", ["array/filter", "@entity.bands", ["fn", "b", ["<=", "@entity.dose", ["object/get", "@b", "upperBound"]]]]],
            ["band", ["if", [">", ["array/len", "@matched"], 0], ["array/first", "@matched"], ["array/nth", "@entity.bands", ["-", ["array/len", "@entity.bands"], 1]]]]
          ], ["object/get", "@band", "label"]]"#;
        assert!(undetermined(expr, entity()).is_empty(), "{:?}", undetermined(expr, entity()));
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
            assert!(undetermined(expr, tags()).is_empty(), "{expr}: {:?}", undetermined(expr, tags()));
            assert!(mismatches(expr, tags()).is_empty(), "{expr}: {:?}", mismatches(expr, tags()));
        }
        assert_eq!(
            mismatches(r#"["array/map", "@entity.tags", ["str/upper", "@index"]]"#, tags()).len(),
            1,
            "control: the index is a number, not a string"
        );
        assert_eq!(
            undetermined(r#"["array/findIndex", "@entity.tags", ["fn", "t", ["==", "@index", 0]]]"#, tags()).len(),
            1,
            "control: findIndex hands its callback no index"
        );
    }

    #[test]
    fn a_branch_a_literal_condition_rules_out_is_not_typed() {
        let dead = r#"["if", ["==", "create", "create"], "x", ["str/upper", 5]]"#;
        assert!(mismatches(dead, HashMap::new()).is_empty(), "{:?}", mismatches(dead, HashMap::new()));
        let dead_then = r#"["if", ["==", "a", "b"], ["str/upper", 5], "x"]"#;
        assert!(mismatches(dead_then, HashMap::new()).is_empty());
        let live = r#"["if", ["==", "@entity.name", "x"], "x", ["str/upper", 5]]"#;
        assert_eq!(mismatches(live, field("name", "string")).len(), 1, "control: an undecided condition types both branches");
        let live_type = r#"["str/upper", ["if", ["==", "a", "a"], 5, "x"]]"#;
        assert_eq!(mismatches(live_type, HashMap::new()).len(), 1, "control: the live branch is a number");
    }

    // std-lifecycle under `fromStatus: ""`: `(or (= "" "") (= (object/get @entity statusField) …))` never reads the field.
    #[test]
    fn an_operand_a_decided_and_or_never_evaluates_is_not_typed() {
        assert!(mismatches(r#"["or", ["=", "", ""], ["=", ["str/upper", 5], "x"]]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["and", ["=", "a", "b"], ["=", ["str/upper", 5], "x"]]"#, HashMap::new()).is_empty());
    }

    #[test]
    fn control_an_operand_before_or_beside_an_undecided_one_is_typed() {
        assert_eq!(mismatches(r#"["or", ["=", ["str/upper", 5], "x"], ["=", "", ""]]"#, HashMap::new()).len(), 1);
        assert_eq!(mismatches(r#"["and", ["=", "a", "a"], ["=", ["str/upper", 5], "x"]]"#, HashMap::new()).len(), 1);
        assert_eq!(mismatches(r#"["or", ["=", "@entity.name", "x"], ["=", ["str/upper", 5], "x"]]"#, field("name", "string")).len(), 1);
    }

    #[test]
    fn a_reference_to_a_row_is_read_through_its_entity() {
        use orbital_core_base::ir::OirType;
        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
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
                ("row".to_string(), OirType::Reference { entity: "Note".into() }),
                ("rows".to_string(), OirType::array(OirType::Reference { entity: "Note".into() })),
                ("ghost".to_string(), OirType::Reference { entity: "Nope".into() }),
            ]
            .into_iter()
            .collect(),
            ..no_types()
        };
        let run = |json: &str| {
            let r = validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &types, Some(&schema));
            (
                r.errors.iter().filter(|e| e.code == ErrorCode::OperatorArgUndetermined).count(),
                r.errors.iter().filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch).count(),
            )
        };
        assert_eq!(run(r#"["str/upper", "@payload.row.title"]"#), (0, 0));
        assert_eq!(run(r#"["str/upper", ["object/get", "@payload.row", "title"]]"#), (0, 0));
        assert_eq!(run(r#"["array/len", "@payload.row.tags"]"#), (0, 0));
        assert_eq!(run(r#"["array/map", "@payload.rows", ["fn", "r", ["str/upper", ["object/get", "@r", "title"]]]]"#), (0, 0));
        assert_eq!(run(r#"["+", "@payload.row.title", 1]"#), (0, 1), "control: title is a string");
        assert_eq!(run(r#"["==", "@payload.ghost.title", "x"]"#), (1, 0), "control: a row of no declared entity");
    }

    // G-ORB-534: std-lifecycle's delete scan `(fetch @config.targetEntity { filter: (= (object/get @entity @config.statusField) …) })`
    // reads the TARGET row (std-otel: `OtelSpan.state`), not the run-log entity the scheduler trait is bound to.
    fn fetch_filter_mismatches(filter: &str) -> usize {
        fetch_filter_findings(filter).len()
    }

    fn fetch_filter_findings(filter: &str) -> Vec<String> {
        effect_findings(&format!(r#"["if", ["=", "@entity.headId", ""], ["fetch", "Span", {{ "filter": {filter} }}], null]"#))
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
            .filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch)
            .map(|e| e.message.clone())
            .collect()
    }

    #[test]
    fn a_fetch_filter_reads_the_row_of_the_fetched_entity() {
        assert_eq!(
            fetch_filter_mismatches(r#"["and", ["=", ["object/get", "@entity", "state"], "exported"], ["<", ["object/get", "@entity", "endTime"], 5]]"#),
            0
        );
    }

    #[test]
    fn control_a_fetch_filter_still_names_missing_target_fields_and_reads_the_trait_through_entity_fields() {
        assert_eq!(fetch_filter_mismatches(r#"["=", ["object/get", "@entity", "headId"], "x"]"#), 1, "headId is the trait's, not Span's");
        assert_eq!(fetch_filter_mismatches(r#"["=", ["object/get", "@entity", "state"], ["+", "@entity.headId", 1]]"#), 1);
        assert_eq!(fetch_filter_mismatches(r#"["=", ["object/get", "@entity", "state"], "@entity.headId"]"#), 0);
        assert_eq!(
            effect_findings(r#"["if", ["=", "delete", "delete"], ["fetch", "Span", { "filter": ["=", ["object/get", "@entity", "state"], "x"] }], ["fetch", "Span", { "filter": ["=", ["object/get", "@entity", "status"], "x"] }]]"#).len(),
            0,
            "a branch the call site's literal config rules out is not typed"
        );
        let found = fetch_filter_findings(r#"["=", ["object/get", "@entity", "headId"], "x"]"#);
        assert!(found[0].contains(r#"object/get(@entity, "headId")"#), "the finding names the read as written: {found:?}");
    }

    #[test]
    fn a_set_value_reads_a_row_field_under_the_same_check_as_a_guard() {
        use orbital_core_base::ir::OirType;
        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
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
                ("byKey".to_string(), OirType::map(OirType::String, OirType::row("Note"))),
            ]
            .into_iter()
            .collect(),
            ..no_types()
        };
        let mismatches = |json: &str| {
            validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &types, Some(&schema))
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch)
                .count()
        };
        assert_eq!(mismatches(r#"["==", ["object/get", "@payload.row", "nope"], "x"]"#), 1, "the guard form");
        assert_eq!(mismatches(r#"["set", "@entity.label", ["object/get", "@payload.row", "nope"]]"#), 1);
        assert_eq!(
            mismatches(r#"["set", "@entity.label", ["object/get", ["object/get", "@payload.byKey", "k"], "nope"]]"#),
            1,
            "through a map of rows"
        );
        assert_eq!(mismatches(r#"["set", "@entity.label", ["object/get", "@payload.row", "title"]]"#), 0, "control");
        assert_eq!(
            mismatches(r#"["set", "@entity.label", ["object/get", ["object/get", "@payload.byKey", "k"], "title"]]"#),
            0,
            "control through the map"
        );
    }

    #[test]
    fn an_object_literal_is_exactly_the_record_it_writes() {
        assert!(undetermined(r#"["str/upper", ["object/get", {"a": "x"}, "a"]]"#, HashMap::new()).is_empty());
        assert!(mismatches(r#"["str/upper", ["object/get", {"a": "x"}, "a"]]"#, HashMap::new()).is_empty());
        assert_eq!(mismatches(r#"["+", ["object/get", {"a": "x"}, "a"], 1]"#, HashMap::new()).len(), 1);
        assert_eq!(mismatches(r#"["object/get", {"a": 1}, "b"]"#, HashMap::new()).len(), 1, "no such key");
        assert!(mismatches(r#"["object/get", {"a": 1}, "b", 0]"#, HashMap::new()).is_empty(), "with a default");
        assert_eq!(
            undetermined(r#"["object/get", {"a": "@entity.nope"}, "a"]"#, HashMap::new()).len(),
            1,
            "control: a field whose value is undetermined leaves the record undetermined"
        );
        let rows = r#"["array/some", ["list", {"from": "a", "to": "b"}, {"from": "b", "to": "c"}], ["fn", "t", ["==", ["object/get", "@t", "from"], "a"]]]"#;
        assert!(undetermined(rows, HashMap::new()).is_empty() && mismatches(rows, HashMap::new()).is_empty());
    }

    #[test]
    fn the_delivery_is_typed_by_the_event_that_triggers_the_transition() {
        use orbital_core_base::ir::OirType;
        let declared = [("id", OirType::String)];
        for expr in [
            r#"["==", "@event.payload.id", "x"]"#,
            r#"["==", ["object/get", "@event.payload", "id"], "x"]"#,
            r#"["str/default", "@event.source.trait", ""]"#,
            r#"["==", "@event.event", "GO"]"#,
            r#"["==", "@event.source.fromBridge", true]"#,
        ] {
            assert!(with_payload(expr, &declared).is_empty(), "{expr}: {:?}", with_payload(expr, &declared));
        }
        assert_eq!(with_payload(r#"["==", "@event.payload.id", "x"]"#, &[]).len(), 1, "control: no declared payload field");
    }

    #[test]
    fn the_empty_list_is_typed_and_refined_by_a_fold() {
        let fold = r#"["array/reduce", ["list", "a", "b"], [], ["fn", ["acc", "e"], ["if", ["array/includes", "@acc", "@e"], "@acc", ["array/append", "@acc", "@e"]]]]"#;
        assert!(undetermined(fold, HashMap::new()).is_empty(), "{:?}", undetermined(fold, HashMap::new()));
        assert!(mismatches(fold, HashMap::new()).is_empty(), "{:?}", mismatches(fold, HashMap::new()));
        let used = format!(r#"["array/len", {fold}]"#);
        assert!(undetermined(&used, HashMap::new()).is_empty());
        assert!(undetermined(r#"["or", "@entity.nope", []]"#, HashMap::new()).len() == 1, "control: the undetermined side still is");
        let record = r#"["list", {"samples": [], "label": "a"}, {"samples": [{"x": 1}], "label": "b"}]"#;
        assert!(undetermined(record, HashMap::new()).is_empty(), "{:?}", undetermined(record, HashMap::new()));
    }

    #[test]
    fn a_fold_from_the_empty_list_reads_its_accumulator_as_the_list_its_body_builds() {
        // The accumulator starts as `[]` and the body filters it before it appends to it: what types the
        // accumulator is the list the body builds, so the filter's lambda sees the elements.
        let dedupe = r#"["array/reduce", ["list", 1, 2, 2], [], ["fn", ["acc", "g"], ["if", [">", ["array/len", ["array/filter", "@acc", ["fn", "x", ["==", "@x", "@g"]]]], 0], "@acc", ["array/append", "@acc", "@g"]]]]"#;
        assert!(undetermined(dedupe, HashMap::new()).is_empty(), "{:?}", undetermined(dedupe, HashMap::new()));
        assert!(mismatches(dedupe, HashMap::new()).is_empty(), "{:?}", mismatches(dedupe, HashMap::new()));
        // control: the elements really are numbers, so comparing one with a string is still found.
        let compared = r#"["array/reduce", ["list", 1, 2], [], ["fn", ["acc", "g"], ["if", [">", ["array/len", ["array/filter", "@acc", ["fn", "x", ["==", "@x", "a"]]]], 0], "@acc", ["array/append", "@acc", "@g"]]]]"#;
        assert_eq!(mismatches(compared, HashMap::new()).len(), 1, "control: a number is not the string `a`");
        // control: a body that builds no typed list leaves the accumulator untyped.
        let nothing = r#"["array/reduce", ["list", 1, 2], [], ["fn", ["acc", "g"], "@acc"]]"#;
        assert!(!undetermined(nothing, HashMap::new()).is_empty(), "{:?}", undetermined(nothing, HashMap::new()));
    }

    #[test]
    fn a_callback_that_is_handed_an_index_binds_item_and_index_for_the_body() {
        // Both evaluators bind `@item` (the element) and `@index` (its position) in such a callback, whatever the
        // lambda names its own parameters.
        let by_index = r#"["array/filter", ["list", 1, 2, 3], ["fn", "b", ["<", "@index", 2]]]"#;
        assert!(undetermined(by_index, HashMap::new()).is_empty(), "{:?}", undetermined(by_index, HashMap::new()));
        assert!(mismatches(by_index, HashMap::new()).is_empty());
        let by_item = r#"["array/map", ["list", 1, 2], ["fn", "b", ["+", "@item", "@index"]]]"#;
        assert!(undetermined(by_item, HashMap::new()).is_empty());
        assert!(mismatches(by_item, HashMap::new()).is_empty());
        // control: `@index` is a number, so a string comparison with it is found.
        let wrong = r#"["array/filter", ["list", 1, 2, 3], ["fn", "b", ["==", "@index", "a"]]]"#;
        assert_eq!(mismatches(wrong, HashMap::new()).len(), 1, "{:?}", mismatches(wrong, HashMap::new()));
        // control: outside a lambda `@index` is nothing.
        assert!(!undetermined(r#"["<", "@index", 2]"#, HashMap::new()).is_empty());
        // control: `@item` is the element, here a string.
        assert_eq!(
            mismatches(r#"["array/map", ["list", "a"], ["fn", "b", ["+", "@item", 1]]]"#, HashMap::new()).len(),
            1
        );
    }

    #[test]
    fn an_earlier_delivery_has_the_payload_every_listened_event_declares() {
        use orbital_core_base::ir::{OirField, OirType};
        let with = |prev: Option<OirType>| BindingTypes { prev_payload: prev, ..no_types() };
        let expr = r#"["array/some", "@prevEvents", ["fn", "e", ["==", ["object/get", ["object/get", "@e", "payload"], "id"], "x"]]]"#;
        let run = |types: &BindingTypes| {
            let r = validate_operator_arg_types(&parse(expr), "p", &HashMap::new(), &HashMap::new(), types, None);
            r.errors.len()
        };
        let declared = OirType::Struct { additional: None, fields: vec![OirField::new("id".to_string(), OirType::String)] };
        assert_eq!(run(&with(Some(declared))), 0);
        assert!(run(&with(None)) > 0, "control: no agreed payload, so reading a field of it is an error");
    }

    #[test]
    fn a_dead_branch_still_types_the_empty_list_the_live_one_is() {
        let folded = r#"["array/map", ["if", ["==", "off", "on"], "@entity.tags", []], ["fn", "t", ["str/upper", "@t"]]]"#;
        assert!(undetermined(folded, tags()).is_empty(), "{:?}", undetermined(folded, tags()));
        assert!(mismatches(folded, tags()).is_empty());
        let wrong = r#"["array/map", ["if", ["==", "off", "on"], "@entity.tags", []], ["fn", "t", ["+", "@t", 1]]]"#;
        assert_eq!(mismatches(wrong, tags()).len(), 1, "control: the elements are strings");
        let nothing = r#"["array/map", ["if", ["==", "off", "on"], "@entity.nope", []], ["fn", "t", ["object/get", "@t", "x"]]]"#;
        assert!(!undetermined(nothing, tags()).is_empty(), "control: a dead branch of no type types nothing");
    }

    #[test]
    fn a_let_binding_sees_the_bindings_before_it() {
        let both = r#"["let", [["xs", ["list", 1, 2]], ["x", ["array/first", "@xs"]]], ["+", "@x", 1]]"#;
        assert!(undetermined(both, HashMap::new()).is_empty(), "{:?}", undetermined(both, HashMap::new()));
        assert!(mismatches(both, HashMap::new()).is_empty());
        let wrong = r#"["let", [["xs", ["list", 1, 2]], ["x", ["array/first", "@xs"]]], ["str/upper", "@x"]]"#;
        assert_eq!(mismatches(wrong, HashMap::new()).len(), 1, "control: @x is a number");
        let unbound = r#"["let", [["x", ["array/first", "@nope"]]], ["+", "@x", 1]]"#;
        assert!(!undetermined(unbound, HashMap::new()).is_empty(), "control: an undetermined list stays undetermined");
    }

    #[test]
    fn an_error_already_reported_is_not_reported_again_by_the_enclosing_call() {
        let got = undetermined(r#"["=", ["+", "a", 1], 3]"#, HashMap::new());
        assert!(got.is_empty(), "{got:?}");
        assert_eq!(mismatches(r#"["=", ["+", "a", 1], 3]"#, HashMap::new()).len(), 1);
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
    fn an_alias_resolves_to_its_operator() {
        assert_eq!(mismatches(r#"["upper", 5]"#, HashMap::new()).len(), 1);
    }

    fn shaped(fields: &[(&str, orbital_core_base::ir::OirType)]) -> HashMap<String, EntityFieldMeta> {
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
            fields: vec![OirField::new("name".to_string(), OirType::String)],
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
        assert!(got[0].contains("argument 2 is a number"), "{got:?}");
        assert!(got[0].contains("an earlier argument is a string"), "{got:?}");
        assert!(mismatches(r#"["array/append", "@entity.tags", "x"]"#, tags()).is_empty());
    }

    #[test]
    fn lists_of_op_tuples_of_unlike_widths_join_into_one_list() {
        let updates = r#"["array/map", "@entity.tags", ["fn", "t", ["upd", "E", "@t", { "a": 1 }]]]"#;
        let prepended = format!(r#"["array/prepend", {updates}, ["del", "E", "x"]]"#);
        assert!(mismatches(&prepended, tags()).is_empty(), "{:?}", mismatches(&prepended, tags()));
        let deletes = r#"["array/map", "@entity.tags", ["fn", "t", ["del", "E", "@t"]]]"#;
        let concatenated = format!(r#"["array/concat", {deletes}, {updates}]"#);
        assert!(mismatches(&concatenated, tags()).is_empty(), "{:?}", mismatches(&concatenated, tags()));
    }

    #[test]
    fn control_lists_of_unlike_scalars_still_do_not_join() {
        assert_eq!(mismatches(r#"["array/concat", [1], ["a"]]"#, tags()).len(), 1);
        assert_eq!(mismatches(r#"["array/prepend", "@entity.tags", 1]"#, tags()).len(), 1);
    }

    fn effects_in_lambdas(json: &str) -> usize {
        validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &no_types(), None)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::EffectInLambda)
            .count()
    }

    #[test]
    fn an_effect_inside_a_collection_lambda_is_refused() {
        assert_eq!(effects_in_lambdas(r#"["array/map", "@payload.items", ["fn", "x", ["emit", "E", { "id": "@x" }]]]"#), 1);
        assert_eq!(effects_in_lambdas(r#"["array/filter", "@payload.items", ["fn", "x", ["do", ["set", "@entity.n", 1], true]]]"#), 1);
        assert_eq!(effects_in_lambdas(r#"["array/map", "@payload.items", ["fn", "x", ["if", true, ["persist", "create", "E", {}], null]]]"#), 1);
    }

    #[test]
    fn control_a_pure_lambda_and_an_effect_outside_one_are_accepted() {
        assert_eq!(effects_in_lambdas(r#"["array/map", "@payload.items", ["fn", "x", ["str/upper", "@x"]]]"#), 0);
        assert_eq!(effects_in_lambdas(r#"["emit", "E", { "ids": ["array/map", "@payload.items", ["fn", "x", ["object/get", "@x", "id"]]] }]"#), 0);
        assert_eq!(effects_in_lambdas(r#"["array/map", "@payload.items", ["fn", "x", ["quote", ["emit", "E", {}]]]]"#), 0, "a quoted program is data");
    }

    #[test]
    fn two_operands_of_one_type_variable_must_share_a_type() {
        assert_eq!(mismatches(r#"["=", "@entity.count", "x"]"#, tags()).len(), 1);
        assert!(mismatches(r#"["=", "@entity.count", 3]"#, tags()).is_empty());
        assert_eq!(mismatches(r#"["if", ["==", "@entity.count", 3], 1, "a"]"#, tags()).len(), 1);
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
        let got = mismatches(r#"["array/map", "@entity.tags", ["fn", "t", ["+", "@t", 1]]]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(mismatches(r#"["array/map", "@entity.tags", ["fn", "t", ["str/upper", "@t"]]]"#, tags()).is_empty());
    }

    #[test]
    fn a_key_that_names_no_field_of_the_record_is_an_error() {
        let got = mismatches(r#"["object/get", "@entity.profile", "nope"]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`nope`"), "{got:?}");
        assert!(mismatches(r#"["object/get", "@entity.profile", "name"]"#, tags()).is_empty());
        assert_eq!(mismatches(r#"["+", ["object/get", "@entity.profile", "name"], 1]"#, tags()).len(), 1);
    }

    #[test]
    fn an_effect_nested_in_a_value_expression_is_still_checked() {
        let got = mismatches(r#"["if", true, ["set", "@entity.count", ["str/upper", 5]], null]"#, tags());
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn a_default_makes_an_undeclared_field_a_legitimate_optional_read() {
        assert!(mismatches(r#"["object/get", "@entity.profile", "roles", []]"#, tags()).is_empty());
        assert_eq!(mismatches(r#"["object/get", "@entity.profile", "roles"]"#, tags()).len(), 1);
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
        validate_operator_arg_types(&parse(json), "p", &HashMap::new(), &HashMap::new(), &no_types(), Some(&schema))
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorArgTypeMismatch)
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
        let got = with_types(r#"["str/upper", ["object/get", ["json/parse", ["str/concat", "{", "}"], "Point"], "x", 0]]"#);
        assert_eq!(got.len(), 1, "{got:?}");
        let missing = with_types(r#"["object/get", ["json/parse", "{}", "Point"], "z"]"#);
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(with_types(r#"["+", ["object/get", ["json/parse", "{}", "Point"], "x"], 1]"#).is_empty());
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
        assert!(with_types(r#"["str/upper", ["object/get", ["json/parse", "{}", "Row"], "any-key"]]"#).is_empty());
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

        fn findings(json: &str, entity: HashMap<String, EntityFieldMeta>, optional_payload: &[&str]) -> usize {
            let payload: HashMap<String, orbital_core_base::ir::OirType> =
                [("q".to_string(), orbital_core_base::ir::OirType::Number)].into_iter().collect();
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
        fn a_divisor_that_can_be_missing_makes_the_result_missing_where_a_value_is_needed() {
            for json in [
                r#"["+", ["/", 1, "@entity.p"], 1]"#,
                r#"["+", ["%", 7, "@entity.p"], 1]"#,
                r#"["+", ["math/mod", 7, "@entity.p"], 1]"#,
                r#"["+", ["math/randomInt", 1, "@entity.p"], 1]"#,
            ] {
                assert_eq!(null_findings(json, entity(false, false)), 1, "{json}");
            }
        }

        #[test]
        fn control_a_required_defaulted_guarded_or_tolerant_operand_is_fine() {
            assert_eq!(null_findings(r#"["/", 1, "@entity.p"]"#, entity(true, false)), 0, "required");
            assert_eq!(null_findings(r#"["/", 1, "@entity.p"]"#, entity(false, true)), 0, "defaulted");
            assert_eq!(null_findings(r#"["/", "@entity.p", 2]"#, entity(false, false)), 0, "the numerator reads null as 0");
            assert_eq!(
                null_findings(r#"["and", ["validate/required", "@entity.p"], ["/", 1, "@entity.p"]]"#, entity(false, false)),
                0,
                "a presence guard narrows the operand"
            );
            assert_eq!(
                null_findings(r#"["when", ["!=", "@entity.p", null], ["/", 1, "@entity.p"]]"#, entity(false, false)),
                0,
                "a null comparison narrows the operand"
            );
            assert_eq!(null_findings(r#"["+", 1, "@entity.p"]"#, entity(false, false)), 0, "an operator that reads null as a default");
        }

        #[test]
        fn an_optional_field_in_an_ordering_position_is_an_error() {
            for op in ["<", "<=", ">", ">="] {
                assert_eq!(findings(&format!(r#"["{op}", "@entity.p", 3]"#), entity(false, false), &[]), 1, "{op}");
                assert_eq!(findings(&format!(r#"["{op}", 3, "@entity.p"]"#), entity(false, false), &[]), 1, "{op} right");
            }
            assert_eq!(findings(r#"["<", ["object/get", "@entity", "p"], 3]"#, entity(false, false), &[]), 1, "object/get");
        }

        #[test]
        fn a_required_or_defaulted_field_is_fine() {
            assert_eq!(findings(r#"["<", "@entity.p", 3]"#, entity(true, false), &[]), 0, "required");
            assert_eq!(findings(r#"["<", "@entity.p", 3]"#, entity(false, true), &[]), 0, "defaulted");
            assert_eq!(findings(r#"["==", "@entity.p", 3]"#, entity(false, false), &[]), 0, "equality is not ordering");
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
            assert_eq!(findings(guarded[7], entity(false, false), &[]), 1, "{}", guarded[7]);
            assert_eq!(findings(guarded[8], entity(false, false), &[]), 0, "{}", guarded[8]);
            assert_eq!(findings(guarded[9], entity(false, false), &[]), 0, "{}", guarded[9]);
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
            assert_eq!(findings(r#"["<", "@payload.q", 3]"#, HashMap::new(), &["q"]), 1);
            assert_eq!(findings(r#"["<", "@payload.q", 3]"#, HashMap::new(), &[]), 0, "required payload field");
            assert_eq!(
                findings(r#"["and", ["validate/required", "@payload.q"], ["<", "@payload.q", 3]]"#, HashMap::new(), &["q"]),
                0
            );
        }

        #[test]
        fn a_local_bound_to_an_optional_field_stays_optional() {
            assert_eq!(findings(r#"["let", [["x", "@entity.p"]], ["<", "@x", 3]]"#, entity(false, false), &[]), 1);
            assert_eq!(findings(r#"["let", [["x", "@entity.p"]], ["<", "@x", 3]]"#, entity(true, false), &[]), 0);
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

    fn marked(json: &str, entity: HashMap<String, EntityFieldMeta>, guard: Option<&str>) -> SExpression {
        let guard = guard.map(parse);
        mark_proven_operator_args(&parse(json), guard.as_ref(), &entity, &HashMap::new(), &no_types(), None)
    }

    #[test]
    fn a_call_whose_argument_type_is_proven_is_marked_at_that_position() {
        assert_eq!(
            marked(r#"["str/upper", "@entity.name"]"#, field("name", "string"), None),
            parse(r#"["proven", [0], "str/upper", "@entity.name"]"#)
        );
    }

    #[test]
    fn an_undetermined_or_mismatched_argument_is_never_marked() {
        let undetermined = r#"["str/upper", "@payload.x"]"#;
        assert_eq!(marked(undetermined, HashMap::new(), None), parse(undetermined));
        let mismatched = r#"["str/upper", "@entity.count"]"#;
        assert_eq!(marked(mismatched, field("count", "number"), None), parse(mismatched));
    }

    #[test]
    fn only_the_proven_positions_of_a_call_are_listed() {
        assert_eq!(
            marked(r#"["str/concat", "@entity.name", "@payload.x"]"#, field("name", "string"), None),
            parse(r#"["proven", [0], "str/concat", "@entity.name", "@payload.x"]"#)
        );
    }

    #[test]
    fn nested_calls_are_marked_each_on_its_own() {
        assert_eq!(
            marked(r#"["str/upper", ["str/trim", "@entity.name"]]"#, field("name", "string"), None),
            parse(r#"["proven", [0], "str/upper", ["proven", [0], "str/trim", "@entity.name"]]"#)
        );
    }

    fn payload_typed(ty: orbital_core_base::ir::OirType) -> BindingTypes {
        let mut types = no_types();
        types.payload.insert("v".to_string(), ty);
        types
    }

    #[test]
    fn an_uninformative_argument_is_never_proven_though_the_static_check_admits_it() {
        use orbital_core_base::ir::OirType;
        let effect = parse(r#"["str/upper", "@payload.v"]"#);
        let json = mark_proven_operator_args(&effect, None, &HashMap::new(), &HashMap::new(), &payload_typed(OirType::Json), None);
        assert_eq!(json, effect, "a json value could be any kind at run time");
        let string = mark_proven_operator_args(&effect, None, &HashMap::new(), &HashMap::new(), &payload_typed(OirType::String), None);
        assert_eq!(string, parse(r#"["proven", [0], "str/upper", "@payload.v"]"#), "control: a declared string is proven");
    }

    #[test]
    fn a_date_at_a_string_position_is_never_proven() {
        let effect = r#"["str/upper", "@entity.due"]"#;
        assert_eq!(marked(effect, field("due", "date"), None), parse(effect));
    }

    #[test]
    fn a_lambda_body_is_marked_only_where_its_parameter_type_is_known() {
        let typed = marked(r#"["array/map", ["list", "a", "b"], ["fn", "t", ["str/upper", "@t"]]]"#, HashMap::new(), None);
        assert_eq!(
            typed,
            parse(r#"["proven", [0], "array/map", ["list", "a", "b"], ["fn", "t", ["proven", [0], "str/upper", "@t"]]]"#)
        );
        let untyped = r#"["array/map", "@payload.xs", ["fn", "t", ["str/upper", "@t"]]]"#;
        assert_eq!(marked(untyped, HashMap::new(), None), parse(untyped), "control: unknown elements");
    }

    #[test]
    fn a_variadic_call_lowered_to_nested_operations_is_not_marked() {
        let three = r#"["+", 1, 2, 3]"#;
        assert_eq!(marked(three, HashMap::new(), None), parse(three));
        assert_eq!(marked(r#"["+", 1, 2]"#, HashMap::new(), None), parse(r#"["proven", [0, 1], "+", 1, 2]"#));
    }

    #[test]
    fn a_value_inside_an_effect_is_marked_and_the_effect_is_not() {
        assert_eq!(
            marked(r#"["set", "@entity.name", ["str/upper", "@entity.name"]]"#, field("name", "string"), None),
            parse(r#"["set", "@entity.name", ["proven", [0], "str/upper", "@entity.name"]]"#)
        );
    }

    #[test]
    fn marking_adds_at_most_one_level_however_many_calls_it_marks() {
        fn depth(v: &serde_json::Value) -> usize {
            match v {
                serde_json::Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
                serde_json::Value::Object(fields) => 1 + fields.values().map(depth).max().unwrap_or(0),
                _ => 0,
            }
        }
        let mut nested = serde_json::json!("@entity.name");
        for _ in 0..120 {
            nested = serde_json::json!(["str/trim", nested]);
        }
        let source: SExpression = serde_json::from_value(nested.clone()).unwrap();
        let marked = mark_proven_operator_args(&source, None, &field("name", "string"), &HashMap::new(), &no_types(), None);
        let text = serde_json::to_string(&marked).unwrap();
        assert!(text.contains("\"proven\""), "the chain is marked");
        let reread: serde_json::Value = serde_json::from_str(&text).expect("a marked program re-parses under the default depth limit");
        assert!(
            depth(&reread) <= depth(&nested) + 1,
            "marks never stack: at most the innermost positions list adds one level ({} vs {})",
            depth(&reread),
            depth(&nested)
        );
    }

    #[test]
    fn a_control_form_holding_effects_is_never_marked_but_its_values_are() {
        let effect = r#"["if", ["=", "@entity.name", "a"], ["emit", "DONE", {}], ["set", "@entity.name", ["str/upper", "@entity.name"]]]"#;
        assert_eq!(
            marked(effect, field("name", "string"), None),
            parse(r#"["if", ["proven", [0, 1], "=", "@entity.name", "a"], ["emit", "DONE", {}], ["set", "@entity.name", ["proven", [0], "str/upper", "@entity.name"]]]"#)
        );
    }

    #[test]
    fn a_quoted_body_is_left_as_written() {
        let quoted = r#"["quote", ["str/upper", "@entity.name"]]"#;
        assert_eq!(marked(quoted, field("name", "string"), None), parse(quoted));
    }
}
