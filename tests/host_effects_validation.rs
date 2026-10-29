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
fn random_bytes_takes_a_string_key() {
    let keyed = |key: Value| json!([["random-bytes", 8, { "key": key, "emit": { "success": "DRAWN_KEYED", "failure": "DRAW_FAILED" } }]]);
    let ok = check(keyed(json!("@entity.id")));
    assert!(!fires(&ok, ErrorCode::CallServiceKeyType), "{:?}", ok.errors);
    assert!(!fires(&ok, ErrorCode::EmitPayloadNotProduced), "{:?}", ok.errors);
    assert!(fires(&check(keyed(json!(5))), ErrorCode::CallServiceKeyType));
    assert!(fires(&check(keyed(json!(""))), ErrorCode::CallServiceKeyType));
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

mod os_grants {
    //! `ORB_OS_*` and `ORB_PLACEMENT_*`: every code has a positive case and a control that stays silent.
    use super::*;

    fn orbital(name: &str, grants: Value, local: bool, effects: Value) -> Value {
        let mut o = json!({
            "name": name,
            "entity": { "name": format!("{name}Row"), "persistence": "runtime", "fields": [{ "name": "id", "type": "string", "required": true }] },
            "traits": [{
                "name": format!("{name}Runner"), "linkedEntity": format!("{name}Row"), "dispatch": local.then_some("client"),
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

}

#[test]
fn the_access_check_verdict_payload_is_checked() {
    let request = json!({ "entity": "Row", "action": "read", "row": {}, "viewer": null });
    let ok = check(json!([["access-check", request.clone(), { "emit": { "success": "VERDICT", "failure": "DRAW_FAILED" } }]]));
    for code in [ErrorCode::EmitPayloadNotProduced] {
        assert!(!fires(&ok, code), "{code:?}: {:?}", ok.errors);
    }
    let spoiled = check(json!([["access-check", request, { "emit": { "success": "SPOILED" } }]]));
    assert!(fires(&spoiled, ErrorCode::EmitPayloadNotProduced), "{:?}", spoiled.errors);
}
