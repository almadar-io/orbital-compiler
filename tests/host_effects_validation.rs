//! `orb validate` rules of `random-bytes`, `credential-status` and `credential-forget`
//! (`docs/Almadar_Embedded_Kernel.md` H.7 P4, P6): every code has a positive case and a control that stays silent.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core::schema::OrbitalSchema;
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
    let r = check(json!([["random-bytes", 8, { "key": "k" }]]));
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
