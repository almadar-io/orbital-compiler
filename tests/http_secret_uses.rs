//! The secret-flow rules reach an atom loaded through `uses`: its knobs are
//! typed in the atom, and inline substitutes their reads, so the walk runs on
//! the files the inline session loaded (`InlineSession::each_loaded_schema`).

use orbital_compiler::phases::inline::{inline_schema, InlineConfig, InlineSession};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::secrets::secret_flow_violations;
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
    assert!(after_inline.is_empty(), "inline already substituted the read: {after_inline:?}");
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
