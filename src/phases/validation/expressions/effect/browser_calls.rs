//! `browser/page-facts`, `browser/prompt`, `browser/embed` and `browser/tab-*` call validation: the `key` and `emit`
//! options and, for a tab op, its request.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::value::Value;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition};

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

struct Site<'a> {
    path: &'a str,
    declared: &'a [DeclaredEmit],
}

pub(super) fn visit_effects(trait_def: &TraitDefinition, path: &str, visit: &mut dyn FnMut(&SExpression, &str)) {
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(guard) = &transition.guard {
            visit(guard, &format!("{path}.stateMachine.transitions[{i}].guard"));
        }
        for (j, effect) in transition.effects.iter().flatten().enumerate() {
            visit(
                effect,
                &format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
            );
        }
    }
    for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
        if let Some(guard) = &tick.guard {
            visit(guard, &format!("{path}.ticks[{i}].guard"));
        }
        for (j, effect) in tick.effects.iter().enumerate() {
            visit(effect, &format!("{path}.ticks[{i}].effects[{j}]"));
        }
    }
}

pub(super) fn each_call_of<'a>(expr: &'a SExpression, head: &str, visit: &mut dyn FnMut(&'a [SExpression])) {
    match expr {
        SExpression::Array(arr) => {
            if matches!(arr.first(), Some(SExpression::String(h)) if h == head) {
                visit(arr);
            }
            for item in arr {
                each_call_of(item, head, visit);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                each_call_of(v, head, visit);
            }
        }
        _ => {}
    }
}

/// `browser/page-facts`: the `key` and `emit` options.
fn check_page_facts_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    if let Some(SExpression::Object(options)) = arr.get(2) {
        result.merge(check_key_and_cancelled(options, site.path, "browser/page-facts"));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure"], Some(site.declared), site.path));
        }
    }
}

/// `browser/prompt` / `browser/embed`: the `key` and `emit` options.
fn check_on_device_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let head = arr.first().and_then(SExpression::as_str).unwrap_or_default();
    let positional = if head == "browser/prompt" { 2 } else { 1 };
    if let Some(SExpression::Object(options)) = arr.get(positional + 1) {
        result.merge(check_key_and_cancelled(options, site.path, head));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure", "cancelled"], Some(site.declared), site.path));
        }
    }
}

/// Whether `value` carries nothing only run time knows: no binding, no operator call.
fn is_literal(value: &SExpression) -> bool {
    match value {
        SExpression::String(text) => !is_dynamic(text),
        SExpression::Array(items) => !matches!(items.first(), Some(SExpression::String(_))) && items.iter().all(is_literal),
        SExpression::Object(fields) => fields.values().all(is_literal),
        _ => true,
    }
}

/// `browser/tab-<op>`: a request object of the op's closed keys, a closed `{ key, emit }` options object
/// whose emit routes are `success`, `failure` and `cancelled`, a string `key`, and, when the request holds nothing only
/// run time knows, the whole request as the kernel's one parser reads it.
fn check_tab_call(
    arr: &[SExpression],
    site: &Site,
    scope: &orbital_core_base::tab_call::WatchScope<'_>,
    result: &mut ValidationResult,
) {
    use orbital_core_base::tab::TabOp;
    let head = arr.first().and_then(SExpression::as_str).unwrap_or_default();
    let Some(op) = TabOp::from_name(head) else { return };
    let suggestion = format!(
        "Write ({head} {{ {} }} {{ key: <string>, emit: {{ success: <EVENT>, failure: <EVENT>, cancelled: <EVENT> }} }})",
        op.request_keys().join(", ")
    );
    match arr.get(1) {
        None => {}
        Some(SExpression::Object(request)) => {
            if let Some(unknown) = request.keys().find(|k| !op.request_keys().contains(&k.as_str())) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TabRequestInvalid,
                        site.path,
                        format!("`{head}` request has no key `{unknown}`"),
                    )
                    .with_valid_values(op.request_keys().iter().map(|k| k.to_string()).collect()),
                );
            } else if is_literal(&arr[1]) {
                let value = Value::from(arr[1].clone());
                if let Err(message) = orbital_core_base::tab_call::parse_tab_call(op, &value, Some(scope), None) {
                    result.add_error(ValidationError::error(ErrorCode::TabRequestInvalid, site.path, message));
                }
            }
        }
        Some(SExpression::String(text)) if is_dynamic(text) => {}
        Some(_) => result.add_error(
            ValidationError::error(ErrorCode::TabRequestInvalid, site.path, format!("`{head}` request must be an object"))
                .with_suggestion(suggestion),
        ),
    }
    if let Some(SExpression::Object(options)) = arr.get(2) {
        result.merge(check_key_and_cancelled(options, site.path, head));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure", "cancelled"], Some(site.declared), site.path));
        }
    }
}

/// Keyed-call and emit checks for every `browser/*` host call.
pub(crate) fn validate_browser_calls(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let inputs = orbital_core_base::external_inputs::external_inputs_of(schema);
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        let orbital = schema.orbitals.iter().find(|o| {
            o.traits.iter().any(|t| matches!(t, orbital_core_base::schema::TraitReference::Inline(td) if std::ptr::eq(&**td, trait_def)))
        });
        let scope = orbital_core_base::tab_call::WatchScope {
            caller_orbital: orbital.map(|o| o.name.as_str()),
            caller_trait: trait_def.name.as_str(),
            inputs: &inputs,
        };
        visit_effects(trait_def, &path, &mut |expr, at| {
            let site = Site {
                path: at,
                declared: &declared,
            };
            each_call_of(expr, "browser/page-facts", &mut |arr| check_page_facts_call(arr, &site, &mut result));
            for head in super::keyed_call::ON_DEVICE_OPERATORS {
                each_call_of(expr, head, &mut |arr| check_on_device_call(arr, &site, &mut result));
            }
            for op in orbital_core_base::tab::TabOp::ALL {
                each_call_of(expr, op.name(), &mut |arr| check_tab_call(arr, &site, &scope, &mut result));
            }
        });
    }
    result
}
