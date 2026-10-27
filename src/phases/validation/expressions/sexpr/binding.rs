//! Binding reference validation.

use super::validate::SExprContext;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use std::collections::HashSet;

/// Validate a binding reference
pub(crate) fn validate_binding(
    binding: &str,
    path: &str,
    context: SExprContext,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let root = binding[1..].split('.').next().unwrap_or_default();
    // A binding with no root is malformed: the std lint's finding (SEXPR_INVALID_BINDING), not an unknown root.
    if root.is_empty() {
        return result;
    }

    // Validate binding root
    match root {
        _ if crate::phases::validation::binding::CORE_BINDING_ROOTS.contains(&root) => {
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

    result
}
