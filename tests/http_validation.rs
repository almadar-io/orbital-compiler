//! `orb validate` rules of the `http` effect (`docs/Almadar_Embedded_Kernel.md` H.5):
//! every code has a positive case and a control that stays silent.

use orbital_compiler::phases::validation::effect::validate_http_secrets;
use orbital_compiler::phases::validation::{validate_comprehensive, ErrorCode, ValidationResult};
use orbital_core::schema::OrbitalSchema;
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
    validate_comprehensive(&program(egress, config, effects))
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
    for scheme in ["bearer", "api-key", "basic", "oauth2-client", "hmac-sha256"] {
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

fn secrets(effects: Value) -> ValidationResult {
    validate_http_secrets(&program(ok_egress(), config(), effects))
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
        assert!(fires(&with(auth.clone()), ErrorCode::HttpAuthCredentialNotSecret), "{auth}");
    }
    for auth in [
        json!({ "scheme": "bearer", "credential": "@config.apiKey" }),
        json!({ "scheme": "basic", "user": "@config.apiKey", "credential": "@config.apiKey" }),
        json!({ "scheme": "api-key", "credential": "@config.apiKey", "name": "k", "in": "header" }),
        json!({ "scheme": "hmac-sha256", "credential": "@config.apiKey", "name": "S", "signs": "@payload.m", "encoding": "hex" }),
    ] {
        let r = with(auth.clone());
        assert!(!fires(&r, ErrorCode::HttpAuthCredentialNotSecret), "{auth}");
        assert!(!fires(&r, ErrorCode::SecretOutsideAuth), "{auth}");
    }
}

#[test]
fn secret_outside_auth() {
    let r = secrets(json!([["set", "@entity.token", "@config.apiKey"]]));
    assert!(fires(&r, ErrorCode::SecretOutsideAuth));

    let r = secrets(json!([["http", "GET", ORIGIN, "/s", { "headers": { "X": "@config.apiKey" } }, emit()]]));
    assert!(fires(&r, ErrorCode::SecretOutsideAuth));

    let r = secrets(json!([["http", "GET", ORIGIN, "/s",
        { "auth": { "scheme": "api-key", "credential": "@config.apiKey", "name": "@config.apiKey", "in": "header" } }, emit()]]));
    assert!(fires(&r, ErrorCode::SecretOutsideAuth));

    let r = secrets(json!([["set", "@entity.token", "@config.plain"]]));
    assert!(!fires(&r, ErrorCode::SecretOutsideAuth));
}

#[test]
fn a_secret_in_another_knobs_config_declaration_stays_legal() {
    let cfg = json!({
        "apiKey": { "type": "secret", "default": "K" },
        "alias": { "type": "secret", "default": "@config.apiKey" }
    });
    let r = validate_http_secrets(&program(ok_egress(), cfg, json!([])));
    assert!(!fires(&r, ErrorCode::SecretOutsideAuth));
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
        validate_comprehensive(&schema)
    };
    let r = reads(json!([{ "name": "status", "type": "number" }, { "name": "body", "type": "string" }, { "name": "headers", "type": "object" }]));
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
