//! Emit declaration collection + effect dispatch.

use super::super::{ErrorCode, PageInitPayload, ValidationError, ValidationResult};
use super::call_service::walk_nested_call_services;
use super::fetch::validate_fetch_stream;
use super::validate_send_server;
use super::{
    effect_operators, extract_emit_object_ref, validate_call_service, validate_call_service_object,
    validate_emit, validate_emit_config, validate_fetch, validate_fetch_object, validate_navigate,
    validate_os_effect, validate_os_watch, validate_persist, validate_program_eval, validate_ref,
    validate_set, validate_set_enum_value, validate_set_object, validate_set_return_type,
    validate_set_value_type, DeclaredEmit, EntityFieldMeta,
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

            let required_fields: Vec<String> = emit
                .payload_schema
                .as_ref()
                .map(|fields| {
                    fields
                        .iter()
                        .filter(|f| f.required.unwrap_or(false))
                        .map(|f| f.name.clone())
                        .collect()
                })
                .unwrap_or_default();

            let is_external = emit
                .scope
                .as_ref()
                .map(|s| *s == orbital_core_base::schema::EventScope::External)
                .unwrap_or(false);

            declared.push(DeclaredEmit {
                event: emit.event.clone(),
                is_external,
                payload_fields,
                required_fields,
            });
        }
    }

    declared
}

/// Validate an emit effect against declared emits
pub(crate) fn validate_emit_against_declaration(
    effect: &SExpression,
    path: &str,
    declared_emits: &[DeclaredEmit],
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Only validate emit effects
    if let SExpression::Array(arr) = effect {
        if arr.is_empty() {
            return result;
        }
        if let SExpression::String(operator) = &arr[0] {
            if operator != "emit" {
                return result;
            }

            // Get the event name
            if arr.len() < 2 {
                return result; // Already validated in validate_emit
            }

            if let SExpression::String(event_name) = &arr[1] {
                // Check if the event is declared
                let declared = declared_emits.iter().find(|e| &e.event == event_name);

                if declared.is_none() {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitEventUndeclared,
                            path,
                            format!(
                                "Event '{}' is emitted but not declared in trait's emits array",
                                event_name
                            ),
                        )
                        .with_suggestion(
                            "Add the event to the trait's 'emits' array with proper payload schema",
                        )
                        .with_expected_shape(
                            r#"{
  "emits": [
    {
      "event": "EVENT_NAME",
      "scope": "internal" | "external",
      "description": "What this event means",
      "payload": [
        { "name": "fieldName", "type": "string", "required": true }
      ]
    }
  ]
}"#,
                        ),
                    );
                    return result;
                }

                let declared = declared.unwrap();

                // For external events, validate payload fields
                if declared.is_external && arr.len() >= 3 {
                    if let SExpression::Object(payload_obj) = &arr[2] {
                        // Check for required fields
                        for required_field in &declared.required_fields {
                            if !payload_obj.contains_key(required_field) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::CrossOrbitalPayloadMismatch,
                                        path,
                                        format!(
                                            "External emit '{}' is missing required payload field '{}'",
                                            event_name, required_field
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Add the '{}' field to the emit payload",
                                        required_field
                                    )),
                                );
                            }
                        }
                    } else if !declared.required_fields.is_empty() {
                        // Payload is missing or not an object but there are required fields
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::CrossOrbitalPayloadMismatch,
                                path,
                                format!(
                                    "External emit '{}' requires payload with fields: {:?}",
                                    event_name, declared.required_fields
                                ),
                            )
                            .with_suggestion("Add a payload object with the required fields"),
                        );
                    }
                }
            }
        }
    }

    result
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
                found_emit_success = Some(s.clone());
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
    )
}

/// The effects a sequencing form (`do`, `if`, `when`, `let`) runs directly, each with its path: `<path>.do[i]`,
/// `.then`, `.else`, `.then[i]` (`when`), `.body[i]` (`let`). A position written as a list of effects (`[[effect…]]`)
/// adds `[k]`. Empty for a leaf effect. The one definition of where an effect nests.
pub(crate) fn nested_effects<'a>(effect: &'a SExpression, path: &str) -> Vec<(&'a SExpression, String)> {
    let SExpression::Array(arr) = effect else {
        return Vec::new();
    };
    let positions: Vec<(&SExpression, String)> = match arr.first().and_then(|e| e.as_str()) {
        Some("do") => arr.iter().enumerate().skip(1).map(|(i, b)| (b, format!("{}.do[{}]", path, i - 1))).collect(),
        Some("if") => [(arr.get(2), "then"), (arr.get(3), "else")]
            .into_iter()
            .filter_map(|(b, label)| b.map(|b| (b, format!("{}.{}", path, label))))
            .collect(),
        Some("when") => arr.iter().enumerate().skip(2).map(|(i, b)| (b, format!("{}.then[{}]", path, i - 2))).collect(),
        Some("let") => arr.iter().enumerate().skip(2).map(|(i, b)| (b, format!("{}.body[{}]", path, i - 2))).collect(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for (position, position_path) in positions {
        match position {
            SExpression::Array(items) if matches!(items.first(), Some(SExpression::Array(_))) => {
                out.extend(items.iter().enumerate().map(|(k, e)| (e, format!("{}[{}]", position_path, k))));
            }
            _ => out.push((position, position_path)),
        }
    }
    out
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
                    let ty = orbital_core_eval::lower::lower_sexpr_keeping_typed(bound)
                        .ok()
                        .and_then(|oir| crate::type_infer::local_initializer_type(&oir, &ctx));
                    match ty {
                        Some(ty) => {
                            scope.locals.insert(name.clone(), ty);
                        }
                        None => {
                            scope.locals.remove(name);
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
            known_services,
            page_init_payloads,
            service_registry,
            declared_emits,
            entity_field_types,
            payload_field_types,
            payload_shapes,
            schema,
            types,
        ));
        if let Some(declared) = declared_emits {
            result.merge(validate_emit_against_declaration(effect, path, declared));
        }
        return result;
    };

    let value = |expr: &SExpression, result: &mut ValidationResult| {
        result.merge(walk_nested_call_services(
            expr,
            path,
            known_services,
            service_registry,
        ));
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
    for (member, at) in nested_effects(effect, path) {
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
    known_services: &HashSet<String>,
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
                    "call-service" | "call_service" => {
                        result.merge(validate_call_service(
                            arr,
                            path,
                            known_services,
                            service_registry,
                            declared_emits,
                        ));
                    }
                    "set" => {
                        result.merge(validate_set(arr, path, declared_emits));
                        result.merge(validate_set_value_type(
                            arr,
                            path,
                            entity_field_types,
                            payload_shapes,
                            schema,
                        ));
                        result.merge(validate_set_enum_value(arr, path, entity_field_types));
                        result.merge(validate_set_return_type(
                            arr,
                            path,
                            entity_field_types,
                            payload_field_types,
                            schema,
                            types,
                        ));
                    }
                    "fetch" => {
                        result.merge(validate_fetch(
                            arr,
                            path,
                            known_entities,
                            entity_definitions,
                            declared_emits,
                        ));
                    }
                    "fetch-stream" => {
                        result.merge(validate_fetch_stream(
                            arr,
                            path,
                            known_entities,
                            declared_emits,
                        ));
                    }
                    "persist" => {
                        result.merge(validate_persist(
                            arr,
                            path,
                            known_entities,
                            entity_definitions,
                            declared_emits,
                        ));
                    }
                    "ref" => {
                        result.merge(validate_ref(arr, path, declared_emits));
                    }
                    "emit" => {
                        result.merge(validate_emit(arr, path));
                    }
                    "render-ui" | "render_ui" => {
                        // render_ui is validated separately in render_ui.rs
                    }
                    "navigate" => {
                        result.merge(validate_navigate(arr, path, page_init_payloads));
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
                        result.merge(validate_send_server(arr, path));
                    }
                    "program/eval" => {
                        result.merge(validate_program_eval(arr, path, declared_emits));
                    }
                    "os/watch-http" => {
                        result.merge(validate_os_watch(arr, path, declared_emits));
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
                                        "browser/beacon" | "browser/prompt" | "browser/embed"
                                    ) || orbital_core_base::tab::TabOp::is_tab_effect(browser_op)
                                    {
                                        &["success", "failure", "cancelled"]
                                    } else {
                                        &["success", "failure"]
                                    };
                                    result.merge(validate_emit_config(
                                        emit_obj,
                                        keys,
                                        declared_emits,
                                        path,
                                    ));
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
        // Object form: { set: { ... } } or { call_service: { ... } }
        SExpression::Object(obj) => {
            if let Some(set_target) = obj.get("set") {
                result.merge(validate_set_object(set_target, path));
            }
            if obj.contains_key("call_service") || obj.contains_key("call-service") {
                result.merge(validate_call_service_object(
                    obj,
                    path,
                    known_services,
                    service_registry,
                ));
            }
            if let Some(fetch_entity) = obj.get("fetch") {
                result.merge(validate_fetch_object(fetch_entity, path, known_entities));
            }
        }
        _ => {
            // Other forms are not effect-like, skip
        }
    }

    // I-8: nested call-service forms inside this effect's arguments get the
    // same name/action/param checks the top-level form gets. The walk starts
    // BELOW the top form so the arms above stay the single validators for it.
    match effect {
        SExpression::Array(arr) => {
            for arg in arr.iter().skip(1) {
                result.merge(walk_nested_call_services(
                    arg,
                    path,
                    known_services,
                    service_registry,
                ));
            }
        }
        SExpression::Object(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                result.merge(walk_nested_call_services(
                    &obj[key],
                    path,
                    known_services,
                    service_registry,
                ));
            }
        }
        _ => {}
    }

    result
}
