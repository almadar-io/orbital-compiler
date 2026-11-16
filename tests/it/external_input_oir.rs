//! A declared external input reaches the OIR as `OirEvent.external`, so every
//! codegen backend can build its input table from the IR.

use orbital_compiler::phases::resolve::resolve_to_oir;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::json;

fn schema(events: serde_json::Value) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "App",
        "orbitals": [{
            "name": "TaskOrbital",
            "entity": { "name": "Task", "collection": "tasks", "persistence": "persistent",
                        "fields": [{ "name": "id", "type": "string", "required": true },
                                   { "name": "title", "type": "string" }] },
            "traits": [{
                "name": "TaskPersistor",
                "linkedEntity": "Task",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": events,
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "DO_CREATE" },
                        { "from": "idle", "to": "idle", "event": "SAVE" }
                    ]
                }
            }],
            "pages": []
        }]
    }))
    .expect("fixture deserializes")
}

fn external_of(module: &orbital_core_base::ir::OirModule, key: &str) -> bool {
    module
        .traits
        .iter()
        .flat_map(|t| t.events.iter())
        .find(|e| e.key == key)
        .unwrap_or_else(|| panic!("event {key} missing"))
        .external
        .is_some()
}

#[test]
fn the_oir_record_is_the_shared_input_record() {
    let module = resolve_to_oir(&schema(json!([
        { "key": "DO_CREATE", "name": "Do create", "input": true, "description": "Create a task",
          "payloadSchema": [{ "name": "title", "type": "string", "required": true }] },
        { "key": "SAVE", "name": "Save" }
    ])))
    .expect("resolves");
    let record = module.traits[0].events.iter().find(|e| e.key == "DO_CREATE").unwrap().external.clone().unwrap();
    assert_eq!(
        serde_json::to_value(&record).unwrap(),
        json!({ "orbital": "TaskOrbital", "trait": "TaskPersistor", "event": "DO_CREATE",
                "payloadSchema": [{ "name": "title", "type": "string", "required": true }],
                "description": "Create a task" })
    );
}

#[test]
fn a_declared_input_is_external_in_the_oir() {
    let module = resolve_to_oir(&schema(json!([
        { "key": "DO_CREATE", "name": "Do create", "input": true,
          "payloadSchema": [{ "name": "title", "type": "string", "required": true }] },
        { "key": "SAVE", "name": "Save" }
    ])))
    .expect("resolves");
    assert!(external_of(&module, "DO_CREATE"));
    assert!(!external_of(&module, "SAVE"));
}

#[test]
fn the_flag_survives_a_duplicate_key_merge() {
    // A bare entry first, then the payload-typed external declaration of the same key.
    let module = resolve_to_oir(&schema(json!([
        { "key": "DO_CREATE", "name": "Do create" },
        { "key": "DO_CREATE", "name": "Do create", "input": true,
          "payloadSchema": [{ "name": "title", "type": "string" }] }
    ])))
    .expect("resolves");
    assert!(external_of(&module, "DO_CREATE"));
}
