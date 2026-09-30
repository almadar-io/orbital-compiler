//! `ORB_LISTEN_REDUNDANT_EMBED_ROUTE` (error).
//!
//! A control inside an embedded trait emits under every scope on its embed chain
//! (`TraitScopeProvider`'s R-EMBEDDED-EMIT-BUBBLE), so a host that embeds `Child` and
//! declares a transition on `EV` already hears `Child`'s control-fired `EV` as a host
//! transition. A same-name route `listens { Child.EV -> EV }` on that host is the same
//! physical event delivered a second time. A route from a trait the host does NOT embed,
//! from a machine-emitted `(emit EV)` (never bubbled), or to a differently named trigger
//! is a real path and stays silent.
//!
//! Runs POST-inline so `use`-imported atoms are checked too.

use crate::phases::embed_graph::EmbedGraph;
use crate::phases::validation::fired_events::{collect_schema_fired_events, FiredEvents};
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{ListenSource, Orbital, OrbitalSchema, TraitReference};
use std::collections::HashSet;

/// Every ancestor trait name of `trait_name`, following `EmbedGraph::direct_host` upward.
fn host_chain(embed: &EmbedGraph, trait_name: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([trait_name.to_string()]);
    let mut current = trait_name.to_string();
    while let Some(host) = embed.direct_host(&current) {
        if !seen.insert(host.to_string()) {
            break;
        }
        chain.push(host.to_string());
        current = host.to_string();
    }
    chain
}

/// True when a control fires `event` from `source` itself or from a trait embedded inside it.
fn control_fires_under(
    orbital: &Orbital,
    embed: &EmbedGraph,
    source: &str,
    event: &str,
    fired: &FiredEvents,
) -> bool {
    orbital.traits.iter().any(|t| {
        let TraitReference::Inline(td) = t else {
            return false;
        };
        fired
            .by_control
            .contains(&(td.name.clone(), event.to_string()))
            && (td.name == source || host_chain(embed, &td.name).iter().any(|h| h == source))
    })
}

pub(crate) fn validate_listen_redundant_embed_route(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let fired = collect_schema_fired_events(schema);

    for orbital in &schema.orbitals {
        let embed = EmbedGraph::build(&orbital.traits);
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(host) = trait_ref else {
                continue;
            };
            let Some(listens) = &host.listens else {
                continue;
            };
            for listen in listens {
                let Some(ListenSource::Trait {
                    trait_name: source, ..
                }) = &listen.source
                else {
                    continue;
                };
                if listen.triggers.as_deref() != Some(listen.event.as_str()) {
                    continue;
                }
                if !host_chain(&embed, source).iter().any(|h| h == &host.name) {
                    continue;
                }
                if !host
                    .state_machine
                    .transitions
                    .iter()
                    .any(|t| t.event == listen.event)
                {
                    continue;
                }
                if fired
                    .by_effect
                    .contains(&(source.clone(), listen.event.clone()))
                    || !control_fires_under(orbital, &embed, source, &listen.event, &fired)
                {
                    continue;
                }
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ListenRedundantEmbedRoute,
                        format!("orbitals.{}.traits.{}.listens", orbital.name, host.name),
                        format!(
                            "listens route {}.{} -> {} on '{}' is redundant: '{}' embeds '{}', so the control's {} already reaches '{}' as its own transition, and the route delivers the same event a second time",
                            source, listen.event, listen.event, host.name, host.name, source,
                            listen.event, host.name
                        ),
                    )
                    .with_suggestion(format!(
                        "delete the `{}.{} -> {}` listen from '{}'",
                        source, listen.event, listen.event, host.name
                    )),
                );
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(event: &str) -> serde_json::Value {
        serde_json::json!(["render-ui", "main", { "type": "button", "action": event, "label": "Go" }])
    }

    fn machine(event: &str, effects: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "states": [], "events": [], "transitions": [
            { "from": "idle", "to": "idle", "event": event, "effects": effects }
        ] })
    }

    fn host(embeds: &str, listens: serde_json::Value, handles: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "Host", "linkedEntity": "Row", "category": "interaction",
            "config": { "body": { "type": "string", "default": format!("@trait.{embeds}") } },
            "listens": listens,
            "stateMachine": machine(handles, vec![]),
        })
    }

    fn child(
        name: &str,
        effects: Vec<serde_json::Value>,
        embeds: Option<&str>,
    ) -> serde_json::Value {
        let mut t = serde_json::json!({
            "name": name, "linkedEntity": "Row", "category": "interaction",
            "stateMachine": machine("INIT", effects),
        });
        if let Some(inner) = embeds {
            t["config"] = serde_json::json!({ "body": { "type": "string", "default": format!("@trait.{inner}") } });
        }
        t
    }

    fn route(source: &str, event: &str, triggers: &str) -> serde_json::Value {
        serde_json::json!([{ "event": event, "triggers": triggers,
            "source": { "kind": "trait", "trait": source } }])
    }

    fn findings(traits: Vec<serde_json::Value>) -> Vec<ValidationError> {
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "redundant-route", "version": "1.0.0",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Row", "persistence": "persistent",
                    "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": traits, "pages": []
            }]
        }))
        .expect("fixture deserializes");
        validate_listen_redundant_embed_route(&schema)
            .errors
            .into_iter()
            .filter(|e| e.code == ErrorCode::ListenRedundantEmbedRoute)
            .collect()
    }

    #[test]
    fn flags_same_name_route_from_a_control_bearing_embedded_child() {
        let f = findings(vec![
            host("Child", route("Child", "REQUEST", "REQUEST"), "REQUEST"),
            child("Child", vec![button("REQUEST")], None),
        ]);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code.as_str(), "ORB_LISTEN_REDUNDANT_EMBED_ROUTE");
    }

    #[test]
    fn flags_through_a_transitive_embed_with_the_control_in_the_grandchild() {
        let f = findings(vec![
            host("Mid", route("Mid", "REQUEST", "REQUEST"), "REQUEST"),
            child("Mid", vec![], Some("Leaf")),
            child("Leaf", vec![button("REQUEST")], None),
        ]);
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn silent_without_the_route() {
        assert!(findings(vec![
            host("Child", serde_json::json!([]), "REQUEST"),
            child("Child", vec![button("REQUEST")], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_when_the_source_is_not_embedded_by_the_host() {
        assert!(findings(vec![
            host("Other", route("Child", "REQUEST", "REQUEST"), "REQUEST"),
            child("Other", vec![], None),
            child("Child", vec![button("REQUEST")], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_for_a_differently_named_trigger() {
        assert!(findings(vec![
            host("Child", route("Child", "REQUEST", "ADMIT"), "ADMIT"),
            child("Child", vec![button("REQUEST")], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_when_the_event_is_machine_emitted_and_never_bubbles() {
        assert!(findings(vec![
            host("Child", route("Child", "REQUEST", "REQUEST"), "REQUEST"),
            child("Child", vec![serde_json::json!(["emit", "REQUEST"])], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_when_the_host_has_no_transition_on_the_event() {
        assert!(findings(vec![
            host("Child", route("Child", "REQUEST", "REQUEST"), "OTHER"),
            child("Child", vec![button("REQUEST")], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_when_the_embedded_child_has_no_control_for_the_event() {
        assert!(findings(vec![
            host("Child", route("Child", "REQUEST", "REQUEST"), "REQUEST"),
            child("Child", vec![], None),
        ])
        .is_empty());
    }

    #[test]
    fn silent_when_the_child_both_renders_and_emits_the_event() {
        assert!(findings(vec![
            host("Child", route("Child", "REQUEST", "REQUEST"), "REQUEST"),
            child(
                "Child",
                vec![button("REQUEST"), serde_json::json!(["emit", "REQUEST"])],
                None
            ),
        ])
        .is_empty());
    }
}
