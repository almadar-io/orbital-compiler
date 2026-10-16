//! `ORB_T_CLIENT_TICK_SERVER_EFFECT`: a tick that runs in the browser (its trait is not
//! server-awaited, and the tick is not `[background]`) must not carry a server effect — a
//! timer would otherwise drop it (nothing replays a tick on the server). Server work goes
//! through an emitted event a server-side trait handles. Data effects on browser-stored
//! entities are client work and stay allowed (owner ruling 2026-10-12, drift D-1).

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn schema(traits: Vec<Value>) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "fixture",
        "version": "1.0.0",
        "orbitals": [{
            "name": "Editing",
            "entity": { "name": "Draft", "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }] },
            "auxiliaryEntities": [
                { "name": "Ledger", "persistence": "persistent", "collection": "ledgers", "fields": [{ "name": "id", "type": "string", "required": true }] },
                { "name": "Notes", "persistence": "persistent", "collection": "notes", "local": true, "fields": [{ "name": "id", "type": "string", "required": true }] },
                { "name": "Doc", "persistence": "persistent", "collection": "docs", "fields": [{ "name": "id", "type": "string", "required": true }] }
            ],
            "traits": traits,
            "pages": []
        }]
    }))
    .expect("schema")
}

fn editor(linked: &str, target: &str, background: bool) -> Value {
    json!({
        "name": "Editor", "linkedEntity": linked, "category": "interaction",
        "stateMachine": {
            "states": [{ "name": "editing", "isInitial": true }],
            "events": [{ "key": "INIT" }],
            "transitions": [{ "from": "editing", "to": "editing", "event": "INIT", "effects": [] }]
        },
        "ticks": [{
            "name": "autosave",
            "interval": "30s",
            "runsInBackground": background,
            "effects": [["persist", "create", target, {}]]
        }]
    })
}

fn codes(result: &ValidationResult) -> Vec<ErrorCode> {
    result.errors.iter().chain(result.warnings.iter()).map(|e| e.code.clone()).collect()
}

fn validate(traits: Vec<Value>) -> Vec<ErrorCode> {
    codes(&validate_comprehensive_without_services(&schema(traits)))
}

#[test]
fn a_browser_run_tick_writing_server_data_is_rejected() {
    let codes = validate(vec![editor("Draft", "Ledger", false)]);
    assert!(codes.contains(&ErrorCode::TraitClientTickServerEffect), "{codes:?}");
}

#[test]
fn control_a_browser_run_tick_writing_browser_stored_data_is_allowed() {
    let codes = validate(vec![editor("Draft", "Notes", false)]);
    assert!(!codes.contains(&ErrorCode::TraitClientTickServerEffect), "{codes:?}");
}

#[test]
fn control_a_background_tick_runs_on_the_server_and_is_allowed() {
    let codes = validate(vec![editor("Draft", "Ledger", true)]);
    assert!(!codes.contains(&ErrorCode::TraitClientTickServerEffect), "{codes:?}");
}

#[test]
fn control_a_server_awaited_trait_ticks_on_the_server_and_is_allowed() {
    let codes = validate(vec![editor("Doc", "Ledger", false)]);
    assert!(!codes.contains(&ErrorCode::TraitClientTickServerEffect), "{codes:?}");
}
