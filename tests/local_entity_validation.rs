//! Browser-stored entities (`[persistent: x, local]`): the residence rules
//! `orb validate` makes decidable. One positive case and one control per code.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core::schema::OrbitalSchema;
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
        "local": true,
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
        "name": "Browse", "linkedEntity": linked, "category": "interaction", "local": local,
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
fn local_on_a_runtime_entity_is_rejected() {
    let e = json!({ "name": "Scene", "persistence": "runtime", "local": true,
        "fields": [{ "name": "id", "type": "string", "required": true }] });
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(has(&r, ErrorCode::EntityLocalRequiresPersistent), "{:?}", codes(&r));
}

#[test]
fn control_local_on_a_persistent_entity_is_accepted() {
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({}))], vec![]));
    assert!(!has(&r, ErrorCode::EntityLocalRequiresPersistent), "{:?}", codes(&r));
}

#[test]
fn mock_seeding_needs_a_local_entity() {
    let e = json!({ "name": "Invoice", "persistence": "persistent", "collection": "invoices", "seedMock": true,
        "fields": [{ "name": "id", "type": "string", "required": true }] });
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(has(&r, ErrorCode::EntityMockSeedNotLocal), "{:?}", codes(&r));
}

#[test]
fn control_mock_seeding_on_a_local_entity_is_accepted() {
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({ "seedMock": true }))], vec![]));
    assert!(!has(&r, ErrorCode::EntityMockSeedNotLocal), "{:?}", codes(&r));
}

#[test]
fn mock_seeding_and_written_rows_conflict() {
    let e = invoice(json!({ "seedMock": true, "instances": [{ "id": "INV-1", "amount": 4 }] }));
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(has(&r, ErrorCode::EntitySeedSourceConflict), "{:?}", codes(&r));
}

#[test]
fn control_written_rows_alone_are_accepted() {
    let e = invoice(json!({ "instances": [{ "id": "INV-1", "amount": 4 }] }));
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(!has(&r, ErrorCode::EntitySeedSourceConflict), "{:?}", codes(&r));
    assert!(!has(&r, ErrorCode::EntityInstanceInvalid), "{:?}", codes(&r));
}

#[test]
fn an_identity_entity_cannot_be_local() {
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({ "identity": true }))], vec![]));
    assert!(has(&r, ErrorCode::EntityIdentityLocal), "{:?}", codes(&r));
}

#[test]
fn a_written_row_with_an_unknown_field_is_rejected() {
    let e = invoice(json!({ "instances": [{ "id": "INV-1", "amonut": 4 }] }));
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(has(&r, ErrorCode::EntityInstanceInvalid), "{:?}", codes(&r));
}

#[test]
fn a_written_row_on_a_local_entity_needs_an_id() {
    let e = invoice(json!({ "instances": [{ "amount": 4 }] }));
    let r = validate_comprehensive_without_services(&schema(vec![e], vec![]));
    assert!(has(&r, ErrorCode::EntityInstanceInvalid), "{:?}", codes(&r));
}

#[test]
fn a_local_trait_may_fetch_and_persist_a_local_entity() {
    let t = browse("Invoice", true, json!([["fetch", "Invoice", {}], ["persist", "update", "Invoice", "@entity"]]));
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({}))], vec![t]));
    assert!(!has(&r, ErrorCode::TraitLocalTouchesServer), "{:?}", codes(&r));
    assert!(!has(&r, ErrorCode::TraitLocalOnNonSharedPersistent), "{:?}", codes(&r));
}

#[test]
fn control_a_local_trait_still_may_not_fetch_a_server_entity() {
    let t = browse("Invoice", true, json!([["fetch", "Ledger", {}]]));
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({})), ledger()], vec![t]));
    assert!(has(&r, ErrorCode::TraitLocalTouchesServer), "{:?}", codes(&r));
}

#[test]
fn control_a_local_trait_still_may_not_call_a_service() {
    let t = browse("Invoice", true, json!([["call-service", "llm", "call", {}]]));
    let r = validate_comprehensive_without_services(&schema(vec![invoice(json!({}))], vec![t]));
    assert!(has(&r, ErrorCode::TraitLocalTouchesServer), "{:?}", codes(&r));
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

fn localized(locales: Value, entity: Value) -> OrbitalSchema {
    let mut s = serde_json::to_value(schema(vec![entity], vec![])).unwrap();
    s["locales"] = locales;
    serde_json::from_value(s).unwrap()
}

fn row(id: &str) -> Value {
    json!({ "id": id, "amount": 1 })
}

#[test]
fn locale_rows_for_every_declared_locale_are_accepted() {
    let s = localized(json!(["en", "ar", "sl"]), invoice(json!({
        "instances": [row("A")],
        "localeInstances": { "ar": [row("A")], "sl": [row("A")] }
    })));
    assert!(codes(&validate_comprehensive_without_services(&s)).iter().all(|c| !matches!(c,
        ErrorCode::EntityLocaleInstancesNotLocal | ErrorCode::EntityLocaleInstancesUnknownLocale | ErrorCode::EntityLocaleInstancesIncomplete)),
        "{:?}", codes(&validate_comprehensive_without_services(&s)));
}

#[test]
fn locale_rows_need_a_browser_stored_entity() {
    let mut e = invoice(json!({ "instances": [row("A")], "localeInstances": { "ar": [row("A")], "sl": [row("A")] } }));
    e["local"] = json!(false);
    assert!(has(&validate_comprehensive_without_services(&localized(json!(["en", "ar", "sl"]), e)), ErrorCode::EntityLocaleInstancesNotLocal));
}

#[test]
fn a_locale_block_for_an_undeclared_locale_is_rejected() {
    let s = localized(json!(["en", "ar"]), invoice(json!({ "instances": [row("A")], "localeInstances": { "ar": [row("A")], "de": [row("A")] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntityLocaleInstancesUnknownLocale));
}

#[test]
fn a_block_for_the_default_locale_is_rejected_its_rows_are_instances() {
    let s = localized(json!(["en", "ar"]), invoice(json!({ "instances": [row("A")], "localeInstances": { "en": [row("A")], "ar": [row("A")] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntityLocaleInstancesUnknownLocale));
}

#[test]
fn a_missing_declared_locale_is_rejected_once_any_block_is_written() {
    let s = localized(json!(["en", "ar", "sl"]), invoice(json!({ "instances": [row("A")], "localeInstances": { "ar": [row("A")] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntityLocaleInstancesIncomplete));
}

#[test]
fn locale_blocks_need_the_default_rows_too() {
    let s = localized(json!(["en", "ar"]), invoice(json!({ "localeInstances": { "ar": [row("A")] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntityLocaleInstancesIncomplete));
}

#[test]
fn locale_rows_are_checked_like_the_default_rows() {
    let s = localized(json!(["en", "ar"]), invoice(json!({ "instances": [row("A")], "localeInstances": { "ar": [{ "amount": 1, "nope": 2 }] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntityInstanceInvalid));
}

#[test]
fn locale_rows_conflict_with_mock_seeding() {
    let s = localized(json!(["en", "ar"]), invoice(json!({ "seedMock": true, "localeInstances": { "ar": [row("A")] } })));
    assert!(has(&validate_comprehensive_without_services(&s), ErrorCode::EntitySeedSourceConflict));
}
