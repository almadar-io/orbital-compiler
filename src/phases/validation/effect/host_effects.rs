//! `random-bytes`, `credential-*` validation
//! (`docs/Almadar_Embedded_Kernel.md` H.7 P4, P6): arity, the closed options
//! object (`emit { success, failure }` only), and a literal byte count in range.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::visit_effects;
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core_base::credential_admin::CredentialAdminOp;
use orbital_core_base::schema::{OrbitalSchema, SExpression};

const HOST_EFFECTS: [&str; 9] = [
    "random-bytes",
    "credential-status",
    "credential-forget",
    "credential-list",
    "credential-set",
    "credential-remove",
    "credential-test",
    "credential-mode",
    "credential-rotate",
];
const HOST_EMIT_KEYS: [&str; 2] = ["success", "failure"];
const RANDOM_BYTES_MAX: f64 = 1024.0;

fn each_host_call<'a>(expr: &'a SExpression, visit: &mut dyn FnMut(&'a str, &'a [SExpression])) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if HOST_EFFECTS.contains(&head.as_str()) {
                    visit(head, arr);
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
            each_host_call(expr, &mut |head, arr| {
                if !(2..=3).contains(&arr.len()) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::HostEffectArity,
                            at,
                            format!("`{head}` takes one argument and an optional options object, got {}", arr.len() - 1),
                        )
                        .with_suggestion(format!("Write ({head} <arg> {{ emit: {{ success: <EVENT> }} }})")),
                    );
                    return;
                }
                if head == "random-bytes" {
                    check_count(&arr[1], at, &mut result);
                }
                if let Some(op) = CredentialAdminOp::from_name(head) {
                    check_request(op, &arr[1], at, &mut result);
                }
                match arr.get(2) {
                    None => {}
                    Some(SExpression::Object(options)) => {
                        let keyed = head == "random-bytes";
                        let allowed: &[&str] = if keyed { &["key", "emit"] } else { &["emit"] };
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
                            result.merge(validate_emit_config(emit, &HOST_EMIT_KEYS, Some(&declared), at));
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
fn check_request(op: CredentialAdminOp, request: &SExpression, at: &str, result: &mut ValidationResult) {
    let (required, optional) = op.params();
    let declared: Vec<&str> = required.iter().chain(optional).copied().collect();
    let invalid = |message: String, result: &mut ValidationResult| {
        result.add_error(
            ValidationError::error(ErrorCode::CredentialRequestInvalid, at, message)
                .with_valid_values(declared.iter().map(|k| k.to_string()).collect()),
        );
    };
    match request {
        SExpression::Object(fields) => {
            if let Some(key) = fields.keys().find(|k| !declared.contains(&k.as_str())) {
                invalid(format!("`{}` takes only {}, got `{key}`", op.name(), declared.join(", ")), result);
            }
            for name in required.iter().filter(|name| !fields.contains_key(**name)) {
                invalid(format!("`{}` needs `{name}` in its request", op.name()), result);
            }
        }
        SExpression::String(_) => {
            if !matches!(request, SExpression::String(text) if text.starts_with('@')) {
                invalid(format!("`{}` takes a request object, not text", op.name()), result);
            }
        }
        SExpression::Number(_) | SExpression::Bool(_) | SExpression::Null | SExpression::Array(_) => {
            invalid(format!("`{}` takes a request object", op.name()), result);
        }
    }
}

fn check_count(count: &SExpression, at: &str, result: &mut ValidationResult) {
    let SExpression::Number(n) = *count else {
        return;
    };
    if n.fract() != 0.0 || !(1.0..=RANDOM_BYTES_MAX).contains(&n) {
        result.add_error(
            ValidationError::error(
                ErrorCode::RandomBytesCount,
                at,
                format!("`random-bytes` draws an integer from 1 to {RANDOM_BYTES_MAX} bytes, got {n}"),
            )
            .with_suggestion("Use a whole number of bytes such as 32"),
        );
    }
}
