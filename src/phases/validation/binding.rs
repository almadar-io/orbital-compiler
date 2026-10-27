//! Binding Validation
//!
//! Validates binding references (@entity, @payload, @state, etc.)
//! Ported from `builder/packages/shared/src/orbitals/validation/binding-validator.ts`.

use super::canonical_operators::get_canonical_operator;
use super::{trait_path, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Core binding roots
pub const CORE_BINDING_ROOTS: &[&str] = &[
    "entity",          // Current entity data
    "payload",         // Event payload
    "state",           // Current state name
    "now",             // Current timestamp
    "config",          // Trait configuration
    "trait",           // Trait context
    "user",            // User context
    "callsitePayload", // Call-site-captured event payload (resolved at the composing effect)
    "pages",           // Render-resolved schema sigil — the host orbital's pages (NavItem[])
    "currentTheme",    // Render-resolved schema sigil — the host orbital's data-theme key
    "locale",          // The active locale code — the catalog `i18n/t` reads
    "event",           // The delivery being processed: `{ event, payload, source }`
    "prevEvents",      // Deliveries this trait already received this dispatch
    "prevStates",      // States this trait left this dispatch
    "fromState",       // The transition's source state
    "toState",         // The transition's target state
    "fromEntity",      // The entity before the transition, inside an `@ensures` only
];

/// Regex for PascalCase entity names
/// A well-formed binding: `@root.field.nested` or `@root[index]`, Unicode letters allowed. A malformed one is the
/// std lint's finding (ORB_BINDING_INVALID_FORMAT); the root checks below never read it.
static BINDING_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@[\p{L}_][\p{L}0-9_]*(\.[\p{L}0-9_]+|\[\d+\])*$").unwrap());

static PASCAL_CASE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Binding context for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingContext {
    /// In a transition effect
    Effect,
    /// In a guard condition
    Guard,
    /// In a tick effect
    Tick,
    /// In a render_ui pattern prop
    RenderUI,
    /// In a transition's `@ensures`: `@entity` is the row after the effects, `@fromEntity` the row before them
    Ensures,
}


/// Validate all bindings in a schema
pub fn validate_all_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_trait_bindings(trait_def, &path);
                ctx.merge(result);
            }
        }

        for (k, e) in std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten()).enumerate() {
            let orbital_core_base::schema::EntityRef::Inline(def) = e else { continue };
            let at = if k == 0 { "entity".to_string() } else { format!("auxiliaryEntities[{}]", k - 1) };
            for (n, inv) in def.invariants.iter().flatten().enumerate() {
                let p = format!("orbitals[{i}].{at}.invariants[{n}]");
                ctx.merge(validate_expr_bindings(inv, &p, BindingContext::Guard));
            }
        }
    }
}

/// Validate all bindings in a trait
pub fn validate_trait_bindings(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        if let Some(guard) = &transition.guard {
            result.merge(validate_expr_bindings(
                guard,
                &format!("{}.guard", trans_path),
                BindingContext::Guard,
            ));
        }

        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path = format!("{}.effects[{}]", trans_path, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &effect_path,
                    BindingContext::Effect,
                ));
            }
        }
    }

    // Propositions read the same roots as a guard; an `@ensures` also reads `@fromEntity`, the same entity before.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        for (k, ensures) in transition.ensures.iter().flatten().enumerate() {
            let p = format!("{}.stateMachine.transitions[{}].ensures[{}]", path, i, k);
            result.merge(validate_expr_bindings(ensures, &p, BindingContext::Ensures));
        }
    }
    for (k, state) in trait_def.state_machine.states.iter().enumerate() {
        if let Some(holds) = &state.holds {
            let p = format!("{}.stateMachine.states[{}].holds", path, k);
            result.merge(validate_expr_bindings(holds, &p, BindingContext::Guard));
        }
    }

    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let tick_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &tick_path,
                    BindingContext::Tick,
                ));
            }
        }
    }

    result
}

/// Validate bindings in an expression
fn validate_expr_bindings(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
) -> ValidationResult {
    validate_expr_bindings_with_locals(expr, path, context, &HashSet::new())
}

/// Validate bindings in an expression, with lambda-local variable tracking.
/// `lambda_locals` contains parameter names bound by enclosing lambdas (e.g., "item", "f", "o").
/// Bindings like `@item.field` are valid when "item" is in `lambda_locals`.
fn validate_expr_bindings_with_locals(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with('@') => {
            // Check if this binding root is a lambda-local variable
            let root = s[1..].split('.').next().unwrap_or("");
            let root = root.split('[').next().unwrap_or(root);
            if lambda_locals.contains(root) {
                // Valid lambda-bound variable — skip standard binding validation
            } else {
                result.merge(validate_binding(s, path, context));
            }
        }
        SExpression::Array(arr) => {
            // Detect lambda expressions: ["lambda", [params...], body]
            if arr.len() == 3 {
                if let SExpression::String(op) = &arr[0] {
                    if op == "lambda" || op == "fn" {
                        if let SExpression::Array(params) = &arr[1] {
                            // Extract lambda parameter names and add to locals
                            // Format: ["fn", ["param1", "param2"], body]
                            let mut extended_locals = lambda_locals.clone();
                            for param in params {
                                if let SExpression::String(name) = param {
                                    extended_locals.insert(name.clone().into_inner());
                                }
                            }
                            // Validate the lambda body with extended locals
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                &extended_locals,
                            );
                        } else if let SExpression::String(param_name) = &arr[1] {
                            // Single string parameter shorthand
                            // Format: ["fn", "param", body]
                            let mut extended_locals = lambda_locals.clone();
                            extended_locals.insert(param_name.clone().into_inner());
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                &extended_locals,
                            );
                        }
                    }
                    // Detect `let`: ["let", [[name, valueExpr], ...], body].
                    // SEQUENTIAL `let`: each binding's value-expr sees the
                    // locals bound by EARLIER bindings in the same `let` (and
                    // the outer scope); the body sees them all. This matches
                    // BOTH runtimes — the Rust evaluator's `LetOp` and the JS
                    // `EffectExecutor` `let` both fold each value against an
                    // accumulating child scope. (The old parallel model —
                    // value-exprs in the outer scope only — diverged from
                    // runtime and falsely rejected `(let ((a …) (b @a)))`.)
                    if op == "let" {
                        let mut extended_locals = lambda_locals.clone();
                        match &arr[1] {
                            SExpression::Array(bindings) => {
                                for (bi, binding) in bindings.iter().enumerate() {
                                    if let SExpression::Array(pair) = binding {
                                        // Validate the value against locals from
                                        // PRIOR bindings only (name inserted
                                        // after), so a self-reference still errors.
                                        if pair.len() >= 2 {
                                            result.merge(validate_expr_bindings_with_locals(
                                                &pair[1],
                                                &format!("{}[1][{}][1]", path, bi),
                                                                context,
                                        &extended_locals,
                                            ));
                                        }
                                        if let Some(SExpression::String(name)) = pair.first() {
                                            extended_locals.insert(name.clone().into_inner());
                                        }
                                    }
                                }
                            }
                            SExpression::Object(bindings) => {
                                // Object-form bindings have no defined order, so
                                // credit every name before validating any value
                                // (permissive — never falsely rejects). lolo only
                                // ever lowers `let` to the ordered array form above.
                                for name in bindings.keys() {
                                    extended_locals.insert(name.clone());
                                }
                                for (name, value_expr) in bindings {
                                    result.merge(validate_expr_bindings_with_locals(
                                        value_expr,
                                        &format!("{}[1].{}", path, name),
                                        context,
                                        &extended_locals,
                                    ));
                                }
                            }
                            _ => {}
                        }
                        result.merge(validate_expr_bindings_with_locals(
                            &arr[2],
                            &format!("{}[2]", path),
                            context,
                            &extended_locals,
                        ));
                        return result;
                    }
                }
            }
            // Detect collection operators (map, filter, find, etc.) that introduce
            // implicit @item/@index lambda scope on their body argument (args[2+]).
            // Uses the canonical operator registry to identify collection/array operators
            // that accept a callback (arity >= 2).
            if arr.len() >= 3 {
                if let SExpression::String(op) = &arr[0] {
                    let is_collection_op = is_lambda_scope_operator(op);
                    if is_collection_op {
                        // Validate the source (args[1]) without implicit locals
                        result.merge(validate_expr_bindings_with_locals(
                            &arr[1],
                            &format!("{}[1]", path),
                            context,
                            lambda_locals,
                        ));
                        // Validate the body (args[2+]) with @item/@index in scope
                        let mut extended_locals = lambda_locals.clone();
                        extended_locals.insert("item".to_string());
                        extended_locals.insert("index".to_string());
                        for (i, item) in arr[2..].iter().enumerate() {
                            result.merge(validate_expr_bindings_with_locals(
                                item,
                                &format!("{}[{}]", path, i + 2),
                                context,
                                &extended_locals,
                            ));
                        }
                        // Validate the operator name itself (skip, it's not a binding)
                        return result;
                    }
                }
            }
            // Normal array — recurse into all elements
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_expr_bindings_with_locals(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    lambda_locals,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_expr_bindings_with_locals(
                    value,
                    &format!("{}.{}", path, key),
                    context,
                    lambda_locals,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate a single binding
pub fn validate_binding(
    binding: &str,
    path: &str,
    context: BindingContext,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !BINDING_REGEX.is_match(binding) {
        return result;
    }
    let Some(rest) = binding.strip_prefix('@') else {
        return result;
    };
    let root = rest.split('.').next().unwrap_or_default();
    // Handle array index in first part (e.g., @items[0])
    let root = root.split('[').next().unwrap_or(root);

    if CORE_BINDING_ROOTS.contains(&root) {
        if root == "payload" && context == BindingContext::Tick {
            result.add_error(
                ValidationError::error(
                    ErrorCode::BindingPayloadInTick,
                    path,
                    "@payload is not available in tick context",
                )
                .with_suggestion("Tick effects don't have event payloads - use @entity instead"),
            );
        }
    } else if PASCAL_CASE_REGEX.is_match(root) {
        // Entity binding (e.g., @Task, @User)
    } else if root == "item" || root == "index" {
        // S16 — same class as `sigil_context.rs`'s arm, same one diagnostic:
        // callers strip lambda-locals before reaching here (see
        // `validate_expr_bindings_with_locals`), so `item`/`index` at this
        // point escaped its collection callback.
        result.add_error(super::error::out_of_scope_collection_local(
            ErrorCode::BindingUnknownRoot,
            path,
            root,
        ));
    } else {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingUnknownRoot,
                path,
                format!("Unknown binding root: '@{}'", root),
            )
            .with_suggestion("Use @entity, @payload, @state, @now, @config, or @EntityName"),
        );
    }

    result
}

/// Validate a set effect target binding
pub fn validate_set_target(binding: &str, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    let Some(rest) = binding.strip_prefix('@') else {
        return result;
    };
    let root = rest.split('.').next().unwrap_or_default();

    // Cannot set @payload (read-only)
    if root == "payload" {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectSetPayloadNotAllowed,
                path,
                "@payload is read-only and cannot be a set target",
            )
            .with_suggestion("Use @entity to update entity state"),
        );
    }

    // Cannot set @state or @now directly
    if root == "state" || root == "now" {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidPath,
                path,
                format!("@{} cannot be a set target", root),
            )
            .with_suggestion("Use @entity.field to update entity data"),
        );
    }

    result
}

/// Check if an operator introduces an implicit @item/@index lambda scope.
/// Uses the embedded operator registry (Phase 3 of Lolo v3): collection and
/// std-array operators with arity >= 2 (i.e., they accept a callback/predicate argument).
/// Also handles unnamespaced aliases (e.g., "map" checks "array/map" too).
fn is_lambda_scope_operator(op: &str) -> bool {
    // Replaced canonical_operators lookup with embedded::get_operator() in Phase 3 of Lolo v3 plan.
    if let Some(meta) = crate::embedded::get_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Legacy fallback for operators not yet in operators.json
    if let Some(meta) = get_canonical_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Check with array/ prefix for unnamespaced names (e.g., "map" -> "array/map")
    if !op.contains('/') {
        let namespaced = format!("array/{}", op);
        if let Some(meta) = crate::embedded::get_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
        if let Some(meta) = get_canonical_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twin of `@almadar/core` `__tests__/binding-roots.test.ts`: both lists are pinned to the author vocabulary.
    #[test]
    fn every_author_sigil_is_a_core_root_and_the_set_matches_the_ts_mirror() {
        let en = &orbital_core_base::i18n::tables::definitions()["en"];
        for sigil in en.sigils.keys() {
            assert!(
                CORE_BINDING_ROOTS.contains(&sigil.as_str()),
                "i18n sigil `{sigil}` is not a validator root"
            );
        }
        let mut roots: Vec<&str> = CORE_BINDING_ROOTS.to_vec();
        roots.sort_unstable();
        assert_eq!(
            roots,
            [
                "callsitePayload",
                "config",
                "currentTheme",
                "entity",
                "event",
                "fromEntity",
                "fromState",
                "locale",
                "now",
                "pages",
                "payload",
                "prevEvents",
                "prevStates",
                "state",
                "toState",
                "trait",
                "user"
            ]
        );
    }

    #[test]
    fn test_valid_bindings() {
        let result = validate_binding(
            "@entity.title",
            "test",
            BindingContext::Effect,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Effect,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@Task.title",
            "test",
            BindingContext::Effect,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn the_dispatch_family_is_valid_in_guards_and_effects() {
        for binding in [
            "@event",
            "@event.event",
            "@event.payload.remaining",
            "@event.source.tick",
            "@event.source.originClientId",
            "@prevEvents",
            "@prevStates",
            "@fromState",
            "@toState",
        ] {
            for context in [
                BindingContext::Guard,
                BindingContext::Effect,
                BindingContext::RenderUI,
            ] {
                assert!(
                    codes(binding, context).is_empty(),
                    "{binding} in {context:?}"
                );
            }
        }
    }

    #[test]
    fn test_payload_in_tick() {
        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Tick,
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn test_set_target_payload() {
        let result = validate_set_target("@payload.field", "test");
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_lambda_bound_item_is_valid() {
        // ["lambda", ["item"], ["=", "@item.status", "pending"]]
        let expr = SExpression::Array(vec![
            SExpression::String(("lambda".to_string()).into()),
            SExpression::Array(vec![SExpression::String(("item".to_string()).into())]),
            SExpression::Array(vec![
                SExpression::String(("=".to_string()).into()),
                SExpression::String(("@item.status".to_string()).into()),
                SExpression::String(("pending".to_string()).into()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        // @item inside lambda should NOT produce BindingUnknownRoot
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @item should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn fc5_item_in_scope_inside_array_map_children() {
        // ["array/map", "@entity.items", ["fn", "item", { type: badge, label: "@item.name" }]]
        let node = SExpression::Object(
            [
                ("type".to_string(), SExpression::String(("badge".to_string()).into())),
                (
                    "label".to_string(),
                    SExpression::String(("@item.name".to_string()).into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let expr = SExpression::Array(vec![
            SExpression::String(("array/map".to_string()).into()),
            SExpression::String(("@entity.items".to_string()).into()),
            SExpression::Array(vec![
                SExpression::String(("fn".to_string()).into()),
                SExpression::String(("item".to_string()).into()),
                node,
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "@item inside an array/map lambda body must be in scope. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn fc5_item_out_of_scope_outside_array_map() {
        // A bare `@item.name` with no enclosing map/lambda → unknown root.
        let expr = SExpression::Object(
            [
                ("type".to_string(), SExpression::String(("badge".to_string()).into())),
                (
                    "label".to_string(),
                    SExpression::String(("@item.name".to_string()).into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "@item outside any lambda must be out of scope (unknown root). Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_fn_string_param_lambda_is_valid() {
        // ["fn", "h", ["=", "@h.owner", "player"]]
        // String param shorthand (not wrapped in array)
        let expr = SExpression::Array(vec![
            SExpression::String(("fn".to_string()).into()),
            SExpression::String(("h".to_string()).into()),
            SExpression::Array(vec![
                SExpression::String(("=".to_string()).into()),
                SExpression::String(("@h.owner".to_string()).into()),
                SExpression::String(("player".to_string()).into()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @h with string param should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_non_lambda_item_is_invalid() {
        // @item.status outside a lambda should still warn
        let expr = SExpression::String(("@item.status".to_string()).into());
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Non-lambda @item should trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_sequential_let_binding_sees_prior_siblings() {
        // (let ((a @entity.n) (b @a)) (set @entity.n @b))
        // `@a` in b's value and `@b` in the body must resolve to the earlier
        // bindings — sequential `let`, matching both runtimes.
        let expr = arr(vec![
            s("let"),
            arr(vec![
                arr(vec![s("a"), s("@entity.n")]),
                arr(vec![s("b"), s("@a")]),
            ]),
            arr(vec![s("set"), s("@entity.n"), s("@b")]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "sequential let: a later binding must see earlier siblings; got {:?}",
            result.errors
        );
    }

    #[test]
    fn test_let_self_reference_still_errors() {
        // (let ((a @a)) (set @entity.n @a)) — `a` references itself before it is
        // bound; sequential scope must still reject it.
        let expr = arr(vec![
            s("let"),
            arr(vec![arr(vec![s("a"), s("@a")])]),
            arr(vec![s("set"), s("@entity.n"), s("@a")]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "self-referencing let binding @a must still error"
        );
    }

    fn codes(binding: &str, context: BindingContext) -> Vec<ErrorCode> {
        validate_binding(binding, "test", context)
            .errors
            .into_iter()
            .map(|e| e.code)
            .collect()
    }

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string().into())
    }
    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }
}
