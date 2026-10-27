//! `orb validate` rules of the `http` effect (`docs/Almadar_Embedded_Kernel.md` H.5):
//! every code has a positive case and a control that stays silent.

use orbital_compiler::embedded::parse_service_registry_data;
use orbital_compiler::phases::validation::{
    validate_comprehensive_without_services, validate_comprehensive_full, ErrorCode, ValidationResult,
};
use orbital_compiler::SchemaLanguage;
use orbital_core_base::language::registry::baked_service_registry;
use orbital_core_base::schema::OrbitalSchema;
use orbital_core_base::secrets::{
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
fn user_grants_cannot_escape_into_an_explicit_app_cache() {
    for mut auth in [
        json!({"scheme":"token-exchange","tokenUrl":format!("{ORIGIN}/token"),"body":{"form":{"refresh_token":{"credential":"@config.apiKey","scope":"user"}}}}),
        json!({"scheme":"jwt-bearer","tokenUrl":format!("{ORIGIN}/token"),"key":{"credential":"@config.apiKey","scope":"user"},"alg":"RS256","claims":{}}),
    ] {
        assert!(!fires(&check(ok_egress(),config(),call(json!(ORIGIN),json!({"auth":auth}))),ErrorCode::HttpAuthInvalid));
        for scope in ["app","user"] {
            auth["storeAs"] = json!({"name":"cache","scope":scope});
            let r = check(ok_egress(),config(),call(json!(ORIGIN),json!({"auth":auth})));
            assert_eq!(fires(&r,ErrorCode::HttpAuthInvalid),scope == "app","{r:?}");
        }
        auth["storeAs"] = json!({"name":"cache"});
        assert!(fires(&check(ok_egress(),config(),call(json!(ORIGIN),json!({"auth":auth}))),ErrorCode::HttpAuthInvalid));
    }
}

#[test]
fn sigv4_auth_and_presign_are_closed_typed_contracts() {
    let auth = json!({"scheme":"aws-sigv4","accessKeyId":"@config.apiKey","secretAccessKey":{"credential":"@config.apiKey","scope":"user"},"region":"us-east-1","service":"s3"});
    let valid = check(ok_egress(), config(), call(json!(ORIGIN), json!({"auth":auth,"presign":{"expiresSec":900}})));
    for code in [ErrorCode::HttpAuthInvalid, ErrorCode::OperatorArgTypeMismatch] {
        assert!(!fires(&valid, code), "{code:?}: {valid:?}");
    }
    for options in [json!({"expiresSec":0}),json!({"expiresSec":604801}),json!({"expiresSec":1.5}),json!({})] {
        let result = check(ok_egress(), config(),call(json!(ORIGIN),json!({"auth":auth,"presign":options})));
        assert!(fires(&result,ErrorCode::HttpAuthInvalid),"{result:?}");
    }
    let missing = check(ok_egress(),config(),call(json!(ORIGIN),json!({"presign":{"expiresSec":900}})));
    assert!(fires(&missing,ErrorCode::HttpAuthInvalid));
}

#[test]
fn webpush_and_vapid_have_closed_typed_options() {
    let request = json!({"webpush":{"subscription":{"endpoint":format!("{ORIGIN}/search"),"keys":{"p256dh":"public","auth":"auth"}},"payload":"message","ttl":60,"urgency":"high"},
        "auth":{"scheme":"vapid","privateKey":"@config.apiKey","publicKey":"public","subject":"mailto:owner@example.com"}});
    let effect = |request: Value| json!([["http","POST",ORIGIN,"/search",request,emit()]]);
    let r = check(ok_egress(),config(),effect(request.clone()));
    for code in [ErrorCode::HttpAuthInvalid,ErrorCode::OperatorArgTypeMismatch] {
        assert!(!fires(&r,code),"{code:?}: {r:?}");
    }
    for options in [json!({"ttl":-1}),json!({"urgency":"immediate"})] {
        let mut bad = request.clone();
        bad["webpush"].as_object_mut().unwrap().extend(options.as_object().unwrap().clone());
        let r = check(ok_egress(),config(),effect(bad));
        assert!(fires(&r,ErrorCode::HttpAuthInvalid),"{r:?}");
    }
    let mut mismatch = request.clone();
    mismatch["webpush"]["subscription"]["endpoint"] = json!("https://other.example.com/search");
    assert!(fires(&check(ok_egress(),config(),effect(mismatch)),ErrorCode::HttpAuthInvalid));
}

#[test]
fn a_stream_with_an_on_message_route_has_a_valid_emit() {
    let with = json!([["http", "GET", ORIGIN, "/s", { "stream": "sse" },
        { "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED", "onMessage": "CHUNK" } }]]);
    let r = check(ok_egress(), config(), with);
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
fn hmac_prefix_must_be_text() {
    let hmac = |prefix: serde_json::Value| {
        json!({"scheme":"hmac-sha256","credential":"@config.apiKey","name":"X-Sig","signs":"data","encoding":"hex","prefix":prefix})
    };
    let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":hmac(json!("sha256="))})));
    assert!(!r.errors.iter().any(|error| error.message.contains("prefix")), "{:?}", r.errors);
    for bad in [json!(7), json!(true)] {
        assert!(
            check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":hmac(bad.clone())})))
                .errors
                .iter()
                .any(|error| error.code == ErrorCode::HttpAuthInvalid && error.message.contains("auth.prefix")),
            "{bad}"
        );
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
fn keyed_http_rejects_statically_non_string_expressions() {
    for key in [json!(["+", 1, 2]), json!("@config.limit"), json!(null), json!("")] {
        let effects = json!([["http", "GET", ORIGIN, "/s", {}, { "key": key }]]);
        assert!(fires(&check(ok_egress(), config(), effects), ErrorCode::CallServiceKeyType), "{key}");
    }
    let effects = json!([["http", "GET", ORIGIN, "/s", {}, { "key": ["str/concat", "request:", "one"] }]]);
    assert!(!fires(&check(ok_egress(), config(), effects), ErrorCode::CallServiceKeyType));
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

fn http_with(extra_request: Value) -> Value {
    json!([["http", "GET", ORIGIN, "/s", extra_request,
        { "key": "@entity.id", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED", "onMessage": "CHUNK" } }]])
}

#[test]
fn the_http_failure_payload_is_error_kind_key() {
    let reads = |fields: Value| with_event_payload(http_with(json!({})), "SEARCH_FAILED", fields);
    let r = reads(json!([field("error", "string"), field("kind", "string"), field("credential", "string"), field("key", "string")]));
    assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", r.errors);
    let r = reads(json!([field("status", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "a failure has no status");
    let r = reads(json!([field("error", "number")]));
    assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "error is a string");
}

#[test]
fn a_presigned_success_has_url_and_key_instead_of_reply_fields() {
    let request = json!({"auth":{"scheme":"aws-sigv4","accessKeyId":"@config.apiKey","secretAccessKey":"@config.apiKey","region":"us-east-1","service":"s3"},"presign":{"expiresSec":900}});
    let effect = http_with(request);
    let r = with_event_payload(effect.clone(),"SEARCHED",json!([field("url","string"),field("key","string")]));
    assert!(!fires(&r,ErrorCode::EmitPayloadNotProduced),"{:?}",r.errors);
    let r = with_event_payload(effect,"SEARCHED",json!([field("status","number")]));
    assert!(fires(&r,ErrorCode::EmitPayloadNotProduced));
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
fn jwt_bearer_rejects_statically_invalid_auth_fields() {
    for patch in [json!({"alg":"HS256"}), json!({"claims":{"iss":"override"}}), json!({"claims":17})] {
        let mut auth = json!({"scheme":"jwt-bearer","key":"@config.apiKey","alg":"RS256",
            "claims":{"aud":ORIGIN,"iat":1000,"exp":4600},"tokenUrl":format!("{ORIGIN}/token")});
        auth.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":auth})));
        assert!(r.errors.iter().any(|error| error.code.as_str() == "ORB_HTTP_AUTH_INVALID"), "{:?}", r.errors);
    }
}

#[test]
fn static_http_header_injection_is_refused_and_valid_headers_are_clean() {
    for headers in [json!({"X-Test":"line\r\ninjection"}), json!({"X Bad":"value"}), json!({"X:Bad":"value"}), json!({"X-Test":"nul\u{0}value"})] {
        let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"headers":headers})));
        assert!(r.errors.iter().any(|error| error.code.as_str() == "ORB_HTTP_HEADER_INVALID"), "{:?}", r.errors);
    }
    let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"headers":{"X-Test":"one line"}})));
    assert!(!r.errors.iter().any(|error| error.code.as_str() == "ORB_HTTP_HEADER_INVALID"));
    for auth in [
        json!({"scheme":"api-key","credential":"@config.apiKey","name":"X:Bad","in":"header"}),
        json!({"scheme":"hmac-sha256","credential":"@config.apiKey","name":"X Bad","signs":"data","encoding":"hex"}),
    ] {
        let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":auth})));
        assert!(r.errors.iter().any(|error| error.code.as_str() == "ORB_HTTP_HEADER_INVALID"));
    }
    let r = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":{"scheme":"api-key","credential":"@config.apiKey","name":"X Bad","in":"query"}})));
    assert!(!r.errors.iter().any(|error| error.code.as_str() == "ORB_HTTP_HEADER_INVALID"));
}

#[test]
fn the_token_exchange_body_slot_rules_are_the_secret_flow_rules() {
    assert!(secrets(post(json!(ORIGIN), exchange(json!({})))).is_empty());
    let leaked = exchange(json!({ "body": { "form": { "k": "@config.apiKey" } } }));
    assert_eq!(secrets(post(json!(ORIGIN), leaked)), vec![SECRET_OUTSIDE_AUTH_CODE]);
    let plain = exchange(json!({ "body": { "form": { "k": { "credential": "@config.plain" } } } }));
    assert_eq!(secrets(post(json!(ORIGIN), plain)), vec![CREDENTIAL_NOT_SECRET_CODE]);
}

fn grant_auths() -> [Value; 3] {
    [
        exchange(json!({}))["auth"].clone(),
        json!({"scheme":"oauth2-client","tokenUrl":format!("{ORIGIN}/token"),
            "clientId":"@config.apiKey","clientSecret":"@config.apiKey","scope":"read"}),
        json!({"scheme":"jwt-bearer","tokenUrl":format!("{ORIGIN}/token"),
            "key":"@config.apiKey","alg":"RS256",
            "claims":{"aud":format!("{ORIGIN}/token"),"iat":1000,"exp":4600}}),
    ]
}

#[test]
fn grant_request_is_optional_and_accepts_closed_options_for_all_three_schemes() {
    for auth in grant_auths() {
        let baseline = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":auth})));
        for code in [ErrorCode::HttpAuthInvalid, ErrorCode::HttpHeaderInvalid, ErrorCode::OperatorArgTypeMismatch] {
            assert!(!fires(&baseline, code), "baseline {auth}, {code:?}: {baseline:?}");
        }
        for options in [
            json!({}), json!({"timeoutMs":null}), json!({"timeoutMs":0}),
            json!({"headers":{},"timeoutMs":15000}),
            json!({"headers":{"X-Grant":"grant-only","X-Empty":""},"timeoutMs":15000,
                "retry":{"max":3,"on":["1xx","408","5xx","429","timeout","failed"],
                    "scheduleMs":[100,500,1500],"noResponseMax":2}}),
            json!({"headers":{"X-Public":"@config.plain","X-Expression":["str/concat","public:","@config.plain"]},
                "timeoutMs":"@config.limit"}),
            json!({"timeoutMs":["+","@config.limit",1]}),
        ] {
            let mut configured = auth.clone();
            configured["grantRequest"] = options.clone();
            let request = json!({"auth":configured,"headers":{"X-Resource":"resource-only"},
                "timeoutMs":20000,"retry":{"max":1,"on":["5xx"],"backoffMs":7}});
            let result = check(ok_egress(), config(), post(json!(ORIGIN), request.clone()));
            for code in [ErrorCode::HttpAuthInvalid, ErrorCode::HttpHeaderInvalid, ErrorCode::OperatorArgTypeMismatch] {
                assert!(!fires(&result, code), "{auth}, {options}, {code:?}: {result:?}");
            }
            assert!(secrets(post(json!(ORIGIN), request)).is_empty(), "{auth}: {options}");
        }
    }
}

#[test]
fn delay_ms_is_a_declared_request_option_typed_as_a_whole_number_of_milliseconds() {
    let errors = |request: Value| {
        let result = check(ok_egress(), config(), post(json!(ORIGIN), request));
        [ErrorCode::HttpAuthInvalid, ErrorCode::HttpRequestOptionInvalid, ErrorCode::OperatorArgTypeMismatch]
            .into_iter()
            .any(|code| fires(&result, code))
    };
    for good in [json!(0), json!(500), json!(null), json!("@config.limit"), json!(["+", "@config.limit", 1])] {
        assert!(!errors(json!({"delayMs": good.clone()})), "{good}");
    }
    // Controls: a negative, fractional or text delay, a non-number expression and a grantRequest delay are refused.
    for bad in [json!(-1), json!(1.5), json!("soon"), json!("@config.plain"), json!(true)] {
        assert!(errors(json!({"delayMs": bad.clone()})), "{bad}");
    }
    for bad in [json!(-1), json!("soon"), json!("@config.plain")] {
        let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"delayMs": bad.clone()})));
        assert!(fires(&result, ErrorCode::HttpRequestOptionInvalid), "{bad}: {result:?}");
        assert!(!fires(&result, ErrorCode::HttpAuthInvalid), "{bad}: {result:?}");
    }
}

#[test]
fn grant_request_rejects_nonobject_options_headers_and_retry() {
    for auth in grant_auths() {
        for invalid in [json!(null), json!(3), json!([]), json!(["literal", "@config.limit"]), json!(true), json!("request")] {
            for (options, code) in [
                (invalid.clone(), ErrorCode::HttpAuthInvalid),
                (json!({"headers":invalid.clone()}), ErrorCode::HttpHeaderInvalid),
                (json!({"retry":invalid.clone()}), ErrorCode::HttpRequestOptionInvalid),
            ] {
                let mut configured = auth.clone();
                configured["grantRequest"] = options.clone();
                let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":configured})));
                assert!(fires(&result, code), "{auth}: {options}: {result:?}");
            }
        }
    }
}

#[test]
fn grant_and_resource_deadlines_share_whole_nonnegative_or_null_validation() {
    for auth in grant_auths() {
        for deadline in [json!(null), json!(0), json!(1), json!(15000), json!("@config.limit"), json!(["+","@config.limit",1])] {
            let mut configured = auth.clone();
            configured["grantRequest"] = json!({"timeoutMs":deadline});
            let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":configured,"timeoutMs":deadline})));
            assert!(!fires(&result, ErrorCode::HttpAuthInvalid), "{auth}: {deadline}: {result:?}");
            assert!(!fires(&result, ErrorCode::OperatorArgTypeMismatch), "{auth}: {deadline}: {result:?}");
        }
        for deadline in [json!(-1), json!(0.5), json!("15000"), json!(true), json!([]), json!({}), json!("@config.plain")] {
            for grant in [false, true] {
                let mut request = json!({"auth":auth});
                if grant {
                    request["auth"]["grantRequest"] = json!({"timeoutMs":deadline});
                } else {
                    request["timeoutMs"] = deadline.clone();
                }
                let result = check(ok_egress(), config(), post(json!(ORIGIN), request));
                assert!(fires(&result, ErrorCode::HttpRequestOptionInvalid) || fires(&result, ErrorCode::HttpAuthInvalid), "{auth}: {deadline}: grant={grant}: {result:?}");
            }
        }
    }
}

#[test]
fn grant_headers_reserve_authorization_and_content_type_even_without_basic_auth() {
    let mut auths = grant_auths().to_vec();
    let mut omitted = grant_auths()[0].clone();
    for key in ["clientAuth", "clientId", "clientSecret"] {
        omitted.as_object_mut().unwrap().remove(key);
    }
    auths.push(omitted);
    let mut basic = grant_auths()[0].clone();
    basic["clientAuth"] = json!("basic");
    auths.push(basic);
    for auth in auths {
        let mut control = auth.clone();
        control["grantRequest"] = json!({"headers":{"X-Grant":"public","X-Empty":""}});
        let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":control})));
        assert!(!fires(&result, ErrorCode::HttpHeaderInvalid), "{auth}: {result:?}");
        assert!(!fires(&result, ErrorCode::HttpAuthInvalid), "{auth}: {result:?}");
        assert!(!fires(&result, ErrorCode::OperatorArgTypeMismatch), "{auth}: {result:?}");
        for name in ["Authorization", "aUtHoRiZaTiOn", "Content-Type", "cOnTeNt-TyPe"] {
            let mut configured = auth.clone();
            configured["grantRequest"] = json!({"headers":{name:"literal"}});
            let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":configured})));
            assert!(fires(&result, ErrorCode::HttpHeaderInvalid), "{auth}: {name}: {result:?}");
        }
    }
}

#[test]
fn grant_headers_keep_canonical_token_single_line_and_static_type_checks() {
    for auth in grant_auths() {
        for headers in [
            json!({"X Bad":"value"}), json!({"X:Bad":"value"}), json!({"":"value"}),
            json!({"X-Test":"first\r\nInjected: second"}), json!({"X-Test":"line\nline"}),
            json!({"X-Test":"nul\u{0}value"}), json!({"X-Test":5}), json!({"X-Test":true}),
            json!({"X-Test":null}), json!({"X-Test":{}}), json!({"X-Test":[]}),
            json!({"X-Test":"@config.limit"}),
            json!({"X-Public":"@config.plain","X Bad":"value"}),
            json!({"X-Public":["str/concat","public:","@config.plain"],"X-Bad":"line\nline"}),
        ] {
            let mut configured = auth.clone();
            configured["grantRequest"] = json!({"headers":headers.clone()});
            let result = check(ok_egress(), config(), post(json!(ORIGIN), json!({"auth":configured})));
            assert!(fires(&result, ErrorCode::HttpHeaderInvalid), "{auth}: {headers}: {result:?}");
        }
    }
}

#[test]
fn extended_retry_policies_are_shared_by_grant_and_resource_requests() {
    for policy in [
        json!({"max":3,"on":["1xx","408","5xx","429","timeout","failed"],"scheduleMs":[100,500,1500],"noResponseMax":2}),
        json!({"max":1,"on":["failed","failed"],"scheduleMs":[0],"noResponseMax":0}),
        json!({"max":2,"on":["timeout"],"backoffMs":0,"noResponseMax":2}),
        json!({"max":1,"on":["5xx","429","timeout"],"backoffMs":7}),
    ] {
        for auth in grant_auths() {
            for grant in [false, true] {
                let mut request = json!({"auth":auth});
                if grant {
                    request["auth"]["grantRequest"] = json!({"retry":policy});
                } else {
                    request["retry"] = policy.clone();
                }
                let result = check(ok_egress(), config(), post(json!(ORIGIN), request));
                for code in [ErrorCode::HttpAuthInvalid, ErrorCode::OperatorArgTypeMismatch] {
                    assert!(!fires(&result, code), "{auth}: {policy}: grant={grant}: {code:?}: {result:?}");
                }
            }
        }
    }
}

#[test]
fn extended_retry_rejects_invalid_forms_lengths_values_and_cap_bounds() {
    for policy in [
        json!({"max":1,"on":["5xx"]}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"scheduleMs":[1]}),
        json!({"max":1,"on":["5xx"],"backoffMs":null,"scheduleMs":[1]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":[]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":[1,2]}),
        json!({"max":2,"on":["5xx"],"scheduleMs":[1]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":[-1]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":[0.5]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":[null]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":["1"]}),
        json!({"max":1,"on":["5xx"],"scheduleMs":null}),
        json!({"max":1,"on":["5xx"],"scheduleMs":1}),
        json!({"max":1,"on":["5xx"],"scheduleMs":{}}),
        json!({"max":1,"on":["5xx"],"scheduleMs":true}),
        json!({"max":1,"on":["5xx"],"backoffMs":-1}),
        json!({"max":1,"on":["5xx"],"backoffMs":0.5}),
        json!({"max":1,"on":["5xx"],"backoffMs":null}),
        json!({"max":1,"on":["5xx"],"backoffMs":"1"}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":-1}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":0.5}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":2}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":null}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":"0"}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":false}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":[]}),
        json!({"max":1,"on":["5xx"],"backoffMs":1,"noResponseMax":{}}),
        json!({"max":0,"on":["5xx"],"scheduleMs":[]}),
        json!({"max":1.5,"on":["5xx"],"scheduleMs":[1]}),
        json!({"max":4294967296u64,"on":["5xx"],"backoffMs":1}),
        json!({"on":["5xx"],"backoffMs":1}),
        json!({"max":1,"on":[],"scheduleMs":[1]}),
        json!({"max":1,"on":"failed","scheduleMs":[1]}),
        json!({"max":1,"on":["4xx"],"scheduleMs":[1]}),
        json!({"max":1,"on":["FAILED"],"scheduleMs":[1]}),
        json!({"max":1,"on":[99],"scheduleMs":[1]}),
    ] {
        for auth in grant_auths() {
            for grant in [false, true] {
                let mut request = json!({"auth":auth});
                if grant {
                    request["auth"]["grantRequest"] = json!({"retry":policy});
                } else {
                    request["retry"] = policy.clone();
                }
                let result = check(ok_egress(), config(), post(json!(ORIGIN), request));
                assert!(fires(&result, ErrorCode::HttpRequestOptionInvalid) || fires(&result, ErrorCode::HttpAuthInvalid), "{auth}: {policy}: grant={grant}: {result:?}");
            }
        }
    }
}

#[test]
fn grant_request_does_not_create_any_new_secret_or_credential_slots() {
    for auth in grant_auths() {
        let mut control = json!({"auth":auth});
        control["auth"]["grantRequest"] = json!({"headers":{"X-Public":"@config.plain"},"timeoutMs":"@config.limit"});
        assert!(secrets(post(json!(ORIGIN), control)).is_empty(), "{auth}");
        for options in [
            json!({"headers":{"X-Grant":"@config.apiKey"}}),
            json!({"headers":{"X-Grant":["str/concat","public:","@config.apiKey"]}}),
            json!({"headers":{"X-Grant":{"credential":"@config.apiKey","scope":"user"}}}),
            json!({"timeoutMs":"@config.apiKey"}),
            json!({"retry":{"max":1,"on":["5xx"],"backoffMs":"@config.apiKey"}}),
            json!({"retry":{"max":1,"on":["5xx"],"scheduleMs":["@config.apiKey"]}}),
            json!({"retry":{"max":1,"on":["failed"],"backoffMs":0,"noResponseMax":"@config.apiKey"}}),
        ] {
            let mut configured = auth.clone();
            configured["grantRequest"] = options.clone();
            assert_eq!(secrets(post(json!(ORIGIN), json!({"auth":configured}))), vec![SECRET_OUTSIDE_AUTH_CODE], "{auth}: {options}");
        }
        for extra in [
            json!({"headers":{"X-Resource":"@config.apiKey"}}),
            json!({"query":{"token":"@config.apiKey"}}),
            json!({"json":{"token":{"credential":"@config.apiKey"}}}),
        ] {
            let mut request = json!({"auth":auth});
            request.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            assert_eq!(secrets(post(json!(ORIGIN), request)), vec![SECRET_OUTSIDE_AUTH_CODE], "{auth}: {extra}");
        }
    }
}

#[test]
fn grant_request_does_not_legalize_plain_values_in_existing_credential_slots() {
    for auth in grant_auths() {
        let slots: &[&str] = if auth["scheme"] == "jwt-bearer" { &["key"] } else { &["clientId", "clientSecret"] };
        for slot in slots {
            for value in [json!("@config.plain"), json!("LITERAL"), json!("@config.missing")] {
                let mut configured = auth.clone();
                configured["grantRequest"] = json!({"headers":{"X-Grant":"public"}});
                configured[*slot] = value.clone();
                assert_eq!(secrets(post(json!(ORIGIN), json!({"auth":configured}))), vec![CREDENTIAL_NOT_SECRET_CODE], "{auth}: {slot}: {value}");
            }
        }
    }
}

fn dynamic_retry_config() -> Value {
    let mut cfg = config();
    cfg["backoff"] = json!({"type":"number","default":0});
    cfg["retryCap"] = json!({"type":"number","default":1});
    cfg["wideLimit"] = json!({"type":"number","default":3});
    cfg["retryOn"] = json!({"type":"[string]","items":{"type":"string"},"default":["5xx"]});
    cfg["retryDelays"] = json!({"type":"[number]","items":{"type":"number"},"default":[0]});
    cfg["threeDelays"] = json!({"type":"[number]","items":{"type":"number"},"default":[10,20,30]});
    cfg
}

#[test]
fn retry_accepts_typed_dynamic_operands_without_guessing_unknown_bounds_or_lengths() {
    for policy in [
        json!({"max":1,"on":["5xx"],"backoffMs":0,"noResponseMax":0}),
        json!({"max":3,"on":["1xx","408","5xx","429","timeout","failed"],"scheduleMs":[100,500,1500],"noResponseMax":2}),
        json!({"max":"@config.limit","on":["5xx"],"backoffMs":"@config.backoff","noResponseMax":"@config.retryCap"}),
        json!({"max":["+","@config.limit",1],"on":["5xx","timeout"],"backoffMs":["+","@config.backoff",1],"noResponseMax":["+","@config.retryCap",0]}),
        json!({"max":1,"on":"@config.retryOn","backoffMs":0}),
        json!({"max":2,"on":[500,502,503,504,"timeout"],"backoffMs":0}),
        json!({"max":2,"on":[503,"@config.limit","5xx"],"backoffMs":0}),
        json!({"max":2,"on":"@config.retryDelays","backoffMs":0}),
        json!({"max":1,"on":["failed"],"scheduleMs":"@config.retryDelays"}),
        json!({"max":"@config.limit","on":"@config.retryOn","backoffMs":"@config.backoff","noResponseMax":"@config.retryCap"}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":"@config.retryDelays","noResponseMax":"@config.retryCap"}),
        json!({"max":2,"on":["5xx","failed"],"scheduleMs":["@config.limit",["+","@config.limit",1]],"noResponseMax":1}),
        json!({"max":2,"on":["array/append","@config.retryOn","failed"],"scheduleMs":["array/append","@config.retryDelays",["+","@config.limit",1]],"noResponseMax":1}),
        json!({"max":"@config.wideLimit","on":["failed"],"scheduleMs":[10,20,30],"noResponseMax":2}),
        json!({"max":3,"on":"@config.retryOn","scheduleMs":"@config.threeDelays","noResponseMax":"@config.retryCap"}),
    ] {
        for auth in grant_auths() {
            for grant in [false, true] {
                let mut request = json!({"auth":auth});
                if grant {
                    request["auth"]["grantRequest"] = json!({"retry":policy});
                } else {
                    request["retry"] = policy.clone();
                }
                let result = check(ok_egress(), dynamic_retry_config(), post(json!(ORIGIN), request.clone()));
                for code in [ErrorCode::HttpAuthInvalid, ErrorCode::OperatorArgTypeMismatch, ErrorCode::OperatorArgUndetermined] {
                    assert!(!fires(&result, code), "{auth}: {policy}: grant={grant}: {code:?}: {result:?}");
                }
                assert!(secret_codes(dynamic_retry_config(), post(json!(ORIGIN), request)).is_empty(), "{auth}: {policy}: grant={grant}");
            }
        }
    }
}

#[test]
fn retry_dynamic_operands_do_not_hide_invalid_static_neighbors_or_wrong_declared_types() {
    for policy in [
        json!({"max":"@config.limit","on":["5xx"],"backoffMs":-1}),
        json!({"max":"@config.limit","on":["4xx"],"backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":["FAILED"],"scheduleMs":"@config.retryDelays"}),
        json!({"max":2,"on":"@config.retryOn","scheduleMs":["@config.limit",-1]}),
        json!({"max":1,"on":"@config.retryOn","scheduleMs":["@config.limit",0]}),
        json!({"max":2,"on":"@config.retryOn","scheduleMs":["@config.limit"]}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":[-1]}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":[]}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":[0],"noResponseMax":2}),
        json!({"max":"@config.limit","on":"@config.retryOn","backoffMs":"@config.backoff","noResponseMax":-1}),
        json!({"max":1,"on":"@config.retryOn","backoffMs":"@config.backoff","noResponseMax":2}),
        json!({"max":"@config.limit","on":"@config.retryOn","backoffMs":"@config.backoff","scheduleMs":"@config.retryDelays"}),
        json!({"max":"@config.limit","on":"@config.retryOn"}),
        json!({"max":0,"on":"@config.retryOn","backoffMs":"@config.backoff"}),
        json!({"max":1.5,"on":"@config.retryOn","backoffMs":"@config.backoff"}),
        json!({"max":"@config.plain","on":"@config.retryOn","backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":"@config.plain","backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":[600],"backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":[99],"backoffMs":"@config.backoff"}),
        json!({"max":1,"on":[500.5],"backoffMs":0}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":"@config.retryOn"}),
        json!({"max":"@config.limit","on":"@config.retryOn","backoffMs":"@config.plain"}),
        json!({"max":"@config.limit","on":"@config.retryOn","backoffMs":"@config.backoff","noResponseMax":"@config.plain"}),
        json!({"max":"@config.limit","on":[],"backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":["failed",true],"backoffMs":"@config.backoff"}),
        json!({"max":"@config.limit","on":"@config.retryOn","scheduleMs":["@config.limit",0.5]}),
        json!({"max":2,"on":"@config.retryOn","scheduleMs":["@config.limit",null]}),
        json!({"max":1,"on":"@config.retryOn","scheduleMs":"@config.retryDelays","noResponseMax":2}),
    ] {
        for auth in grant_auths() {
            for grant in [false, true] {
                let mut request = json!({"auth":auth});
                if grant {
                    request["auth"]["grantRequest"] = json!({"retry":policy});
                } else {
                    request["retry"] = policy.clone();
                }
                let result = check(ok_egress(), dynamic_retry_config(), post(json!(ORIGIN), request));
                assert!(fires(&result, ErrorCode::HttpRequestOptionInvalid) || fires(&result, ErrorCode::HttpAuthInvalid), "{auth}: {policy}: grant={grant}: {result:?}");
            }
        }
    }
}

// --- token-exchange rotate / onUnauthorized / queryEncoding ---------------------------------------------------

#[test]
fn on_unauthorized_is_a_closed_choice() {
    for good in ["reply", "refresh"] {
        let r = check(ok_egress(), config(), post(json!(ORIGIN), exchange(json!({ "onUnauthorized": good }))));
        assert!(!fires(&r, ErrorCode::HttpAuthInvalid), "{good}: {:?}", r.errors);
    }
    let r = check(ok_egress(), config(), post(json!(ORIGIN), exchange(json!({ "onUnauthorized": "retry" }))));
    assert!(fires(&r, ErrorCode::HttpAuthInvalid));
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
                    "name": "Reporter", "linkedEntity": "Row", "category": "interaction",
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

    fn beacon_routes() -> Value {
        json!({ "emit": { "success": "BEACON_DONE", "failure": "BEACON_FAILED", "cancelled": "BEACON_CANCELLED" } })
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
    fn a_cancelled_route_needs_a_key_in_the_request_or_the_options() {
        let no_key = check(json!([["browser/beacon", "https://collector.example.com/e", { "text": "x" }, beacon_routes()]]));
        assert!(fires(&no_key, ErrorCode::CallServiceCancelledWithoutKey), "{:?}", no_key.errors);
        for call in [
            json!(["browser/beacon", "https://collector.example.com/e", { "text": "x", "key": "k" }, beacon_routes()]),
            json!(["browser/beacon", "https://collector.example.com/e", { "text": "x" }, { "key": "k", "emit": beacon_routes()["emit"].clone() }]),
        ] {
            let r = check(json!([call]));
            assert!(!fires(&r, ErrorCode::CallServiceCancelledWithoutKey), "{:?}", r.errors);
        }
    }

    #[test]
    fn a_keyed_page_facts_adds_key_to_what_an_event_may_read_and_the_key_must_be_a_string() {
        let keyed = json!([["browser/page-facts", ["referrer"], { "key": "@entity.id", "emit": { "success": "FACTS", "failure": "FACTS_FAILED" } }]]);
        let r = check_reading(keyed.clone(), &["referrer", "key"]);
        assert!(!fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
        let unkeyed = json!([["browser/page-facts", ["referrer"], facts_routes()]]);
        let r = check_reading(unkeyed, &["referrer", "key"]);
        assert!(fires(&r, ErrorCode::PageFactsUnrequestedRead), "{:?}", r.errors);
        let bad = json!([["browser/page-facts", ["referrer"], { "key": 5, "emit": { "success": "FACTS" } }]]);
        assert!(fires(&check(bad), ErrorCode::CallServiceKeyType));
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
            ErrorCode::CallServiceKeyType,
            ErrorCode::CancelCallNoKeyedCall,
            ErrorCode::EffectUnknownType,
        ] {
            assert!(!fires(&r, code), "{code:?}: {:?}", r.errors);
        }
    }

    #[test]
    fn an_on_device_cancelled_route_needs_a_key_and_the_key_must_be_a_string() {
        let r = check(json!([["browser/prompt", "t", SCHEMA, model_routes()]]));
        assert!(fires(&r, ErrorCode::CallServiceCancelledWithoutKey), "{:?}", r.errors);
        let r = check(json!([["browser/embed", ["a"], { "key": 5, "emit": { "success": "FACTS" } }]]));
        assert!(fires(&r, ErrorCode::CallServiceKeyType), "{:?}", r.errors);
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
                    "name": "Driver", "linkedEntity": "Row", "category": "interaction",
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

    const CODES: [ErrorCode; 1] = [ErrorCode::TabRequestInvalid];

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
    fn a_request_key_outside_the_ops_closed_set_is_flagged_even_when_the_rest_is_dynamic() {
        let r = check(json!([["browser/tab-close", { "tab": "@entity.tab", "nope": 1 }, routes()]]));
        assert!(fires(&r, ErrorCode::TabRequestInvalid), "{:?}", r.errors);
        let r = check(json!([["browser/tab-close", { "tab": "@entity.tab" }, routes()]]));
        assert!(!fires(&r, ErrorCode::TabRequestInvalid), "{:?}", r.errors);
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
            assert!(fires(&r, ErrorCode::TabRequestInvalid), "{call}: {:?}", r.errors);
        }
    }

    #[test]
    fn a_request_that_is_not_an_object_is_flagged() {
        let r = check(json!([["browser/tab-close", 7, routes()]]));
        assert!(fires(&r, ErrorCode::TabRequestInvalid), "{:?}", r.errors);
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
            .push(json!({ "name": "callKey", "type": "CallKey" }));
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
        let call = json!([["http", "GET", ORIGIN, "/s", {}, { "key": "@entity.callKey", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }]]);
        let r = run(call);
        assert!(!fires(&r, ErrorCode::CallServiceKeyType), "{:?}", r.errors);
    }

    #[test]
    fn a_literal_call_key_no_call_runs_under_is_unused_and_a_matching_one_is_not() {
        let unused = run(json!([["cancel-call", ["as", "search", "CallKey"]]]));
        assert!(fires(&unused, ErrorCode::CallKeyUnused), "{:?}", unused.errors);
        let keyed = json!([
            ["http", "GET", ORIGIN, "/s", {}, { "key": ["as", "search", "CallKey"], "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }],
            ["cancel-call", ["as", "search", "CallKey"]]
        ]);
        let matched = run(keyed);
        assert!(!fires(&matched, ErrorCode::CallKeyUnused), "control: {:?}", matched.errors);
        let other = json!([
            ["http", "GET", ORIGIN, "/s", {}, { "key": "other", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }],
            ["cancel-call", ["as", "search", "CallKey"]]
        ]);
        assert!(fires(&run(other), ErrorCode::CallKeyUnused), "a different literal key is still unused");
    }

    #[test]
    fn a_computed_call_key_silences_the_unused_rule() {
        let computed = json!([
            ["http", "GET", ORIGIN, "/s", {}, { "key": "@entity.callKey", "emit": { "success": "SEARCHED", "failure": "SEARCH_FAILED" } }],
            ["cancel-call", ["as", "search", "CallKey"]]
        ]);
        let r = run(computed);
        assert!(!fires(&r, ErrorCode::CallKeyUnused), "{:?}", r.errors);
    }
}

mod call_key_final {
    use super::*;
    use orbital_compiler::phases::validation::final_program::validate_final_program;

    fn final_of(effects: Value) -> ValidationResult {
        let mut schema = serde_json::to_value(program(ok_egress(), config(), effects)).unwrap();
        let fields = schema["orbitals"][0]["entity"]["fields"].as_array_mut().unwrap();
        fields.push(json!({ "name": "callKey", "type": "CallKey" }));
        fields.push(json!({ "name": "callKeys", "type": "array", "items": { "type": "CallKey" } }));
        validate_final_program(&serde_json::from_value(schema).expect("schema"))
    }

    fn keyed(key: &str) -> Value {
        json!(["http", "GET", ORIGIN, "/s", {}, { "key": key, "emit": { "success": "OK", "failure": "BAD" } }])
    }

    #[test]
    fn a_call_key_cancel_with_a_call_key_typed_call_is_clean() {
        let r = final_of(json!([keyed("@entity.callKey"), ["cancel-call", "@entity.callKey"]]));
        assert!(!fires(&r, ErrorCode::CancelCallKeyNeverCalled), "{:?}", r.errors);
    }

    #[test]
    fn a_call_keyed_by_a_cast_counts_as_call_key_typed() {
        let r = final_of(json!([keyed_cast(), ["cancel-call", "@entity.callKey"]]));
        assert!(!fires(&r, ErrorCode::CancelCallKeyNeverCalled), "{:?}", r.errors);
    }

    fn keyed_cast() -> Value {
        json!(["http", "GET", ORIGIN, "/s", {}, { "key": ["as", "@entity.id", "CallKey"], "emit": { "success": "OK", "failure": "BAD" } }])
    }

    #[test]
    fn a_call_key_cancel_whose_only_calls_are_keyed_by_plain_strings_never_reaches_one() {
        let r = final_of(json!([keyed("@entity.id"), ["cancel-call", "@entity.callKey"]]));
        assert!(fires(&r, ErrorCode::CancelCallKeyNeverCalled), "{:?}", r.errors);
    }

    #[test]
    fn a_call_key_cancel_in_a_program_with_no_keyed_call_never_reaches_one() {
        let r = final_of(json!([["cancel-call", "@entity.callKey"]]));
        assert!(fires(&r, ErrorCode::CancelCallKeyNeverCalled), "{:?}", r.errors);
    }

    #[test]
    fn a_list_of_call_keys_follows_the_same_rule() {
        let none = final_of(json!([keyed("@entity.id"), ["cancel-call", "@entity.callKeys"]]));
        assert!(fires(&none, ErrorCode::CancelCallKeyNeverCalled), "{:?}", none.errors);
        let typed = final_of(json!([keyed("@entity.callKey"), ["cancel-call", "@entity.callKeys"]]));
        assert!(!fires(&typed, ErrorCode::CancelCallKeyNeverCalled), "{:?}", typed.errors);
    }

    #[test]
    fn control_a_string_cancel_is_not_judged_by_this_rule() {
        let r = final_of(json!([keyed("@entity.id"), ["cancel-call", "@entity.id"]]));
        assert!(!fires(&r, ErrorCode::CancelCallKeyNeverCalled), "{:?}", r.errors);
    }
}
