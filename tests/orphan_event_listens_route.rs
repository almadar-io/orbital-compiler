//! `CIRCUIT_ORPHAN_EVENT` must count `listens` routes, not just transitions.
//!
//! The bus broadcasts orbital-wide, so an action event a SIBLING handles closes
//! the circuit. That was already true for a sibling TRANSITION on the same
//! event name — but a `listens` route consumes the event under its own local
//! name: `listens { Browse.DELETE -> REVERT }` means "this orbital handles
//! DELETE, by firing REVERT". Counting only transition triggers missed every
//! renamed-target route, so a wired affordance read as orphaned.
//!
//! Live specimen: std-forum's `ModQueueOrbital` has no `DELETE` transition
//! anywhere in the orbital — the browse list's built-in DELETE action is wired
//! to the revert flow via exactly this route, and `audit_listens` reports it
//! `wired: true, via: listens`. It produced 9 false `CIRCUIT_ORPHAN_EVENT`
//! errors (3 view variants x 3 transitions).

use orbital_compiler::phases::validation::validate_comprehensive;
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

const ORPHAN: &str = "CIRCUIT_ORPHAN_EVENT";

fn orphan_errors(schema: Value) -> Vec<String> {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    validate_comprehensive(&schema)
        .errors
        .iter()
        .filter(|e| e.code.as_str() == ORPHAN)
        .map(|e| e.message.clone())
        .collect()
}

/// `Lister` renders a `DELETE` item action. `Reverter` handles it — but under
/// a different local name, via a `listens` route, exactly as the corpus does.
fn schema_with(listens: Option<Value>) -> Value {
    let mut reverter = json!({
        "name": "Reverter",
        "scope": "instance",
        "linkedEntity": "Row",
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "REVERT", "name": "Revert" }],
            "transitions": [{
                "from": "idle", "to": "idle", "event": "REVERT",
                "effects": [["render-ui", "modal", { "type": "confirm-dialog", "title": "Revert?" }]]
            }]
        }
    });
    if let Some(listens) = listens {
        reverter["listens"] = listens;
    }
    json!({
        "name": "App",
        "version": "1.0.0",
        "orbitals": [{
            "name": "O",
            "entity": {
                "name": "Row", "collection": "rows", "persistence": "persistent",
                "fields": [{ "name": "id", "type": "string", "required": true }]
            },
            "traits": [
                {
                    "name": "Lister", "scope": "instance", "linkedEntity": "Row",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "INIT", "name": "Init" }],
                        "transitions": [{
                            "from": "idle", "to": "idle", "event": "INIT",
                            "effects": [
                                ["fetch", "Row"],
                                ["render-ui", "main", {
                                    "type": "data-grid",
                                    "entity": "@payload.data",
                                    "itemActions": [{ "event": "DELETE", "label": "Delete" }]
                                }]
                            ]
                        }]
                    }
                },
                reverter
            ],
            "pages": [{
                "name": "P", "path": "/rows", "linkedEntity": "Row",
                "traits": [{ "ref": "Lister" }, { "ref": "Reverter" }]
            }]
        }]
    })
}

#[test]
fn a_listens_route_closes_the_circuit_even_when_it_renames_the_event() {
    let listens = json!([{
        "event": "DELETE",
        "source": { "kind": "trait", "trait": "Lister" },
        "triggers": "REVERT"
    }]);
    assert_eq!(
        orphan_errors(schema_with(Some(listens))),
        Vec::<String>::new(),
        "an orbital that listens for DELETE handles DELETE, whatever it fires in response"
    );
}

#[test]
fn without_the_route_the_action_is_still_reported_orphaned() {
    // The baseline the fix must not erase: nothing anywhere handles DELETE.
    let errors = orphan_errors(schema_with(None));
    assert_eq!(errors.len(), 1, "expected exactly one orphan, got {errors:?}");
    assert!(errors[0].contains("DELETE"), "got {errors:?}");
}

#[test]
fn a_route_for_a_different_event_does_not_launder_the_orphan() {
    // Listening for something else must not blanket-satisfy the check.
    let listens = json!([{
        "event": "ARCHIVE",
        "source": { "kind": "trait", "trait": "Lister" },
        "triggers": "REVERT"
    }]);
    let errors = orphan_errors(schema_with(Some(listens)));
    assert_eq!(errors.len(), 1, "expected exactly one orphan, got {errors:?}");
    assert!(errors[0].contains("DELETE"), "got {errors:?}");
}
