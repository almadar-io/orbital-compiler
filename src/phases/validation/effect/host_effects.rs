//! `random-bytes`, `credential-status` and `credential-forget` validation
//! (`docs/Almadar_Embedded_Kernel.md` H.7 P4, P6): arity, the closed options
//! object (`emit { success, failure }` only), and a literal byte count in range.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::visit_effects;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core::schema::{OrbitalSchema, SExpression};

const HOST_EFFECTS: [&str; 3] = ["random-bytes", "credential-status", "credential-forget"];
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
                match arr.get(2) {
                    None => {}
                    Some(SExpression::Object(options)) => {
                        if let Some(key) = options.keys().find(|k| k.as_str() != "emit") {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::HostEffectOptionUnknown,
                                    at,
                                    format!("`{head}` options accept only `emit`, got `{key}`"),
                                )
                                .with_valid_values(vec!["emit".to_string()]),
                            );
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
