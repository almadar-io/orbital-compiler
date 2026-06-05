//! os/watch-* effect validation.

use super::super::ValidationResult;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::SExpression;

/// Validate any `os/watch-*` effect. Each is a streaming subscription —
/// `on_message` fires per event, `failure` fires if the subscription breaks.
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
