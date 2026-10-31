//! Emit declaration collection + effect dispatch.

use super::super::{ErrorCode, PageInitPayload, ValidationError, ValidationResult};
use super::{
    effect_operators, extract_emit_object_ref,
    validate_os_effect, validate_program_eval,
    validate_set_return_type, validate_set_value_type,
    DeclaredEmit, EntityFieldMeta,
};
use crate::embedded::ServiceRegistryData;
use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, SExpression, TraitDefinition};
use std::collections::{HashMap, HashSet};

/// Collect all declared emits from a trait definition
pub(crate) fn collect_declared_emits(trait_def: &TraitDefinition) -> Vec<DeclaredEmit> {
    let mut declared = Vec::new();

    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            let payload_fields: Vec<String> = emit
                .payload_schema
                .as_ref()
                .map(|fields| fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            declared.push(DeclaredEmit {
                event: emit.event.clone(),
                payload_fields,
            });
        }
    }

    declared
}

/// Generic constraint: when the operator's canonical metadata declares
/// `requiresEmitSuccess: true`, the effect call site MUST carry a trailing
/// options object with `emit.success` set to a non-empty event key. Driven
/// entirely by `canonical-operators.json` so future "this op requires X"
/// constraints land as one-line JSON flips without new validator passes.
///
/// Iteration shape mirrors `collectServerEmittedEvents` in verify's planner:
/// scan from arg index 2 to the end, find the first object that has an
/// `emit` property, and check that property for a non-empty `success` string.
pub(crate) fn validate_required_emit_success(
    operator: &str,
    arr: &[SExpression],
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let meta = match crate::embedded::get_operator(operator) {
        Some(m) => m,
        None => return result,
    };
    if !meta.requires_emit_success {
        return result;
    }

    // Find the first options-object arg (anything from index 2 onward that's
    // a plain object — not an array, not null).
    let mut found_emit_success: Option<String> = None;
    let mut saw_options_object = false;
    for arg in arr.iter().skip(2) {
        let SExpression::Object(obj) = arg else {
            continue;
        };
        saw_options_object = true;
        let Some(emit) = obj.get("emit") else {
            continue;
        };
        let SExpression::Object(emit_obj) = emit else {
            continue;
        };
        let Some(success) = emit_obj.get("success") else {
            continue;
        };
        if let SExpression::String(s) = success {
            if !s.is_empty() {
                found_emit_success = Some(s.clone().into_inner());
                break;
            }
        }
    }

    if found_emit_success.is_none() {
        let detail = if !saw_options_object {
            format!(
                "Operator '{}' requires a trailing options object with `emit: {{ success: \"<EVENT>\" }}`. None found.",
                operator
            )
        } else {
            format!(
                "Operator '{}' requires `emit.success` to be a non-empty event key in the trailing options object.",
                operator
            )
        };
        result.add_error(
            ValidationError::error(ErrorCode::EffectMissingRequiredEmit, path, detail)
                .with_suggestion(format!(
                    "Add `{{ emit: {{ success: \"<EVENT_KEY>\" }} }}` as the last arg of the {} effect (declare the success event in the trait's `emits` array too).",
                    operator
                )),
        );
    }

    result
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_effect(
    effect: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_registry: &ServiceRegistryData,
    declared_emits: Option<&[DeclaredEmit]>,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
) -> ValidationResult {
    validate_effect_in(
        effect,
        path,
        known_entities,
        entity_definitions,
        known_services,
        page_init_payloads,
        service_registry,
        declared_emits,
        entity_field_types,
        payload_field_types,
        &HashMap::new(),
        None,
        None,
        None,
    )
}

/// The effects a sequencing form (`do`, `if`, `when`, `let`) runs directly, each with its path: `<path>.do[i]`,
/// `.then`, `.else`, `.then[i]` (`when`), `.body[i]` (`let`). A position written as a list of effects (`[[effect…]]`)
/// adds `[k]`. Empty for a leaf effect. The one definition of where an effect nests.
///
/// Each effect also carries the condition it runs under: `condition` (the transition's guard, or the branch it is
/// already in) and the test of the `if` / `when` it is a branch of (`(not test)` for an `else`). A `let` ends the
/// facts about the names it rebinds, and an effect after a `set` in the same sequence no longer holds facts read
/// from `@entity`.
pub(crate) fn nested_effects<'a>(
    effect: &'a SExpression,
    path: &str,
    condition: Option<&SExpression>,
) -> Vec<(&'a SExpression, String, Option<SExpression>)> {
    let SExpression::Array(arr) = effect else {
        return Vec::new();
    };
    let head = arr.first().and_then(|e| e.as_str());
    let test = |negate: bool| {
        arr.get(1).map(|test| {
            if negate {
                SExpression::Array(vec![SExpression::String("not".into()), test.clone()].into())
            } else {
                test.clone()
            }
        })
    };
    let (positions, under): (Vec<(&SExpression, String)>, Vec<Option<SExpression>>) = match head {
        Some("do") => {
            let positions: Vec<_> = arr.iter().enumerate().skip(1).map(|(i, b)| (b, format!("{}.do[{}]", path, i - 1))).collect();
            let under = vec![condition.cloned(); positions.len()];
            (positions, under)
        }
        Some("if") => {
            let mut positions = Vec::new();
            let mut under = Vec::new();
            for (branch, label, negate) in [(arr.get(2), "then", false), (arr.get(3), "else", true)] {
                if let Some(branch) = branch {
                    positions.push((branch, format!("{}.{}", path, label)));
                    under.push(and_condition(condition, test(negate)));
                }
            }
            (positions, under)
        }
        Some("when") => {
            let positions: Vec<_> = arr.iter().enumerate().skip(2).map(|(i, b)| (b, format!("{}.then[{}]", path, i - 2))).collect();
            let under = vec![and_condition(condition, test(false)); positions.len()];
            (positions, under)
        }
        Some("let") => {
            let bound = let_names(arr);
            let held = condition.and_then(|c| conjunction(conjuncts(c).into_iter().filter(|part| !bound.iter().any(|name| mentions(part, name))).collect()));
            let positions: Vec<_> = arr.iter().enumerate().skip(2).map(|(i, b)| (b, format!("{}.body[{}]", path, i - 2))).collect();
            let under = vec![held; positions.len()];
            (positions, under)
        }
        _ => return Vec::new(),
    };
    let sequenced = matches!(head, Some("do" | "when" | "let"));
    let mut written = false;
    let mut out = Vec::new();
    for ((position, position_path), mut held) in positions.into_iter().zip(under) {
        if written {
            held = held.and_then(|c| conjunction(conjuncts(&c).into_iter().filter(|part| !mentions(part, "entity")).collect()));
        }
        match position {
            SExpression::Array(items) if matches!(items.first(), Some(SExpression::Array(_))) => {
                for (k, e) in items.iter().enumerate() {
                    let mut at = held.clone();
                    if written {
                        at = at.and_then(|c| conjunction(conjuncts(&c).into_iter().filter(|part| !mentions(part, "entity")).collect()));
                    }
                    out.push((e, format!("{}[{}]", position_path, k), at));
                    written |= sequenced && writes_entity(e);
                }
            }
            _ => {
                out.push((position, position_path, held));
                written |= sequenced && writes_entity(position);
            }
        }
    }
    out
}

fn let_names(arr: &[SExpression]) -> Vec<String> {
    match arr.get(1) {
        Some(SExpression::Array(bindings)) => bindings
            .iter()
            .filter_map(|binding| match binding {
                SExpression::Array(pair) => pair.first().and_then(|name| name.as_str()).map(str::to_string),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn and_condition(outer: Option<&SExpression>, extra: Option<SExpression>) -> Option<SExpression> {
    let mut parts = outer.map(conjuncts).unwrap_or_default();
    parts.extend(extra.as_ref().map(conjuncts).unwrap_or_default());
    conjunction(parts)
}

fn conjuncts(condition: &SExpression) -> Vec<SExpression> {
    match condition {
        SExpression::Array(items) if items.first().and_then(|head| head.as_str()) == Some("and") => {
            items.iter().skip(1).flat_map(conjuncts).collect()
        }
        other => vec![other.clone()],
    }
}

fn conjunction(mut parts: Vec<SExpression>) -> Option<SExpression> {
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => {
            parts.insert(0, SExpression::String("and".into()));
            Some(SExpression::Array(parts.into()))
        }
    }
}

/// Whether `expr` reads the binding `@name` (or a path under it).
fn mentions(expr: &SExpression, name: &str) -> bool {
    match expr {
        SExpression::String(text) => text
            .strip_prefix('@')
            .is_some_and(|rest| rest.strip_prefix(name).is_some_and(|tail| tail.is_empty() || tail.starts_with('.'))),
        SExpression::Array(items) => items.iter().any(|item| mentions(item, name)),
        SExpression::Object(fields) => fields.values().any(|item| mentions(item, name)),
        _ => false,
    }
}

/// Whether running `effect` can write the entity (a `set` anywhere in it).
fn writes_entity(effect: &SExpression) -> bool {
    match effect {
        SExpression::Array(items) => {
            items.first().and_then(|head| head.as_str()) == Some("set") || items.iter().any(writes_entity)
        }
        SExpression::Object(fields) => fields.values().any(writes_entity),
        _ => false,
    }
}

/// The binding types the body of `["let", [[name, value]…], body…]` is checked against: `types` plus each local, typed
/// as it is bound (later bindings see earlier ones). A local whose type is not decidable is removed, so it stays
/// permissive. `on_value` sees each binding's value expression. `None` when there are no outer types to extend.
pub(crate) fn scope_let_locals(
    arr: &[SExpression],
    types: Option<&super::set::BindingTypes>,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    schema: Option<&OrbitalSchema>,
    on_value: &mut dyn FnMut(&SExpression),
) -> Option<super::set::BindingTypes> {
    let mut scoped = types.cloned();
    if let Some(SExpression::Array(bindings)) = arr.get(1) {
        for binding in bindings {
            if let SExpression::Array(pair) = binding {
                if let Some(bound) = pair.get(1) {
                    on_value(bound);
                }
                if let (Some(SExpression::String(name)), Some(bound), Some(scope)) =
                    (pair.first(), pair.get(1), scoped.as_mut())
                {
                    let ctx = super::set::ExprReturnCtx {
                        entity: entity_field_types,
                        payload: payload_field_types,
                        types: Some(&*scope),
                        schema,
                    };
                    let declaration = pair
                        .get(2)
                        .and_then(|shape| orbital_core_eval::lower::lower_sexpr(shape).ok());
                    let ty = orbital_core_eval::lower::lower_sexpr(bound).ok().and_then(|oir| {
                        crate::type_infer::local_initializer_type(&oir, declaration.as_ref(), &ctx)
                    });
                    match ty {
                        Some(ty) => {
                            scope.locals.insert(name.clone().into_inner(), ty);
                        }
                        None => {
                            scope.locals.remove(name.as_str());
                        }
                    }
                }
            }
        }
    }
    scoped
}

/// Validate an effect and every effect nested in its control flow. `do`, `if`, `when` and `let` are the
/// effect-sequencing forms (`exec_effect_expr` in the runtime kernel); each child effect is validated
/// exactly like a top-level one, at `<path>.do[i]` / `.then` / `.else` / `.body[i]`. A branch written as a
/// list of effects (`[[effect…]]`) adds `[k]`. Conditions and `let` values are value positions: they only get
/// the nested call-service walk.
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_effect_in(
    effect: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_registry: &ServiceRegistryData,
    declared_emits: Option<&[DeclaredEmit]>,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    payload_shapes: &HashMap<String, orbital_core_base::ir::OirType>,
    schema: Option<&OrbitalSchema>,
    types: Option<&super::set::BindingTypes>,
    condition: Option<&SExpression>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let head = match effect {
        SExpression::Array(arr) => arr.first().and_then(|e| e.as_str()),
        _ => None,
    };
    let (Some(head @ ("do" | "if" | "when" | "let")), SExpression::Array(arr)) = (head, effect)
    else {
        result.merge(validate_leaf_effect(
            effect,
            path,
            known_entities,
            entity_definitions,
            page_init_payloads,
            service_registry,
            declared_emits,
            entity_field_types,
            payload_field_types,
            payload_shapes,
            schema,
            types,
            condition,
        ));
        return result;
    };

    let value = |expr: &SExpression, result: &mut ValidationResult| {
    };
    let mut scoped = None;
    match head {
        "if" | "when" => {
            if let Some(cond) = arr.get(1) {
                value(cond, &mut result);
            }
        }
        "let" => {
            scoped = scope_let_locals(arr, types, entity_field_types, payload_field_types, schema, &mut |bound| {
                value(bound, &mut result)
            });
        }
        _ => {}
    }
    for (member, at, under) in nested_effects(effect, path, condition) {
        result.merge(validate_effect_in(
            member,
            &at,
            known_entities,
            entity_definitions,
            known_services,
            page_init_payloads,
            service_registry,
            declared_emits,
            entity_field_types,
            payload_field_types,
            payload_shapes,
            schema,
            scoped.as_ref().or(types),
            under.as_ref(),
        ));
    }
    result
}

/// Validate a single non-control-flow effect
#[allow(clippy::too_many_arguments)]
fn validate_leaf_effect(
    effect: &SExpression,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_registry: &ServiceRegistryData,
    declared_emits: Option<&[DeclaredEmit]>,
    // SET-EMITS-PAYLOAD + ENUM-UNENFORCED backstop inputs: the linked entity's
    // per-field metadata (type + enum members) and the trait's declared payload
    // field types (`{ name -> type }`).
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    payload_shapes: &HashMap<String, orbital_core_base::ir::OirType>,
    schema: Option<&OrbitalSchema>,
    types: Option<&super::set::BindingTypes>,
    condition: Option<&SExpression>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match effect {
        // S-expression array form: ["operator", ...args]
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(operator) = &arr[0] {
                // Generic constraint check driven by canonical-operators.json:
                // when an operator's `requiresEmitSuccess: true`, the call site
                // MUST carry a trailing options object with `emit.success = "X"`.
                // Future "this op requires X" constraints can be added by
                // extending `EmbeddedOperator` with new boolean flags + matching
                // checks here — no per-operator special cases needed.
                result.merge(validate_required_emit_success(operator, arr, path));

                match operator.as_str() {
                    "call-service" => {
                    }
                    "set" => {
                        result.merge(validate_set_value_type(
                            arr,
                            path,
                            entity_field_types,
                            payload_shapes,
                            schema,
                            types,
                            condition,
                        ));
                        result.merge(validate_set_return_type(
                            arr,
                            path,
                            entity_field_types,
                            payload_field_types,
                            schema,
                            types,
                            condition,
                        ));
                    }
                    "persist" => {
                    }
                    "emit" => {
                    }
                    "render-ui" => {
                        // render_ui is validated separately in render_ui.rs
                    }
                    "navigate" => {
                    }
                    "notify" => {
                        // `notify` is `.lolo`-only sugar for `render-ui toast`
                        // (owner ruling 2026-09-11), desugared at lowering — a
                        // tuple reaching L2 is a hand-authored `.orb` or a
                        // registry not yet re-emitted. Recognised, not crashed
                        // on, so this reads as a real diagnostic.
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectNotifyRetired,
                                path,
                                "`notify` is sugar for `render-ui toast { type: alert … }` — re-emit the `.lolo`",
                            ),
                        );
                    }
                    "send-server" => {
                    }
                    "program/eval" => {
                        result.merge(validate_program_eval(arr, path, declared_emits));
                    }
                    os_op
                        if orbital_core_base::language::registry::os_host_effects()
                            .any(|name| name == os_op) =>
                    {
                        result.merge(validate_os_effect(os_op, arr, path, declared_emits));
                    }
                    browser_op if browser_op.starts_with("browser/") => {
                        // Client device effects (file picker / clipboard /
                        // geolocation). One-shot host calls carrying a trailing
                        // `{ emit: { success, failure } }` envelope — validate
                        // that the emit targets are declared events, mirroring
                        // the os/watch-* arm but with success/failure keys.
                        for arg in arr.iter().skip(1) {
                            if let SExpression::Object(options) = arg {
                                if let Some(emit_obj) = extract_emit_object_ref(options) {
                                    let keys: &[&str] = if matches!(
                                        browser_op,
                                        "browser/prompt" | "browser/embed"
                                    ) || orbital_core_base::tab::TabOp::is_tab_effect(browser_op)
                                    {
                                        &["success", "failure", "cancelled"]
                                    } else {
                                        &["success", "failure"]
                                    };
                                }
                            }
                        }
                    }
                    op if !effect_operators().contains(&op) => {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectUnknownType,
                                path,
                                format!("Unknown effect operator: '{}'", op),
                            )
                            .with_valid_values(
                                effect_operators().iter().map(|s| s.to_string()).collect(),
                            ),
                        );
                    }
                    _ => {
                        // Known operator, no additional validation needed
                    }
                }
            }
        }
        _ => {
            // Other forms are not effect-like, skip
        }
    }

    // I-8: nested call-service forms inside this effect's arguments get the
    // same name/action/param checks the top-level form gets. The walk starts
    // BELOW the top form so the arms above stay the single validators for it.
    if let SExpression::Array(arr) = effect {
        for arg in arr.iter().skip(1) {
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::set::BindingTypes;
    use super::super::EntityFieldMeta;

    fn set_findings(effect: &str, guard: Option<&str>) -> usize {
        let types = BindingTypes {
            payload: HashMap::new(),
            user: orbital_core_base::ir::OirType::Struct { additional: None, fields: Default::default() },
            config: HashMap::new(),
            prev_payload: None,
            optional_payload: Default::default(),
            locals: Default::default(),
            type_params: Default::default(),
        };
        let meta = |shape| EntityFieldMeta { field_type: "number".into(), shape, required: false, defaulted: false };
        let number = orbital_core_base::type_compat::compat_kind_oir("number");
        let entity: HashMap<String, EntityFieldMeta> = [
            ("n".to_string(), meta(number.clone())),
            ("m".to_string(), meta(orbital_core_base::ir::OirType::optional(number))),
        ]
        .into_iter()
        .collect();
        let effect: SExpression = serde_json::from_str(effect).unwrap();
        let guard: Option<SExpression> = guard.map(|g| serde_json::from_str(g).unwrap());
        validate_effect_in(
            &effect,
            "p",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &ServiceRegistryData::default(),
            None,
            &entity,
            &HashMap::new(),
            &HashMap::new(),
            None,
            Some(&types),
            guard.as_ref(),
        )
        .errors
        .iter()
        .filter(|e| e.code == super::super::super::ErrorCode::SetReturnTypeMismatch)
        .count()
    }

    fn set_in_let(body: &str) -> String {
        format!(r#"["let", [["x", ["str/to-number", "a"]]], {body}]"#)
    }

    #[test]
    fn a_null_test_on_a_let_local_narrows_it_in_a_set() {
        let write = r#"["set", "@entity.n", ["if", true, "@x", 0]]"#;
        let fail = r#"["log", "info", "none"]"#;
        assert_eq!(set_findings(&set_in_let(write), None), 1, "control: untested, the local may be null");
        assert_eq!(set_findings(&set_in_let(&format!(r#"["if", ["=", "@x", null], {fail}, {write}]"#)), None), 0);
        assert_eq!(set_findings(&set_in_let(&format!(r#"["if", ["and", true, ["!=", "@x", null]], {write}, {fail}]"#)), None), 0);
        assert_eq!(set_findings(&set_in_let(&format!(r#"["if", ["=", "@x", null], {write}, {fail}]"#)), None), 1, "control: the null branch");
        let shadowed = format!(r#"["let", [["x", ["str/to-number", "b"]]], {write}]"#);
        assert_eq!(set_findings(&set_in_let(&format!(r#"["if", ["!=", "@x", null], {shadowed}, {fail}]"#)), None), 1, "a shadowing let ends the narrowing");
    }

    #[test]
    fn a_guard_narrows_a_set_and_a_set_ends_a_narrowing_read_from_the_entity() {
        let write = r#"["set", "@entity.n", ["if", true, "@entity.m", 0]]"#;
        let guard = r#"["!=", "@entity.m", null]"#;
        assert_eq!(set_findings(write, None), 1, "control: the optional field may be null");
        assert_eq!(set_findings(write, Some(guard)), 0, "the transition guard holds for the set");
        let tested = |body: &str| format!(r#"["if", {guard}, {body}, ["log", "info", "none"]]"#);
        assert_eq!(set_findings(&tested(write), None), 0, "an if test holds in its then branch");
        let after_write = format!(r#"["do", ["set", "@entity.m", null], {write}]"#);
        assert_eq!(set_findings(&tested(&after_write), None), 1, "a set of the entity ends what the test showed of it");
    }
}
