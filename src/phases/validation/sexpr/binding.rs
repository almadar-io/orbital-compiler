//! Binding reference validation.

use super::validate::SExprContext;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use std::collections::HashSet;

/// Validate a binding reference
pub(crate) fn validate_binding(
    binding: &str,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Parse binding: @root.field.nested or @root
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() || parts[0].is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SExprInvalidBinding,
                path,
                format!("Invalid binding: '{}'", binding),
            )
            .with_suggestion("Bindings should be @root.field format"),
        );
        return result;
    }

    let root = parts[0];

    // Validate binding root
    match root {
        "entity" | "payload" | "state" | "now" | "config" | "trait" | "user"
        | "callsitePayload" | "pages" | "currentTheme" => {
            // Valid core binding root. `callsitePayload` is emitted by the
            // inline-trait hoisting phase (phases/inline/trait.rs) when an
            // extracted render block captures `@payload` — same contract the
            // config-default validator (validation/binding.rs) already allows.
        }
        // Check if this is a lambda-local variable from an enclosing fn/lambda
        _ if lambda_locals.contains(root) => {
            // Valid lambda-bound variable
        }
        // Lambda/iterator variable bindings (used in map, filter, reduce, etc.)
        // Common patterns: @item, @index, @x, @y, @f, @acc, @elem, @val, @key, @value
        "item" | "index" | "x" | "y" | "f" | "acc" | "elem" | "val" | "key" | "value" | "i"
        | "j" | "k" | "n" | "a" | "b" | "c" | "o" | "result" => {
            // Valid lambda/iterator variable binding
        }
        _ if root
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false) =>
        {
            // Entity binding: @EntityName
            if !known_entities.is_empty() && !known_entities.contains(root) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SExprUnknownEntity,
                        path,
                        format!("Entity '{}' in binding may not exist", root),
                    )
                    .with_suggestion("Ensure the entity is defined in the schema"),
                );
            }
        }
        _ => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::BindingUnknownRoot,
                    path,
                    format!("Unknown binding root: '@{}'", root),
                )
                .with_suggestion("Use @entity, @payload, @state, @now, @config, @EntityName, or lambda variables (@item, @index, @x, etc.)"),
            );
        }
    }

    // Context-specific validation
    if context == SExprContext::TickEffect && root == "payload" {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingPayloadInTick,
                path,
                "@payload is not available in tick context",
            )
            .with_suggestion("Tick effects don't have event payloads - use @entity instead"),
        );
    }

    // @state and @now should not have paths
    if (root == "state" || root == "now") && parts.len() > 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingStateNoPath,
                path,
                format!("@{} should not have a path", root),
            )
            .with_suggestion(format!("Use @{} without path", root)),
        );
    }

    result
}
