//! `browser/tab-*` literal requests: a request whose keys the op declares and whose values hold nothing only run
//! time knows is parsed as the kernel's one tab-call parser reads it. The request's shape (an object or text from
//! data, the op's closed key set) is the std lint `browser-effects`.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition};
use orbital_core_base::value::Value;

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

pub(super) fn visit_effects(
    trait_def: &TraitDefinition,
    path: &str,
    visit: &mut dyn FnMut(&SExpression, &str),
) {
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(guard) = &transition.guard {
            visit(
                guard,
                &format!("{path}.stateMachine.transitions[{i}].guard"),
            );
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

pub(super) fn each_call_of<'a>(
    expr: &'a SExpression,
    head: &str,
    visit: &mut dyn FnMut(&'a [SExpression]),
) {
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

/// Whether `value` carries nothing only run time knows: no binding, no operator call.
fn is_literal(value: &SExpression) -> bool {
    match value {
        SExpression::String(text) => !is_dynamic(text),
        SExpression::Array(items) => {
            !matches!(items.first(), Some(SExpression::String(_))) && items.iter().all(is_literal)
        }
        SExpression::Object(fields) => fields.values().all(is_literal),
        _ => true,
    }
}

/// A literal `browser/tab-<op>` request the kernel's tab-call parser refuses.
fn check_tab_request(
    arr: &[SExpression],
    path: &str,
    scope: &orbital_core_base::tab_call::WatchScope<'_>,
    result: &mut ValidationResult,
) {
    use orbital_core_base::tab::TabOp;
    let head = arr
        .first()
        .and_then(SExpression::as_str)
        .unwrap_or_default();
    let Some(op) = TabOp::from_name(head) else {
        return;
    };
    let Some(SExpression::Object(request)) = arr.get(1) else {
        return;
    };
    if request
        .keys()
        .any(|k| !op.request_keys().contains(&k.as_str()))
        || !is_literal(&arr[1])
    {
        return;
    }
    let value = Value::from(arr[1].clone());
    if let Err(message) = orbital_core_base::tab_call::parse_tab_call(op, &value, Some(scope), None)
    {
        result.add_error(ValidationError::error(
            ErrorCode::TabRequestRefused,
            path,
            message,
        ));
    }
}

/// Literal-request checks for every `browser/tab-*` call.
pub(crate) fn validate_browser_calls(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let inputs = orbital_core_base::external_inputs::external_inputs_of(schema);
    for (trait_def, path) in inline_traits(schema) {
        let orbital = schema.orbitals.iter().find(|o| {
            o.traits.iter().any(|t| matches!(t, orbital_core_base::schema::TraitReference::Inline(td) if std::ptr::eq(&**td, trait_def)))
        });
        let scope = orbital_core_base::tab_call::WatchScope {
            caller_orbital: orbital.map(|o| o.name.as_str()),
            caller_trait: trait_def.name.as_str(),
            inputs: &inputs,
        };
        visit_effects(trait_def, &path, &mut |expr, at| {
            for op in orbital_core_base::tab::TabOp::ALL {
                each_call_of(expr, op.name(), &mut |arr| {
                    check_tab_request(arr, at, &scope, &mut result)
                });
            }
        });
    }
    result
}

/// A transition or tick whose delegated effects reach browser-stored rows and server work in one run: the
/// client-role kernel refuses it (EXEC-DIST-6), classified by core's `LegTouch`.
pub(crate) fn validate_dispatch_residence(
    schema: &OrbitalSchema,
    entities: &std::collections::HashMap<String, orbital_core_base::schema::EntityDefinition>,
) -> ValidationResult {
    use orbital_core_base::runs_on::{expr_leg_touches, Residence};
    use std::collections::BTreeSet;
    let mut result = ValidationResult::new();
    let residence = Residence::of_entities(entities.values());
    for (trait_def, path) in inline_traits(schema) {
        let arms = trait_def
            .state_machine
            .transitions
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    format!("{path}.stateMachine.transitions[{i}]"),
                    t.event.as_str(),
                    t.effects.as_deref().unwrap_or_default(),
                )
            })
            .chain(trait_def.ticks.iter().flatten().enumerate().map(|(i, t)| {
                (
                    format!("{path}.ticks[{i}]"),
                    t.name.as_str(),
                    t.effects.as_slice(),
                )
            }));
        for (at, event, effects) in arms {
            let touches = effects.iter().fold(
                BTreeSet::from([orbital_core_base::runs_on::LegTouch::NONE]),
                |acc, e| {
                    let next = expr_leg_touches(e, &residence);
                    acc.iter()
                        .flat_map(|a| next.iter().map(move |b| a.then(*b)))
                        .collect()
                },
            );
            if touches.iter().any(|t| t.is_mixed()) {
                result.add_error(ValidationError::error(
                    ErrorCode::DispatchMixedResidence,
                    &at,
                    format!(
                        "one dispatch of {} on {event} reaches browser-stored data and server work; split it with an emit and a listen",
                        trait_def.name
                    ),
                ));
            }
        }
    }
    result
}
