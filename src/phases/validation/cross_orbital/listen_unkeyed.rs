//! ORB_LISTEN_WILDCARD_UNKEYED (warning).
//!
//! A `listens { *.EVENT -> TRIGGER }` route hears EVERY emitter of `EVENT`. When the event's declared
//! payload carries a `CallKey`-typed `key` (the key of the call that produced it), a route whose guard
//! never reads that key delivers other callers' replies and failures to this trait. Only a trait that
//! makes call keys (an `(as … CallKey)` in its own definition) is a caller; a service that echoes the
//! key it was handed hears every caller's request on purpose. Declared contract only: an emitter's
//! `emits` payload schema, the route's recorded `source`, whether its `guard` reads `@payload.key`,
//! and whether the listening trait makes a `CallKey`.

use crate::phases::validation::{ErrorCode, Severity, ValidationError, ValidationResult};
use orbital_core_base::schema::{ListenSource, OrbitalSchema, SExpression, TraitReference};
use std::collections::HashSet;

fn reads_key(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) => s == "@payload.key" || s.starts_with("@payload.key."),
        SExpression::Array(items) => items.iter().any(reads_key),
        SExpression::Object(map) => map.values().any(reads_key),
        _ => false,
    }
}

fn makes_call_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            let is_cast = items.len() == 3
                && items[0].as_str() == Some("as")
                && items[2].as_str() == Some("CallKey");
            is_cast || items.iter().any(makes_call_key)
        }
        serde_json::Value::Object(map) => map.values().any(makes_call_key),
        _ => false,
    }
}

pub(crate) fn validate_wildcard_listen_keyed(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut keyed_events: HashSet<&str> = HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for emit in td.emits.iter().flatten() {
                let carries_call_key = emit
                    .payload_schema
                    .iter()
                    .flatten()
                    .any(|f| f.name == "key" && f.field_type == "CallKey");
                if carries_call_key {
                    keyed_events.insert(emit.event.as_str());
                }
            }
        }
    }
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let is_caller = serde_json::to_value(td).is_ok_and(|v| makes_call_key(&v));
            if !is_caller {
                continue;
            }
            for listen in td.listens.iter().flatten() {
                if !matches!(listen.source, Some(ListenSource::Any))
                    || !keyed_events.contains(listen.event.as_str())
                    || listen.guard.as_ref().is_some_and(reads_key)
                {
                    continue;
                }
                let triggered = listen.triggers.as_deref().unwrap_or(listen.event.as_str());
                result.add_error(ValidationError {
                    code: ErrorCode::ListenWildcardUnkeyed,
                    message: format!(
                        "listens route `*.{} -> {}` in `{}.{}` hears every caller's `{}`, whose payload carries the call's `key`, and its guard never reads it",
                        listen.event, triggered, orbital.name, td.name, listen.event
                    ),
                    path: format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                    severity: Severity::Warning,
                    suggestion: Some(format!(
                        "guard the route on the call's own key: `*.{} -> {} when (= ?key <the key this trait sent>)`",
                        listen.event, triggered
                    )),
                    valid_values: None,
                    expected_shape: None,
                    fix_guidance: None,
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program(
        key_type: &str,
        source: serde_json::Value,
        guard: Option<serde_json::Value>,
    ) -> OrbitalSchema {
        let mut listen =
            json!({ "event": "KNOWLEDGE_FAILED", "triggers": "LOAD_FAILED", "source": source });
        if let Some(g) = guard {
            listen["guard"] = g;
        }
        serde_json::from_value(json!({
            "name": "p",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [
                    { "name": "Provider", "linkedEntity": "E",
                      "emits": [{ "event": "KNOWLEDGE_FAILED", "payloadSchema": [{ "name": "key", "type": key_type }] }],
                      "stateMachine": { "states": [{ "name": "on", "isInitial": true }], "transitions": [] } },
                    { "name": "Consumer", "linkedEntity": "E",
                      "listens": [listen],
                      "stateMachine": { "states": [{ "name": "on", "isInitial": true }], "transitions": [
                          { "from": "on", "to": "on", "event": "ASK", "effects": [["emit", "KNOWLEDGE_ASK", { "key": ["as", "c|ask", "CallKey"] }]] }
                      ] } }
                ]
            }]
        }))
        .unwrap()
    }

    fn any() -> serde_json::Value {
        json!({ "kind": "any" })
    }

    #[test]
    fn an_unguarded_wildcard_listen_on_a_call_keyed_event_is_flagged() {
        let result = validate_wildcard_listen_keyed(&program("CallKey", any(), None));
        assert_eq!(result.errors.len() + result.warnings.len(), 1);
    }

    #[test]
    fn a_guard_that_does_not_read_the_key_is_flagged() {
        let guard = json!(["=", "@payload.action", "x"]);
        let result = validate_wildcard_listen_keyed(&program("CallKey", any(), Some(guard)));
        assert_eq!(result.errors.len() + result.warnings.len(), 1);
    }

    #[test]
    fn control_a_keyed_guard_is_accepted() {
        let guard = json!(["=", "@payload.key", ["str/concat", "a", "|b"]]);
        let result = validate_wildcard_listen_keyed(&program("CallKey", any(), Some(guard)));
        assert_eq!(result.errors.len() + result.warnings.len(), 0);
    }

    #[test]
    fn control_an_explicit_source_listen_is_not_flagged() {
        let source = json!({ "kind": "trait", "trait": "Provider" });
        let result = validate_wildcard_listen_keyed(&program("CallKey", source, None));
        assert_eq!(result.errors.len() + result.warnings.len(), 0);
    }

    #[test]
    fn control_an_event_without_a_call_key_key_is_not_flagged() {
        let result = validate_wildcard_listen_keyed(&program("string", any(), None));
        assert_eq!(result.errors.len() + result.warnings.len(), 0);
    }

    #[test]
    fn control_a_service_that_only_echoes_keys_is_not_flagged() {
        let program: OrbitalSchema = serde_json::from_value(json!({
            "name": "p",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [
                    { "name": "Caller", "linkedEntity": "E",
                      "emits": [{ "event": "KNOWLEDGE_ASK", "payloadSchema": [{ "name": "key", "type": "CallKey" }] }],
                      "stateMachine": { "states": [{ "name": "on", "isInitial": true }], "transitions": [] } },
                    { "name": "Service", "linkedEntity": "E",
                      "listens": [{ "event": "KNOWLEDGE_ASK", "triggers": "SERVE", "source": { "kind": "any" } }],
                      "stateMachine": { "states": [{ "name": "on", "isInitial": true }], "transitions": [
                          { "from": "on", "to": "on", "event": "SERVE", "effects": [["emit", "KNOWLEDGE_ASK_RESULT", { "key": "@payload.key" }]] }
                      ] } }
                ]
            }]
        }))
        .unwrap();
        let result = validate_wildcard_listen_keyed(&program);
        assert_eq!(result.errors.len() + result.warnings.len(), 0);
    }
}
