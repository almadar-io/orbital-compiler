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
fn the_success_payload_a_handler_declares_must_be_one_the_effect_produces() {
    let spoiled = check(json!([["random-bytes", 8, { "emit": { "success": "SPOILED" } }]]));
    assert!(fires(&spoiled, ErrorCode::EmitPayloadNotProduced), "{:?}", spoiled.errors);
    let ok = check(json!([["random-bytes", 8, { "emit": { "success": "DRAWN" } }]]));
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "{:?}", ok.errors);
    let status = check(json!([["credential-status", "@config.tok", { "emit": { "success": "KNOWN" } }]]));
    assert!(!fires(&status, ErrorCode::EmitPayloadNotProduced), "{:?}", status.errors);
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
                "name": format!("{name}Runner"), "linkedEntity": format!("{name}Row"), "category": "interaction", "local": local,
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
            json!(["os/read-file", ["str/concat", "/data/", "@payload.name"]]),
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
