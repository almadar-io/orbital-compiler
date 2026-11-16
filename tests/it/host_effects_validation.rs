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
                "name": "Drawer", "linkedEntity": "Row", 
                "config": { "tok": { "type": "secret", "default": "TOK" } },
                "emits": [
                    { "event": "DRAWN", "payloadSchema": [{ "name": "value", "type": "string", "required": true }] },
                    { "event": "DRAW_FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] },
                    { "event": "DRAWN_KEYED", "payloadSchema": [{ "name": "value", "type": "string", "required": true }, { "name": "key", "type": "string", "required": true }] },
                    { "event": "KNOWN", "payloadSchema": [{ "name": "connected", "type": "boolean", "required": true }] },
                    { "event": "VERDICT", "payloadSchema": [{ "name": "allowed", "type": "boolean", "required": true }] },
                    { "event": "SPOILED", "payloadSchema": [{ "name": "bytes", "type": "string", "required": true }] },
                    { "event": "NAMES_LISTED", "payloadSchema": [{ "name": "result", "type": "[string]", "required": true }] },
                    { "event": "COUNT_LISTED", "payloadSchema": [{ "name": "result", "type": "number", "required": true }] },
                    { "event": "MEMORIES_READ", "payloadSchema": [{ "name": "result", "type": "[object]", "required": true, "properties": [{ "name": "id", "type": "string" }, { "name": "bogus", "type": "string" }] }] },
                    { "event": "MEMORIES_READ_BY_ID", "payloadSchema": [{ "name": "result", "type": "[object]", "required": true, "properties": [{ "name": "id", "type": "string" }] }] }
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

fn check_declaring(effects: Value, types: Value) -> ValidationResult {
    let mut schema = serde_json::to_value(program(effects)).expect("program");
    schema["types"] = types;
    validate_comprehensive_without_services(&serde_json::from_value(schema).expect("schema"))
}

fn fires(r: &ValidationResult, code: ErrorCode) -> bool {
    r.errors.iter().chain(r.warnings.iter()).any(|e| e.code == code)
}

fn routes() -> Value {
    json!({ "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } })
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
fn random_bytes_key_routes_what_the_call_produces() {
    let keyed = |key: Value| json!([["random-bytes", 8, { "key": key, "emit": { "success": "DRAWN_KEYED", "failure": "DRAW_FAILED" } }]]);
    let ok = check(keyed(json!("@entity.id")));
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "{:?}", ok.errors);
    let unkeyed = check(json!([["random-bytes", 8, { "emit": { "success": "DRAWN_KEYED" } }]]));
    assert!(fires(&unkeyed, ErrorCode::EmitPayloadNotProduced), "an unkeyed call produces no key");
}

#[test]
fn every_credential_effect_is_a_known_effect() {
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
    assert!(!fires(&ok, ErrorCode::EffectUnknownType), "every credential effect is a known effect: {:?}", ok.errors);
}

#[test]
fn the_credential_effect_payloads_are_checked() {
    let ok = check(json!([["credential-rotate", { "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } }]]));
    assert!(fires(&ok, ErrorCode::EmitPayloadNotProduced), "DRAWN reads `value`, which rotate does not produce");
}

#[test]
fn the_access_check_verdict_payload_is_checked() {
    let request = json!({ "entity": "Row", "action": "read", "row": {}, "identity": null });
    let ok = check(json!([["access-check", request.clone(), { "emit": { "success": "VERDICT", "failure": "DRAW_FAILED" } }]]));
    for code in [ErrorCode::EmitPayloadNotProduced] {
        assert!(!fires(&ok, code), "{code:?}: {:?}", ok.errors);
    }
    let spoiled = check(json!([["access-check", request, { "emit": { "success": "SPOILED" } }]]));
    assert!(fires(&spoiled, ErrorCode::EmitPayloadNotProduced), "{:?}", spoiled.errors);
}

#[test]
fn the_host_port_effects_are_known_effects() {
    for effect in [json!(["auth/sign-out"]), json!(["audio/play-sound", "footstep"]), json!(["audio/stop-music"])] {
        let result = check(json!([effect]));
        assert!(!fires(&result, ErrorCode::EffectUnknownType), "{effect}: {:?}", result.errors);
    }
}

#[test]
fn control_an_operator_no_table_declares_is_still_unknown() {
    let result = check(json!([["auth/sign-sideways"]]));
    assert!(fires(&result, ErrorCode::EffectUnknownType), "{:?}", result.errors);
}

#[test]
fn a_substrate_operators_success_payload_is_its_result_and_is_checked() {
    let route = |event: &str| json!([["workspace/list-orbitals", { "emit": { "success": event } }]]);
    let wrong_field = check(route("DRAWN"));
    assert!(fires(&wrong_field, ErrorCode::EmitPayloadNotProduced), "DRAWN reads `value`, which a substrate op never answers: {:?}", wrong_field.errors);
    let wrong_type = check(route("COUNT_LISTED"));
    assert!(fires(&wrong_type, ErrorCode::EmitPayloadNotProduced), "the result is a list of names, not a number: {:?}", wrong_type.errors);
    let ok = check(route("NAMES_LISTED"));
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "control: result as the list it is: {:?}", ok.errors);
}

#[test]
fn a_bare_name_return_type_is_judged_only_when_a_declared_shape_resolves_it() {
    let read = json!([["session/read-memory", "k", { "emit": { "success": "MEMORIES_READ" } }]]);
    let record = json!({ "AgentMemoryRecord": [{ "name": "id", "type": "string" }] });
    let declared = check_declaring(read.clone(), record.clone());
    assert!(
        fires(&declared, ErrorCode::EmitPayloadNotProduced),
        "`bogus` is not a member of the declared AgentMemoryRecord: {:?}",
        declared.errors
    );
    let by_id = check_declaring(json!([["session/read-memory", "k", { "emit": { "success": "MEMORIES_READ_BY_ID" } }]]), record);
    assert!(!fires(&by_id, ErrorCode::EmitPayloadNotProduced), "control: a member the shape declares: {:?}", by_id.errors);
    let undeclared = check(read);
    assert!(
        !fires(&undeclared, ErrorCode::EmitPayloadNotProduced),
        "control: a name no program or language declares is undecidable, not judged: {:?}",
        undeclared.errors
    );
}
