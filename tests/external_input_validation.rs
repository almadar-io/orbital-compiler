//! A declared external input (`listens { EVENT -> external { … } }`, lowered as
//! `stateMachine.events[].input`) is a complete command: the trait must accept it
//! from its initial (resting) state. An input only valid mid-flow, or never handled,
//! is `ORB_T_EXTERNAL_INPUT_NOT_RESTING`.

use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

const CODE: &str = "ORB_T_EXTERNAL_INPUT_NOT_RESTING";

fn findings(transitions: Value) -> Vec<String> {
    let schema = json!({
        "name": "App",
        "version": "1.0.0",
        "orbitals": [{
            "name": "TaskOrbital",
            "entity": {
                "name": "Task", "collection": "tasks", "persistence": "persistent",
                "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "title", "type": "string" }
                ]
            },
            "traits": [{
                "name": "TaskPersistor",
                "scope": "instance",
                "linkedEntity": "Task",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }, { "name": "open" }],
                    "events": [
                        { "key": "INIT", "name": "Init" },
                        { "key": "OPEN", "name": "Open" },
                        { "key": "DO_CREATE", "name": "Do create", "input": true,
                          "payloadSchema": [{ "name": "title", "type": "string", "required": true }] }
                    ],
                    "transitions": transitions
                }
            }],
            "pages": [{ "name": "TaskPage", "path": "/", "traits": [{ "ref": "TaskPersistor" }] }]
        }]
    });
    let schema: OrbitalSchema = serde_json::from_value(schema).expect("fixture deserializes");
    validate_comprehensive_without_services(&schema)
        .errors
        .iter()
        .chain(validate_comprehensive_without_services(&schema).warnings.iter())
        .filter(|e| e.code.as_str() == CODE)
        .map(|e| e.message.clone())
        .collect()
}

fn base() -> Vec<Value> {
    vec![
        json!({ "from": "idle", "to": "idle", "event": "INIT" }),
        json!({ "from": "idle", "to": "open", "event": "OPEN" }),
        json!({ "from": "open", "to": "idle", "event": "INIT" }),
    ]
}

#[test]
fn an_input_handled_from_the_initial_state_passes() {
    let mut t = base();
    t.push(json!({ "from": "idle", "to": "idle", "event": "DO_CREATE",
                   "effects": [["persist", "create", "Task", "@payload"]] }));
    assert!(findings(Value::Array(t)).is_empty());
}

#[test]
fn an_input_handled_from_any_state_passes() {
    let mut t = base();
    t.push(json!({ "from": "*", "to": "idle", "event": "DO_CREATE" }));
    assert!(findings(Value::Array(t)).is_empty());
}

#[test]
fn an_input_handled_only_mid_flow_is_rejected() {
    let mut t = base();
    t.push(json!({ "from": "open", "to": "idle", "event": "DO_CREATE" }));
    let f = findings(Value::Array(t));
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(
        f[0].contains("DO_CREATE") && f[0].contains("idle"),
        "{}",
        f[0]
    );
}

#[test]
fn an_input_never_handled_is_rejected() {
    let f = findings(Value::Array(base()));
    assert_eq!(f.len(), 1, "{f:?}");
}

#[test]
fn control_a_non_input_event_handled_mid_flow_is_fine() {
    let mut t = base();
    t.push(json!({ "from": "idle", "to": "idle", "event": "DO_CREATE" }));
    t.push(json!({ "from": "open", "to": "idle", "event": "SAVE" }));
    assert!(findings(Value::Array(t)).is_empty());
}
