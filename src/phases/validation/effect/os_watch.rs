//! `os/` effect `emit` validation: the legacy `os/watch-http` trigger and the `os_host_effects()` family.

use super::super::ValidationResult;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::language::registry::{get_operator, OperatorTypeRef, OperatorTypeRefKind};
use orbital_core_base::schema::SExpression;

/// Validate the legacy `os/watch-http` trigger: `on_message` fires per event, `failure` if the subscription breaks.
pub(crate) fn validate_os_watch(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Walk the tail looking for an options object carrying `emit:`. The
    // operator-specific arg positions vary but `emit:` is always inside a
    // trailing options object, detected by having an `emit` key.
    for arg in arr.iter().skip(1) {
        if let SExpression::Object(options) = arg {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["on_message", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// The `emit` keys an operator declares on its trailing `options` parameter.
fn declared_emit_keys(op: &str) -> Vec<&'static str> {
    let Some(options) =
        get_operator(op).and_then(|o| o.params.iter().find(|p| p.name == "options"))
    else {
        return Vec::new();
    };
    let OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) = &options.ty else {
        return Vec::new();
    };
    let Some(OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields: emit, .. })) =
        fields.get("emit")
    else {
        return Vec::new();
    };
    emit.keys().map(String::as_str).collect()
}

/// Validate the trailing `{ emit: { … } }` of an `os/` effect against the keys the operator declares.
pub(crate) fn validate_os_effect(
    op: &str,
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let keys = declared_emit_keys(op);
    if let Some(SExpression::Object(options)) = arr.last() {
        if let Some(emit_obj) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit_obj, &keys, declared_emits, path));
        }
    }
    result
}
