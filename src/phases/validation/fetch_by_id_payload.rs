//! `ORB_EFF_FETCH_BY_ID_PAYLOAD_LIST`: `(fetch X { id: … })` delivers ONE row as `payload.data`
//! (std `ui-document-details`: `Event { data : Item }`). A success event that only by-id fetches
//! produce, declared `data : [X]`, types every read of it as a list, and `(array/first ?data)` on
//! the row is null (G-ORB-185). An event a list fetch in the same trait also produces carries both
//! shapes; that class is tracked separately and not reported here.

use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{PayloadField, SExpression, TraitDefinition};
use std::collections::HashSet;

pub(crate) fn validate_fetch_by_id_payload(
    trait_def: &TraitDefinition,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut by_id: Vec<(usize, String)> = Vec::new();
    let mut by_list: HashSet<String> = HashSet::new();
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        for effect in transition.effects.iter().flatten() {
            collect_fetch_successes(effect, &mut |event, keyed| {
                if keyed {
                    by_id.push((i, event.to_string()));
                } else {
                    by_list.insert(event.to_string());
                }
            });
        }
    }
    let mut reported: HashSet<&str> = HashSet::new();
    for (i, event) in &by_id {
        if by_list.contains(event) || reported.contains(event.as_str()) {
            continue;
        }
        let Some(field) = declared_data_field(trait_def, event) else {
            continue;
        };
        if !field.field_type.starts_with('[') {
            continue;
        }
        reported.insert(event.as_str());
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectFetchByIdPayloadList,
                format!("{path}.stateMachine.transitions[{i}].effects"),
                format!(
                    "`(fetch … {{ id: … }})` delivers one row as `payload.data`, but `{event}` declares `data : {}`",
                    field.field_type
                ),
            )
            .with_suggestion(format!(
                "Declare `{event} {{ data : <Row> }}` (a struct type of the fields read) and read it with `(object/get ?data <field> <default>)`"
            )),
        );
    }
    result
}

fn collect_fetch_successes(expr: &SExpression, found: &mut dyn FnMut(&str, bool)) {
    let SExpression::Array(items) = expr else {
        return;
    };
    if matches!(items.first(), Some(SExpression::String(op)) if op == "fetch") {
        for arg in items.iter().skip(2) {
            let SExpression::Object(options) = arg else {
                continue;
            };
            let success = match options.get("emit") {
                Some(SExpression::Object(emit)) => match emit.get("success") {
                    Some(SExpression::String(s)) if !s.is_empty() => s,
                    _ => continue,
                },
                _ => continue,
            };
            found(success, options.contains_key("id"));
        }
    }
    for item in items {
        collect_fetch_successes(item, found);
    }
}

fn declared_data_field<'a>(
    trait_def: &'a TraitDefinition,
    event: &str,
) -> Option<&'a PayloadField> {
    let emits = trait_def
        .emits
        .iter()
        .flatten()
        .filter(|e| e.event == event)
        .filter_map(|e| e.payload_schema.as_ref());
    let events = trait_def
        .state_machine
        .events
        .iter()
        .flatten()
        .filter(|e| e.key == event)
        .filter_map(|e| e.payload_schema.as_ref());
    emits
        .chain(events)
        .flat_map(|fields| fields.iter())
        .find(|f| f.name == "data")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trait_with(fetches: &[(&str, bool)], data_type: &str) -> TraitDefinition {
        let effects: Vec<String> = fetches
            .iter()
            .map(|(event, keyed)| {
                let id = if *keyed {
                    r#", "id": "@payload.id""#
                } else {
                    ""
                };
                format!(r#"["fetch", "Product", {{ "emit": {{ "success": "{event}" }}{id} }}]"#)
            })
            .collect();
        let json = format!(
            r#"{{ "name": "ProductEdit", "emits": [{{ "event": "ProductLoaded", "payloadSchema": [{{ "name": "data", "type": "{data_type}" }}] }}],
                "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
                  "transitions": [{{ "from": "idle", "to": "idle", "event": "EDIT", "effects": [{}] }}] }} }}"#,
            effects.join(", ")
        );
        serde_json::from_str(&json).expect("trait json")
    }

    #[test]
    fn a_by_id_only_success_declared_as_a_list_is_an_error() {
        let r =
            validate_fetch_by_id_payload(&trait_with(&[("ProductLoaded", true)], "[Product]"), "t");
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == ErrorCode::EffectFetchByIdPayloadList),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn control_a_row_declaration_passes() {
        let r =
            validate_fetch_by_id_payload(&trait_with(&[("ProductLoaded", true)], "object"), "t");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn control_a_list_fetch_declared_as_a_list_passes() {
        let r = validate_fetch_by_id_payload(
            &trait_with(&[("ProductLoaded", false)], "[Product]"),
            "t",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn control_an_event_shared_by_list_and_by_id_fetches_is_not_reported_here() {
        let r = validate_fetch_by_id_payload(
            &trait_with(
                &[("ProductLoaded", false), ("ProductLoaded", true)],
                "[Product]",
            ),
            "t",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
