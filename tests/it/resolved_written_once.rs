//! The resolved program's text (`orb resolve -o`, a lazy page beside it) writes each repeated list or record once
//! (`$def`, G-CROSS-637) at any position, and reads back as the program it was written from.

use orbital_compiler::phases::lazy::{resolved_json, resolved_tree};
use orbital_core_base::def_refs::DEF_MARKER;
use orbital_core_base::json_value::from_json_value;
use orbital_core_base::schema::{parse_schema, OrbitalSchema};

fn program(second_event: &str) -> OrbitalSchema {
    let transition = |event: &str| serde_json::json!({ "from": "idle", "to": "idle", "event": event, "effects": [["set", format!("@entity.{}", event.to_lowercase()), { "big": [1, 2, 3, 4, 5, 6, 7, 8] }]] });
    from_json_value(&serde_json::json!({
        "name": "app",
        "orbitals": [{ "name": "A", "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] }, "traits": [{ "name": "T", "linkedEntity": "E", "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [transition("X"), transition(second_event)]
        } }] }]
    }))
    .unwrap()
}

#[test]
fn a_repeated_transition_is_written_once_and_reads_back_as_the_resolved_program() {
    let resolved = program("X");
    let text = resolved_json(&resolved).unwrap();
    let written: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(written["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][1].get(DEF_MARKER).is_some(), "{text}");
    let back = parse_schema(&text).unwrap();
    assert_eq!(resolved_tree(&back).unwrap(), resolved_tree(&resolved).unwrap());
}

#[test]
fn control_a_resolved_program_with_no_repeat_is_written_as_its_plain_tree() {
    let resolved: OrbitalSchema = from_json_value(&serde_json::json!({ "name": "app", "orbitals": [{ "name": "A", "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] }, "traits": [] }] })).unwrap();
    let text = resolved_json(&resolved).unwrap();
    assert!(!text.contains(DEF_MARKER), "{text}");
    assert_eq!(text, serde_json::to_string(&resolved_tree(&resolved).unwrap()).unwrap());
}

#[test]
fn the_compiled_bundle_writes_a_repeated_transition_once_and_reads_back_as_the_program() {
    let compiled = program("X");
    let text = orbital_compiler::phases::lazy::program_json(&compiled).unwrap();
    assert!(text.contains(DEF_MARKER), "{text}");
    let back = parse_schema(&text).unwrap();
    assert_eq!(orbital_core_eval::util::canonical_json::to_canonical_string(&back).unwrap(), orbital_core_eval::util::canonical_json::to_canonical_string(&compiled).unwrap());
}

#[test]
fn control_a_compiled_bundle_with_no_repeat_is_its_canonical_text() {
    let compiled: OrbitalSchema = from_json_value(&serde_json::json!({ "name": "app", "orbitals": [{ "name": "A", "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] }, "traits": [] }] })).unwrap();
    assert_eq!(orbital_compiler::phases::lazy::program_json(&compiled).unwrap(), orbital_core_eval::util::canonical_json::to_canonical_string(&compiled).unwrap());
}
