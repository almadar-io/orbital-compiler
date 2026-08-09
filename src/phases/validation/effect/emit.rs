//! emit effect validation + emit config block validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{is_valid_event_identifier, DeclaredEmit};
use orbital_core::schema::SExpression;

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
    match &arr[1] {
        SExpression::String(event_name) => {
            if !is_valid_event_identifier(event_name) {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::TraitEventInvalidName,
                        path,
                        format!(
                            "Event name '{}' is not a valid identifier",
                            event_name
                        ),
                    )
                    .with_suggestion(
                        "Declare the event via `type EventName = Event<Payload>` and reference it by identifier",
                    ),
                );
            }
        }
        // A non-string first argument previously fell through validation and
        // died in the resolve phase as a bare "Expected string" with no
        // location (T-RESOLVE-OPAQUE). The object shape is the signature of
        // a forgotten event name: `(emit { data: ?data })`.
        SExpression::Object(_) => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectEmitInvalidName,
                    path,
                    "emit's first argument must be the event name, but an object was passed — this looks like the payload with the event name missing",
                )
                .with_suggestion(
                    "Write `(emit EVENT_NAME { … })` — the event name comes first, then the payload object. The event must be declared in the trait's `emits` block.",
                ),
            );
        }
        other => {
            let shape = match other {
                SExpression::Array(_) => "a list",
                SExpression::Number(_) => "a number",
                SExpression::Bool(_) => "a boolean",
                SExpression::Null => "null",
                SExpression::Object(_) | SExpression::String(_) => unreachable!(),
            };
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectEmitInvalidName,
                    path,
                    format!("emit's first argument must be the event name (a string identifier), got {shape}"),
                )
                .with_suggestion(
                    "Write `(emit EVENT_NAME)` or `(emit EVENT_NAME { … })` with an event declared in the trait's `emits` block",
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
    emit_obj: &std::collections::HashMap<String, SExpression>,
    allowed_keys: &[&str],
    declared_emits: Option<&[DeclaredEmit]>,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Normalize both camelCase and snake_case forms that the resolver
    // accepts so the validator sees the same keys users typed.
    let key_aliases: &[(&str, &str)] = &[("onChange", "on_change"), ("onMessage", "on_message")];

    for (key, value) in emit_obj {
        // Canonical name (as it appears in allowed_keys).
        let canonical = key_aliases
            .iter()
            .find(|(alt, _)| alt == key)
            .map(|(_, canon)| *canon)
            .unwrap_or(key.as_str());

        if !allowed_keys.contains(&canonical) {
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
                            ValidationError::warning(
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
    options: &std::collections::HashMap<String, SExpression>,
) -> Option<&std::collections::HashMap<String, SExpression>> {
    match options.get("emit")? {
        SExpression::Object(obj) => Some(obj),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_with_object_first_arg_is_a_hard_error_naming_the_missing_event() {
        // `(emit { data: ?data })` — the run-19 TopicManager shape that
        // previously fell through validation and died in the resolve phase as
        // an uncoded "Expected string" (T-RESOLVE-OPAQUE).
        let arr = vec![
            SExpression::String("emit".to_string()),
            SExpression::Object(std::collections::HashMap::from([(
                "data".to_string(),
                SExpression::String("?data".to_string()),
            )])),
        ];

        let result = validate_emit(&arr, "test.path");
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::EffectEmitInvalidName)
            .expect("hard error expected for object-as-event-name");
        assert!(err.message.contains("event name missing"), "got: {}", err.message);
        assert!(err
            .suggestion
            .as_deref()
            .unwrap_or_default()
            .contains("(emit EVENT_NAME { … })"));
    }

    #[test]
    fn emit_with_number_first_arg_is_a_hard_error() {
        let arr = vec![
            SExpression::String("emit".to_string()),
            SExpression::Number(42.0),
        ];

        let result = validate_emit(&arr, "test.path");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectEmitInvalidName && e.message.contains("a number")));
    }

    #[test]
    fn emit_with_valid_identifier_stays_clean() {
        let arr = vec![
            SExpression::String("emit".to_string()),
            SExpression::String("TOPIC_SAVED".to_string()),
        ];

        assert!(validate_emit(&arr, "test.path").errors.is_empty());
    }
}
