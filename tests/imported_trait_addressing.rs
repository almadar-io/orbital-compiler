//! A consumer addresses an imported orbital's trait by the import's LOCAL
//! orbital name and the trait's UPSTREAM name — `Things.ThingPersistor.SAVED`
//! in `listens`, `Things.ThingPersistor.DO_SAVE` as an event value — and inline
//! lands both on the materialized `<Local><Upstream>` trait.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core::schema::{ListenSource, OrbitalSchema, SExpression, TraitReference};

const UPSTREAM: &str = r#"{
  "name": "upstream",
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
          { "from": "idle", "to": "idle", "event": "DO_SAVE",
            "effects": [ ["persist", "create", "Thing", "@payload"], ["emit", "SAVED", {}] ] }
        ]
      },
      "emits": [ { "event": "SAVED", "scope": "external" } ]
    }],
    "pages": [ { "name": "ThingsPage", "path": "/things", "traits": [ { "ref": "ThingPersistor" } ] } ]
  }]
}"#;

fn consumer(listen_trait: &str, tool: &str) -> String {
    consumer_with(listen_trait, tool, "")
}

/// `body_traits`: traits the consumer authors inside the import body.
fn consumer_with(listen_trait: &str, tool: &str, body_traits: &str) -> String {
    format!(
        r#"{{
  "name": "consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Things",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [{body_traits}],
      "reference": {{ "ref": "Up.orbitals.ThingOrbital" }}
    }},
    {{
      "name": "Watcher",
      "entity": {{
        "name": "Note", "persistence": "persistent", "collection": "notes",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [{{
        "name": "NoteLog",
        "linkedEntity": "Note",
        "category": "interaction",
        "config": {{
          "tool": {{ "type": "event", "default": "{tool}" }},
          "tools": {{
            "type": "array",
            "items": {{ "type": "object", "properties": {{ "event": {{ "type": "event" }} }} }},
            "default": [ {{ "event": "{tool}" }} ]
          }}
        }},
        "stateMachine": {{
          "states": [ {{ "name": "idle", "isInitial": true }} ],
          "events": [ {{ "key": "INIT" }}, {{ "key": "LOG" }} ],
          "transitions": [
            {{ "from": "idle", "to": "idle", "event": "INIT",
               "effects": [ ["render-ui", "main", {{ "type": "stack" }}] ] }},
            {{ "from": "idle", "to": "idle", "event": "LOG" }}
          ]
        }},
        "listens": [
          {{ "event": "SAVED", "triggers": "LOG",
             "source": {{ "kind": "orbital", "orbital": "Things", "trait": "{listen_trait}" }} }}
        ]
      }}],
      "pages": [ {{ "name": "NotesPage", "path": "/notes", "traits": [ {{ "ref": "NoteLog" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn inline(listen_trait: &str, tool: &str) -> OrbitalSchema {
    inline_src(&consumer(listen_trait, tool))
}

fn inline_src(src: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, src).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    schema
}

fn note_log(schema: &OrbitalSchema) -> &orbital_core::schema::TraitDefinition {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "NoteLog" => Some(td),
            _ => None,
        })
        .expect("NoteLog")
}

fn values(schema: &OrbitalSchema) -> (String, String) {
    let cfg = note_log(schema).config.clone().unwrap_or_default();
    let tool = match cfg.get("tool").and_then(|f| f.default.clone()) {
        Some(SExpression::String(s)) => s,
        other => panic!("tool: {other:?}"),
    };
    let tools = cfg
        .get("tools")
        .and_then(|f| f.default.clone())
        .and_then(|d| serde_json::to_value(d).ok())
        .and_then(|v| v[0]["event"].as_str().map(str::to_string))
        .expect("tools[0].event");
    (tool, tools)
}

fn listen_trait(schema: &OrbitalSchema) -> String {
    match note_log(schema)
        .listens
        .as_deref()
        .unwrap_or(&[])
        .first()
        .and_then(|l| l.source.clone())
    {
        Some(ListenSource::Orbital { trait_name, .. }) => trait_name,
        other => panic!("listen source: {other:?}"),
    }
}

fn codes(schema: &OrbitalSchema) -> Vec<String> {
    let result = validate_comprehensive_without_services(schema);
    result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .filter(|c| c.contains("LISTEN_SOURCE") || c.contains("EVENT_REF"))
        .collect()
}

#[test]
fn a_consumer_listen_on_an_imported_trait_lands_on_the_materialized_name() {
    let schema = inline("ThingPersistor", "Things.ThingPersistor.DO_SAVE");
    assert_eq!(listen_trait(&schema), "ThingsThingPersistor");
    assert!(codes(&schema).is_empty(), "{:?}", codes(&schema));
}

#[test]
fn a_consumer_event_value_on_an_imported_input_lands_on_the_materialized_name() {
    let schema = inline("ThingPersistor", "Things.ThingPersistor.DO_SAVE");
    let (tool, tools) = values(&schema);
    assert_eq!(tool, "Things.ThingsThingPersistor.DO_SAVE");
    assert_eq!(tools, "Things.ThingsThingPersistor.DO_SAVE");
}

#[test]
fn an_unknown_trait_of_an_import_stays_unresolved() {
    let schema = inline("NoSuchTrait", "Things.NoSuchTrait.DO_SAVE");
    let c = codes(&schema);
    assert!(
        c.iter().any(|c| c == "ORB_X_LISTEN_SOURCE_UNRESOLVED"),
        "{c:?}"
    );
    assert!(c.iter().any(|c| c == "ORB_T_EVENT_REF_UNRESOLVED"), "{c:?}");
}

#[test]
fn control_the_materialized_name_also_resolves() {
    let schema = inline(
        "ThingsThingPersistor",
        "Things.ThingsThingPersistor.DO_SAVE",
    );
    assert_eq!(listen_trait(&schema), "ThingsThingPersistor");
    assert!(codes(&schema).is_empty(), "{:?}", codes(&schema));
}

/// A trait the consumer authors in the import body under an upstream trait's
/// name is the trait that name means — it is not remapped onto the import.
#[test]
fn a_body_trait_named_like_an_upstream_trait_keeps_its_own_name() {
    let body = r#"{
        "name": "ThingPersistor",
        "linkedEntity": "Thing",
        "category": "interaction",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["emit", "SAVED", {}] ] } ]
        },
        "emits": [ { "event": "SAVED", "scope": "external" } ]
      }"#;
    let schema = inline_src(&consumer_with(
        "ThingPersistor",
        "Things.ThingsThingPersistor.DO_SAVE",
        body,
    ));
    assert_eq!(listen_trait(&schema), "ThingPersistor");
}
