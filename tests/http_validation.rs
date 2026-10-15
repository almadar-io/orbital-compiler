//! `orb validate` rules of the `http` effect (`docs/Almadar_Embedded_Kernel.md` H.5):
//! every code has a positive case and a control that stays silent.

use orbital_compiler::embedded::parse_service_registry_data;
use orbital_compiler::phases::validation::{
    validate_comprehensive_without_services, validate_comprehensive_full, ErrorCode, ValidationResult,
};
use orbital_compiler::SchemaLanguage;
use orbital_core::language::registry::baked_service_registry;
use orbital_core::schema::OrbitalSchema;
use orbital_core::secrets::{
    secret_flow_violations, CREDENTIAL_NOT_SECRET_CODE, SECRET_OUTSIDE_AUTH_CODE,
};
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
                "name": "Searcher", "linkedEntity": "Row", "category": "interaction",
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

fn check(egress: Value, config: Value, effects: Value) -> ValidationResult {
    validate_comprehensive_without_services(&program(egress, config, effects))
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

fn emit() -> Value {
    json!({ "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } })
}

fn call(origin: Value, request: Value) -> Value {
    json!([["http", "GET", origin, "/search", request, emit()]])
}

fn ok_egress() -> Value {
    json!([ORIGIN])
}

#[test]
fn a_declared_literal_origin_is_clean() {
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({})));
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
    assert!(!fires(&r, ErrorCode::HttpOriginNotStatic));
}

#[test]
fn egress_undeclared_literal_origin() {
    let r = check(json!(["https://other.example"]), config(), call(json!(ORIGIN), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
}

#[test]
fn egress_undeclared_without_any_egress() {
    let r = check(Value::Null, config(), call(json!(ORIGIN), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
}

#[test]
fn egress_match_is_exact_on_port_and_scheme() {
    let r = check(json!(["https://api.iconify.design:8443"]), config(), call(json!(ORIGIN), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
    let r = check(json!(["http://api.iconify.design"]), config(), call(json!(ORIGIN), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
    let r = check(json!(["https://API.iconify.design:443"]), config(), call(json!(ORIGIN), json!({})));
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
}

#[test]
fn egress_undeclared_url_knob_default() {
    let r = check(json!(["https://other.example"]), config(), call(json!("@config.origin"), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
    let r = check(ok_egress(), config(), call(json!("@config.origin"), json!({})));
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
    assert!(!fires(&r, ErrorCode::HttpOriginNotStatic));
}

#[test]
fn egress_undeclared_token_url_origin() {
    let auth = |token_url: &str| {
        json!({ "auth": { "scheme": "oauth2-client", "tokenUrl": token_url,
            "clientId": "@config.apiKey", "clientSecret": "@config.apiKey" } })
    };
    let r = check(ok_egress(), config(), call(json!(ORIGIN), auth("https://auth.example/oauth/token")));
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
    let r = check(
        json!([ORIGIN, "https://auth.example"]),
        config(),
        call(json!(ORIGIN), auth("https://auth.example/oauth/token")),
    );
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
}

#[test]
fn egress_entry_invalid() {
    for bad in ["https://a.com/path", "a.com", "ftp://a.com", "https://a.com/", "https://a.com:x", "https://"] {
        let r = check(json!([bad]), config(), json!([]));
        assert!(fires(&r, ErrorCode::HttpEgressEntryInvalid), "{bad}");
    }
    for good in ["https://a.com", "http://localhost:8080", "https://[::1]:9000"] {
        let r = check(json!([good]), config(), json!([]));
        assert!(!fires(&r, ErrorCode::HttpEgressEntryInvalid), "{good}");
    }
}

#[test]
fn origin_not_static() {
    for origin in [json!("@payload.base"), json!("@config.plain"), json!("@config.limit"), json!("@config.missing"), json!(["str/concat", "https://", "x"])] {
        let r = check(ok_egress(), config(), call(origin.clone(), json!({})));
        assert!(fires(&r, ErrorCode::HttpOriginNotStatic), "{origin}");
    }
}

#[test]
fn method_unknown() {
    let r = check(ok_egress(), config(), json!([["http", "FETCH", ORIGIN, "/s", {}, emit()]]));
    assert!(fires(&r, ErrorCode::HttpMethodUnknown));
    for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        let r = check(ok_egress(), config(), json!([["http", m, ORIGIN, "/s", {}, emit()]]));
        assert!(!fires(&r, ErrorCode::HttpMethodUnknown), "{m}");
    }
}

#[test]
fn option_unknown_in_request_auth_and_paginate() {
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "bogus": 1 })));
    assert!(fires(&r, ErrorCode::HttpOptionUnknown));

    let r = check(
        ok_egress(),
        config(),
        call(json!(ORIGIN), json!({ "auth": { "scheme": "bearer", "credential": "@config.apiKey", "name": "X" } })),
    );
    assert!(fires(&r, ErrorCode::HttpOptionUnknown));

    let r = check(
        ok_egress(),
        config(),
        call(json!(ORIGIN), json!({ "paginate": { "style": "cursor", "max": 3, "limit": 1 } })),
    );
    assert!(fires(&r, ErrorCode::HttpOptionUnknown));

    let clean = json!({
        "query": { "q": "x" }, "headers": { "Accept": "a" }, "timeoutMs": 10, "responseAs": "text",
        "auth": { "scheme": "bearer", "credential": "@config.apiKey" },
        "paginate": { "style": "cursor", "param": "c", "next": "@response.n", "collect": "@response.i", "max": 3 }
    });
    let r = check(ok_egress(), config(), call(json!(ORIGIN), clean));
    assert!(!fires(&r, ErrorCode::HttpOptionUnknown));
}

#[test]
fn auth_scheme_unknown() {
    for auth in [json!({ "scheme": "digest" }), json!({ "credential": "@config.apiKey" })] {
        let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "auth": auth })));
        assert!(fires(&r, ErrorCode::HttpAuthSchemeUnknown));
    }
    for scheme in ["bearer", "api-key", "basic", "oauth2-client", "token-exchange", "hmac-sha256"] {
        let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "auth": { "scheme": scheme } })));
        assert!(!fires(&r, ErrorCode::HttpAuthSchemeUnknown), "{scheme}");
    }
}

#[test]
fn body_conflict() {
    let post = |request: Value| json!([["http", "POST", ORIGIN, "/s", request, emit()]]);
    let r = check(ok_egress(), config(), post(json!({ "json": { "a": 1 }, "text": "x" })));
    assert!(fires(&r, ErrorCode::HttpBodyConflict));
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "json": { "a": 1 } })));
    assert!(fires(&r, ErrorCode::HttpBodyConflict));
    let r = check(ok_egress(), config(), post(json!({ "json": { "a": 1 } })));
    assert!(!fires(&r, ErrorCode::HttpBodyConflict));
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "query": { "a": 1 } })));
    assert!(!fires(&r, ErrorCode::HttpBodyConflict));
}

#[test]
fn paginate_max_missing() {
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "paginate": { "style": "cursor" } })));
    assert!(fires(&r, ErrorCode::HttpPaginateMaxMissing));
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "paginate": { "style": "cursor", "max": 5 } })));
    assert!(!fires(&r, ErrorCode::HttpPaginateMaxMissing));
}

#[test]
fn stream_without_on_message() {
    let r = check(ok_egress(), config(), call(json!(ORIGIN), json!({ "stream": "sse" })));
    assert!(fires(&r, ErrorCode::HttpStreamWithoutOnMessage));
    let with = json!([["http", "GET", ORIGIN, "/s", { "stream": "sse" },
        { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED", "onMessage": "CHUNK" } }]]);
    let r = check(ok_egress(), config(), with);
    assert!(!fires(&r, ErrorCode::HttpStreamWithoutOnMessage));
    assert!(!fires(&r, ErrorCode::EffectEmitKeyInvalid));
}

fn secret_codes(cfg: Value, effects: Value) -> Vec<&'static str> {
    let program = serde_json::to_value(program(ok_egress(), cfg, effects)).unwrap();
    secret_flow_violations(&program).iter().map(|v| v.code()).collect()
}

fn secrets(effects: Value) -> Vec<&'static str> {
    secret_codes(config(), effects)
}

#[test]
fn credential_not_secret() {
    let with = |auth: Value| secrets(call(json!(ORIGIN), json!({ "auth": auth })));
    for auth in [
        json!({ "scheme": "bearer", "credential": "@config.plain" }),
        json!({ "scheme": "bearer", "credential": "LITERAL" }),
        json!({ "scheme": "bearer", "credential": "@config.missing" }),
        json!({ "scheme": "basic", "user": "alice", "credential": "@config.apiKey" }),
        json!({ "scheme": "oauth2-client", "tokenUrl": ORIGIN, "clientId": "@config.apiKey", "clientSecret": "@payload.s" }),
    ] {
        assert!(with(auth.clone()).contains(&CREDENTIAL_NOT_SECRET_CODE), "{auth}");
    }
    for auth in [
        json!({ "scheme": "bearer", "credential": "@config.apiKey" }),
        json!({ "scheme": "basic", "user": "@config.apiKey", "credential": "@config.apiKey" }),
        json!({ "scheme": "api-key", "credential": "@config.apiKey", "name": "k", "in": "header" }),
        json!({ "scheme": "hmac-sha256", "credential": "@config.apiKey", "name": "S", "signs": "@payload.m", "encoding": "hex" }),
    ] {
        assert!(with(auth.clone()).is_empty(), "{auth}");
    }
}

#[test]
fn secret_outside_auth() {
    let r = secrets(json!([["set", "@entity.token", "@config.apiKey"]]));
    assert!(r.contains(&SECRET_OUTSIDE_AUTH_CODE));

    let r = secrets(json!([["http", "GET", ORIGIN, "/s", { "headers": { "X": "@config.apiKey" } }, emit()]]));
    assert!(r.contains(&SECRET_OUTSIDE_AUTH_CODE));

    let r = secrets(json!([["http", "GET", ORIGIN, "/s",
        { "auth": { "scheme": "api-key", "credential": "@config.apiKey", "name": "@config.apiKey", "in": "header" } }, emit()]]));
    assert!(r.contains(&SECRET_OUTSIDE_AUTH_CODE));

    let r = secrets(json!([["set", "@entity.token", "@config.plain"]]));
    assert!(!r.contains(&SECRET_OUTSIDE_AUTH_CODE));
}

#[test]
fn a_secret_in_another_knobs_config_declaration_stays_legal() {
    let cfg = json!({
        "apiKey": { "type": "secret", "default": "K" },
        "alias": { "type": "secret", "default": "@config.apiKey" }
    });
    assert!(secret_codes(cfg, json!([])).is_empty());
}

fn keyed(key: bool, cancelled: bool) -> Value {
    let mut emit = json!({ "success": "SEARCHED", "failure": "SEARCH_FAILED" });
    if cancelled {
        emit["cancelled"] = json!("SEARCH_CANCELLED");
    }
    let mut options = json!({ "emit": emit });
    if key {
        options["key"] = json!("@entity.id");
    }
    json!([["http", "GET", ORIGIN, "/s", {}, options]])
}

#[test]
fn keyed_http_contract_is_the_call_service_one() {
    assert!(fires(&check(ok_egress(), config(), keyed(false, true)), ErrorCode::CallServiceCancelledWithoutKey));
    assert!(!fires(&check(ok_egress(), config(), keyed(true, true)), ErrorCode::CallServiceCancelledWithoutKey));

    let numeric_key = json!([["http", "GET", ORIGIN, "/s", {}, { "key": 7, "emit": { "success": "SEARCHED" } }]]);
    assert!(fires(&check(ok_egress(), config(), numeric_key), ErrorCode::CallServiceKeyType));
}

#[test]
fn cancel_call_accepts_a_keyed_http_call() {
    let mut effects = keyed(true, true);
    effects.as_array_mut().unwrap().push(json!(["cancel-call", "@entity.id"]));
    assert!(!fires(&check(ok_egress(), config(), effects), ErrorCode::CancelCallNoKeyedCall));

    let mut unkeyed = keyed(false, false);
    unkeyed.as_array_mut().unwrap().push(json!(["cancel-call", "@entity.id"]));
    assert!(fires(&check(ok_egress(), config(), unkeyed), ErrorCode::CancelCallNoKeyedCall));
}

#[test]
fn the_success_payload_is_the_http_reply() {
    let reads = |payload: Value| {
        let mut schema = program(ok_egress(), config(), call(json!(ORIGIN), json!({})));
        let value = serde_json::to_value(&mut schema).unwrap();
        let mut value = value;
        value["orbitals"][0]["traits"][0]["emits"][0]["payloadSchema"] = payload;
        let schema: OrbitalSchema = serde_json::from_value(value).unwrap();
        validate_comprehensive_without_services(&schema)
    };
    let r = reads(json!([{ "name": "status", "type": "number" }, { "name": "body", "type": "string" }, { "name": "headers", "type": "object" }]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = reads(json!([{ "name": "key", "type": "string" }]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = reads(json!([{ "name": "items", "type": "array" }]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced));
    let r = reads(json!([{ "name": "status", "type": "string" }]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced));
}

#[test]
fn a_paginated_call_produces_pages_items_last() {
    let paged = json!([["http", "GET", ORIGIN, "/s",
        { "paginate": { "style": "cursor", "max": 2 } },
        { "emit": { "success": "PAGED", "failure": "SEARCH_FAILED" } }]]);
    let r = check(ok_egress(), config(), paged.clone());
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);

    let reply_on_paged = json!([["http", "GET", ORIGIN, "/s",
        { "paginate": { "style": "cursor", "max": 2 } },
        { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    assert!(fires(&check(ok_egress(), config(), reply_on_paged), ErrorCode::EmitPayloadNotProduced));
}

fn call_service(service: &str, action: &str, options: Value) -> Value {
    json!([["call-service", service, action, {}, options]])
}

fn keyed_options() -> Value {
    json!({ "key": "@entity.id", "emit": { "success": "SEARCHED" } })
}

#[test]
fn keyed_call_service_whose_result_has_key_is_an_error() {
    let r = check_with_installed_services(call_service("storage", "upload", keyed_options()));
    assert!(fires(&r, ErrorCode::CallServiceResultHasKey));
}

#[test]
fn call_service_result_has_key_controls() {
    let unkeyed = json!({ "emit": { "success": "SEARCHED" } });
    let r = check_with_installed_services(call_service("storage", "upload", unkeyed));
    assert!(!fires(&r, ErrorCode::CallServiceResultHasKey));
    let r = check_with_installed_services(call_service("llm", "generate", keyed_options()));
    assert!(!fires(&r, ErrorCode::CallServiceResultHasKey));
}

#[test]
fn a_keyed_call_service_success_payload_is_the_record_plus_key() {
    let reads = |name: &str| {
        let mut value = serde_json::to_value(program(ok_egress(), config(), call_service("llm", "generate", keyed_options()))).unwrap();
        value["orbitals"][0]["traits"][0]["emits"][0]["payloadSchema"] = json!([{ "name": name, "type": "string" }]);
        validate_comprehensive_full(
            &serde_json::from_value::<OrbitalSchema>(value).unwrap(),
            None,
            SchemaLanguage::English,
            baked_service_registry().clone(),
            Vec::new(),
        )
    };
    assert!(!fires(&reads("key"), ErrorCode::EmitPayloadNotProduced));
    assert!(!fires(&reads("content"), ErrorCode::EmitPayloadNotProduced));
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

fn http_with(extra_request: Value) -> Value {
    json!([["http", "GET", ORIGIN, "/s", extra_request,
        { "key": "@entity.id", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED", "onMessage": "CHUNK" } }]])
}

#[test]
fn the_http_failure_payload_is_error_kind_key() {
    let reads = |fields: Value| with_event_payload(http_with(json!({})), "SEARCH_FAILED", fields);
    let r = reads(json!([field("error", "string"), field("kind", "string"), field("key", "string")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = reads(json!([field("status", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "a failure has no status");
    let r = reads(json!([field("error", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "error is a string");
}

#[test]
fn the_http_stream_event_is_data_event_id_key() {
    let streaming = http_with(json!({ "stream": "sse" }));
    let reads = |fields: Value| with_event_payload(streaming.clone(), "CHUNK", fields);
    let r = reads(json!([field("data", "string"), field("event", "string"), field("id", "string"), field("key", "string")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = reads(json!([field("error", "string")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "a stream event has no error");
}

#[test]
fn a_keyed_call_service_failure_is_error_and_key() {
    let keyed = json!([["call-service", "llm", "generate", {},
        { "key": "@entity.id", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    let r = with_event_payload(keyed.clone(), "SEARCH_FAILED", json!([field("error", "string"), field("key", "string")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = with_event_payload(keyed, "SEARCH_FAILED", json!([field("status", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced));

    let unkeyed = json!([["call-service", "llm", "generate", {},
        { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
    let r = with_event_payload(unkeyed, "SEARCH_FAILED", json!([field("status", "number")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "control: an unkeyed failure shape is not decided");
}

#[test]
fn a_non_literal_token_url_is_not_static() {
    let auth = |token_url: Value| {
        json!({ "auth": { "scheme": "oauth2-client", "tokenUrl": token_url,
            "clientId": "@config.apiKey", "clientSecret": "@config.apiKey" } })
    };
    for token_url in [json!("@payload.t"), json!(["str/concat", "https://auth.example", "/t"]), json!("@config.plain"), json!("@config.missing")] {
        let r = check(json!([ORIGIN, "https://auth.example"]), config(), call(json!(ORIGIN), auth(token_url.clone())));
        assert!(fires(&r, ErrorCode::HttpOriginNotStatic), "{token_url}");
    }
    for token_url in [json!("https://auth.example/oauth/token"), json!("@config.origin")] {
        let r = check(json!([ORIGIN, "https://auth.example"]), config(), call(json!(ORIGIN), auth(token_url.clone())));
        assert!(!fires(&r, ErrorCode::HttpOriginNotStatic), "{token_url}");
    }
}

#[test]
fn the_result_has_key_check_reads_the_registry_it_is_given() {
    let registry = |shape: &str| {
        parse_service_registry_data(&format!(
            r#"{{"services": {{"files": {{"actions": [{{"name": "pull", "params": [], "responseShape": {shape}}}]}}}}}}"#
        ))
        .unwrap()
    };
    let run = |registry| {
        let schema = program(ok_egress(), config(), call_service("files", "pull", keyed_options()));
        validate_comprehensive_full(&schema, None, SchemaLanguage::English, registry, Vec::new())
    };
    assert!(fires(&run(registry(r#"{"key": "string", "url": "string"}"#)), ErrorCode::CallServiceResultHasKey));
    assert!(!fires(&run(registry(r#"{"url": "string"}"#)), ErrorCode::CallServiceResultHasKey));
}

fn post(origin: Value, request: Value) -> Value {
    json!([["http", "POST", origin, "/s", request, emit()]])
}

fn exchange(extra: Value) -> Value {
    let mut auth = json!({
        "scheme": "token-exchange", "tokenUrl": format!("{ORIGIN}/oauth/token"), "grant": "refresh_token",
        "body": { "form": { "refresh_token": { "credential": "@config.apiKey", "scope": "user" }, "scope": "read" } },
        "clientAuth": "post", "clientId": "@config.apiKey", "clientSecret": "@config.apiKey",
        "accessField": "access_token", "expiresField": "expires_in", "expiresMarginMs": 1000,
        "storeAs": { "name": "drive:access", "scope": "user" }
    });
    auth.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    json!({ "auth": auth })
}

#[test]
fn a_token_exchange_with_every_key_is_clean_and_each_unknown_key_is_reported() {
    let r = check(ok_egress(), config(), post(json!(ORIGIN), exchange(json!({}))));
    assert!(!fires(&r, ErrorCode::HttpOptionUnknown), "{:?}", r.errors);
    assert!(!fires(&r, ErrorCode::HttpAuthSchemeUnknown));
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
    for patch in [
        json!({ "bogus": 1 }),
        json!({ "body": { "xml": {} } }),
        json!({ "storeAs": { "name": "n", "ttl": 1 } }),
        json!({ "body": { "form": { "k": { "credential": "@config.apiKey", "extra": 1 } } } }),
    ] {
        let r = check(ok_egress(), config(), post(json!(ORIGIN), exchange(patch.clone())));
        assert!(fires(&r, ErrorCode::HttpOptionUnknown), "{patch}");
    }
}

#[test]
fn a_token_exchange_token_url_must_be_in_egress() {
    let r = check(
        ok_egress(),
        config(),
        post(json!(ORIGIN), exchange(json!({ "tokenUrl": "https://auth.example/token" }))),
    );
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared));
    let r = check(
        json!([ORIGIN, "https://auth.example"]),
        config(),
        post(json!(ORIGIN), exchange(json!({ "tokenUrl": "https://auth.example/token" }))),
    );
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared));
}

#[test]
fn the_token_exchange_body_slot_rules_are_the_secret_flow_rules() {
    assert!(secrets(post(json!(ORIGIN), exchange(json!({})))).is_empty());
    let leaked = exchange(json!({ "body": { "form": { "k": "@config.apiKey" } } }));
    assert_eq!(secrets(post(json!(ORIGIN), leaked)), vec![SECRET_OUTSIDE_AUTH_CODE]);
    let plain = exchange(json!({ "body": { "form": { "k": { "credential": "@config.plain" } } } }));
    assert_eq!(secrets(post(json!(ORIGIN), plain)), vec![CREDENTIAL_NOT_SECRET_CODE]);
}

#[test]
fn the_new_request_keys_are_known_and_their_inner_keys_are_closed() {
    let clean = json!({
        "retry": { "max": 2, "on": ["5xx"], "backoffMs": 100 },
        "form": { "a": 1 }, "formEncoding": "rfc3986"
    });
    let r = check(ok_egress(), config(), post(json!(ORIGIN), clean));
    assert!(!fires(&r, ErrorCode::HttpOptionUnknown), "{:?}", r.errors);
    let multipart = json!({ "multipart": { "kind": "related", "parts": [{ "headers": { "A": "b" }, "text": "x" }, { "bytes": "AA==" }] } });
    let r = check(ok_egress(), config(), post(json!(ORIGIN), multipart));
    assert!(!fires(&r, ErrorCode::HttpOptionUnknown), "{:?}", r.errors);
    for bad in [
        json!({ "retry": { "max": 2, "on": ["5xx"], "backoffMs": 100, "jitter": true } }),
        json!({ "multipart": { "kind": "related", "parts": [], "boundary": "x" } }),
        json!({ "multipart": { "kind": "related", "parts": [{ "text": "x", "name": "n" }] } }),
    ] {
        let r = check(ok_egress(), config(), post(json!(ORIGIN), bad.clone()));
        assert!(fires(&r, ErrorCode::HttpOptionUnknown), "{bad}");
    }
}

#[test]
fn multipart_is_one_of_the_body_kinds() {
    let both = json!({ "multipart": { "kind": "related", "parts": [{ "text": "x" }] }, "json": {} });
    assert!(fires(&check(ok_egress(), config(), post(json!(ORIGIN), both)), ErrorCode::HttpBodyConflict));
    let get = json!({ "multipart": { "kind": "related", "parts": [{ "text": "x" }] } });
    assert!(fires(&check(ok_egress(), config(), call(json!(ORIGIN), get)), ErrorCode::HttpBodyConflict));
    let alone = json!({ "multipart": { "kind": "related", "parts": [{ "text": "x" }] } });
    assert!(!fires(&check(ok_egress(), config(), post(json!(ORIGIN), alone)), ErrorCode::HttpBodyConflict));
}

#[test]
fn an_origin_may_carry_a_path_prefix_that_egress_does_not_name() {
    for origin in [format!("{ORIGIN}/esign/v2"), format!("{ORIGIN}/esign/v2/")] {
        let r = check(ok_egress(), config(), call(json!(origin), json!({})));
        assert!(!fires(&r, ErrorCode::HttpEgressUndeclared), "{origin}");
        assert!(!fires(&r, ErrorCode::HttpOriginNotStatic), "{origin}");
    }
    let r = check(
        json!(["https://other.example"]),
        config(),
        call(json!(format!("{ORIGIN}/esign/v2")), json!({})),
    );
    assert!(fires(&r, ErrorCode::HttpEgressUndeclared), "the prefix does not make a different host allowed");
    let mut prefixed = config();
    prefixed["origin"]["default"] = json!(format!("{ORIGIN}/esign/v2"));
    let r = check(ok_egress(), prefixed, call(json!("@config.origin"), json!({})));
    assert!(!fires(&r, ErrorCode::HttpEgressUndeclared), "a url knob default may carry a prefix too");
}

#[test]
fn an_egress_entry_with_a_path_is_still_invalid() {
    let r = check(json!([format!("{ORIGIN}/esign")]), config(), call(json!(ORIGIN), json!({})));
    assert!(fires(&r, ErrorCode::HttpEgressEntryInvalid));
}
