//! `ORB_HTTP_AUTH_ON_CLIENT` (`docs/Almadar_Embedded_Kernel.md` H.7, P12): an `http` with `auth`
//! is server-only, so it is refused where its trait runs on the client; a keyless `http` is
//! allowed there. Each refusal has a keyless control.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

const ORIGIN: &str = "https://api.example.com";

fn keyless() -> Value {
    json!(["http", "GET", ORIGIN, "/x", { "query": { "q": "a" } }])
}

fn keyed() -> Value {
    json!(["http", "GET", ORIGIN, "/x", { "auth": { "scheme": "bearer", "credential": "@config.key" } }])
}

fn program(local: bool, persistence: &str, effects: Value, ticks: Value) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "fixture", "version": "1.0.0",
        "egress": [ORIGIN],
        "orbitals": [{
            "name": "Feed",
            "entity": {
                "name": "Row", "persistence": persistence, "collection": "rows",
                "fields": [{ "name": "id", "type": "string", "required": true }]
            },
            "traits": [{
                "name": "Loader", "linkedEntity": "Row", "category": "interaction", "local": local,
                "config": { "key": { "type": "secret", "default": "API_KEY" } },
                "ticks": ticks,
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

fn codes(schema: &OrbitalSchema) -> Vec<ErrorCode> {
    let result = validate_comprehensive_without_services(schema);
    result.errors.iter().chain(result.warnings.iter()).map(|e| e.code.clone()).collect()
}

fn tick(effects: Value, background: bool) -> Value {
    json!([{ "name": "poll", "interval": "30s", "runsInBackground": background, "effects": effects }])
}

#[test]
fn an_authenticated_http_in_a_local_trait_is_refused() {
    let found = codes(&program(true, "runtime", json!([keyed()]), json!([])));
    assert!(found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
}

#[test]
fn control_a_keyless_http_in_a_local_trait_is_allowed() {
    let found = codes(&program(true, "runtime", json!([keyless()]), json!([])));
    assert!(!found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
    assert!(!found.contains(&ErrorCode::TraitLocalTouchesServer), "{found:?}");
}

#[test]
fn an_authenticated_http_in_a_browser_run_tick_is_refused() {
    let found = codes(&program(false, "runtime", json!([]), tick(json!([keyed()]), false)));
    assert!(found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
}

#[test]
fn control_a_keyless_http_in_a_browser_run_tick_is_allowed() {
    let found = codes(&program(false, "runtime", json!([]), tick(json!([keyless()]), false)));
    assert!(!found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
    assert!(!found.contains(&ErrorCode::TraitClientTickServerEffect), "{found:?}");
}

#[test]
fn control_a_background_tick_runs_on_the_server_and_may_carry_auth() {
    let found = codes(&program(false, "runtime", json!([]), tick(json!([keyed()]), true)));
    assert!(!found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
}

#[test]
fn control_a_server_awaited_trait_may_carry_auth_in_a_transition_and_a_tick() {
    let found = codes(&program(false, "persistent", json!([keyed()]), tick(json!([keyed()]), false)));
    assert!(!found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
}

#[test]
fn edge_an_authenticated_http_nested_in_a_conditional_is_found() {
    let nested = json!(["if", true, keyed()]);
    let found = codes(&program(true, "runtime", json!([nested]), json!([])));
    assert!(found.contains(&ErrorCode::HttpAuthOnClient), "{found:?}");
}

#[test]
fn a_transition_mixing_a_keyless_http_with_a_server_effect_is_mixed_residence() {
    let call = json!(["call-service", "stripe", "createCustomer", {}]);
    let found = codes(&program(false, "persistent", json!([keyless(), call]), json!([])));
    assert!(found.contains(&ErrorCode::CrossMixedResidence), "{found:?}");
}

#[test]
fn control_a_keyless_http_beside_only_client_work_is_not_mixed() {
    let render = json!(["render-ui", "main", { "type": "typography", "content": "x" }]);
    let found = codes(&program(false, "persistent", json!([keyless(), render]), json!([])));
    assert!(!found.contains(&ErrorCode::CrossMixedResidence), "{found:?}");
}

#[test]
fn control_a_server_effect_beside_an_authenticated_http_is_not_mixed() {
    let call = json!(["call-service", "stripe", "createCustomer", {}]);
    let found = codes(&program(false, "persistent", json!([keyed(), call]), json!([])));
    assert!(!found.contains(&ErrorCode::CrossMixedResidence), "{found:?}");
}
