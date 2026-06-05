//! Sigil context validator (Phase 3.4 of Lolo v3 plan).
//!
//! Walks every binding (`@entity.x`, `@payload.y`, `@state`, `@now`) used in
//! transitions, ticks, and guards. For each binding, checks the sigil root
//! against the allowed list defined by the embedded `BINDING_CONTEXT_RULES`
//! (mirrored from `packages/almadar-core/src/types/bindings.ts`).
//!
//! The legacy `binding.rs` validator already catches `@payload` in ticks
//! (the most common rule), but this validator is the canonical, table-driven
//! enforcement of all sigil context rules. It is wired into the validate
//! pipeline alongside `binding::validate_all_bindings`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::embedded;
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition};
use std::collections::HashSet;

/// Allowed sigil context names that map to entries in `BINDING_CONTEXT_RULES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigilContext {
    Guard,
    Effect,
    Tick,
}

impl SigilContext {
    fn name(self) -> &'static str {
        match self {
            SigilContext::Guard => "guard",
            SigilContext::Effect => "effect",
            SigilContext::Tick => "tick",
        }
    }
}

/// Run sigil context validation across the whole schema.
pub fn validate_all_sigil_contexts(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_sigil_contexts(trait_def, &path);
        ctx.merge(result);
    }
}

/// Validate sigil contexts for one trait definition.
pub fn validate_trait_sigil_contexts(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Transitions: guards (Guard context) + effects (Effect context).
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        if let Some(guard) = &transition.guard {
            walk_expr(
                guard,
                &format!("{}.guard", trans_path),
                SigilContext::Guard,
                &HashSet::new(),
                &mut result,
            );
        }

        if let Some(effects) = &transition.effects {
            for (k, effect) in effects.iter().enumerate() {
                walk_expr(
                    effect,
                    &format!("{}.effects[{}]", trans_path, k),
                    SigilContext::Effect,
                    &HashSet::new(),
                    &mut result,
                );
            }
        }
    }

    // Ticks: no @payload allowed.
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (k, effect) in tick.effects.iter().enumerate() {
                walk_expr(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, k),
                    SigilContext::Tick,
                    &HashSet::new(),
                    &mut result,
                );
            }
        }
    }

    result
}

/// Walk an expression and check every binding sigil against the embedded rules.
fn walk_expr(
    expr: &SExpression,
    path: &str,
    context: SigilContext,
    lambda_locals: &HashSet<String>,
    result: &mut ValidationResult,
) {
    match expr {
        SExpression::String(s) if s.starts_with('@') => {
            check_binding_sigil(s, path, context, lambda_locals, result);
        }
        SExpression::Array(arr) => {
            // Detect lambda expressions: ["lambda"|"fn", params, body]
            if arr.len() == 3 {
                if let SExpression::String(op) = &arr[0] {
                    if op == "lambda" || op == "fn" {
                        let mut extended = lambda_locals.clone();
                        match &arr[1] {
                            SExpression::Array(params) => {
                                for p in params {
                                    if let SExpression::String(name) = p {
                                        extended.insert(name.clone());
                                    }
                                }
                            }
                            SExpression::String(name) => {
                                extended.insert(name.clone());
                            }
                            _ => {}
                        }
                        walk_expr(&arr[2], &format!("{}[2]", path), context, &extended, result);
                        return;
                    }
                }
            }
            // Collection ops introduce implicit @item / @index in body args.
            if arr.len() >= 3 {
                if let SExpression::String(op) = &arr[0] {
                    if is_collection_callback_operator(op) {
                        // First arg is the source, validated as-is.
                        walk_expr(
                            &arr[1],
                            &format!("{}[1]", path),
                            context,
                            lambda_locals,
                            result,
                        );
                        let mut extended = lambda_locals.clone();
                        extended.insert("item".to_string());
                        extended.insert("index".to_string());
                        for (i, item) in arr[2..].iter().enumerate() {
                            walk_expr(
                                item,
                                &format!("{}[{}]", path, i + 2),
                                context,
                                &extended,
                                result,
                            );
                        }
                        return;
                    }
                }
            }
            for (i, item) in arr.iter().enumerate() {
                walk_expr(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    lambda_locals,
                    result,
                );
            }
        }
        SExpression::Object(obj) => {
            for (k, v) in obj {
                walk_expr(
                    v,
                    &format!("{}.{}", path, k),
                    context,
                    lambda_locals,
                    result,
                );
            }
        }
        _ => {}
    }
}

fn is_collection_callback_operator(op: &str) -> bool {
    if let Some(meta) = embedded::get_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    false
}

/// Validate a single sigil binding against the embedded `BINDING_CONTEXT_RULES`.
fn check_binding_sigil(
    binding: &str,
    path: &str,
    context: SigilContext,
    lambda_locals: &HashSet<String>,
    result: &mut ValidationResult,
) {
    if binding.len() < 2 {
        return;
    }
    let body = &binding[1..]; // strip leading @
    let root = body.split(['.', '[']).next().unwrap_or("");
    if root.is_empty() {
        return;
    }
    // Lambda-bound variables are always valid.
    if lambda_locals.contains(root) {
        return;
    }
    // Entity bindings (PascalCase like @GameConfig.x) are always allowed by sigil rules.
    if root
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
    {
        return;
    }
    // Anything else: check the embedded rules.
    let allowed = embedded::allowed_sigils_for_context(context.name())
        .expect("BINDING_CONTEXT_RULES must define this context");
    if !allowed.contains(&root) {
        let allowed_list = allowed.join(", ");
        result.add_error(
            ValidationError::error(
                error_code_for_context(context, root),
                path,
                format!(
                    "Binding @{} is not allowed in {} context. Allowed sigils: {}",
                    root,
                    context.name(),
                    allowed_list
                ),
            )
            .with_suggestion(format!(
                "Use one of: {}",
                allowed
                    .iter()
                    .map(|a| format!("@{}", a))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        );
    }
}

fn error_code_for_context(context: SigilContext, root: &str) -> ErrorCode {
    // Reuse the closest matching error codes already defined in `error.rs`.
    match (context, root) {
        (SigilContext::Tick, "payload") => ErrorCode::BindingPayloadInTick,
        _ => ErrorCode::BindingUnknownRoot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_in_tick_is_rejected() {
        let mut result = ValidationResult::new();
        check_binding_sigil(
            "@payload.amount",
            "test",
            SigilContext::Tick,
            &HashSet::new(),
            &mut result,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn payload_in_effect_is_allowed() {
        let mut result = ValidationResult::new();
        check_binding_sigil(
            "@payload.amount",
            "test",
            SigilContext::Effect,
            &HashSet::new(),
            &mut result,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn entity_binding_is_allowed_in_all_contexts() {
        for ctx in [
            SigilContext::Guard,
            SigilContext::Effect,
            SigilContext::Tick,
        ] {
            let mut result = ValidationResult::new();
            check_binding_sigil("@entity.health", "test", ctx, &HashSet::new(), &mut result);
            assert!(result.is_valid(), "entity should be allowed in {:?}", ctx);
        }
    }

    #[test]
    fn pascalcase_entity_bindings_skip_sigil_check() {
        let mut result = ValidationResult::new();
        check_binding_sigil(
            "@GameConfig.gravity",
            "test",
            SigilContext::Tick,
            &HashSet::new(),
            &mut result,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn lambda_locals_are_skipped() {
        let mut locals = HashSet::new();
        locals.insert("item".to_string());
        let mut result = ValidationResult::new();
        check_binding_sigil(
            "@item.name",
            "test",
            SigilContext::Tick,
            &locals,
            &mut result,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn unknown_sigil_root_in_guard_is_rejected() {
        let mut result = ValidationResult::new();
        check_binding_sigil(
            "@bogus.field",
            "test",
            SigilContext::Guard,
            &HashSet::new(),
            &mut result,
        );
        assert!(!result.is_valid());
    }
}
