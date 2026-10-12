//! `ORB_EMBED_BRIDGE_LISTENS_UNSENT` / `ORB_EMBED_BRIDGE_EMITS_UNHEARD`: a behavior value
//! embedded with a declared bridge (`(behavior/apply v { bridge: { listens, emits } })`)
//! delivers the host's `listens` events into the embedded program and re-emits its `emits`
//! events on the host. A bridged event the host never produces, or never consumes, is a
//! dead wire, decidable on the program, so it is an error.
//!
//! Produced = fired anywhere in the program (an `emit` effect or a rendered control,
//! `fired_events`). Consumed = a transition on the event, or a rendered pattern's
//! `event-listen` prop bound to it (a code editor's `onSetMode`), source-tagged against
//! the pattern registry.

use std::collections::HashSet;

use orbital_core::schema::{OrbitalSchema, SExpression, TraitReference};

use super::{
    error::{ErrorCode, ValidationError, ValidationResult},
    fired_events::collect_schema_fired_events,
    ValidationContext,
};
use crate::embedded::patterns;

struct Bridge {
    listens: Vec<String>,
    emits: Vec<String>,
    path: String,
}

fn literal_events(value: Option<&SExpression>) -> Option<Vec<String>> {
    match value {
        Some(SExpression::Array(items)) => items
            .iter()
            .map(|item| match item {
                SExpression::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn collect_bridges(expr: &SExpression, path: &str, out: &mut Vec<Bridge>) {
    match expr {
        SExpression::Array(arr) => {
            if let [SExpression::String(op), _, SExpression::Object(overrides)] = arr.as_slice() {
                if op == "behavior/apply" {
                    if let Some(SExpression::Object(bridge)) = overrides.get("bridge") {
                        if let (Some(listens), Some(emits)) = (
                            literal_events(bridge.get("listens")),
                            literal_events(bridge.get("emits")),
                        ) {
                            out.push(Bridge {
                                listens,
                                emits,
                                path: path.to_string(),
                            });
                        }
                    }
                }
            }
            for item in arr {
                collect_bridges(item, path, out);
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                collect_bridges(value, path, out);
            }
        }
        _ => {}
    }
}

fn collect_listen_bindings(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::Array(arr) => {
            for item in arr {
                collect_listen_bindings(item, out);
            }
        }
        SExpression::Object(obj) => {
            if let Some(SExpression::String(pattern_type)) = obj.get("type") {
                if let Some(pattern) = patterns().get(pattern_type) {
                    for (prop, value) in obj {
                        let listens = pattern
                            .props_schema
                            .get(prop)
                            .is_some_and(|p| p.kind.as_deref() == Some("event-listen"));
                        if let (true, SExpression::String(event)) = (listens, value) {
                            out.insert(event.clone());
                        }
                    }
                }
            }
            for value in obj.values() {
                collect_listen_bindings(value, out);
            }
        }
        _ => {}
    }
}

/// Every event a bridged embed in these traits listens for: delivered into the embed, so a
/// control firing one is handled (`ORB_EMIT_CONTROL_UNHANDLED`).
pub(crate) fn bridged_listens<'a>(
    traits: impl IntoIterator<Item = &'a orbital_core::schema::TraitDefinition>,
) -> HashSet<String> {
    let mut bridges = Vec::new();
    for td in traits {
        for transition in &td.state_machine.transitions {
            for effect in transition.effects.iter().flatten() {
                collect_bridges(effect, "", &mut bridges);
            }
        }
    }
    bridges.into_iter().flat_map(|b| b.listens).collect()
}

/// Every bridge declared in a render tree, checked against what the program produces and consumes.
pub fn validate_embed_bridges(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(check_embed_bridges(schema));
}

pub(crate) fn check_embed_bridges(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut bridges = Vec::new();
    let mut consumed: HashSet<String> = HashSet::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for (xi, transition) in td.state_machine.transitions.iter().enumerate() {
                consumed.insert(transition.event.clone());
                let path = format!("orbitals[{oi}].traits[{ti}].stateMachine.transitions[{xi}]");
                for effect in transition.effects.iter().flatten() {
                    collect_bridges(effect, &path, &mut bridges);
                    collect_listen_bindings(effect, &mut consumed);
                }
            }
        }
    }
    if bridges.is_empty() {
        return result;
    }
    let fired = collect_schema_fired_events(schema);
    for bridge in &bridges {
        for event in &bridge.listens {
            if !fired.contains_any_trait(event) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EmbedBridgeListensUnsent,
                        &bridge.path,
                        format!("the embed's bridge delivers '{event}' into it, but nothing in this program sends '{event}'"),
                    )
                    .with_suggestion(format!("Send '{event}' from the host (an emit effect or a control), or remove it from the bridge's `listens`")),
                );
            }
        }
        for event in &bridge.emits {
            if !consumed.contains(event) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EmbedBridgeEmitsUnheard,
                        &bridge.path,
                        format!("the embed's bridge re-emits '{event}' on the host, but nothing in this program handles '{event}'"),
                    )
                    .with_suggestion(format!("Handle '{event}' on the host (a transition or a pattern that listens for it), or remove it from the bridge's `emits`")),
                );
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(render: serde_json::Value, extra_transitions: serde_json::Value) -> OrbitalSchema {
        let mut transitions = vec![
            serde_json::json!({ "from": "idle", "to": "idle", "event": "INIT", "effects": [["render-ui", "main", render]] }),
        ];
        transitions.extend(extra_transitions.as_array().cloned().unwrap_or_default());
        serde_json::from_value(serde_json::json!({
            "name": "Host",
            "orbitals": [{
                "name": "Host",
                "entity": { "name": "HostView", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [{
                    "name": "Workspace",
                    "linkedEntity": "HostView",
                    "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": transitions }
                }],
                "pages": []
            }]
        }))
        .unwrap()
    }

    fn embed(listens: &[&str], emits: &[&str]) -> serde_json::Value {
        serde_json::json!({ "type": "box", "children": [["behavior/apply", "@entity.plugin", { "bridge": { "listens": listens, "emits": emits } }]] })
    }

    fn codes(result: &ValidationResult) -> Vec<ErrorCode> {
        result.errors.iter().map(|e| e.code.clone()).collect()
    }

    #[test]
    fn a_bridged_event_the_host_sends_and_one_it_handles_are_fine() {
        let render = serde_json::json!({ "type": "stack", "children": [
            embed(&["GO"], &["DONE"]),
            { "type": "button", "label": "Go", "action": "GO" }
        ] });
        let handled = serde_json::json!([{ "from": "idle", "to": "idle", "event": "DONE" }, { "from": "idle", "to": "idle", "event": "GO" }]);
        assert!(codes(&check_embed_bridges(&schema(render, handled))).is_empty());
    }

    #[test]
    fn a_listens_event_nothing_sends_is_an_error() {
        let handled = serde_json::json!([{ "from": "idle", "to": "idle", "event": "DONE" }]);
        assert_eq!(
            codes(&check_embed_bridges(&schema(
                embed(&["GO"], &["DONE"]),
                handled
            ))),
            vec![ErrorCode::EmbedBridgeListensUnsent]
        );
    }

    #[test]
    fn an_emits_event_nothing_handles_is_an_error() {
        let render = serde_json::json!({ "type": "stack", "children": [embed(&[], &["DONE"])] });
        assert_eq!(
            codes(&check_embed_bridges(&schema(render, serde_json::json!([])))),
            vec![ErrorCode::EmbedBridgeEmitsUnheard]
        );
    }

    #[test]
    fn a_pattern_listening_for_an_emitted_event_consumes_it() {
        let render = serde_json::json!({ "type": "stack", "children": [
            embed(&[], &["SET_MODE"]),
            { "type": "code-block", "code": "", "editable": true, "onSetMode": "SET_MODE" }
        ] });
        assert!(codes(&check_embed_bridges(&schema(render, serde_json::json!([])))).is_empty());
    }

    #[test]
    fn control_a_program_with_no_bridged_embed_reports_nothing() {
        let render = serde_json::json!({ "type": "box", "children": [["behavior/apply", "@entity.widget", { "config": { "x": 1 } }]] });
        assert!(codes(&check_embed_bridges(&schema(render, serde_json::json!([])))).is_empty());
    }
}
