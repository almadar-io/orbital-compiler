//! G-CROSS-046: a consumer's import-body `config` override that names another
//! import's trait (`Things.ThingPersistor.DO_SAVE`, local orbital + upstream
//! trait) must land on the materialized `ThingsThingPersistor` everywhere the
//! knob's value reaches, including the copy substituted into an effect arg
//! (`call-service test-svc run { tools: @config.tools }`), not only the
//! config default.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};

const THINGS: &str = r#"{
  "name": "things",
  "version": "1.0.0",
  "orbitals": [{
    "name": "ThingOrbital",
    "entity": {
      "name": "Thing", "persistence": "persistent", "collection": "things",
      "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ]
    },
    "traits": [{
      "name": "ThingPersistor",
      "linkedEntity": "Thing",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [
          { "key": "INIT" },
          { "key": "DO_SAVE", "input": true, "payloadSchema": [ { "name": "title", "type": "string", "required": true } ] }
        ],
        "transitions": [
          { "from": "idle", "to": "idle", "event": "INIT" },
          { "from": "idle", "to": "idle", "event": "DO_SAVE", "effects": [ ["persist", "create", "Thing", "@payload"] ] }
        ]
      }
    }],
    "pages": [ { "name": "ThingsPage", "path": "/things", "traits": [ { "ref": "ThingPersistor" } ] } ]
  }]
}"#;

const ASSISTANT: &str = r#"{
  "name": "assistant",
  "version": "1.0.0",
  "orbitals": [{
    "name": "AssistantOrbital",
    "config": {
      "tools": {
        "type": "array",
        "items": { "type": "object", "properties": { "event": { "type": "event" } } },
        "default": []
      }
    },
    "entity": {
      "name": "AssistantState", "persistence": "runtime",
      "fields": [ { "name": "id", "type": "string", "required": true } ]
    },
    "traits": [{
      "name": "AssistantLoop",
      "linkedEntity": "AssistantState",
      "config": {
        "tools": {
          "type": "array",
          "items": { "type": "object", "properties": { "event": { "type": "event" } } },
          "default": "@config.tools"
        }
      },
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [ { "key": "INIT" }, { "key": "ASK" }, { "key": "ANSWERED" } ],
        "transitions": [
          { "from": "idle", "to": "idle", "event": "INIT" },
          { "from": "idle", "to": "idle", "event": "ASK",
            "effects": [ ["call-service", "test-svc", "run", { "messages": [], "tools": "@config.tools" }, { "emit": { "success": "ANSWERED" } }] ] },
          { "from": "idle", "to": "idle", "event": "ANSWERED" }
        ]
      }
    }],
    "pages": [ { "name": "AssistantPage", "path": "/assistant", "traits": [ { "ref": "AssistantLoop" } ] } ]
  }]
}"#;

fn consumer(tool: &str) -> String {
    format!(
        r#"{{
  "name": "consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Things",
      "uses": [ {{ "from": "./things.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.ThingOrbital" }}
    }},
    {{
      "name": "Helper",
      "uses": [ {{ "from": "./assistant.orb", "as": "Ag" }} ],
      "entity": "Ag.orbitals.AssistantOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Ag.orbitals.AssistantOrbital",
        "config": {{ "tools": {{ "type": "array", "default": [ {{ "event": "{tool}" }} ] }} }}
      }}
    }}
  ]
}}"#
    )
}

fn inline(tool: &str) -> OrbitalSchema {
    inline_with(tool, THINGS)
}

fn inline_with(tool: &str, things: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("things.orb"), things).expect("write things");
    std::fs::write(dir.path().join("assistant.orb"), ASSISTANT).expect("write assistant");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer(tool)).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    schema
}

fn assistant_loop(schema: &OrbitalSchema) -> &orbital_core_base::schema::TraitDefinition {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name.ends_with("AssistantLoop") => Some(td),
            _ => None,
        })
        .expect("the assistant loop is inlined")
}

/// The `tools[0].event` of the `call-service` effect on the ASK transition.
fn effect_tool_event(schema: &OrbitalSchema) -> String {
    let td = assistant_loop(schema);
    let ask = td
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "ASK")
        .expect("ASK transition");
    let effect = &ask.effects.as_ref().expect("ASK effects")[0];
    let SExpression::Array(items) = effect else {
        panic!("effect is a list: {effect:?}")
    };
    let SExpression::Object(args) = &items[3] else {
        panic!("call-service args: {:?}", items[3])
    };
    let Some(SExpression::Array(tools)) = args.get("tools") else {
        panic!("tools substituted: {args:?}")
    };
    let SExpression::Object(first) = &tools[0] else {
        panic!("tool entry: {:?}", tools[0])
    };
    match first.get("event") {
        Some(SExpression::String(s)) => s.clone().into_inner(),
        other => panic!("tool event: {other:?}"),
    }
}

fn config_tool_event(schema: &OrbitalSchema) -> String {
    let td = assistant_loop(schema);
    let tools = td
        .config
        .as_ref()
        .and_then(|c| c.get("tools"))
        .and_then(|f| f.default.clone())
        .expect("tools default");
    let SExpression::Array(tools) = tools else {
        panic!("tools list")
    };
    let SExpression::Object(first) = &tools[0] else {
        panic!("tool entry")
    };
    match first.get("event") {
        Some(SExpression::String(s)) => s.clone().into_inner(),
        other => panic!("tool event: {other:?}"),
    }
}

#[test]
fn import_config_address_reaches_the_effect_copy() {
    let schema = inline("Things.ThingPersistor.DO_SAVE");
    assert_eq!(
        effect_tool_event(&schema),
        "Things.ThingsThingPersistor.DO_SAVE"
    );
}

#[test]
fn control_config_default_lands_on_the_materialized_trait_too() {
    let schema = inline("Things.ThingPersistor.DO_SAVE");
    assert_eq!(
        config_tool_event(&schema),
        "Things.ThingsThingPersistor.DO_SAVE"
    );
}

#[test]
fn edge_any_source_address_is_never_rewritten() {
    let schema = inline("*.DO_SAVE");
    assert_eq!(effect_tool_event(&schema), "*.DO_SAVE");
}

#[test]
fn edge_address_on_an_orbital_that_is_not_imported_is_untouched() {
    let schema = inline("Elsewhere.ThingPersistor.DO_SAVE");
    assert_eq!(
        effect_tool_event(&schema),
        "Elsewhere.ThingPersistor.DO_SAVE"
    );
}

const GEN_ATOM: &str = r#"{
  "name": "genatom",
  "version": "1.0.0",
  "orbitals": [{
    "name": "AtomOrb",
    "entity": {
      "name": "GenState", "persistence": "runtime",
      "fields": [ { "name": "id", "type": "string", "required": true } ]
    },
    "traits": [{
      "name": "Gen",
      "linkedEntity": "GenState",
      "config": {
        "chooseEvent": { "type": "EventAddress", "default": "AtomOrb.Gen.CHOOSE" }
      },
      "emits": [
        { "event": "CONVERSE", "payloadSchema": [
          { "name": "label", "type": "string" },
          { "name": "tools", "type": "[object]", "properties": [
            { "name": "event", "type": "EventAddress", "required": true },
            { "name": "note", "type": "string" }
          ] }
        ] }
      ],
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [ { "key": "INIT" }, { "key": "ASK" }, { "key": "CHOOSE", "input": true }, { "key": "CONVERSE" } ],
        "transitions": [
          { "from": "idle", "to": "idle", "event": "INIT" },
          { "from": "idle", "to": "idle", "event": "CHOOSE" },
          { "from": "idle", "to": "idle", "event": "CONVERSE" },
          { "from": "idle", "to": "idle", "event": "ASK",
            "effects": [ ["emit", "CONVERSE", { "label": "AtomOrb.Gen.CHOOSE", "tools": [
              { "event": "AtomOrb.Gen.CHOOSE", "note": "AtomOrb.Gen.CHOOSE" },
              { "event": "Elsewhere.Gen.CHOOSE" }
            ] }] ] }
        ]
      }
    }],
    "pages": [ { "name": "GenPage", "path": "/gen", "traits": [ { "ref": "Gen" } ] } ]
  }]
}"#;

fn inline_gen_atom() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("genatom.orb"), GEN_ATOM).expect("write atom");
    let consumer = r#"{
      "name": "consumer", "version": "1.0.0",
      "orbitals": [{
        "name": "Local",
        "uses": [ { "from": "./genatom.orb", "as": "Up" } ],
        "entity": "Up.orbitals.AtomOrb.entity",
        "traits": [],
        "reference": { "ref": "Up.orbitals.AtomOrb" }
      }]
    }"#;
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    schema
}

fn gen_trait(schema: &OrbitalSchema) -> &orbital_core_base::schema::TraitDefinition {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "LocalGen" => Some(td),
            _ => None,
        })
        .expect("the renamed trait is inlined")
}

fn string_at(value: Option<&SExpression>) -> String {
    match value {
        Some(SExpression::String(s)) => s.clone().into_inner(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn emitted_payload(schema: &OrbitalSchema) -> impl std::ops::Deref<Target = orbital_core_base::schema::SObject> {
    let td = gen_trait(schema);
    let ask = td
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "ASK")
        .expect("ASK transition");
    let SExpression::Array(items) = &ask.effects.as_ref().expect("ASK effects")[0] else {
        panic!("effect is a list")
    };
    let SExpression::Object(payload) = &items[2] else {
        panic!("emit payload: {:?}", items[2])
    };
    payload.clone()
}

#[test]
fn import_repoints_an_own_address_in_an_event_address_config_default() {
    let schema = inline_gen_atom();
    let default = gen_trait(&schema)
        .config
        .as_ref()
        .and_then(|c| c.get("chooseEvent"))
        .and_then(|f| f.default.as_ref())
        .map(|d| string_at(Some(d)));
    assert_eq!(default.as_deref(), Some("Local.LocalGen.CHOOSE"));
}

#[test]
fn import_repoints_an_own_address_in_an_emitted_event_address_member() {
    let schema = inline_gen_atom();
    let payload = emitted_payload(&schema);
    let SExpression::Array(tools) = payload.get("tools").expect("tools") else {
        panic!("tools list")
    };
    let SExpression::Object(first) = &tools[0] else {
        panic!("tool entry")
    };
    assert_eq!(string_at(first.get("event")), "Local.LocalGen.CHOOSE");
}

#[test]
fn control_a_string_typed_member_and_a_foreign_address_stay_as_written() {
    let schema = inline_gen_atom();
    let payload = emitted_payload(&schema);
    assert_eq!(string_at(payload.get("label")), "AtomOrb.Gen.CHOOSE");
    let SExpression::Array(tools) = payload.get("tools").expect("tools") else {
        panic!("tools list")
    };
    let SExpression::Object(first) = &tools[0] else {
        panic!("tool entry")
    };
    assert_eq!(string_at(first.get("note")), "AtomOrb.Gen.CHOOSE");
    let SExpression::Object(second) = &tools[1] else {
        panic!("tool entry")
    };
    assert_eq!(string_at(second.get("event")), "Elsewhere.Gen.CHOOSE");
}

fn prove_tool_names(event_key: &str) -> Vec<orbital_core_eval::proof::Finding> {
    let long_key = THINGS.replace("DO_SAVE", event_key);
    let mut schema = inline_with(&format!("Things.ThingPersistor.{event_key}"), &long_key);
    let loop_name = assistant_loop(&schema).name.clone();
    let holds = serde_json::json!(["array/every", "@config.tools", ["fn", "t",
        ["if", ["object/has", "@t", "event"],
            ["validate/pattern", ["str/replaceAll", ["object/get", "@t", "event"], ".", "__"], "^[a-zA-Z0-9_-]{1,64}$"],
            true]]]);
    let expect = serde_json::from_value(serde_json::json!({
        "kind": "state", "traitName": loop_name, "state": "idle", "holds": holds, "strict": true
    }))
    .expect("expectation");
    schema.orbitals[1].expects.get_or_insert_with(Default::default).push(expect);
    orbital_core_eval::proof::prove(&schema)
}

#[test]
fn a_short_literal_whose_resolved_address_is_over_64_chars_violates_the_tool_name_proposition() {
    let key = "DO_SAVE_THE_THING_WITH_A_VERY_LONG_KEY";
    let schema = inline_with(&format!("Things.ThingPersistor.{key}"), &THINGS.replace("DO_SAVE", key));
    let resolved = config_tool_event(&schema);
    assert!(resolved.len() > 64 - 2 && format!("Things.ThingPersistor.{key}").replace('.', "__").len() <= 64, "{resolved}");
    let findings = prove_tool_names(key);
    assert!(
        findings.iter().any(|f| matches!(f.verdict, orbital_core_eval::proof::Verdict::Violated { .. }))
            && findings.iter().all(|f| !matches!(f.verdict, orbital_core_eval::proof::Verdict::Unproved { .. })),
        "{findings:#?}"
    );
}

#[test]
fn control_a_short_resolved_address_proves_the_tool_name_proposition() {
    let findings = prove_tool_names("DO_SAVE");
    assert!(
        findings.iter().all(|f| matches!(f.verdict, orbital_core_eval::proof::Verdict::Proved)) && !findings.is_empty(),
        "{findings:#?}"
    );
}
