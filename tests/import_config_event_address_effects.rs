//! G-CROSS-046: a consumer's import-body `config` override that names another
//! import's trait (`Things.ThingPersistor.DO_SAVE`, local orbital + upstream
//! trait) must land on the materialized `ThingsThingPersistor` everywhere the
//! knob's value reaches, including the copy substituted into an effect arg
//! (`call-service webhook send { tools: @config.tools }`), not only the
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
      "category": "lifecycle",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [
          { "key": "INIT" },
          { "key": "DO_SAVE", "external": true, "payloadSchema": [ { "name": "title", "type": "string", "required": true } ] }
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
      "category": "interaction",
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
            "effects": [ ["call-service", "webhook", "send", { "messages": [], "tools": "@config.tools" }, { "emit": { "success": "ANSWERED" } }] ] },
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
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("things.orb"), THINGS).expect("write things");
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
        Some(SExpression::String(s)) => s.clone(),
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
        Some(SExpression::String(s)) => s.clone(),
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
