//! Payload spread at inline (P-040): an emit's or an event's `payloadSpread: "$q"` takes the fields of the record the
//! call site binds `q` to (or the parameter's declared default) first, flat, then the payload's own fields, and the
//! marker is cleared. A field the record and the payload both name leaves the marker for the type-parameter sweep.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, TraitDefinition, TraitReference};

fn atom(type_params: &str, own_field: &str) -> String {
    format!(
        r#"{{
  "name": "relay-atom",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "RelayAtomOrbital",
    "entity": {{ "name": "RelayRun", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
    "traits": [{{
      "name": "Front",
      "linkedEntity": "RelayRun",
      "typeParams": {type_params},
      "emits": [ {{ "event": "Asked", "payloadSpread": "$q", "payloadSchema": [ {{ "name": "{own_field}", "type": "string" }} ] }} ],
      "stateMachine": {{
        "states": [ {{ "name": "idle", "isInitial": true }} ],
        "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT" }} ],
        "events": [ {{ "key": "ASK", "payloadSpread": "$q", "payloadSchema": [ {{ "name": "{own_field}", "type": "string" }} ] }} ]
      }}
    }}],
    "types": {{ "DefaultReq": [ {{ "name": "query", "type": "string" }} ] }}
  }}]
}}"#
    )
}

fn composer(type_args: &str) -> String {
    format!(
        r#"{{
  "name": "relay-composer",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "RelayComposerOrbital",
    "uses": [ {{ "from": "./atom.orb", "as": "Atom" }} ],
    "entity": {{ "name": "Run", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
    "traits": [ {{ "ref": "Atom.traits.Front", "linkedEntity": "Run"{type_args} }} ],
    "types": {{ "ListReq": [ {{ "name": "search", "type": "string" }}, {{ "name": "limit", "type": "number" }} ] }}
  }}]
}}"#
    )
}

fn inline(type_params: &str, own_field: &str, type_args: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom(type_params, own_field)).expect("write atom");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&composer_path, composer(type_args)).expect("write composer");
    let mut schema = parse_file(&composer_path).expect("composer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&composer_path)).expect("inline");
    schema
}

fn front(schema: &OrbitalSchema) -> TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name.ends_with("Front") => Some((**def).clone()),
            _ => None,
        })
        .expect("the inlined Front")
}

fn emit_names(def: &TraitDefinition) -> (Vec<String>, Option<String>) {
    let emit = def.emits.as_ref().unwrap().iter().find(|e| e.event == "Asked").unwrap().clone();
    (emit.payload_schema.unwrap().iter().map(|f| f.name.clone()).collect(), emit.payload_spread)
}

fn event_names(def: &TraitDefinition) -> (Vec<String>, Option<String>) {
    let ev = def.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "ASK").unwrap().clone();
    (ev.payload_schema.unwrap().iter().map(|f| f.name.clone()).collect(), ev.payload_spread)
}

#[test]
fn a_call_site_struct_argument_fills_the_spread_ahead_of_the_payloads_own_fields() {
    let def = front(&inline(r#"[{"name": "q", "kind": "Type", "default": "DefaultReq"}]"#, "key", r#", "typeArgs": {"q": "ListReq"}"#));
    assert_eq!(emit_names(&def), (vec!["search".to_string(), "limit".into(), "key".into()], None));
    assert_eq!(event_names(&def), (vec!["search".to_string(), "limit".into(), "key".into()], None));
}

#[test]
fn control_with_no_call_site_argument_the_declared_default_fills_the_spread() {
    let def = front(&inline(r#"[{"name": "q", "kind": "Type", "default": "DefaultReq"}]"#, "key", ""));
    assert_eq!(emit_names(&def), (vec!["query".to_string(), "key".into()], None));
}

#[test]
fn a_field_the_record_and_the_payload_both_name_leaves_the_marker() {
    let def = front(&inline(r#"[{"name": "q", "kind": "Type", "default": "DefaultReq"}]"#, "search", r#", "typeArgs": {"q": "ListReq"}"#));
    assert_eq!(emit_names(&def), (vec!["search".to_string()], Some("$q".to_string())));
}
