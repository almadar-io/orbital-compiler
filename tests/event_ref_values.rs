//! An `event`-typed config value addressed like a `listens` source
//! (`Trait.EVENT` within the orbital, `Orbital.Trait.EVENT` across the program)
//! must name a trait in the program and one of its declared external inputs.

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
                { "key": input, "name": input, "input": true,
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

fn findings(one: &str, tool: &str) -> Vec<(String, String)> {
    let assistant = json!({
        "name": "Assistant",
        "linkedEntity": "Task",
        "config": {
            "one": { "type": "event", "default": one },
            "tools": {
                "type": "[AgentTool]",
                "items": { "type": "object", "properties": {
                    "event": { "name": "event", "type": "event", "required": false }
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
fn a_trait_qualified_input_in_the_same_orbital_resolves() {
    let f = findings("TaskPersistor.DO_CREATE", "TaskPersistor.DO_CREATE");
    assert!(f.is_empty(), "{f:?}");
}

#[test]
fn an_orbital_qualified_input_in_a_sibling_orbital_resolves() {
    let f = findings(
        "NoteOrbital.NotePersistor.DO_ADD",
        "NoteOrbital.NotePersistor.DO_ADD",
    );
    assert!(f.is_empty(), "{f:?}");
}

#[test]
fn a_handled_event_that_is_not_a_declared_input_is_rejected() {
    let f = findings("TaskPersistor.OPEN", "TaskOrbital.TaskPersistor.OPEN");
    assert_eq!(codes(&f), vec![NOT_INPUT, NOT_INPUT], "{f:?}");
    assert!(
        f[0].1.contains("DO_CREATE"),
        "names the declared inputs: {}",
        f[0].1
    );
}

#[test]
fn an_unknown_trait_is_unresolved() {
    let f = findings("Missing.DO_CREATE", "TaskOrbital.Missing.DO_CREATE");
    assert_eq!(codes(&f), vec![UNRESOLVED, UNRESOLVED], "{f:?}");
}

#[test]
fn an_orbital_not_in_the_program_is_unresolved() {
    let f = findings(
        "TaskPersistor.DO_CREATE",
        "Elsewhere.TaskPersistor.DO_CREATE",
    );
    assert_eq!(codes(&f), vec![UNRESOLVED], "{f:?}");
}

#[test]
fn a_trait_qualified_ref_resolves_only_within_its_own_orbital() {
    let f = findings("NotePersistor.DO_ADD", "TaskPersistor.DO_CREATE");
    assert_eq!(codes(&f), vec![UNRESOLVED], "{f:?}");
    assert!(
        f[0].1.contains("NoteOrbital.NotePersistor.DO_ADD"),
        "suggests the qualified form: {}",
        f[0].1
    );
}

#[test]
fn control_a_bare_value_keeps_the_own_trait_membership_check() {
    let f = findings("DO_CREATE", "TaskPersistor.DO_CREATE");
    assert_eq!(codes(&f), vec![INVALID], "{f:?}");
}
