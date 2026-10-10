//! Cross-registry consistency checks (Phase 3.5 of Lolo v3 plan).
//!
//! Walks the schema and verifies every operator, pattern, and behavior
//! reference exists in the embedded registries (operators.json,
//! patterns-registry.json, behaviors-registry.json).
//!
//! - Unknown operators are reported as warnings (some are still in the
//!   legacy hardcoded `sexpr.rs` constants and will move to operators.json
//!   in a future phase).
//! - Unknown patterns referenced inside `(render-ui ...)` calls are reported
//!   as warnings (project-local patterns may extend the core registry).
//! - Unknown behaviors referenced inside unresolved `behavior/*` calls are
//!   reported as errors (the Compose phase should resolve them; an unresolved
//!   `behavior/*` with a missing name is a hard schema bug).

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::embedded;
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition};

/// Run all cross-registry consistency checks across the schema.
pub fn validate_cross_registry(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_cross_registry(trait_def, &path);
        ctx.merge(result);
    }
}

fn validate_trait_cross_registry(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);
        if let Some(guard) = &transition.guard {
            walk_for_references(guard, &format!("{}.guard", trans_path), &mut result);
        }
        if let Some(effects) = &transition.effects {
            for (k, effect) in effects.iter().enumerate() {
                walk_for_references(
                    effect,
                    &format!("{}.effects[{}]", trans_path, k),
                    &mut result,
                );
            }
        }
    }

    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (k, effect) in tick.effects.iter().enumerate() {
                walk_for_references(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, k),
                    &mut result,
                );
            }
        }
    }

    result
}

fn walk_for_references(expr: &SExpression, path: &str, result: &mut ValidationResult) {
    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            // Operator existence: only namespaced operators (containing '/') are
            // checked here, because legacy code in sexpr.rs already runs the
            // unnamespaced fast path. This avoids double-reporting.
            if let SExpression::String(op) = &arr[0] {
                check_operator_reference(op, path, result);

                // render-ui pattern existence check
                if op == "render-ui" || op == "render_ui" {
                    if let Some((i, pattern)) = arr.iter().enumerate().skip(1).find_map(|(i, a)| {
                        if let SExpression::Object(obj) = a {
                            Some((i, obj))
                        } else {
                            None
                        }
                    }) {
                        if let Some(SExpression::String(pattern_type)) = pattern.get("type") {
                            check_pattern_reference(pattern_type, &format!("{path}[{i}].type"), result);
                        }
                    }
                }

                // behavior/* existence check (in case Compose left any unresolved)
                if let Some(suffix) = op.strip_prefix("behavior/") {
                    // Phase 0 composers + substrate operators take the behavior
                    // name as the second arg — the suffix is an operator verb,
                    // not a behavior name. Only non-verb suffixes are treated as
                    // behavior shorthand references (behavior/<name>).
                    let composer = matches!(suffix, "compose" | "wire" | "detect-layout" | "pipe");
                    // A registered value/reflection operator (`apply`, `ref`, `catalog`, …):
                    // the suffix is its verb, not a behavior name.
                    let behavior_name = if !composer && embedded::is_known_operator(op) {
                        None
                    } else if matches!(suffix, "compose" | "wire" | "detect-layout" | "pipe") {
                        if let Some(SExpression::String(s)) = arr.get(1) {
                            if s.starts_with('@') {
                                None
                            } else {
                                Some(s.as_str())
                            }
                        } else {
                            None
                        }
                    } else {
                        Some(suffix)
                    };
                    if let Some(name) = behavior_name {
                        check_behavior_reference(name, path, result);
                    }
                }
            }

            for (i, item) in arr.iter().enumerate() {
                walk_for_references(item, &format!("{}[{}]", path, i), result);
            }
        }
        SExpression::Object(obj) => {
            for (k, v) in obj {
                walk_for_references(v, &format!("{}.{}", path, k), result);
            }
        }
        _ => {}
    }
}

fn check_operator_reference(op: &str, path: &str, result: &mut ValidationResult) {
    // Skip non-operator first-positions: lambda placeholders, well-known
    // shorthand sigils that are not operators per se.
    if op == "lambda" || op == "fn" {
        return;
    }
    // Only flag namespaced operators (e.g. `nn/forward`, `tensor/from`) so we
    // don't drown the validator in noise from data arrays whose first element
    // is a string (which the legacy validator already handles).
    if !op.contains('/') {
        return;
    }
    if embedded::is_known_operator(op) {
        return;
    }
    // Legacy fallback: many ML operators are still in sexpr.rs constants.
    // Don't report them here — sexpr.rs already handles them.
    // Only flag operators that are completely unknown to BOTH systems.
    // We check this with a conservative whitelist of legacy namespaces.
    if op.starts_with("nn/")
        || op.starts_with("tensor/")
        || op.starts_with("train/")
        || op.starts_with("os/")
        || op.starts_with("graph/")
        || op.starts_with("contract/")
        || op.starts_with("data/")
        || op.starts_with("agent/")
        || op.starts_with("checkpoint/")
        || op.starts_with("behavior/")
    {
        return;
    }
    let suggestion = super::sexpr::operators::suggest_operator(op).unwrap_or_else(|| {
        "Use a registered operator — `orb signatures` lists the canonical set".to_string()
    });
    result.add_error(
        ValidationError::error(
            ErrorCode::SExprInvalidOperator,
            path,
            format!(
                "Operator '{}' is not in operators.json (cross-registry check)",
                op
            ),
        )
        .with_suggestion(suggestion),
    );
}

fn check_pattern_reference(pattern_type: &str, path: &str, result: &mut ValidationResult) {
    if embedded::is_known_pattern(pattern_type) {
        return;
    }
    // Project-local patterns may legitimately extend the core registry; emit
    // a warning rather than an error.
    result.add_error(
        ValidationError::error(
            ErrorCode::RenderUIInvalidPattern,
            path,
            format!(
                "Pattern '{}' is not in patterns-registry.json (cross-registry check)",
                pattern_type
            ),
        )
        .with_suggestion(
            "Either add the pattern to almadar-patterns or declare it in projects/<name>/patterns.json",
        ),
    );
}

fn check_behavior_reference(name: &str, path: &str, result: &mut ValidationResult) {
    if embedded::is_known_behavior(name) {
        return;
    }
    result.add_error(
        ValidationError::error(
            ErrorCode::SExprInvalidOperator,
            path,
            format!(
                "Behavior '{}' is not in behaviors-registry.json (cross-registry check)",
                name
            ),
        )
        .with_suggestion(format!(
            "Create packages/almadar-std/behaviors/registry/<atoms|molecules|organisms>/{}.orb (the registry is the canonical source of truth since Phase E, 2026-04-08), then run `npx tsx tools/behavior-registry-sync/index.ts` to refresh behaviors-registry.json.",
            name
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_operator_passes() {
        let mut result = ValidationResult::new();
        check_operator_reference("nn/forward", "test", &mut result);
        // nn/forward is in legacy whitelist, so check passes silently.
        assert!(result.is_valid());
    }

    #[test]
    fn unknown_namespaced_operator_warns() {
        let mut result = ValidationResult::new();
        check_operator_reference("xyz/totally-fake", "test", &mut result);
        assert!(!result.errors.is_empty() || !result.errors.is_empty());
    }

    #[test]
    fn known_pattern_passes() {
        let mut result = ValidationResult::new();
        check_pattern_reference("button", "test", &mut result);
        assert!(result.is_valid());
    }

    fn behavior_errors(expr: serde_json::Value) -> usize {
        let mut result = ValidationResult::new();
        let expr: SExpression = serde_json::from_value(expr).expect("sexpr");
        walk_for_references(&expr, "t", &mut result);
        result.errors.iter().filter(|e| e.message.contains("behaviors-registry")).count()
    }

    #[test]
    fn a_registered_behavior_operator_is_a_verb_not_a_behavior_name() {
        assert_eq!(behavior_errors(serde_json::json!(["behavior/ref", "./orbitals/shop"])), 0);
        assert_eq!(behavior_errors(serde_json::json!(["behavior/apply", "@entity.w", {}])), 0);
    }

    #[test]
    fn control_an_unregistered_behavior_shorthand_is_still_checked() {
        assert_eq!(behavior_errors(serde_json::json!(["behavior/no-such-behavior-xyz"])), 1);
    }

    #[test]
    fn unknown_pattern_warns() {
        let mut result = ValidationResult::new();
        check_pattern_reference("totally-fake-pattern-xyz", "test", &mut result);
        assert!(!result.errors.is_empty());
    }
}
