//! Browser-stored entities (`[persistent: x, local]`): the residence rules
//! `orb validate` makes decidable. One positive case and one control per code.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn schema(entities: Vec<Value>, traits: Vec<Value>) -> OrbitalSchema {
    let mut entities = entities.into_iter();
    let primary = entities.next().expect("a primary entity");
    serde_json::from_value(json!({
        "name": "fixture",
        "version": "1.0.0",
        "orbitals": [{
            "name": "Books",
            "entity": primary,
            "auxiliaryEntities": entities.collect::<Vec<_>>(),
            "traits": traits,
            "pages": []
        }]
    }))
    .expect("schema")
}

fn invoice(extra: Value) -> Value {
    let mut e = json!({
        "name": "Invoice",
        "persistence": "persistent",
        "collection": "invoices",
        "browser": true,
        "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "amount", "type": "number" }
        ]
    });
    e.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    e
}

fn ledger() -> Value {
    json!({
        "name": "Ledger", "persistence": "persistent", "collection": "ledgers",
        "fields": [{ "name": "id", "type": "string", "required": true }]
    })
}

fn browse(linked: &str, local: bool, effects: Value) -> Value {
    json!({
        "name": "Browse", "linkedEntity": linked, "dispatch": local.then_some("client"),
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT" }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
        }
    })
}

fn codes(result: &ValidationResult) -> Vec<ErrorCode> {
    result.errors.iter().chain(result.warnings.iter()).map(|e| e.code.clone()).collect()
}

fn has(result: &ValidationResult, code: ErrorCode) -> bool {
    codes(result).contains(&code)
}

#[test]
fn one_transition_touching_browser_and_server_data_is_rejected() {
    let t = browse("Invoice", false, json!([["fetch", "Invoice", {}], ["fetch", "Ledger", {}]]));
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({})), ledger()], vec![t]));
    assert!(has(&r, ErrorCode::CrossMixedResidence), "{:?}", codes(&r));
}

#[test]
fn control_a_transition_touching_only_browser_data_is_accepted() {
    let t = browse("Invoice", false, json!([["fetch", "Invoice", {}]]));
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({}))], vec![t]));
    assert!(!has(&r, ErrorCode::CrossMixedResidence), "{:?}", codes(&r));
}

