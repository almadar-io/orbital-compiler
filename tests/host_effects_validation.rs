//! `orb validate` rules of `random-bytes`, `credential-status` and `credential-forget`
//! (`docs/Almadar_Embedded_Kernel.md` H.7 P4, P6): every code has a positive case and a control that stays silent.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn program(effects: Value) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "fixture", "version": "1.0.0",
        "orbitals": [{
            "name": "Host",
            "entity": {
                "name": "Row", "persistence": "runtime",
                "fields": [{ "name": "id", "type": "string", "required": true }]
            },
            "traits": [{
                "name": "Drawer", "linkedEntity": "Row", "category": "interaction",
                "config": { "tok": { "type": "secret", "default": "TOK" } },
                "emits": [
                    { "event": "DRAWN", "payloadSchema": [{ "name": "value", "type": "string", "required": true }] },
                    { "event": "DRAW_FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] },
                    { "event": "DRAWN_KEYED", "payloadSchema": [{ "name": "value", "type": "string", "required": true }, { "name": "key", "type": "string", "required": true }] },
                    { "event": "KNOWN", "payloadSchema": [{ "name": "connected", "type": "boolean", "required": true }] },
                    { "event": "VERDICT", "payloadSchema": [{ "name": "allowed", "type": "boolean", "required": true }] },
                    { "event": "SPOILED", "payloadSchema": [{ "name": "bytes", "type": "string", "required": true }] }
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
    validate_comprehensive_without_services(&program(effects))
}

fn fires(r: &ValidationResult, code: ErrorCode) -> bool {
    r.errors.iter().chain(r.warnings.iter()).any(|e| e.code == code)
}

fn routes() -> Value {
    json!({ "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } })
}

#[test]
fn control_a_well_formed_call_of_each_effect_raises_none_of_the_host_codes() {
    let r = check(json!([
        ["random-bytes", 32, routes()],
        ["credential-status", "@config.tok", { "emit": { "success": "KNOWN" } }],
        ["credential-forget", "@config.tok"]
    ]));
    for code in [ErrorCode::HostEffectArity, ErrorCode::HostEffectOptionUnknown, ErrorCode::RandomBytesCount] {
        assert!(!fires(&r, code), "{code:?}: {:?}", r.errors);
    }
}

#[test]
fn the_wrong_number_of_arguments_is_an_arity_error() {
    for effect in [
        json!(["random-bytes"]),
        json!(["random-bytes", 8, {}, {}]),
        json!(["credential-status"]),
        json!(["credential-forget", "@config.tok", {}, {}]),
    ] {
        assert!(fires(&check(json!([effect.clone()])), ErrorCode::HostEffectArity), "{effect}");
    }
}

#[test]
fn only_emit_is_an_option_and_only_success_and_failure_are_emit_keys() {
    let r = check(json!([["credential-forget", "@config.tok", { "key": "k" }]]));
    assert!(fires(&r, ErrorCode::HostEffectOptionUnknown));
    let r = check(json!([["random-bytes", 8, "not-an-object"]]));
    assert!(fires(&r, ErrorCode::HostEffectOptionUnknown));
    for key in ["cancelled", "onMessage"] {
        let r = check(json!([["credential-status", "@config.tok", { "emit": { key: "DRAWN" } }]]));
        assert!(fires(&r, ErrorCode::EffectEmitKeyInvalid), "emit.{key}");
    }
}

#[test]
fn a_literal_count_outside_one_to_1024_is_refused() {
    for bad in [json!(0), json!(1025), json!(1.5), json!(-1)] {
        assert!(fires(&check(json!([["random-bytes", bad.clone()]])), ErrorCode::RandomBytesCount), "{bad}");
    }
    for good in [json!(1), json!(1024)] {
        assert!(!fires(&check(json!([["random-bytes", good.clone()]])), ErrorCode::RandomBytesCount), "{good}");
    }
    assert!(!fires(&check(json!([["random-bytes", "@payload.n"]])), ErrorCode::RandomBytesCount));
}

#[test]
fn only_the_tables_closed_calls_are_held_to_the_host_effect_shape() {
    let r = check(json!([["fetch", "Item", { "key": "k", "cancelled": "X", "emit": { "success": "LOADED" } }]]));
    assert!(!fires(&r, ErrorCode::HostEffectOptionUnknown) && !fires(&r, ErrorCode::HostEffectArity), "control: fetch is a data effect: {:?}", r.errors);
    assert!(fires(&check(json!([["access-check", {}, { "cancelled": "X" }]])), ErrorCode::HostEffectOptionUnknown));
}

#[test]
fn the_success_payload_a_handler_declares_must_be_one_the_effect_produces() {
    let spoiled = check(json!([["random-bytes", 8, { "emit": { "success": "SPOILED" } }]]));
    assert!(fires(&spoiled, ErrorCode::EmitPayloadNotProduced), "{:?}", spoiled.errors);
    let ok = check(json!([["random-bytes", 8, { "emit": { "success": "DRAWN" } }]]));
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "{:?}", ok.errors);
    let status = check(json!([["credential-status", "@config.tok", { "emit": { "success": "KNOWN" } }]]));
    assert!(!fires(&status, ErrorCode::EmitPayloadNotProduced), "{:?}", status.errors);
}

#[test]
fn os_binary_payloads_reject_string_handlers_and_accept_bytes() {
    for effect in [
        json!(["os/read-file", "/data/a", { "emit": { "success": "DATA" } }]),
        json!(["os/read", 1, 64, { "emit": { "success": "DATA" } }]),
        json!(["os/recv", 1, 64, { "emit": { "success": "DATA" } }]),
        json!(["os/recvfrom", 1, 64, { "emit": { "success": "DATA" } }]),
        json!(["os/stdin-read", 64, { "key": "stdin", "emit": { "onMessage": "DATA" } }]),
    ] {
        for (field_type, rejected) in [("string", true), ("bytes", false)] {
            let mut schema = serde_json::to_value(program(json!([effect.clone()]))).unwrap();
            schema["orbitals"][0]["traits"][0]["emits"] = json!([
                { "event": "DATA", "payloadSchema": [{ "name": "data", "type": field_type, "required": true }] }
            ]);
            let schema: OrbitalSchema = serde_json::from_value(schema).unwrap();
            let result = validate_comprehensive_without_services(&schema);
            assert_eq!(fires(&result, ErrorCode::EmitPayloadNotProduced), rejected, "{effect}, {field_type}: {:?}", result.errors);
        }
    }
}

#[test]
fn stdin_success_allows_data_only_for_a_single_read() {
    for options in [
        json!({ "emit": { "success": "DATA" } }),
        json!({ "key": null, "emit": { "success": "DATA" } }),
        json!({ "key": "stdin", "emit": { "success": "DATA" } }),
        json!({ "key": "stdin", "emit": { "success": "DATA", "onMessage": "CHUNK" } }),
    ] {
        for include_data in [false, true] {
            let mut schema = serde_json::to_value(program(json!([["os/stdin-read", 64, options.clone()]]))).unwrap();
            let mut payload = vec![json!({ "name": "eof", "type": "boolean", "required": true })];
            if include_data {
                payload.push(json!({ "name": "data", "type": "bytes", "required": true }));
            }
            schema["orbitals"][0]["traits"][0]["emits"] = json!([
                { "event": "DATA", "payloadSchema": payload },
                { "event": "CHUNK", "payloadSchema": [{ "name": "data", "type": "bytes", "required": true }] }
            ]);
            let schema: OrbitalSchema = serde_json::from_value(schema).unwrap();
            let result = validate_comprehensive_without_services(&schema);
            let streamed = options["key"] == "stdin" && options["emit"]["onMessage"] == "CHUNK";
            assert_eq!(fires(&result, ErrorCode::EmitPayloadNotProduced), streamed && include_data, "{options}, data={include_data}: {:?}", result.errors);
        }
    }
}

#[test]
fn os_keys_extend_success_failure_and_stream_messages_only_when_keyed() {
    for (operator, route, field, field_type) in [
        ("os/read", "success", "data", "bytes"),
        ("os/read", "failure", "error", "string"),
        ("os/stdin-read", "onMessage", "data", "bytes"),
    ] {
        for keyed in [false, true] {
            let options = if keyed {
                json!({ "key": "read", "emit": { route: "DATA" } })
            } else {
                json!({ "emit": { route: "DATA" } })
            };
            let effect = if operator == "os/read" {
                json!([operator, 1, 64, options])
            } else {
                json!([operator, 64, options])
            };
            let mut schema = serde_json::to_value(program(json!([effect]))).unwrap();
            schema["orbitals"][0]["traits"][0]["emits"] = json!([
                { "event": "DATA", "payloadSchema": [
                    { "name": field, "type": field_type, "required": true },
                    { "name": "key", "type": "string", "required": true }
                ] }
            ]);
            let schema: OrbitalSchema = serde_json::from_value(schema).unwrap();
            let result = validate_comprehensive_without_services(&schema);
            assert_eq!(fires(&result, ErrorCode::EmitPayloadNotProduced), !keyed, "{operator}, {route}, keyed={keyed}: {:?}", result.errors);
        }
    }
}

#[test]
fn random_bytes_takes_a_string_key_and_only_it_and_emit() {
    let keyed = |key: Value| json!([["random-bytes", 8, { "key": key, "emit": { "success": "DRAWN_KEYED", "failure": "DRAW_FAILED" } }]]);
    let ok = check(keyed(json!("@entity.id")));
    assert!(!fires(&ok, ErrorCode::HostEffectOptionUnknown) && !fires(&ok, ErrorCode::CallServiceKeyType), "{:?}", ok.errors);
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "{:?}", ok.errors);
    assert!(fires(&check(keyed(json!(5))), ErrorCode::CallServiceKeyType));
    assert!(fires(&check(keyed(json!(""))), ErrorCode::CallServiceKeyType));
    let other = check(json!([["random-bytes", 8, { "cancelled": "X" }]]));
    assert!(fires(&other, ErrorCode::HostEffectOptionUnknown));
    let status = check(json!([["credential-status", "@config.tok", { "key": "k" }]]));
    assert!(fires(&status, ErrorCode::HostEffectOptionUnknown), "only random-bytes is keyed here");
    let unkeyed = check(json!([["random-bytes", 8, { "emit": { "success": "DRAWN_KEYED" } }]]));
    assert!(fires(&unkeyed, ErrorCode::EmitPayloadNotProduced), "an unkeyed call produces no key");
}

#[test]
fn a_literal_credential_request_names_only_declared_parameters_and_all_required_ones() {
    let ok = check(json!([
        ["credential-list", {}, routes()],
        ["credential-list", { "service": "llm" }, routes()],
        ["credential-set", { "service": "a", "envVar": "B", "value": "@payload.v" }, routes()],
        ["credential-remove", { "service": "a", "envVar": "B" }],
        ["credential-test", { "service": "a" }],
        ["credential-mode", { "mode": "real" }],
        ["credential-rotate"],
        ["credential-test", "@payload.request"]
    ]));
    assert!(!fires(&ok, ErrorCode::CredentialRequestInvalid), "{:?}", ok.errors);
    assert!(!fires(&ok, ErrorCode::EffectUnknownType), "every credential effect is a known effect: {:?}", ok.errors);
    for bad in [
        json!(["credential-set", { "service": "a", "envVar": "B" }]),
        json!(["credential-set", { "service": "a", "envVar": "B", "value": "v", "force": true }]),
        json!(["credential-remove", {}]),
        json!(["credential-list", { "name": "x" }]),
        json!(["credential-mode", 5]),
        json!(["credential-test", "plain text"]),
    ] {
        assert!(fires(&check(json!([bad.clone()])), ErrorCode::CredentialRequestInvalid), "{bad}");
    }
}

#[test]
fn the_credential_effects_take_one_request_and_optional_routes_and_their_payloads_are_checked() {
    for effect in [json!(["credential-list"]), json!(["credential-set", {}, {}, {}]), json!(["credential-rotate", {}, {}])] {
        assert!(fires(&check(json!([effect.clone()])), ErrorCode::HostEffectArity), "{effect}");
    }
    let r = check(json!([["credential-rotate", { "key": "k" }]]));
    assert!(fires(&r, ErrorCode::HostEffectOptionUnknown));
    let ok = check(json!([["credential-rotate", { "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } }]]));
    assert!(fires(&ok, ErrorCode::EmitPayloadNotProduced), "DRAWN reads `value`, which rotate does not produce");
}

mod oauth {
    //! `ORB_OAUTH_*`: every code has a positive case and a control that stays silent.
    use super::*;

    const CODES: [ErrorCode; 6] = [
        ErrorCode::OAuthClientInvalid,
        ErrorCode::OAuthOpUnknown,
        ErrorCode::OAuthEndpointMismatch,
        ErrorCode::OAuthRequestConflict,
        ErrorCode::OAuthProviderUnknown,
        ErrorCode::OAuthOptionUnknown,
    ];

    fn client() -> Value {
        json!({ "tag": "google", "mode": "real", "issuer": "https://idp.example.com", "clientId": "tok", "clientSecret": "tok", "clientAuth": "post" })
    }

    fn token_call(auth: Value) -> Value {
        json!(["http", "POST", "oauth-endpoint", "token", { "auth": auth }, { "emit": { "success": "KNOWN", "failure": "DRAW_FAILED" } }])
    }

    fn connect() -> Value {
        json!({ "scheme": "oauth-client", "op": "connect", "client": client(), "provider": "google", "code": "@payload.code", "state": "@payload.state" })
    }

    fn with_egress(effects: Value) -> ValidationResult {
        let mut schema = serde_json::to_value(program(effects)).expect("json");
        schema["egress"] = json!(["https://idp.example.com"]);
        validate_comprehensive_without_services(&serde_json::from_value(schema).expect("schema"))
    }

    fn fired(r: &ValidationResult) -> Vec<ErrorCode> {
        CODES.into_iter().filter(|code| fires(r, *code)).collect()
    }

    #[test]
    fn control_well_formed_oauth_calls_raise_no_oauth_code() {
        let r = with_egress(json!([
            token_call(connect()),
            ["http", "GET", "https://idp.example.com", "/.well-known/openid-configuration",
                { "auth": { "scheme": "oauth-client", "op": "discovery", "client": client() } }],
            ["oauth-authorize", client(), { "redirectUri": "https://app/cb", "scope": ["openid"] }, { "emit": { "success": "KNOWN" } }],
            ["oauth-connection-status", "google", { "emit": { "success": "KNOWN" } }],
            ["oauth-connection-forget", "github"]
        ]));
        assert!(fired(&r).is_empty(), "{:?}", r.errors);
        assert!(!fires(&r, ErrorCode::HttpEgressUndeclared), "{:?}", r.errors);
    }

    #[test]
    fn a_malformed_client_is_refused() {
        let mut cases = Vec::new();
        for (what, edit) in [
            ("unknown key", json!({ "bogus": 1 })),
            ("bad tag", json!({ "tag": "okta" })),
            ("bad mode", json!({ "mode": "fake" })),
            ("bad clientAuth", json!({ "clientAuth": "digest" })),
            ("both", json!({ "endpoints": { "authorization": "https://idp.example.com/a", "token": "https://idp.example.com/t" } })),
        ] {
            let mut c = client();
            for (k, v) in edit.as_object().unwrap() {
                c[k] = v.clone();
            }
            cases.push((what, c));
        }
        let mut neither = client();
        neither.as_object_mut().unwrap().remove("issuer");
        cases.push(("neither", neither));
        let mut no_id = client();
        no_id.as_object_mut().unwrap().remove("clientId");
        cases.push(("no clientId", no_id));
        for (what, c) in cases {
            let mut auth = connect();
            auth["client"] = c;
            assert!(fires(&with_egress(json!([token_call(auth)])), ErrorCode::OAuthClientInvalid), "{what}");
        }
    }

    #[test]
    fn an_unknown_op_or_a_wrong_input_set_is_refused() {
        let mut auth = connect();
        auth["op"] = json!("rotate");
        assert!(fires(&with_egress(json!([token_call(auth)])), ErrorCode::OAuthOpUnknown));
        let mut auth = connect();
        auth.as_object_mut().unwrap().remove("provider");
        assert!(fires(&with_egress(json!([token_call(auth)])), ErrorCode::OAuthOpUnknown), "connect needs provider");
        let mut auth = json!({ "scheme": "oauth-client", "op": "code", "client": client(), "code": "c", "state": "s", "provider": "google" });
        assert!(fires(&with_egress(json!([token_call(auth.clone())])), ErrorCode::OAuthOpUnknown), "code takes no provider");
        auth.as_object_mut().unwrap().remove("provider");
        assert!(!fires(&with_egress(json!([token_call(auth)])), ErrorCode::OAuthOpUnknown));
    }

    #[test]
    fn the_target_word_and_method_must_serve_the_op() {
        let wrong_word = json!(["http", "POST", "oauth-endpoint", "userinfo", { "auth": connect() }]);
        assert!(fires(&with_egress(json!([wrong_word])), ErrorCode::OAuthEndpointMismatch));
        let wrong_method = json!(["http", "GET", "oauth-endpoint", "token", { "auth": connect() }]);
        assert!(fires(&with_egress(json!([wrong_method])), ErrorCode::OAuthEndpointMismatch));
        let literal = json!(["http", "POST", "https://idp.example.com", "/token", { "auth": connect() }]);
        assert!(fires(&with_egress(json!([literal])), ErrorCode::OAuthEndpointMismatch), "connect is not sent to a literal origin");
        let bearer = json!(["http", "POST", "oauth-endpoint", "token", { "auth": { "scheme": "bearer", "credential": "tok" } }]);
        assert!(fires(&with_egress(json!([bearer])), ErrorCode::OAuthEndpointMismatch));
        let discovery = json!(["http", "GET", "oauth-endpoint", "token", { "auth": { "scheme": "oauth-client", "op": "discovery", "client": client() } }]);
        assert!(fires(&with_egress(json!([discovery])), ErrorCode::OAuthEndpointMismatch));
    }

    #[test]
    fn op_code_routes_by_clients_and_no_other_op_does() {
        let mut github = client();
        github["tag"] = json!("github");
        let routed = |clients: Value| json!({"scheme":"oauth-client","op":"code","clients":clients,"code":"@payload.code","state":"@payload.state"});
        let ok = with_egress(json!([token_call(routed(json!({"google": client(), "github": github.clone()})))]));
        assert!(fired(&ok).is_empty(), "{:?}", ok.errors);
        let wrong_key = with_egress(json!([token_call(routed(json!({"github": client()})))]));
        assert!(fires(&wrong_key, ErrorCode::OAuthClientInvalid));
        let unknown_tag = with_egress(json!([token_call(routed(json!({"okta": client()})))]));
        assert!(fires(&unknown_tag, ErrorCode::OAuthProviderUnknown));
        let mut both = routed(json!({"google": client()}));
        both["client"] = client();
        assert!(fires(&with_egress(json!([token_call(both)])), ErrorCode::OAuthClientInvalid));
        let mut refresh = json!({"scheme":"oauth-client","op":"refresh","clients":{"google": client()},"refreshToken":"R"});
        assert!(fires(&with_egress(json!([token_call(refresh.clone())])), ErrorCode::OAuthOpUnknown));
        refresh.as_object_mut().unwrap().remove("clients");
        refresh["client"] = client();
        assert!(!fires(&with_egress(json!([token_call(refresh)])), ErrorCode::OAuthOpUnknown));
    }

    #[test]
    fn oauth_authorize_takes_a_key_and_only_it_and_emit() {
        let keyed = json!(["oauth-authorize", client(), {"redirectUri":"https://app/cb"}, {"key":"@entity.id","emit":{"success":"KNOWN"}}]);
        let r = with_egress(json!([keyed]));
        assert!(!fires(&r, ErrorCode::HostEffectOptionUnknown) && !fires(&r, ErrorCode::CallServiceKeyType), "{:?}", r.errors);
        let other = json!(["oauth-authorize", client(), {"redirectUri":"https://app/cb"}, {"cancelled":"X"}]);
        assert!(fires(&with_egress(json!([other])), ErrorCode::HostEffectOptionUnknown));
        let empty = json!(["oauth-authorize", client(), {"redirectUri":"https://app/cb"}, {"key":""}]);
        assert!(fires(&with_egress(json!([empty])), ErrorCode::CallServiceKeyType));
        let status = json!(["oauth-connection-status", "google", {"key":"k"}]);
        assert!(fires(&with_egress(json!([status])), ErrorCode::HostEffectOptionUnknown), "only authorize is keyed");
    }

    #[test]
    fn an_oauth_request_carries_only_auth() {
        let call = json!(["http", "POST", "oauth-endpoint", "token", { "auth": connect(), "json": { "a": 1 } }]);
        assert!(fires(&with_egress(json!([call])), ErrorCode::OAuthRequestConflict));
    }

    #[test]
    fn an_unknown_provider_word_is_refused_on_the_effects_and_on_connect() {
        for effect in [
            json!(["oauth-connection-status", "okta"]),
            json!(["oauth-connection-forget", 7]),
        ] {
            assert!(fires(&with_egress(json!([effect.clone()])), ErrorCode::OAuthProviderUnknown), "{effect}");
        }
        let mut auth = connect();
        auth["provider"] = json!("okta");
        assert!(fires(&with_egress(json!([token_call(auth)])), ErrorCode::OAuthProviderUnknown));
    }

    #[test]
    fn oauth_authorize_options_are_closed_and_need_a_redirect() {
        let unknown = json!(["oauth-authorize", client(), { "redirectUri": "https://app/cb", "state": "mine" }]);
        assert!(fires(&with_egress(json!([unknown])), ErrorCode::OAuthOptionUnknown));
        let missing = json!(["oauth-authorize", client(), { "scope": ["openid"] }]);
        assert!(fires(&with_egress(json!([missing])), ErrorCode::OAuthOptionUnknown));
        assert!(fires(&with_egress(json!([["oauth-authorize", client()]])), ErrorCode::HostEffectArity));
        assert!(fires(&with_egress(json!([["oauth-connection-status"]])), ErrorCode::HostEffectArity));
    }

    #[test]
    fn an_issuer_outside_egress_is_undeclared() {
        let r = check(json!([token_call(connect())]));
        assert!(fires(&r, ErrorCode::HttpEgressUndeclared), "the issuer is not in egress: {:?}", r.errors);
    }
}


mod os_grants {
    //! `ORB_OS_*` and `ORB_PLACEMENT_*`: every code has a positive case and a control that stays silent.
    use super::*;

    fn orbital(name: &str, grants: Value, local: bool, effects: Value) -> Value {
        let mut o = json!({
            "name": name,
            "entity": { "name": format!("{name}Row"), "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }] },
            "traits": [{
                "name": format!("{name}Runner"), "linkedEntity": format!("{name}Row"), "category": "interaction", "dispatch": local.then_some("client"),
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "INIT" }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
                }
            }],
            "pages": []
        });
        if !grants.is_null() {
            o["osGrants"] = grants;
        }
        o
    }

    fn run(app: Value, deployment: Value, orbitals: Vec<Value>) -> ValidationResult {
        let mut schema = json!({ "name": "fixture", "version": "1.0.0", "orbitals": orbitals });
        if !app.is_null() {
            schema["osGrants"] = app;
        }
        if !deployment.is_null() {
            schema["deployment"] = deployment;
        }
        validate_comprehensive_without_services(&serde_json::from_value::<OrbitalSchema>(schema).expect("schema"))
    }

    fn one(grants: Value, effects: Value) -> ValidationResult {
        run(Value::Null, Value::Null, vec![orbital("Orders", grants, false, effects)])
    }

    fn read_data() -> Value {
        json!({ "fs": [{ "glob": "/data/**", "modes": ["read"] }] })
    }

    fn warned(r: &ValidationResult, code: ErrorCode) -> bool {
        r.warnings.iter().any(|e| e.code == code)
    }

    #[test]
    fn a_call_outside_the_orbitals_grants_is_missing_a_grant_and_inside_is_not() {
        let outside = one(read_data(), json!([["os/read-file", "/etc/passwd"]]));
        assert!(fires(&outside, ErrorCode::OsGrantMissing), "{:?}", outside.errors);
        let wrong_mode = one(read_data(), json!([["os/write-file", "/data/a", "x"]]));
        assert!(fires(&wrong_mode, ErrorCode::OsGrantMissing), "{:?}", wrong_mode.errors);
        let none = one(Value::Null, json!([["os/memory-info"]]));
        assert!(fires(&none, ErrorCode::OsGrantMissing), "{:?}", none.errors);
        let inside = one(read_data(), json!([["os/read-file", "/data/a"]]));
        assert!(!fires(&inside, ErrorCode::OsGrantMissing), "{:?}", inside.errors);
    }

    fn guarded(grants: Value, guard: Value, effects: Value) -> ValidationResult {
        let mut o = orbital("Orders", grants, false, effects);
        o["traits"][0]["stateMachine"]["transitions"][0]["guard"] = guard;
        run(Value::Null, Value::Null, vec![o])
    }

    #[test]
    fn an_arm_whose_guard_is_statically_false_needs_no_grant() {
        let passwd = json!([["os/read-file", "/etc/passwd"]]);
        let dead = guarded(read_data(), json!(["!=", "", ""]), passwd.clone());
        assert!(!fires(&dead, ErrorCode::OsGrantMissing), "a dead arm never runs: {:?}", dead.errors);
        let live = guarded(read_data(), json!(["=", "", ""]), passwd.clone());
        assert!(fires(&live, ErrorCode::OsGrantMissing), "control: a statically true guard still runs: {:?}", live.errors);
        let open = guarded(read_data(), json!(["=", "@entity.id", ""]), passwd);
        assert!(fires(&open, ErrorCode::OsGrantMissing), "control: an undecided guard may run: {:?}", open.errors);
        let only_dead = guarded(read_data(), json!(["!=", "", ""]), json!([["os/read-file", "/data/a"]]));
        assert!(warned(&only_dead, ErrorCode::OsGrantUnused), "a grant only a dead arm uses is unused: {:?}", only_dead.warnings);
    }

    #[test]
    fn nested_calls_and_the_call_envelope_are_read_too() {
        let nested = one(read_data(), json!([["if", true, ["os/read-file", "/etc/passwd", { "emit": { "success": "OK" } }]]]));
        assert!(fires(&nested, ErrorCode::OsGrantMissing), "{:?}", nested.errors);
        let enveloped = one(read_data(), json!([["os/read-file", "/data/a", { "emit": { "success": "OK" } }]]));
        assert!(!fires(&enveloped, ErrorCode::OsGrantMissing), "{:?}", enveloped.errors);
    }

    #[test]
    fn grants_are_per_orbital_and_the_app_level_reaches_every_orbital() {
        let read = json!([["os/read-file", "/data/a"]]);
        let split = run(
            Value::Null,
            Value::Null,
            vec![orbital("Orders", read_data(), false, read.clone()), orbital("Billing", Value::Null, false, read.clone())],
        );
        let at: Vec<&str> = split.errors.iter().filter(|e| e.code == ErrorCode::OsGrantMissing).map(|e| e.path.as_str()).collect();
        assert_eq!(at.len(), 1, "{:?}", split.errors);
        assert!(at[0].contains("Billing"), "{at:?}");
        let app = run(
            read_data(),
            Value::Null,
            vec![orbital("Orders", Value::Null, false, read.clone()), orbital("Billing", Value::Null, false, read)],
        );
        assert!(!fires(&app, ErrorCode::OsGrantMissing), "{:?}", app.errors);
    }

    #[test]
    fn a_computed_target_is_not_static_and_a_literal_is() {
        for effect in [
            json!(["os/read-file", "@payload.path"]),
            json!(["os/read-file", ["str/concat", "@payload.name", "/x"]]),
            json!(["os/spawn", "@payload.cmd", []]),
            json!(["os/tcp-connect", "db.internal", "@payload.port"]),
            json!(["os/getenv", "@payload.name"]),
        ] {
            assert!(fires(&one(read_data(), json!([effect.clone()])), ErrorCode::OsTargetNotStatic), "{effect}");
        }
        let literal = one(read_data(), json!([["os/read-file", "/data/a"]]));
        assert!(!fires(&literal, ErrorCode::OsTargetNotStatic), "{:?}", literal.errors);
        let handle = one(Value::Null, json!([["os/close", "@payload.fd"]]));
        assert!(!fires(&handle, ErrorCode::OsTargetNotStatic) && !fires(&handle, ErrorCode::OsGrantMissing), "{:?}", handle.errors);
    }

    #[test]
    fn a_config_rooted_target_is_deferred_and_other_runtime_targets_are_not() {
        let config = one(Value::Null, json!([["os/spawn", "@config.cmd", []]]));
        assert!(!fires(&config, ErrorCode::OsTargetNotStatic), "{:?}", config.errors);
        assert!(!fires(&config, ErrorCode::OsGrantMissing), "{:?}", config.errors);
        let fed = one(
            Value::Null,
            json!([["os/spawn", ["object/get", ["array/nth", "@config.children", "@entity.cursor"], "command"], []]]),
        );
        assert!(!fires(&fed, ErrorCode::OsTargetNotStatic), "{:?}", fed.errors);
        assert!(!fires(&fed, ErrorCode::OsGrantMissing), "{:?}", fed.errors);
        for target in [json!("@payload.cmd"), json!("@entity.cmd"), json!(["object/get", "@payload.job", "command"])] {
            let r = one(Value::Null, json!([["os/spawn", target.clone(), []]]));
            assert!(fires(&r, ErrorCode::OsTargetNotStatic), "{target}: {:?}", r.errors);
        }
        let literal = one(Value::Null, json!([["os/spawn", "git", []]]));
        assert!(fires(&literal, ErrorCode::OsGrantMissing), "{:?}", literal.errors);
    }

    fn finished(grants: Value, config: Value, target: Value) -> ValidationResult {
        let mut schema = json!({
            "name": "fixture", "version": "1.0.0",
            "orbitals": [orbital("Orders", grants, false, json!([["os/spawn", target, []]]))]
        });
        schema["orbitals"][0]["traits"][0]["config"] = config;
        orbital_compiler::phases::validation::final_program::validate_final_program(
            &serde_json::from_value::<OrbitalSchema>(schema).expect("schema"),
        )
    }

    fn children(commands: &[&str]) -> Value {
        let list: Vec<Value> = commands.iter().map(|c| json!({ "command": c })).collect();
        json!({ "children": { "type": "array", "default": list } })
    }

    fn fed() -> Value {
        json!(["object/get", ["array/nth", "@config.children", "@entity.cursor"], "command"])
    }

    #[test]
    fn a_config_fed_target_is_grant_checked_in_the_finished_program() {
        let granted = json!({ "exec": ["git"] });
        let outside = finished(granted.clone(), children(&["git", "rm"]), fed());
        assert!(fires(&outside, ErrorCode::OsGrantMissing), "`rm` is not granted: {:?}", outside.errors);
        let inside = finished(granted.clone(), children(&["git"]), fed());
        assert!(!fires(&inside, ErrorCode::OsGrantMissing), "control: {:?}", inside.errors);
        let none = finished(granted.clone(), children(&[]), fed());
        assert!(!fires(&none, ErrorCode::OsGrantMissing), "control: no child, nothing to check: {:?}", none.errors);
        let direct = finished(
            granted.clone(),
            json!({ "cmd": { "type": "string", "default": "rm" } }),
            json!("@config.cmd"),
        );
        assert!(fires(&direct, ErrorCode::OsGrantMissing), "{:?}", direct.errors);
        let direct_ok = finished(granted, json!({ "cmd": { "type": "string", "default": "git" } }), json!("@config.cmd"));
        assert!(!fires(&direct_ok, ErrorCode::OsGrantMissing), "control: {:?}", direct_ok.errors);
    }

    #[test]
    fn a_config_fed_target_the_pass_cannot_resolve_is_not_static() {
        let unresolvable = finished(
            json!({ "exec": ["git"] }),
            children(&["git"]),
            json!(["str/concat", "@config.children", "x"]),
        );
        assert!(fires(&unresolvable, ErrorCode::OsTargetNotStatic), "{:?}", unresolvable.errors);
    }

    fn data_rw() -> Value {
        json!({ "fs": [{ "glob": "/data/**", "modes": ["read"] }] })
    }

    #[test]
    fn a_computed_file_target_under_a_covered_directory_prefix_validates() {
        let covered = one(data_rw(), json!([["os/read-file", ["str/concat", "/data/", "@payload.name"]]]));
        assert!(!fires(&covered, ErrorCode::OsTargetNotStatic), "{:?}", covered.errors);
        assert!(!fires(&covered, ErrorCode::OsGrantMissing), "{:?}", covered.errors);
        assert!(!warned(&covered, ErrorCode::OsGrantUnused), "{:?}", covered.warnings);
        let nested = one(data_rw(), json!([["os/open", ["str/concat", "/data/", "sub/", "@payload.name"], "r"]]));
        assert!(!fires(&nested, ErrorCode::OsTargetNotStatic) && !fires(&nested, ErrorCode::OsGrantMissing), "{:?}", nested.errors);
    }

    #[test]
    fn a_computed_file_target_outside_its_grant_or_mode_still_needs_a_grant() {
        let uncovered = one(data_rw(), json!([["os/read-file", ["str/concat", "/etc/", "@payload.name"]]]));
        assert!(fires(&uncovered, ErrorCode::OsGrantMissing), "{:?}", uncovered.errors);
        let wrong_mode = one(data_rw(), json!([["os/write-file", ["str/concat", "/data/", "@payload.name"], "x"]]));
        assert!(fires(&wrong_mode, ErrorCode::OsGrantMissing), "{:?}", wrong_mode.errors);
        let single_star = one(json!({ "fs": [{ "glob": "/data/*", "modes": ["read"] }] }), json!([["os/read-file", ["str/concat", "/data/", "@payload.name"]]]));
        assert!(fires(&single_star, ErrorCode::OsGrantMissing), "a `/data/*` grant does not cover the tree: {:?}", single_star.errors);
    }

    #[test]
    fn a_computed_file_target_without_a_directory_prefix_stays_not_static() {
        for target in [
            json!(["str/concat", "@payload.name", "/x"]),
            json!(["str/concat", "/data", "@payload.name"]),
            json!(["str/concat", "@payload.root", "/", "@payload.name"]),
            json!(["str/upper", "/data/"]),
        ] {
            let r = one(data_rw(), json!([["os/read-file", target.clone()]]));
            assert!(fires(&r, ErrorCode::OsTargetNotStatic), "{target}: {:?}", r.errors);
        }
        let spawn = one(json!({ "exec": ["git"] }), json!([["os/spawn", ["str/concat", "git", "/", "@payload.x"], []]]));
        assert!(fires(&spawn, ErrorCode::OsTargetNotStatic), "only a file path may be computed: {:?}", spawn.errors);
    }

    fn finished_fs(grants: Value, config: Value, target: Value) -> ValidationResult {
        let mut schema = json!({
            "name": "fixture", "version": "1.0.0",
            "orbitals": [orbital("Orders", grants, false, json!([["os/read-file", target]]))]
        });
        schema["orbitals"][0]["traits"][0]["config"] = config;
        orbital_compiler::phases::validation::final_program::validate_final_program(
            &serde_json::from_value::<OrbitalSchema>(schema).expect("schema"),
        )
    }

    fn root_knob(value: &str) -> Value {
        json!({ "root": { "type": "string", "default": value } })
    }

    fn rooted() -> Value {
        json!(["str/concat", "@config.root", "/", "@payload.path"])
    }

    #[test]
    fn a_config_rooted_computed_file_target_is_grant_checked_in_the_finished_program() {
        let covered = finished_fs(data_rw(), root_knob("/data"), rooted());
        assert!(!fires(&covered, ErrorCode::OsTargetNotStatic), "{:?}", covered.errors);
        assert!(!fires(&covered, ErrorCode::OsGrantMissing), "{:?}", covered.errors);
        let uncovered = finished_fs(data_rw(), root_knob("/etc"), rooted());
        assert!(fires(&uncovered, ErrorCode::OsGrantMissing), "{:?}", uncovered.errors);
        let no_slash = finished_fs(data_rw(), root_knob("/data"), json!(["str/concat", "@config.root", "@payload.path"]));
        assert!(fires(&no_slash, ErrorCode::OsTargetNotStatic), "{:?}", no_slash.errors);
        let undeclared = finished_fs(data_rw(), root_knob("/data"), json!(["str/concat", "@config.other", "/", "@payload.path"]));
        assert!(fires(&undeclared, ErrorCode::OsTargetNotStatic), "{:?}", undeclared.errors);
    }

    #[test]
    fn os_effects_are_admitted_in_effect_position_with_their_emit_keys_checked() {
        let granted = one(read_data(), json!([["os/read-file", "/data/a"]]));
        assert!(!fires(&granted, ErrorCode::EffectUnknownType), "{:?}", granted.errors);
        assert!(!fires(&granted, ErrorCode::OsGrantMissing), "{:?}", granted.errors);
        let envelope = one(read_data(), json!([["os/read-file", "/data/a", { "emit": { "success": "INIT", "failure": "INIT" } }]]));
        assert!(!fires(&envelope, ErrorCode::EffectUnknownType), "{:?}", envelope.errors);
        assert!(!fires(&envelope, ErrorCode::EffectEmitKeyInvalid), "{:?}", envelope.errors);
        let bad_key = one(read_data(), json!([["os/read-file", "/data/a", { "emit": { "on_message": "INIT" } }]]));
        assert!(fires(&bad_key, ErrorCode::EffectEmitKeyInvalid), "{:?}", bad_key.errors);
        let spawn = one(Value::Null, json!([["os/spawn", "@config.cmd", []]]));
        assert!(!fires(&spawn, ErrorCode::EffectUnknownType), "{:?}", spawn.errors);
        let unknown = one(read_data(), json!([["os/nope", "/data/a"]]));
        assert!(fires(&unknown, ErrorCode::EffectUnknownType), "{:?}", unknown.errors);
    }

    #[test]
    fn an_entry_no_call_uses_is_a_warning_and_a_used_one_is_not() {
        let grants = json!({ "fs": [{ "glob": "/data/**", "modes": ["read", "write"] }], "exec": ["git"] });
        let r = one(grants, json!([["os/read-file", "/data/a"]]));
        let labels: Vec<&str> = r.warnings.iter().filter(|e| e.code == ErrorCode::OsGrantUnused).map(|e| e.message.as_str()).collect();
        assert_eq!(labels.len(), 2, "{labels:?} {:?}", r.errors);
        assert!(labels.iter().any(|m| m.contains("write")) && labels.iter().any(|m| m.contains("exec")), "{labels:?}");
        assert!(!fires_as_error(&r, ErrorCode::OsGrantUnused));
        let used = one(read_data(), json!([["os/read-file", "/data/a"]]));
        assert!(!warned(&used, ErrorCode::OsGrantUnused), "{:?}", used.warnings);
    }

    #[test]
    fn an_inherited_spawn_env_entry_uses_its_env_grant_and_needs_one() {
        let spawn = json!([["os/spawn", "git", [], { "env": { "PATH": null } }]]);
        let granted = one(json!({ "exec": ["git"], "env": ["PATH"] }), spawn.clone());
        assert!(!warned(&granted, ErrorCode::OsGrantUnused), "{:?}", granted.warnings);
        assert!(!fires(&granted, ErrorCode::OsGrantMissing), "{:?}", granted.errors);
        let missing = one(json!({ "exec": ["git"] }), spawn);
        assert!(fires(&missing, ErrorCode::OsGrantMissing), "{:?}", missing.errors);
        let literal = one(
            json!({ "exec": ["git"], "env": ["PATH"] }),
            json!([["os/spawn", "git", [], { "env": { "PATH": "/bin" } }]]),
        );
        assert!(warned(&literal, ErrorCode::OsGrantUnused), "control: a literal value uses no env grant: {:?}", literal.warnings);
    }

    fn fires_as_error(r: &ValidationResult, code: ErrorCode) -> bool {
        r.errors.iter().any(|e| e.code == code)
    }

    #[test]
    fn an_app_level_entry_is_used_when_any_orbital_uses_it() {
        let read = json!([["os/read-file", "/data/a"]]);
        let used = run(
            read_data(),
            Value::Null,
            vec![orbital("Orders", Value::Null, false, read), orbital("Billing", Value::Null, false, json!([]))],
        );
        assert!(!warned(&used, ErrorCode::OsGrantUnused), "{:?}", used.warnings);
        let idle = run(read_data(), Value::Null, vec![orbital("Orders", Value::Null, false, json!([]))]);
        assert!(warned(&idle, ErrorCode::OsGrantUnused), "{:?}", idle.warnings);
    }

    #[test]
    fn an_entry_that_can_match_nothing_is_invalid() {
        for grants in [
            json!({ "fs": [{ "glob": "data/**", "modes": ["read"] }] }),
            json!({ "fs": [{ "glob": "/a/../b", "modes": ["read"] }] }),
            json!({ "fs": [{ "glob": "/a/**", "modes": [] }] }),
            json!({ "exec": ["git status"] }),
            json!({ "net": [{ "target": "db.internal", "kinds": ["tcp"] }] }),
            json!({ "env": ["A=B"] }),
        ] {
            assert!(fires(&one(grants.clone(), json!([])), ErrorCode::OsGrantEntryInvalid), "{grants}");
        }
        let good = json!({ "fs": [{ "glob": "/a/**", "modes": ["read"] }], "exec": ["git"], "net": [{ "target": "db.internal:5432", "kinds": ["tcp"] }], "env": ["HOME"] });
        assert!(!fires(&one(good, json!([])), ErrorCode::OsGrantEntryInvalid));
        let app = run(json!({ "exec": [""] }), Value::Null, vec![orbital("Orders", Value::Null, false, json!([]))]);
        assert!(fires(&app, ErrorCode::OsGrantEntryInvalid), "{:?}", app.errors);
    }

    #[test]
    fn an_os_call_on_a_client_leg_is_rejected() {
        let call = json!([["os/read-file", "/data/a"]]);
        let local = run(Value::Null, Value::Null, vec![orbital("Orders", read_data(), true, call.clone())]);
        assert!(fires(&local, ErrorCode::TraitLocalTouchesServer), "{:?}", local.errors);
        let server = run(Value::Null, Value::Null, vec![orbital("Orders", read_data(), false, call)]);
        assert!(!fires(&server, ErrorCode::TraitLocalTouchesServer), "{:?}", server.errors);
    }

    fn two(deployment: Value) -> ValidationResult {
        run(
            Value::Null,
            deployment,
            vec![orbital("Orders", Value::Null, false, json!([])), orbital("Billing", Value::Null, false, json!([]))],
        )
    }

    #[test]
    fn a_deployment_places_every_orbital_exactly_once_in_a_non_empty_unit() {
        let ok = two(json!({ "api": ["Orders"], "web": ["Billing"] }));
        for code in [
            ErrorCode::PlacementOrbitalUnknown,
            ErrorCode::PlacementOrbitalDuplicate,
            ErrorCode::PlacementOrbitalUnplaced,
            ErrorCode::PlacementUnitEmpty,
        ] {
            assert!(!fires(&ok, code), "{code:?}: {:?}", ok.errors);
        }
        assert!(!fires(&two(Value::Null), ErrorCode::PlacementOrbitalUnplaced), "no deployment = one unit");
        assert!(fires(&two(json!({ "api": ["Orders"] })), ErrorCode::PlacementOrbitalUnplaced));
        assert!(fires(&two(json!({ "api": ["Orders", "Billing"], "web": [] })), ErrorCode::PlacementUnitEmpty));
        assert!(fires(&two(json!({ "api": ["Orders", "Billing", "Ghost"] })), ErrorCode::PlacementOrbitalUnknown));
        assert!(fires(&two(json!({ "api": ["Orders", "Billing"], "web": ["Billing"] })), ErrorCode::PlacementOrbitalDuplicate));
        assert!(fires(&two(json!({ "api": ["Orders", "Orders", "Billing"] })), ErrorCode::PlacementOrbitalDuplicate));
    }
}

mod redis {
    //! `redis` effect validation: the origin and its egress, the command, the closed options and the reply union (the
    //! credential slot is `orbital_core::secrets`', pinned in its unit tests). Every rule has a positive case and a control that stays silent.
    use super::*;

    fn app(effects: Value, egress: Value, payload_type: &str) -> ValidationResult {
        let schema: OrbitalSchema = serde_json::from_value(json!({
            "name": "fixture", "version": "1.0.0", "egress": egress,
            "orbitals": [{
                "name": "Cache",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "Reader", "linkedEntity": "Row", "category": "interaction",
                    "config": {
                        "pw": { "type": "secret", "default": "REDIS_PW" },
                        "plain": { "type": "string", "default": "x" },
                        "origin": { "type": "url", "default": "redis://cache.internal:6379" }
                    },
                    "emits": [
                        { "event": "GOT", "payloadSchema": [{ "name": "reply", "type": payload_type, "required": true }] },
                        { "event": "GOT_KEYED", "payloadSchema": [{ "name": "reply", "type": payload_type, "required": true }, { "name": "key", "type": "string", "required": true }] },
                        { "event": "WRONG", "payloadSchema": [{ "name": "value", "type": "string", "required": true }] },
                        { "event": "FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }, { "name": "kind", "type": "string", "required": true }, { "name": "code", "type": "string", "required": true }] }
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
        .expect("schema");
        validate_comprehensive_without_services(&schema)
    }

    fn check(effects: Value) -> ValidationResult {
        app(effects, json!(["redis://cache.internal:6379"]), "string")
    }

    fn routes() -> Value {
        json!({ "emit": { "success": "GOT", "failure": "FAILED" } })
    }

    fn call(origin: Value, command: Value, options: Vec<Value>) -> Value {
        let mut effect = vec![json!("redis"), origin, command];
        effect.extend(options);
        Value::Array(effect)
    }

    const CODES: [ErrorCode; 6] = [
        ErrorCode::RedisCallInvalid,
        ErrorCode::HostEffectArity,
        ErrorCode::HostEffectOptionUnknown,
        ErrorCode::HttpEgressUndeclared,
        ErrorCode::HttpOriginNotStatic,
        ErrorCode::EmitPayloadNotProduced,
    ];

    fn fired(r: &ValidationResult) -> Vec<ErrorCode> {
        CODES.into_iter().filter(|code| fires(r, *code)).collect()
    }

    #[test]
    fn control_well_formed_calls_raise_none_of_the_redis_codes() {
        let r = check(json!([
            call(json!("redis://cache.internal:6379"), json!(["GET", "@payload.k"]), vec![routes()]),
            call(json!("@config.origin"), json!(["SET", "k", "v", "PX", "1000"]), vec![json!({ "auth": "@config.pw", "user": "app", "db": 2 }), routes()]),
            call(json!("redis://cache.internal:6379"), json!(["INCR", "k"]), vec![json!({ "auth": { "credential": "@config.pw", "scope": "app" }, "key": "@entity.id", "emit": { "success": "GOT_KEYED", "failure": "FAILED" } })]),
            call(json!("redis://cache.internal"), json!(["PING"]), vec![]),
        ]));
        assert!(fired(&r).is_empty(), "{:?}", r.errors);
        assert!(!fires(&r, ErrorCode::HttpEgressUndeclared), "{:?}", r.errors);
    }

    #[test]
    fn an_origin_outside_egress_is_undeclared_and_an_http_entry_never_authorizes_it() {
        let call = call(json!("redis://other.internal:6379"), json!(["PING"]), vec![]);
        assert!(fires(&check(json!([call.clone()])), ErrorCode::HttpEgressUndeclared));
        let http_only = app(json!([call]), json!(["https://other.internal"]), "string");
        assert!(fires(&http_only, ErrorCode::HttpEgressUndeclared));
        let rediss = app(json!([self::call(json!("rediss://cache.internal:6379"), json!(["PING"]), vec![])]), json!(["redis://cache.internal:6379"]), "string");
        assert!(fires(&rediss, ErrorCode::HttpEgressUndeclared), "rediss is a different scheme from redis");
    }

    #[test]
    fn the_origin_is_a_redis_origin_without_a_path() {
        for origin in ["https://cache.internal", "postgres://cache.internal:5432", "redis://cache.internal:6379/0", "cache.internal"] {
            let r = app(
                json!([call(json!(origin), json!(["PING"]), vec![])]),
                json!(["redis://cache.internal:6379", "https://cache.internal", "postgres://cache.internal:5432"]),
                "string",
            );
            assert!(fires(&r, ErrorCode::RedisCallInvalid), "{origin}: {:?}", r.errors);
        }
    }

    #[test]
    fn an_origin_computed_at_run_time_is_static_only_when_egress_is_declared() {
        let dynamic = call(json!("@payload.host"), json!(["PING"]), vec![]);
        assert!(!fires(&check(json!([dynamic.clone()])), ErrorCode::HttpOriginNotStatic), "the driver refuses what egress does not list");
        let none = app(json!([dynamic]), json!([]), "string");
        assert!(fires(&none, ErrorCode::HttpOriginNotStatic));
    }

    #[test]
    fn the_command_is_a_non_empty_list_of_text() {
        for command in [json!([]), json!([""]), json!(["SET", "k", 5]), json!(["SET", "k", true]), json!(["GET", null]), json!(["GET", { "a": 1 }])] {
            let r = check(json!([call(json!("redis://cache.internal"), command.clone(), vec![])]));
            assert!(fires(&r, ErrorCode::RedisCallInvalid), "{command}: {:?}", r.errors);
        }
        let bound = check(json!([call(json!("redis://cache.internal"), json!(["GET", "@payload.k", "@entity.id"]), vec![])]));
        assert!(!fires(&bound, ErrorCode::RedisCallInvalid), "{:?}", bound.errors);
    }

    #[test]
    fn the_wrong_number_of_arguments_is_an_arity_error() {
        for effect in [json!(["redis"]), json!(["redis", "redis://cache.internal"]), json!(["redis", "redis://cache.internal", ["PING"], {}, {}, {}])] {
            assert!(fires(&check(json!([effect.clone()])), ErrorCode::HostEffectArity), "{effect}");
        }
    }

    #[test]
    fn the_options_are_closed_objects_that_share_one_namespace() {
        let unknown = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "timeout": 5 })])]));
        assert!(fires(&unknown, ErrorCode::HostEffectOptionUnknown));
        let cancelled = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "emit": { "cancelled": "GOT" } })])]));
        assert!(fires(&cancelled, ErrorCode::EffectEmitKeyInvalid), "a redis call is not cancellable");
        let not_object = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!("x")])]));
        assert!(fires(&not_object, ErrorCode::HostEffectOptionUnknown));
        let twice = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "db": 1 }), json!({ "db": 2 })])]));
        assert!(fires(&twice, ErrorCode::RedisCallInvalid));
        let split = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "db": 1 }), json!({ "key": "k" })])]));
        assert!(!fires(&split, ErrorCode::RedisCallInvalid) && !fires(&split, ErrorCode::HostEffectOptionUnknown), "{:?}", split.errors);
    }

    #[test]
    fn a_user_needs_a_password_and_a_database_is_a_whole_number() {
        let user_alone = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "user": "app" })])]));
        assert!(fires(&user_alone, ErrorCode::RedisCallInvalid));
        for db in [json!(1.5), json!(-1)] {
            let r = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "db": db.clone() })])]));
            assert!(fires(&r, ErrorCode::RedisCallInvalid), "{db}");
        }
        let fine = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "db": 0 })])]));
        assert!(!fires(&fine, ErrorCode::RedisCallInvalid));
    }

    #[test]
    fn the_key_is_a_non_empty_string() {
        for key in [json!(""), json!(5)] {
            let r = check(json!([call(json!("redis://cache.internal"), json!(["PING"]), vec![json!({ "key": key.clone() })])]));
            assert!(fires(&r, ErrorCode::CallServiceKeyType), "{key}");
        }
    }

    #[test]
    fn the_declared_success_payload_must_fit_what_redis_produces() {
        let unread = check(json!([call(json!("redis://cache.internal"), json!(["GET", "k"]), vec![json!({ "emit": { "success": "WRONG" } })])]));
        assert!(fires(&unread, ErrorCode::EmitPayloadNotProduced), "{:?}", unread.errors);
        let unkeyed = check(json!([call(json!("redis://cache.internal"), json!(["GET", "k"]), vec![json!({ "emit": { "success": "GOT_KEYED" } })])]));
        assert!(!fires(&unkeyed, ErrorCode::EmitPayloadNotProduced), "a redis reply always carries its key: {:?}", unkeyed.errors);
    }

    #[test]
    fn the_reply_fits_a_declared_union_of_text_numbers_null_and_lists_of_them_and_nothing_else() {
        for fits in ["string", "number", "string | null", "[string]", "string | number | null", "[string | null]", "[[string]]"] {
            let r = app(json!([call(json!("redis://cache.internal"), json!(["GET", "k"]), vec![routes()])]), json!(["redis://cache.internal:6379"]), fits);
            assert!(!fires(&r, ErrorCode::EmitPayloadNotProduced), "{fits}: {:?}", r.errors);
        }
        for rejects in ["boolean"] {
            let r = app(json!([call(json!("redis://cache.internal"), json!(["GET", "k"]), vec![routes()])]), json!(["redis://cache.internal:6379"]), rejects);
            assert!(fires(&r, ErrorCode::EmitPayloadNotProduced), "{rejects}: {:?}", r.errors);
        }
    }
}

mod postgres {
    //! `postgres` effect validation: the origin and its egress, the read-only statement, the params, the closed options
    //! and the result payload. Every rule has a positive case and a control that stays silent.
    use super::*;

    fn app(effects: Value, egress: Value, rows_type: &str) -> ValidationResult {
        let schema: OrbitalSchema = serde_json::from_value(json!({
            "name": "fixture", "version": "1.0.0", "egress": egress,
            "orbitals": [{
                "name": "Db",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "Reader", "linkedEntity": "Row", "category": "interaction",
                    "config": {
                        "pw": { "type": "secret", "default": "PG_PW" },
                        "origin": { "type": "url", "default": "postgres://db.internal:5432" }
                    },
                    "emits": [
                        { "event": "ROWS", "payloadSchema": [{ "name": "rows", "type": rows_type, "required": true }, { "name": "rowCount", "type": "number", "required": true }] },
                        { "event": "WRONG", "payloadSchema": [{ "name": "reply", "type": "string", "required": true }] },
                        { "event": "FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }, { "name": "kind", "type": "string", "required": true }, { "name": "code", "type": "string", "required": true }] }
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
        .expect("schema");
        validate_comprehensive_without_services(&schema)
    }

    fn check(effects: Value) -> ValidationResult {
        app(effects, json!(["postgres://db.internal:5432"]), "array")
    }

    fn call(origin: Value, sql: Value, params: Value, options: Vec<Value>) -> Value {
        let mut effect = vec![json!("postgres"), origin, sql, params];
        effect.extend(options);
        Value::Array(effect)
    }

    fn user() -> Value {
        json!({ "user": "app" })
    }

    fn routes() -> Value {
        json!({ "emit": { "success": "ROWS", "failure": "FAILED" } })
    }

    const CODES: [ErrorCode; 5] = [
        ErrorCode::PostgresCallInvalid,
        ErrorCode::HostEffectArity,
        ErrorCode::HostEffectOptionUnknown,
        ErrorCode::HttpEgressUndeclared,
        ErrorCode::EmitPayloadNotProduced,
    ];

    fn fired(r: &ValidationResult) -> Vec<ErrorCode> {
        CODES.into_iter().filter(|code| fires(r, *code)).collect()
    }

    #[test]
    fn control_well_formed_calls_raise_none_of_the_postgres_codes() {
        let r = check(json!([
            call(json!("postgres://db.internal:5432"), json!("SELECT id FROM t WHERE id = $1"), json!(["@payload.id"]), vec![json!({ "auth": "@config.pw", "user": "app", "database": "main", "tls": true, "timeoutMs": 5000 }), routes()]),
            call(json!("@config.origin"), json!("WITH x AS (SELECT 1) SELECT * FROM x"), json!([]), vec![user(), routes()]),
            call(json!("postgres://db.internal"), json!("@payload.sql"), json!("@payload.params"), vec![user()]),
        ]));
        assert!(fired(&r).is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn an_origin_outside_egress_or_of_the_wrong_scheme_is_refused() {
        let outside = call(json!("postgres://other.internal"), json!("SELECT 1"), json!([]), vec![user()]);
        assert!(fires(&check(json!([outside.clone()])), ErrorCode::HttpEgressUndeclared));
        assert!(fires(&app(json!([outside]), json!(["https://other.internal"]), "array"), ErrorCode::HttpEgressUndeclared));
        for origin in ["redis://db.internal:5432", "https://db.internal", "postgres://db.internal:5432/main"] {
            let r = app(json!([call(json!(origin), json!("SELECT 1"), json!([]), vec![user()])]), json!(["postgres://db.internal:5432", "redis://db.internal:5432", "https://db.internal"]), "array");
            assert!(fires(&r, ErrorCode::PostgresCallInvalid), "{origin}: {:?}", r.errors);
        }
    }

    #[test]
    fn a_literal_statement_must_be_one_read_only_select() {
        for sql in ["DELETE FROM t", "UPDATE t SET a = 1", "SELECT 1; DROP TABLE t", "SELECT * INTO c FROM t", "INSERT INTO t VALUES (1)", "SELECT * FROM t FOR UPDATE", ""] {
            let r = check(json!([call(json!("postgres://db.internal:5432"), json!(sql), json!([]), vec![user()])]));
            assert!(fires(&r, ErrorCode::PostgresCallInvalid), "{sql}: {:?}", r.errors);
        }
        let ok = check(json!([call(json!("postgres://db.internal:5432"), json!("SELECT 'delete into' AS note -- DROP"), json!([]), vec![user()])]));
        assert!(!fires(&ok, ErrorCode::PostgresCallInvalid), "{:?}", ok.errors);
    }

    #[test]
    fn the_params_are_a_list_and_the_arity_is_four_to_six() {
        for params in [json!("x"), json!(5), json!({ "a": 1 })] {
            let r = check(json!([call(json!("postgres://db.internal:5432"), json!("SELECT $1"), params.clone(), vec![user()])]));
            assert!(fires(&r, ErrorCode::PostgresCallInvalid), "{params}");
        }
        for effect in [json!(["postgres"]), json!(["postgres", "postgres://db.internal:5432", "SELECT 1"]), json!(["postgres", "postgres://db.internal:5432", "SELECT 1", [], {}, {}, {}])] {
            assert!(fires(&check(json!([effect.clone()])), ErrorCode::HostEffectArity), "{effect}");
        }
    }

    #[test]
    fn the_options_are_closed_share_one_namespace_and_name_a_user() {
        let sql = json!("SELECT 1");
        let c = |opts: Vec<Value>| check(json!([call(json!("postgres://db.internal:5432"), sql.clone(), json!([]), opts)]));
        assert!(fires(&c(vec![]), ErrorCode::PostgresCallInvalid), "a user is required");
        assert!(fires(&c(vec![json!({ "user": "a", "timeout": 1 })]), ErrorCode::HostEffectOptionUnknown));
        assert!(fires(&c(vec![json!({ "user": "a", "emit": { "cancelled": "ROWS" } })]), ErrorCode::EffectEmitKeyInvalid));
        assert!(fires(&c(vec![json!("x")]), ErrorCode::HostEffectOptionUnknown));
        assert!(fires(&c(vec![json!({ "user": "a" }), json!({ "user": "b" })]), ErrorCode::PostgresCallInvalid), "given twice");
        assert!(!fires(&c(vec![json!({ "user": "a" }), json!({ "key": "k" })]), ErrorCode::PostgresCallInvalid));
        for bad in [json!({ "user": "a", "timeoutMs": 0 }), json!({ "user": "a", "timeoutMs": 1.5 }), json!({ "user": "a", "tls": "yes" }), json!({ "user": "" }), json!({ "user": "a", "database": "" })] {
            assert!(fires(&c(vec![bad.clone()]), ErrorCode::PostgresCallInvalid), "{bad}");
        }
        for key in [json!(""), json!(5)] {
            assert!(fires(&c(vec![json!({ "user": "a", "key": key.clone() })]), ErrorCode::CallServiceKeyType), "{key}");
        }
    }

    #[test]
    fn the_declared_success_payload_must_fit_what_postgres_produces() {
        let wrong = check(json!([call(json!("postgres://db.internal:5432"), json!("SELECT 1"), json!([]), vec![user(), json!({ "emit": { "success": "WRONG" } })])]));
        assert!(fires(&wrong, ErrorCode::EmitPayloadNotProduced), "{:?}", wrong.errors);
        let right = check(json!([call(json!("postgres://db.internal:5432"), json!("SELECT 1"), json!([]), vec![user(), routes()])]));
        assert!(!fires(&right, ErrorCode::EmitPayloadNotProduced), "{:?}", right.errors);
    }
}

#[test]
fn access_check_takes_one_request_and_optional_routes_and_its_verdict_payload_is_checked() {
    let request = json!({ "entity": "Row", "action": "read", "row": {}, "viewer": null });
    let ok = check(json!([["access-check", request.clone(), { "emit": { "success": "VERDICT", "failure": "DRAW_FAILED" } }]]));
    for code in [ErrorCode::HostEffectArity, ErrorCode::HostEffectOptionUnknown, ErrorCode::EmitPayloadNotProduced] {
        assert!(!fires(&ok, code), "{code:?}: {:?}", ok.errors);
    }
    assert!(fires(&check(json!([["access-check"]])), ErrorCode::HostEffectArity));
    assert!(fires(&check(json!([["access-check", request.clone(), {}, {}]])), ErrorCode::HostEffectArity));
    assert!(fires(&check(json!([["access-check", request.clone(), { "key": "k" }]])), ErrorCode::HostEffectOptionUnknown));
    let spoiled = check(json!([["access-check", request, { "emit": { "success": "SPOILED" } }]]));
    assert!(fires(&spoiled, ErrorCode::EmitPayloadNotProduced), "{:?}", spoiled.errors);
}
