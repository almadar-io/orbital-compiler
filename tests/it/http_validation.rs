//! `orb validate` rules of the keyed host calls: every code has a positive case and a control that stays silent.
//! The `http` effect is retired (EXEC-HTTP-1): HTTP is `std-http-client` over `os/*`, so `(http …)` is an unknown operator.

use orbital_compiler::embedded::parse_service_registry_data;
use orbital_compiler::phases::validation::{
    validate_comprehensive_without_services, validate_comprehensive_full, ErrorCode, ValidationResult,
};
use orbital_compiler::SchemaLanguage;
use orbital_core_base::language::registry::baked_service_registry;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

const ORIGIN: &str = "https://api.iconify.design";

fn config() -> Value {
    json!({
        "origin": { "type": "url", "default": ORIGIN },
        "apiKey": { "type": "secret", "default": "ICONIFY_KEY" },
        "plain": { "type": "string", "default": "x" },
        "limit": { "type": "number", "default": 1 }
    })
}

fn program(egress: Value, config: Value, effects: Value) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "fixture", "version": "1.0.0",
        "egress": egress,
        "orbitals": [{
            "name": "Icons",
            "entity": {
                "name": "Row", "persistence": "runtime",
                "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "token", "type": "string" }]
            },
            "traits": [{
                "name": "Searcher", "linkedEntity": "Row", 
                "config": config,
                "emits": [
                    { "event": "SEARCHED", "payloadSchema": [{ "name": "status", "type": "number", "required": true }] },
                    { "event": "SEARCH_FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] },
                    { "event": "SEARCH_CANCELLED" },
                    { "event": "CHUNK", "payloadSchema": [{ "name": "data", "type": "string", "required": true }] },
                    { "event": "PAGED", "payloadSchema": [{ "name": "pages", "type": "number", "required": true }] }
                ],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "INIT" }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
                }
            }],
            "pages": []
        }]
    }))
    .expect("schema")
}

fn check_with_installed_services(effects: Value) -> ValidationResult {
    validate_comprehensive_full(
        &program(ok_egress(), config(), effects),
        None,
        SchemaLanguage::English,
        baked_service_registry().clone(),
        Vec::new(),
    )
}

fn fires(r: &ValidationResult, code: ErrorCode) -> bool {
    r.errors.iter().chain(r.warnings.iter()).any(|e| e.code == code)
}

fn ok_egress() -> Value {
    json!([ORIGIN])
}

fn call_service(service: &str, action: &str, options: Value) -> Value {
    json!([["call-service", service, action, {}, options]])
}

fn keyed_options() -> Value {
    json!({ "key": "@entity.id", "emit": { "success": "SEARCHED" } })
}

/// No installed service answers a record with a `key` field, so the rule's positive case is pinned on a test-local
/// registry (`the_result_has_key_check_reads_the_registry_it_is_given` below); against the installed services a keyed call is clean.
#[test]
fn call_service_result_has_key_controls() {
    let r = check_with_installed_services(call_service("queue", "enqueue", keyed_options()));
    assert!(!fires(&r, ErrorCode::CallServiceResultHasKey));
    let unkeyed = json!({ "emit": { "success": "SEARCHED" } });
    let r = check_with_installed_services(call_service("queue", "enqueue", unkeyed));
    assert!(!fires(&r, ErrorCode::CallServiceResultHasKey));
}

/// The installed registry no longer carries `queue` (host services moved into the kernel), so the
/// service is pinned on a test-local registry.
#[test]
fn a_keyed_call_service_success_payload_is_the_record_plus_key() {
    let registry = || {
        parse_service_registry_data(
            r#"{"services": {"queue": {"actions": [{"name": "enqueue", "params": [], "responseShape": {"jobId": "string"}}]}}}"#,
        )
        .unwrap()
    };
    let reads = |name: &str| {
        let mut value = serde_json::to_value(program(ok_egress(), config(), call_service("queue", "enqueue", keyed_options()))).unwrap();
        value["orbitals"][0]["traits"][0]["emits"][0]["payloadSchema"] = json!([{ "name": name, "type": "string" }]);
        validate_comprehensive_full(
            &serde_json::from_value::<OrbitalSchema>(value).unwrap(),
            None,
            SchemaLanguage::English,
            registry(),
            Vec::new(),
        )
    };
    assert!(!fires(&reads("key"), ErrorCode::EmitPayloadNotProduced));
    assert!(!fires(&reads("jobId"), ErrorCode::EmitPayloadNotProduced));
    assert!(fires(&reads("nonsense"), ErrorCode::EmitPayloadNotProduced));
}

fn with_event_payload(effects: Value, event: &str, payload: Value) -> ValidationResult {
    let mut value = serde_json::to_value(program(ok_egress(), config(), effects)).unwrap();
    let emits = value["orbitals"][0]["traits"][0]["emits"].as_array_mut().unwrap();
    let declared = emits.iter_mut().find(|e| e["event"] == event).unwrap();
    declared["payloadSchema"] = payload;
    validate_comprehensive_without_services(&serde_json::from_value::<OrbitalSchema>(value).unwrap())
}

fn field(name: &str, ty: &str) -> Value {
    json!({ "name": name, "type": ty })
}

fn http_call() -> Value {
    json!([["http", "GET", ORIGIN, "/s", {}, { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]])
}

#[test]
fn an_http_effect_is_an_unknown_operator() {
    let r = check_with_installed_services(http_call());
    assert!(
        r.errors.iter().any(|e| e.code == ErrorCode::EffectUnknownType && e.message.contains("'http'")),
        "{:?}",
        r.errors
    );
}

#[test]
fn control_a_call_service_of_the_same_shape_is_a_known_operator() {
    let known = json!([["call-service", "test-svc", "run", {}, { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    let r = check_with_installed_services(known);
    assert!(!fires(&r, ErrorCode::EffectUnknownType), "{:?}", r.errors);
}

#[test]
fn a_keyed_call_service_failure_is_error_and_key() {
    let keyed = json!([["call-service", "test-svc", "run", {},
        { "key": "@entity.id", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    let r = with_event_payload(keyed.clone(), "SEARCH_FAILED", json!([field("error", "string"), field("key", "string")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = with_event_payload(keyed, "SEARCH_FAILED", json!([field("status", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced));

    let unkeyed = json!([["call-service", "test-svc", "run", {},
        { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    let r = with_event_payload(unkeyed, "SEARCH_FAILED", json!([field("status", "number")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "control: an unkeyed failure shape is not decided");
}


mod browser_effects_validation {
    // `orb validate` rules of `browser/page-facts` and `browser/beacon` (G-CROSS-221): every code has a positive case
    // and a control that stays silent.

    use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
    use orbital_core_base::schema::OrbitalSchema;
    use serde_json::{json, Value};

    const COLLECTOR: &str = "https://collector.example.com";

    fn program(effects: Value, payload: Value) -> OrbitalSchema {
        serde_json::from_value(json!({
            "name": "fixture", "version": "1.0.0",
            "egress": [COLLECTOR],
            "orbitals": [{
                "name": "Host",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "body", "type": "string" }] },
                "traits": [{
                    "name": "Reporter", "linkedEntity": "Row", 
                    "config": {
                        "endpoint": { "type": "url", "default": "https://collector.example.com/e" },
                        "relative": { "type": "url", "default": "/e" }
                    },
                    "emits": [
                        { "event": "FACTS", "payloadSchema": payload },
                        { "event": "FACTS_FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] },
                        { "event": "BEACON_DONE", "payloadSchema": [{ "name": "sent", "type": "boolean", "required": true }, { "name": "via", "type": "string" }] },
                        { "event": "BEACON_FAILED", "payloadSchema": [{ "name": "kind", "type": "string", "required": true }] },
                        { "event": "BEACON_CANCELLED" }
                    ],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "INIT" }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("schema")
    }

    fn check(effects: Value) -> ValidationResult {
        validate_comprehensive_without_services(&program(effects, json!([{ "name": "available", "type": "boolean", "required": true }])))
    }

    fn check_reading(effects: Value, read: &[&str]) -> ValidationResult {
        let payload: Vec<Value> = read.iter().map(|name| json!({ "name": name, "type": "string", "required": true })).collect();
        validate_comprehensive_without_services(&program(effects, Value::Array(payload)))
    }

    fn fires(r: &ValidationResult, code: ErrorCode) -> bool {
        r.errors.iter().chain(r.warnings.iter()).any(|e| e.code == code)
    }

    fn facts_routes() -> Value {
        json!({ "emit": { "success": "FACTS", "failure": "FACTS_FAILED" } })
    }

    const PAGE_FACT_CODES: [ErrorCode; 1] = [ErrorCode::PageFactsUnrequestedRead];
    #[test]
    fn control_well_formed_page_facts_raise_none_of_the_codes() {
        let r = check(json!([
            ["browser/page-facts", ["referrer", "screenWidth"], facts_routes()],
            ["browser/page-facts", ["hostname"], facts_routes()]
        ]));
        for code in PAGE_FACT_CODES {
            assert!(!fires(&r, code), "{code:?}: {:?}", r.errors);
        }
    }

    #[test]
    fn an_event_reading_a_fact_that_was_not_requested_is_flagged() {
        let effects = json!([["browser/page-facts", ["referrer"], facts_routes()]]);
        let r = check_reading(effects.clone(), &["hostname"]);
        assert!(fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
        let r = check_reading(effects, &["referrer", "available"]);
        assert!(!fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
    }

    #[test]
    fn href_is_readable_when_requested_and_flagged_when_not() {
        let asked = json!([["browser/page-facts", ["href"], facts_routes()]]);
        let r = check_reading(asked, &["href", "available"]);
        assert!(!fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
        let other = json!([["browser/page-facts", ["referrer"], facts_routes()]]);
        let r = check_reading(other, &["href"]);
        assert!(fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
    }

    #[test]
    fn a_keyed_page_facts_adds_key_to_what_an_event_may_read() {
        let keyed = json!([["browser/page-facts", ["referrer"], { "key": "@entity.id", "emit": { "success": "FACTS", "failure": "FACTS_FAILED" } }]]);
        let r = check_reading(keyed.clone(), &["referrer", "key"]);
        assert!(!fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
        let unkeyed = json!([["browser/page-facts", ["referrer"], facts_routes()]]);
        let r = check_reading(unkeyed, &["referrer", "key"]);
        assert!(fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
    }

    // `browser/prompt` / `browser/embed`: the on-device model effects.

    fn model_routes() -> Value {
        json!({ "emit": { "success": "FACTS", "failure": "FACTS_FAILED", "cancelled": "BEACON_CANCELLED" } })
    }

    fn keyed_model_routes() -> Value {
        json!({ "key": "@entity.id", "emit": model_routes()["emit"].clone() })
    }

    const SCHEMA: &str = "{\"type\":\"object\"}";

    #[test]
    fn control_well_formed_on_device_calls_raise_none_of_the_codes() {
        let r = check(json!([
            ["browser/prompt", "@entity.body", SCHEMA, keyed_model_routes()],
            ["browser/prompt", "hello", ["json/stringify", { "type": "object" }]],
            ["browser/prompt", "@payload.text", "@payload.schema", keyed_model_routes()],
            ["browser/embed", ["a", "@entity.body"], keyed_model_routes()],
            ["browser/embed", ["a"]],
            ["cancel-call", "@entity.id"]
        ]));
        for code in [
            ErrorCode::CancelCallNoKeyedCall,
            ErrorCode::EffectUnknownType,
        ] {
            assert!(!fires(&r, code), "{code:?}: {:?}", r.errors);
        }
    }

    #[test]
    fn an_on_device_call_has_no_on_message_route() {
        let r = check(json!([["browser/embed", ["a"], { "key": "k", "emit": { "success": "FACTS", "onMessage": "FACTS" } }]]));
        assert!(!r.errors.is_empty(), "onMessage is not an on-device route");
    }

    #[test]
    fn an_on_device_success_event_may_read_what_the_call_produces_and_nothing_else() {
        let prompt = json!([["browser/prompt", "t", SCHEMA, { "key": "k", "emit": { "success": "FACTS" } }]]);
        let r = check_reading(prompt.clone(), &["reply", "key"]);
        assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
        let r = check_reading(prompt, &["embeddings"]);
        assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
        let embed = json!([["browser/embed", ["a"], { "emit": { "success": "FACTS" } }]]);
        let r = check_reading(embed, &["reply"]);
        assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    }

    #[test]
    fn a_cancel_call_is_reachable_through_a_keyed_on_device_call_and_stranded_without_one() {
        let keyed = check(json!([["browser/prompt", "t", SCHEMA, keyed_model_routes()], ["cancel-call", "@entity.id"]]));
        assert!(!fires(&keyed, ErrorCode::CancelCallNoKeyedCall), "{:?}", keyed.errors);
        let unkeyed = check(json!([["browser/prompt", "t", SCHEMA, model_routes_without_cancel()], ["cancel-call", "@entity.id"]]));
        assert!(fires(&unkeyed, ErrorCode::CancelCallNoKeyedCall), "{:?}", unkeyed.errors);
    }

    fn model_routes_without_cancel() -> Value {
        json!({ "emit": { "success": "FACTS", "failure": "FACTS_FAILED" } })
    }

}

mod tab_effects_validation {
    // `orb validate` rules of `browser/tab-*` (docs/Almadar_Embedded_Kernel.md D-A92): the request is a closed object,
    // a request with nothing only run time knows is read by the kernel's one parser, a watch names a declared
    // external input, and the effects never sit where the server runs them. Every code has a positive case and a
    // control that stays silent.

    use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
    use orbital_core_base::schema::OrbitalSchema;
    use serde_json::{json, Value};

    fn program(effects: Value, ticks: Value, payload: Value) -> OrbitalSchema {
        serde_json::from_value(json!({
            "name": "fixture", "version": "1.0.0",
            "orbitals": [{
                "name": "Host",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "tab", "type": "number" }] },
                "traits": [{
                    "name": "Driver", "linkedEntity": "Row", 
                    "ticks": ticks,
                    "emits": [
                        { "event": "TAB_OK", "payloadSchema": payload },
                        { "event": "TAB_FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] },
                        { "event": "TAB_CANCELLED" }
                    ],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "INIT" }, { "key": "ITEM_FOUND", "input": true }, { "key": "CANCEL" }],
                        "transitions": [
                            { "from": "idle", "to": "idle", "event": "INIT", "effects": effects },
                            { "from": "idle", "to": "idle", "event": "ITEM_FOUND" },
                            { "from": "idle", "to": "idle", "event": "CANCEL", "effects": [["cancel-call", "@entity.id"]] }
                        ]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("schema")
    }

    fn check(effects: Value) -> ValidationResult {
        validate_comprehensive_without_services(&program(effects, json!([]), json!([{ "name": "tab", "type": "number", "required": true }])))
    }

    fn check_reading(effects: Value, read: &[(&str, &str)]) -> ValidationResult {
        let payload: Vec<Value> = read.iter().map(|(name, ty)| json!({ "name": name, "type": ty, "required": true })).collect();
        validate_comprehensive_without_services(&program(effects, json!([]), Value::Array(payload)))
    }

    fn fires(r: &ValidationResult, code: ErrorCode) -> bool {
        r.errors.iter().chain(r.warnings.iter()).any(|e| e.code == code)
    }

    fn routes() -> Value {
        json!({ "emit": { "success": "TAB_OK", "failure": "TAB_FAILED", "cancelled": "TAB_CANCELLED" } })
    }

    const CODES: [ErrorCode; 1] = [ErrorCode::TabRequestRefused];

    #[test]
    fn control_well_formed_tab_effects_raise_none_of_the_codes() {
        let r = check(json!([
            ["browser/tab-list", { "match": "https://example.com/*" }, routes()],
            ["browser/tab-open", { "url": "https://example.com/search", "params": { "q": "@payload.query" }, "match": "https://example.com/*", "active": false }, routes()],
            ["browser/tab-close", { "tab": "@entity.tab" }, routes()],
            ["browser/tab-navigate", { "tab": "@entity.tab", "url": "https://example.com/next" }, routes()],
            ["browser/tab-query", { "tab": "@entity.tab", "selector": ".r", "fields": { "t": { "selector": "h2" } }, "limit": 3, "waitMs": 100 }, routes()],
            ["browser/tab-watch", { "match": "https://example.com/*", "selector": ".item", "fields": { "t": { "selector": "h2", "required": true } }, "event": "Driver.ITEM_FOUND" }, routes()],
            ["browser/tab-unwatch", { "watchId": "@entity.id" }, routes()],
            ["browser/tab-apply", { "tab": "@entity.tab", "elementId": "@entity.id", "treatment": "hide", "label": "x" }, routes()],
            ["browser/tab-apply-many", { "items": "@payload.items" }, routes()],
            ["browser/tab-act", { "tab": "@entity.tab", "elementId": "@entity.id", "action": "click" }, routes()]
        ]));
        for code in CODES {
            assert!(!fires(&r, code), "{code:?}: {:?}", r.errors);
        }
    }

    #[test]
    fn a_literal_request_the_kernel_would_refuse_is_flagged_before_it_runs() {
        for call in [
            json!(["browser/tab-close", { "tab": "7" }, routes()]),
            json!(["browser/tab-list", { "match": "example.com" }, routes()]),
            json!(["browser/tab-open", { "url": "/relative" }, routes()]),
            json!(["browser/tab-open", { "url": "https://a.test/", "match": "https://b.test/*" }, routes()]),
            json!(["browser/tab-apply", { "tab": 1, "elementId": "e", "treatment": "explode" }, routes()]),
            json!(["browser/tab-act", { "tab": 1, "elementId": "e", "action": "eval" }, routes()]),
            json!(["browser/tab-query", { "tab": 1, "selector": ".a", "fields": { "b": { "template": "{missing}" } } }, routes()]),
            json!(["browser/tab-watch", { "match": "https://example.com/*", "selector": ".item", "fields": {}, "event": "NOPE" }, routes()]),
        ] {
            let r = check(json!([call.clone()]));
            assert!(fires(&r, ErrorCode::TabRequestRefused), "{call}: {:?}", r.errors);
        }
    }

    #[test]
    fn a_success_event_may_read_what_the_op_produces_and_nothing_else() {
        let query = json!([["browser/tab-query", { "tab": "@entity.tab", "selector": ".r", "fields": {} }, routes()]]);
        let r = check_reading(query.clone(), &[("tab", "number"), ("elements", "array"), ("key", "string")]);
        assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
        let r = check_reading(query, &[("tabs", "array")]);
        assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
        let list = json!([["browser/tab-list", {}, routes()]]);
        let r = check_reading(list.clone(), &[("tabs", "array")]);
        assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
        let r = check_reading(list, &[("elements", "array")]);
        assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    }

    #[test]
    fn a_keyed_tab_call_is_what_a_cancel_call_reaches_and_without_one_the_cancel_is_stranded() {
        let keyed = check(json!([["browser/tab-close", { "tab": 1 }, { "key": "job", "emit": routes()["emit"].clone() }]]));
        assert!(!fires(&keyed, ErrorCode::CancelCallNoKeyedCall), "{:?}", keyed.errors);
        let unkeyed = check(json!([["browser/tab-close", { "tab": 1 }, routes()]]));
        assert!(fires(&unkeyed, ErrorCode::CancelCallNoKeyedCall), "{:?}", unkeyed.errors);
    }
}

mod call_key {
    use super::*;

    fn run(effects: Value) -> ValidationResult {
        let mut schema = serde_json::to_value(program(ok_egress(), config(), effects)).unwrap();
        schema["orbitals"][0]["entity"]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "callKey", "type": "CallKey", "required": true }));
        validate_comprehensive_without_services(&serde_json::from_value(schema).expect("schema"))
    }

    fn mismatch(r: &ValidationResult) -> bool {
        r.errors.iter().chain(r.warnings.iter()).any(|e| {
            matches!(
                e.code,
                ErrorCode::OperatorArgTypeMismatch
                    | ErrorCode::SetValueTypeMismatch
                    | ErrorCode::SetReturnTypeMismatch
            )
        })
    }

    #[test]
    fn a_cancel_on_a_call_key_needs_no_keyed_call_in_the_program() {
        let r = run(json!([["cancel-call", "@entity.callKey"]]));
        assert!(!fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
    }

    #[test]
    fn a_cancel_on_a_call_key_field_that_may_be_absent_is_still_a_call_key_cancel() {
        let mut schema = serde_json::to_value(program(ok_egress(), config(), json!([["cancel-call", "@entity.callKey"]]))).unwrap();
        schema["orbitals"][0]["entity"]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "callKey", "type": "CallKey" }));
        let r = validate_comprehensive_without_services(&serde_json::from_value(schema).expect("schema"));
        assert!(!fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
    }

    #[test]
    fn control_a_cancel_on_a_plain_string_still_needs_a_keyed_call() {
        let r = run(json!([["cancel-call", "@entity.id"]]));
        assert!(fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
    }

    fn run_with_key_lists(effects: Value) -> ValidationResult {
        let mut schema = serde_json::to_value(program(ok_egress(), config(), effects)).unwrap();
        let fields = schema["orbitals"][0]["entity"]["fields"].as_array_mut().unwrap();
        fields.push(json!({ "name": "callKeys", "type": "array", "items": { "type": "CallKey" } }));
        fields.push(json!({ "name": "names", "type": "array", "items": { "type": "string" } }));
        validate_comprehensive_without_services(&serde_json::from_value(schema).expect("schema"))
    }

    #[test]
    fn a_cancel_on_a_list_of_call_keys_needs_no_keyed_call_in_the_program() {
        let r = run_with_key_lists(json!([["cancel-call", "@entity.callKeys"]]));
        assert!(!fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
        assert!(!mismatch(&r), "{:?}", r.errors);
    }

    #[test]
    fn control_a_cancel_on_a_list_of_plain_strings_still_needs_a_keyed_call() {
        let r = run_with_key_lists(json!([["cancel-call", "@entity.names"]]));
        assert!(fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
    }

    #[test]
    fn a_cancel_on_a_cast_literal_is_a_call_key_cancel() {
        let r = run(json!([["cancel-call", ["as", "search", "CallKey"]]]));
        assert!(!fires(&r, ErrorCode::CancelCallNoKeyedCall), "{:?}", r.errors);
    }

    #[test]
    fn a_cast_makes_a_call_key_and_a_bare_string_cannot_be_set() {
        let ok = run(json!([["set", "@entity.callKey", ["as", "search", "CallKey"]]]));
        assert!(!mismatch(&ok), "{:?}", ok.errors);
        let bare = run(json!([["set", "@entity.callKey", ["str/concat", "search", "x"]]]));
        assert!(mismatch(&bare), "a bare string must not assign: {:?}", bare.errors);
    }

    #[test]
    fn a_cast_of_a_value_with_no_conversion_is_rejected() {
        let number = run(json!([["set", "@entity.callKey", ["as", 7, "CallKey"]]]));
        assert!(mismatch(&number), "{:?}", number.errors);
        let from_string_field = run(json!([["set", "@entity.callKey", ["as", "@entity.id", "CallKey"]]]));
        assert!(!mismatch(&from_string_field), "control: a string field casts: {:?}", from_string_field.errors);
    }

    #[test]
    fn a_call_key_is_accepted_as_the_key_of_a_call() {
        let call = json!([["call-service", "Search", "run", {}, { "key": "@entity.callKey", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
        let r = run(call);
        assert!(!fires(&r, ErrorCode::OperatorArgTypeMismatch), "{:?}", r.errors);
    }
}
