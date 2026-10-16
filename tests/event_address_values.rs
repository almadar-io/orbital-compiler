//! `EventAddress`: a value that must ADDRESS a declared external input
//! (`Trait.EVENT` within the orbital, `Orbital.Trait.EVENT` across the program) —
//! the contract `llm call-tools` reads. Unlike `event`, a bare name is never one.

use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

const UNRESOLVED: &str = "ORB_T_EVENT_REF_UNRESOLVED";
const NOT_INPUT: &str = "ORB_T_EVENT_REF_NOT_EXTERNAL_INPUT";
const INVALID: &str = "ORB_T_INVALID_EVENT_DEFAULT";

fn persistor(name: &str, input: &str) -> Value {
    json!({
        "name": name,
        "linkedEntity": "Task",
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }, { "name": "open" }],
            "events": [
                { "key": "INIT", "name": "Init" },
                { "key": "OPEN", "name": "Open" },
                { "key": input, "name": input, "external": true,
                  "payloadSchema": [{ "name": "title", "type": "string", "required": true }] }
            ],
            "transitions": [
                { "from": "idle", "to": "idle", "event": "INIT" },
                { "from": "idle", "to": "open", "event": "OPEN" },
                { "from": "open", "to": "idle", "event": "INIT" },
                { "from": "idle", "to": "idle", "event": input }
            ]
        }
    })
}

fn findings(member_type: &str, tool: &str) -> Vec<(String, String)> {
    findings_value(member_type, json!(tool))
}

fn findings_value(member_type: &str, tool: Value) -> Vec<(String, String)> {
    let assistant = json!({
        "name": "Assistant",
        "linkedEntity": "Task",
        "config": {
                        "tools": {
                "type": "[AgentTool]",
                "items": { "type": "object", "properties": {
                    "event": { "name": "event", "type": member_type, "required": false }
                } },
                "default": [{ "event": tool }]
            }
        },
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT", "name": "Init" }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
        }
    });
    let entity = json!({
        "name": "Task", "collection": "tasks", "persistence": "persistent",
        "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" }]
    });
    let schema = json!({
        "name": "App",
        "version": "1.0.0",
        "orbitals": [
            { "name": "TaskOrbital", "entity": entity.clone(),
              "traits": [persistor("TaskPersistor", "DO_CREATE"), assistant],
              "pages": [{ "name": "TaskPage", "path": "/", "traits": [{ "ref": "TaskPersistor" }, { "ref": "Assistant" }] }] },
            { "name": "NoteOrbital", "entity": entity,
              "traits": [persistor("NotePersistor", "DO_ADD")],
              "pages": [{ "name": "NotePage", "path": "/notes", "traits": [{ "ref": "NotePersistor" }] }] }
        ]
    });
    let schema: OrbitalSchema = serde_json::from_value(schema).expect("fixture deserializes");
    let result = validate_comprehensive_without_services(&schema);
    result
        .errors
        .iter()
        .chain(result.warnings.iter())
        .filter(|e| [UNRESOLVED, NOT_INPUT, INVALID].contains(&e.code.as_str()))
        .filter(|e| e.path.contains("Assistant"))
        .map(|e| {
            let suggestion = e.suggestion.clone().unwrap_or_default();
            (
                e.code.as_str().to_string(),
                format!("{} | {}", e.message, suggestion),
            )
        })
        .collect()
}

fn codes(f: &[(String, String)]) -> Vec<&str> {
    f.iter().map(|(c, _)| c.as_str()).collect()
}

#[test]
fn a_declared_input_address_is_clean() {
    let f = findings("EventAddress", "TaskPersistor.DO_CREATE");
    assert!(f.is_empty(), "{f:?}");
    let f = findings("EventAddress", "NoteOrbital.NotePersistor.DO_ADD");
    assert!(f.is_empty(), "{f:?}");
}

#[test]
fn a_bare_event_is_not_an_address() {
    let f = findings("EventAddress", "INIT");
    assert_eq!(codes(&f), vec![UNRESOLVED], "{f:?}");
}

#[test]
fn an_address_of_a_handled_event_that_is_not_an_input_is_rejected() {
    let f = findings("EventAddress", "TaskPersistor.OPEN");
    assert_eq!(codes(&f), vec![NOT_INPUT], "{f:?}");
}

#[test]
fn a_non_string_value_is_rejected() {
    let f = findings_value("EventAddress", json!(7));
    assert_eq!(codes(&f), vec![INVALID], "{f:?}");
}

#[test]
fn control_an_event_member_keeps_its_bare_name_rule() {
    let f = findings("event", "INIT");
    assert!(f.is_empty(), "{f:?}");
}

#[test]
fn control_bindings_and_the_empty_outlet_defer() {
    assert!(findings("EventAddress", "@config.other").is_empty());
    assert!(findings("EventAddress", "").is_empty());
}
