//! emit effect validation + emit config block validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{is_valid_event_identifier, DeclaredEmit};
use orbital_core_base::schema::SExpression;

/// Validate emit effect
pub(crate) fn validate_emit(arr: &[SExpression], path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["emit", "EVENT_NAME", payload?]
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::CircuitOrphanEvent,
                path,
                "emit requires event name",
            )
            .with_suggestion("Use: [\"emit\", \"EVENT_NAME\"]"),
        );
        return result;
    }

    // Post-Phase 2.5: event names are nominal identifiers declared via
    // `type X = Event<T>`. Any valid identifier (MixedCase or UPPER_SNAKE)
    // is accepted; the type system checks resolvability.
    if let SExpression::String(event_name) = &arr[1] {
        if !is_valid_event_identifier(event_name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitEventInvalidName,
                    path,
                    format!("Event name '{}' is not a valid identifier", event_name),
                )
                .with_suggestion(
                    "Declare the event via `type EventName = Event<Payload>` and reference it by identifier",
                ),
            );
        }
    }

    result
}

/// Validate an `emit:` config block on an async/reactive data operator.
///
/// `allowed_keys` restricts which lifecycle hooks apply to the operator
/// (e.g. `fetch` only supports `success` + `failure`; `set` only `success`;
/// `os/watch-*` only `on_message` + `failure`). Unknown keys get hard errors.
///
/// Event names must be UPPER_SNAKE_CASE. Events not present in the trait's
/// declared `emits` array produce a warning (forward-compatibility hint) —
/// the language-level rule is still enforced in `validate_emit_against_declaration`
/// once the runtime actually fires the event.
pub(crate) fn validate_emit_config(
    emit_obj: &orbital_core_base::schema::SObject,
    allowed_keys: &[&str],
    declared_emits: Option<&[DeclaredEmit]>,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (key, value) in emit_obj {
        if !allowed_keys.contains(&key.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectEmitKeyInvalid,
                    path,
                    format!(
                        "emit.{} is not supported on this operator (allowed: {})",
                        key,
                        allowed_keys.join(", ")
                    ),
                )
                .with_valid_values(allowed_keys.iter().map(|s| s.to_string()).collect())
                .with_suggestion(
                    "Remove the invalid key or use a different operator that supports it",
                ),
            );
            continue;
        }

        // Post-Phase 2.5: emit event values are nominal identifiers (declared
        // via `type X = Event<T>`). Any valid identifier is accepted; the
        // type registry verifies resolvability. Mixed-case and UPPER_SNAKE
        // both pass. Empty strings and malformed identifiers are still hard
        // errors.
        match value {
            SExpression::String(event_name) => {
                if !is_valid_event_identifier(event_name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EffectEmitInvalidName,
                            path,
                            format!(
                                "emit.{} = '{}' is not a valid identifier",
                                key, event_name
                            ),
                        )
                        .with_suggestion(
                            "Declare an `Event<T>` type alias and reference its name (e.g. 'PatientLoaded' or 'PATIENT_LOADED')",
                        ),
                    );
                } else if let Some(declared) = declared_emits {
                    if !declared.iter().any(|d| d.event == *event_name) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectEmitUndeclaredEvent,
                                path,
                                format!(
                                    "emit.{} = '{}' is not declared in the trait's emits array",
                                    key, event_name
                                ),
                            )
                            .with_suggestion(
                                "Declare the event in the trait's `emits` block with its payload shape",
                            ),
                        );
                    }
                }
            }
            _ => {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectEmitInvalidName,
                        path,
                        format!("emit.{} must be a string event name", key),
                    )
                    .with_suggestion("e.g. emit.success = 'PATIENT_LOADED'"),
                );
            }
        }
    }

    result
}

/// Extract the `emit:` object from a trailing options SExpression::Object.
/// Returns None if the map has no `emit` key or if `emit` is not an object.
pub(crate) fn extract_emit_object_ref(
    options: &orbital_core_base::schema::SObject,
) -> Option<&orbital_core_base::schema::SObject> {
    match options.get("emit")? {
        SExpression::Object(obj) => Some(&**obj),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_with_valid_identifier_stays_clean() {
        let arr = vec![
            SExpression::String(("emit".to_string()).into()),
            SExpression::String(("TOPIC_SAVED".to_string()).into()),
        ];

        assert!(validate_emit(&arr, "test.path").errors.is_empty());
    }
}
