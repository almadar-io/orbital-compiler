//! Effect validation helpers: payload extraction + identifier checks.

use orbital_core::schema::SExpression;
use std::collections::HashSet;

/// Extract payload field names from a navigate effect
pub(crate) fn extract_payload_fields(arr: &[SExpression]) -> HashSet<String> {
    let mut fields = HashSet::new();

    // Payload is typically the third argument: ["navigate", "/path", { ... }]
    if arr.len() >= 3 {
        if let SExpression::Object(payload) = &arr[2] {
            for key in payload.keys() {
                fields.insert(key.clone());
            }
        }
    }

    fields
}

/// Check if a string is UPPER_SNAKE_CASE
pub(crate) fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

/// Accept a `.lolo`-shaped identifier: starts with a letter, contains only
/// letters / digits / underscores. Used by the emit validator post-Phase 2.5
/// since event names are now nominal identifiers in the type system (declared
/// as `type X = Event<T>`) rather than string literals tagged by convention.
/// Both UPPER_SNAKE_CASE and MixedCase shapes are valid identifiers.
pub(crate) fn is_valid_event_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
