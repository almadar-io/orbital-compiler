//! `oauth-authorize` / `oauth-connection-*` effects: the `key` and `emit` options. The closed client, op and
//! provider vocabulary and the request shape are reported by the std lint.

use super::super::{inline_traits, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::{each_call_of, visit_effects};
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::schema::{OrbitalSchema, SExpression};

const EMIT_KEYS: [&str; 2] = ["success", "failure"];

fn check_options(options: Option<&SExpression>, head: &str, declared: &[DeclaredEmit], at: &str, result: &mut ValidationResult) {
    if let Some(SExpression::Object(options)) = options {
        if head == "oauth-authorize" {
            result.merge(check_key_and_cancelled(options, at, head));
        }
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &EMIT_KEYS, Some(declared), at));
        }
    }
}

pub(crate) fn validate_oauth_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_call_of(expr, "oauth-authorize", &mut |arr| {
                if (3..=4).contains(&arr.len()) {
                    check_options(arr.get(3), "oauth-authorize", &declared, at, &mut result);
                }
            });
            for head in ["oauth-connection-status", "oauth-connection-forget"] {
                each_call_of(expr, head, &mut |arr| {
                    if (2..=3).contains(&arr.len()) {
                        check_options(arr.get(2), head, &declared, at, &mut result);
                    }
                });
            }
        });
    }
    result
}
