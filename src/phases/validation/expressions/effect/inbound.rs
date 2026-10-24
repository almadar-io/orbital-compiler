//! Inbound deliveries (`listens { EVENT -> inbound "name" answers ANSWER { … } }`). Only the handler's
//! own emit of its answer event answers (EXEC-INBOUND-1), so a sibling's emit of it the handler does
//! not relay is `ORB_INBOUND_ANSWER_FOREIGN`. The entry, undeclared, duplicate, verifies and
//! answer-undeclared checks are the std `.lolo` lint `std-orb-lint-inbound`.

use super::super::fired_events::collect_schema_fired_events;
use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{ListenSource, OrbitalSchema, TraitDefinition};
use std::collections::HashSet;

/// Whether `handler` listens to `emitter`'s `event` (a relay can then re-emit it as the handler's own answer).
fn relays(handler: &TraitDefinition, emitter: &str, event: &str) -> bool {
    handler
        .listens
        .iter()
        .flatten()
        .any(|listen| match &listen.source {
            None | Some(ListenSource::Any) => {
                listen.event == event || listen.event == format!("{emitter}.{event}")
            }
            Some(
                ListenSource::Trait { trait_name, .. } | ListenSource::Orbital { trait_name, .. },
            ) => trait_name == emitter && listen.event == event,
        })
}

pub(crate) fn validate_inbound(schema: &OrbitalSchema) -> ValidationResult {
    let fired = collect_schema_fired_events(schema);
    let answering: HashSet<(String, String)> =
        orbital_core_base::inbound::inbound_routes_of(schema)
            .into_iter()
            .map(|route| (route.trait_name, route.answer))
            .collect();
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        for (i, event) in trait_def.state_machine.events.iter().flatten().enumerate() {
            let Some(inbound) = &event.inbound else {
                continue;
            };
            let at = format!("{path}.stateMachine.events[{i}]");
            let mut foreign: Vec<&str> = fired
                .by_effect
                .iter()
                .filter(|(emitter, event)| {
                    *event == inbound.answers
                        && *emitter != trait_def.name
                        && !answering.contains(&(emitter.clone(), event.clone()))
                        && !relays(trait_def, emitter, event)
                })
                .map(|(emitter, _)| emitter.as_str())
                .collect();
            foreign.sort_unstable();
            foreign.dedup();
            for emitter in foreign {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::InboundAnswerForeign,
                        &at,
                        format!(
                            "trait '{emitter}' emits '{}', the answer of '{}''s inbound hook '{}', but only '{}''s own emit answers the hook, so that emit never reaches the caller",
                            inbound.answers, trait_def.name, inbound.name, trait_def.name
                        ),
                    )
                    .with_suggestion(format!(
                        "Relay it: in '{}' add `listens {{ {emitter}.{} -> <RELAYED> }}` and an arm on <RELAYED> that emits '{}' with the payload's status and body",
                        trait_def.name, inbound.answers, inbound.answers
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
    use serde_json::json;

    fn codes(schema: &OrbitalSchema) -> Vec<ErrorCode> {
        validate_inbound(schema)
            .errors
            .iter()
            .map(|e| e.code)
            .collect()
    }

    /// `Cli` handles `cli-run` answering `DONE`; `Engine` emits `engine_emits` on `GO`. `relay` makes `Cli` listen
    /// to `Engine`'s `DONE` and re-emit it as its own answer.
    fn split_fixture(engine_emits: &str, relay: bool) -> OrbitalSchema {
        let mut cli_transitions = vec![json!({ "from": "idle", "to": "idle", "event": "RUN",
            "effects": [["emit", "GO", {}]] })];
        let mut listens = Vec::new();
        if relay {
            cli_transitions.push(json!({ "from": "idle", "to": "idle", "event": "RELAYED",
                "effects": [["emit", "DONE", { "status": "@payload.status", "body": "@payload.body" }]] }));
            listens.push(json!({ "event": "DONE", "triggers": "RELAYED", "source": { "kind": "trait", "trait": "Engine" } }));
        }
        serde_json::from_value(json!({
            "name": "App",
            "inbound": ["cli-run"],
            "orbitals": [{
                "name": "RunOrbital",
                "entity": { "name": "Req", "collection": "reqs", "persistence": "runtime",
                            "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "Cli",
                    "linkedEntity": "Req",
                    "emits": [{ "event": "DONE" }, { "event": "GO" }],
                    "listens": listens,
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "RUN", "inbound": { "name": "cli-run", "answers": "DONE" } }],
                        "transitions": cli_transitions
                    }
                }, {
                    "name": "Engine",
                    "linkedEntity": "Req",
                    "emits": [{ "event": engine_emits }],
                    "listens": [{ "event": "GO", "triggers": "GO", "source": { "kind": "trait", "trait": "Cli" } }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "GO" }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO",
                            "effects": [["emit", engine_emits, { "status": 200, "body": "ok" }]] }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("fixture deserializes")
    }

    #[test]
    fn an_answer_only_a_sibling_trait_emits_is_an_error() {
        let s = split_fixture("DONE", false);
        assert_eq!(codes(&s), vec![ErrorCode::InboundAnswerForeign]);
        let message = &validate_inbound(&s).errors[0].message;
        assert!(
            message.contains("Engine") && message.contains("cli-run") && message.contains("DONE"),
            "{message}"
        );
    }

    #[test]
    fn control_a_handler_that_relays_the_siblings_answer_is_clean() {
        assert!(codes(&split_fixture("DONE", true)).is_empty());
    }

    #[test]
    fn control_a_sibling_emitting_another_event_is_clean() {
        assert!(codes(&split_fixture("PROGRESS", false)).is_empty());
    }
}
