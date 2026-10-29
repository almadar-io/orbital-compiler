//! `closedCall` host effects (`random-bytes`, `credential-*`): the `key` and `emit` options. Arity, option keys,
//! the request fields and the integer range are reported by the std lint.

use super::super::{inline_traits, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::visit_effects;
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref};
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
                if operator.closed_call.is_none() {
                    return;
                }
                let first = operator.min_arity as usize + 1;
                let last = operator.max_arity.map_or(first, |max| max as usize) + 1;
                if !(first..=last).contains(&arr.len()) {
                    return;
                }
                let (allowed, emit_keys) = operator.option_keys().unwrap_or_default();
                if let Some(SExpression::Object(options)) = arr.get(first) {
                    if allowed.contains(&"key") {
                        result.merge(check_key_and_cancelled(options, at, head));
                    }
                    if let Some(emit) = extract_emit_object_ref(options) {
                    }
                }
            });
        });
    }
    result
}
