//! `random-bytes`, `access-check`, `credential-*` validation
//! (`docs/Almadar_Embedded_Kernel.md` H.7 P4, P6): arity, the closed options
//! object and a literal integer in range. The effects are the operator table's `closedCall` entries; the options
//! keys, the request fields and the range are its parameters and `closedCall` data.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::visit_effects;
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core_base::language::registry::{try_get_operator, EmbeddedOperator};
use orbital_core_base::schema::{OrbitalSchema, SExpression};

fn each_host_call<'a>(expr: &'a SExpression, visit: &mut dyn FnMut(&'a str, &'a [SExpression], &'static EmbeddedOperator)) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if let Some(operator) = try_get_operator(head).filter(|op| op.closed_call.is_some()) {
                    visit(head, arr, operator);
                }
            }
            for item in arr {
                each_host_call(item, visit);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                each_host_call(v, visit);
            }
        }
        _ => {}
    }
}

/// Shape checks for every `random-bytes`, `credential-status` and `credential-forget` effect.
pub(crate) fn validate_host_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_host_call(expr, &mut |head, arr, operator| {
                let Some(closed) = operator.closed_call.as_ref() else { return };
                let first = operator.min_arity as usize + 1;
                let last = operator.max_arity.map_or(first, |max| max as usize) + 1;
                if !(first..=last).contains(&arr.len()) {
                    let (takes, write) = if first == 1 {
                        ("only an optional options object", format!("Write ({head} {{ emit: {{ success: <EVENT> }} }})"))
                    } else {
                        ("one argument and an optional options object", format!("Write ({head} <arg> {{ emit: {{ success: <EVENT> }} }})"))
                    };
                    result.add_error(
                        ValidationError::error(ErrorCode::HostEffectArity, at, format!("`{head}` takes {takes}, got {}", arr.len() - 1))
                            .with_suggestion(write),
                    );
                    return;
                }
                if let Some(range) = closed.int_range {
                    if let Some(count) = arr.get(range.arg + 1) {
                        check_count(head, range.arg, (range.min, range.max), count, at, &mut result);
                    }
                }
                if closed.request {
                    check_request(head, operator, &arr[1], at, &mut result);
                }
                let (allowed, emit_keys) = operator.option_keys().unwrap_or_default();
                match arr.get(first) {
                    None => {}
                    Some(SExpression::Object(options)) => {
                        let keyed = allowed.contains(&"key");
                        let allowed: &[&str] = &allowed;
                        if let Some(key) = options.keys().find(|k| !allowed.contains(&k.as_str())) {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::HostEffectOptionUnknown,
                                    at,
                                    format!("`{head}` options accept only {}, got `{key}`", allowed.join(" and ")),
                                )
                                .with_valid_values(allowed.iter().map(|k| k.to_string()).collect()),
                            );
                        }
                        if keyed {
                            result.merge(check_key_and_cancelled(options, at, head));
                        }
                        if let Some(emit) = extract_emit_object_ref(options) {
                            result.merge(validate_emit_config(emit, &emit_keys, Some(&declared), at));
                        }
                    }
                    Some(_) => result.add_error(
                        ValidationError::error(
                            ErrorCode::HostEffectOptionUnknown,
                            at,
                            format!("`{head}` options must be an object"),
                        )
                        .with_suggestion("Write { emit: { success: <EVENT>, failure: <EVENT> } }"),
                    ),
                }
            });
        });
    }
    result
}

/// A literal request of a credential effect: an object of the effect's declared parameters, with the required ones
/// present. A request that is a binding is the runtime's to check.
fn check_request(head: &str, operator: &EmbeddedOperator, request: &SExpression, at: &str, result: &mut ValidationResult) {
    let Some((declared, required)) = operator.request_fields() else { return };
    let invalid = |message: String, result: &mut ValidationResult| {
        result.add_error(
            ValidationError::error(ErrorCode::CredentialRequestInvalid, at, message)
                .with_valid_values(declared.iter().map(|k| k.to_string()).collect()),
        );
    };
    match request {
        SExpression::Object(fields) => {
            if let Some(key) = fields.keys().find(|k| !declared.contains(&k.as_str())) {
                invalid(format!("`{}` takes only {}, got `{key}`", head, declared.join(", ")), result);
            }
            for name in required.iter().filter(|name| !fields.contains_key(**name)) {
                invalid(format!("`{}` needs `{name}` in its request", head), result);
            }
        }
        SExpression::String(_) => {
            if !matches!(request, SExpression::String(text) if text.starts_with('@')) {
                invalid(format!("`{}` takes a request object, not text", head), result);
            }
        }
        SExpression::Number(_) | SExpression::Bool(_) | SExpression::Null | SExpression::Array(_) => {
            invalid(format!("`{}` takes a request object", head), result);
        }
    }
}

fn check_count(head: &str, arg: usize, (min, max): (i64, i64), count: &SExpression, at: &str, result: &mut ValidationResult) {
    let SExpression::Number(n) = *count else {
        return;
    };
    if n.fract() != 0.0 || !((min as f64)..=(max as f64)).contains(&n) {
        result.add_error(
            ValidationError::error(
                ErrorCode::RandomBytesCount,
                at,
                format!("`{head}` argument {} is an integer from {min} to {max}, got {n}", arg + 1),
            )
            .with_suggestion("Use a whole number of bytes such as 32"),
        );
    }
}
