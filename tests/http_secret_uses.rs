//! The secret-flow rules reach an atom loaded through `uses`: its knobs are
//! typed in the atom, so the walk runs on the files the inline session loaded
//! (`InlineSession::each_loaded_schema`). Inline keeps every read of a secret
//! knob as `@config.<knob>`, so the same walk holds on the resolved program.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig, InlineSession};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::secrets::secret_flow_violations;
use serde_json::{json, Value};

fn upstream(effects: Value) -> String {
    json!({
        "name": "secret-upstream", "version": "1.0.0",
        "orbitals": [{
            "name": "ThingOrbital",
            "entity": { "name": "Thing", "persistence": "runtime",
                "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "token", "type": "string" }] },
            "traits": [{
                "name": "ThingFetch", "linkedEntity": "Thing", "category": "interaction",
                "config": { "apiKey": { "type": "secret", "default": "THING_KEY" } },
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "GO" }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": effects }]
                }
            }],
            "pages": []
        }]
    })
    .to_string()
}

const CONSUMER: &str = r#"{
  "name": "secret-consumer", "version": "1.0.0",
  "orbitals": [{
    "name": "Imported",
    "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
    "entity": "Up.orbitals.ThingOrbital.entity",
    "traits": [],
    "reference": { "ref": "Up.orbitals.ThingOrbital" }
  }]
}"#;

fn violation_codes(effects: Value) -> (Vec<&'static str>, Vec<&'static str>) {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("upstream.orb"), upstream(effects)).unwrap();
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).unwrap();

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let session = InlineSession::default();
    let config = InlineConfig { session: Some(session.clone()), ..InlineConfig::default() };
    inline_schema(&mut schema, &config, Some(&consumer_path)).expect("inline");

    let mut loaded = Vec::new();
    session.each_loaded_schema(&mut |l| {
        let program = serde_json::to_value(&l.schema).unwrap();
        loaded.extend(secret_flow_violations(&program).iter().map(|v| v.code()));
    });
    let inlined = serde_json::to_value(&schema).unwrap();
    let after_inline = secret_flow_violations(&inlined).iter().map(|v| v.code()).collect();
    (loaded, after_inline)
}

#[test]
fn a_secret_misused_in_an_atom_reached_through_uses_is_found_in_the_loaded_file() {
    let (loaded, after_inline) =
        violation_codes(json!([["set", "@entity.token", "@config.apiKey"]]));
    assert_eq!(loaded, vec!["ORB_SECRET_OUTSIDE_AUTH"]);
    assert_eq!(after_inline, vec!["ORB_SECRET_OUTSIDE_AUTH"], "the resolved program keeps the read");
}

fn inlined_trait(effects: Value) -> Value {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("upstream.orb"), upstream(effects)).unwrap();
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).unwrap();
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("inline");
    serde_json::to_value(&schema).unwrap()["orbitals"][0]["traits"][0].clone()
}

#[test]
fn a_secret_credential_stays_a_config_read_in_the_resolved_program_with_its_value_on_the_knob() {
    let call = json!([["http", "GET", "https://a.example", "/s",
        { "auth": { "scheme": "bearer", "credential": "@config.apiKey" } }, { "emit": { "success": "GO" } }]]);
    let resolved = inlined_trait(call.clone());
    let effect = &resolved["stateMachine"]["transitions"][0]["effects"][0];
    assert_eq!(effect[4]["auth"]["credential"], "@config.apiKey", "{effect}");
    assert_eq!(resolved["config"]["apiKey"]["default"], "THING_KEY");
    let (_, after_inline) = violation_codes(call);
    assert!(after_inline.is_empty(), "{after_inline:?}");
}

#[test]
fn control_a_non_secret_knob_read_is_still_substituted() {
    let mut upstream_program: Value = serde_json::from_str(&upstream(json!([["set", "@entity.token", "@config.label"]]))).unwrap();
    upstream_program["orbitals"][0]["traits"][0]["config"]["label"] = json!({ "type": "string", "default": "Plain" });
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("upstream.orb"), upstream_program.to_string()).unwrap();
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).unwrap();
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("inline");
    let resolved = serde_json::to_value(&schema).unwrap();
    assert_eq!(resolved["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0], json!(["set", "@entity.token", "Plain"]));
}

#[test]
fn a_secret_read_in_a_standalone_program_stays_a_config_read() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("standalone.orb");
    std::fs::write(&path, upstream(json!([["http", "GET", "https://a.example", "/s",
        { "auth": { "scheme": "bearer", "credential": "@config.apiKey" } }, { "emit": { "success": "GO" } }]]))).unwrap();
    let mut schema = parse_file(&path).expect("standalone parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    let resolved = serde_json::to_value(&schema).unwrap();
    assert_eq!(resolved["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0][4]["auth"]["credential"], "@config.apiKey");
    assert!(secret_flow_violations(&resolved).is_empty());
}

#[test]
fn a_non_secret_credential_in_an_atom_reached_through_uses_is_found() {
    let call = json!([["http", "GET", "https://a.example", "/s",
        { "auth": { "scheme": "bearer", "credential": "LITERAL" } }, { "emit": { "success": "GO" } }]]);
    let (loaded, _) = violation_codes(call);
    assert_eq!(loaded, vec!["ORB_HTTP_AUTH_CREDENTIAL_NOT_SECRET"]);
}

#[test]
fn control_an_atom_keeping_its_secret_in_auth_is_clean() {
    let call = json!([["http", "GET", "https://a.example", "/s",
        { "auth": { "scheme": "bearer", "credential": "@config.apiKey" } }, { "emit": { "success": "GO" } }]]);
    let (loaded, _) = violation_codes(call);
    assert!(loaded.is_empty(), "{loaded:?}");
}

fn grant_auths() -> [Value; 3] {
    [
        json!({"scheme":"token-exchange","tokenUrl":"https://a.example/token","grant":"refresh_token",
            "body":{"form":{"refresh_token":{"credential":"@config.apiKey","scope":"user"}}},
            "clientAuth":"post","clientId":"@config.apiKey","clientSecret":"@config.apiKey"}),
        json!({"scheme":"oauth2-client","tokenUrl":"https://a.example/token",
            "clientId":"@config.apiKey","clientSecret":"@config.apiKey"}),
        json!({"scheme":"jwt-bearer","tokenUrl":"https://a.example/token","key":"@config.apiKey",
            "alg":"RS256","claims":{"aud":"https://a.example/token","iat":1000,"exp":4600}}),
    ]
}

#[test]
fn loaded_grant_atoms_keep_declared_credentials_legal_with_public_grant_options() {
    for mut auth in grant_auths() {
        auth["grantRequest"] = json!({"headers":{"X-Grant":"public"},"timeoutMs":15000,
            "retry":{"max":3,"on":["1xx","408","5xx","429","timeout","failed"],
                "scheduleMs":[100,500,1500],"noResponseMax":2}});
        let effects = json!([["http","POST","https://a.example","/s",{"auth":auth},{"emit":{"success":"GO"}}]]);
        let (loaded, after_inline) = violation_codes(effects);
        assert!(loaded.is_empty(), "{auth}: {loaded:?}");
        assert!(!after_inline.contains(&"ORB_SECRET_OUTSIDE_AUTH"), "{auth}: {after_inline:?}");
    }
}

#[test]
fn grant_request_secret_leaks_are_found_in_the_loaded_atom_and_the_resolved_program() {
    for auth in grant_auths() {
        for options in [
            json!({"headers":{"X-Grant":"@config.apiKey"}}),
            json!({"headers":{"X-Grant":["str/concat","public:","@config.apiKey"]}}),
            json!({"headers":{"X-Grant":{"credential":"@config.apiKey","scope":"user"}}}),
            json!({"timeoutMs":"@config.apiKey"}),
            json!({"retry":{"max":1,"on":["failed"],"scheduleMs":["@config.apiKey"]}}),
        ] {
            let mut configured = auth.clone();
            configured["grantRequest"] = options.clone();
            let effects = json!([["http","POST","https://a.example","/s",{"auth":configured},{"emit":{"success":"GO"}}]]);
            let (loaded, after_inline) = violation_codes(effects);
            assert_eq!(loaded, vec!["ORB_SECRET_OUTSIDE_AUTH"], "{auth}: {options}");
            assert_eq!(after_inline, vec!["ORB_SECRET_OUTSIDE_AUTH"], "the resolved program keeps the read: {auth}: {options}");
        }
    }
}

#[test]
fn loaded_grant_atoms_still_reject_non_secret_fixed_credential_slots() {
    for auth in grant_auths() {
        let slots: &[&str] = if auth["scheme"] == "jwt-bearer" { &["key"] } else { &["clientId", "clientSecret"] };
        for slot in slots {
            let mut configured = auth.clone();
            configured[*slot] = json!("LITERAL");
            configured["grantRequest"] = json!({"headers":{"X-Grant":"public"}});
            let effects = json!([["http","POST","https://a.example","/s",{"auth":configured},{"emit":{"success":"GO"}}]]);
            let (loaded, _) = violation_codes(effects);
            assert_eq!(loaded, vec!["ORB_HTTP_AUTH_CREDENTIAL_NOT_SECRET"], "{auth}: {slot}");
        }
    }
}
