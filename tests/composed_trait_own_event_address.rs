//! G-CROSS-046 follow-up: an `event`-typed knob that addresses the atom's OWN
//! trait (`PageWatch.ITEM_FOUND`) must follow a composed-trait rename
//! (`trait FeedWatch = Watch.traits.PageWatch`) everywhere its value reaches,
//! including the copy `@config.itemEvent` substitutes into an effect arg, not
//! only the config default.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};

fn atom(default_address: &str) -> String {
    format!(
        r#"{{
  "name": "page-watch",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "PageWatchOrbital",
    "entity": {{
      "name": "Watched", "persistence": "runtime",
      "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
    }},
    "traits": [{{
      "name": "PageWatch",
      "linkedEntity": "Watched",
      "category": "lifecycle",
      "config": {{ "itemEvent": {{ "type": "event", "default": "{default_address}" }} }},
      "stateMachine": {{
        "states": [ {{ "name": "idle", "isInitial": true }} ],
        "events": [
          {{ "key": "INIT" }},
          {{ "key": "ITEM_FOUND", "input": true }}
        ],
        "transitions": [
          {{ "from": "idle", "to": "idle", "event": "INIT",
            "effects": [ ["browser/tab-watch", {{ "event": "@config.itemEvent" }}] ] }},
          {{ "from": "idle", "to": "idle", "event": "ITEM_FOUND" }}
        ]
      }}
    }}],
    "pages": [ {{ "name": "WatchPage", "path": "/watch", "traits": [ {{ "ref": "PageWatch" }} ] }} ]
  }}]
}}"#
    )
}

fn consumer(events: &str) -> String {
    format!(
        r#"{{
  "name": "consumer",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "FeedOrbital",
    "uses": [ {{ "from": "./page-watch.orb", "as": "Watch" }} ],
    "entity": {{
      "name": "Feed", "persistence": "runtime",
      "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
    }},
    "traits": [ {{ "ref": "Watch.traits.PageWatch", "name": "FeedWatch"{events} }} ],
    "pages": [ {{ "name": "Home", "path": "/", "traits": [ {{ "ref": "FeedWatch" }} ] }} ]
  }}]
}}"#
    )
}

fn inline(atom_json: &str, consumer_json: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("page-watch.orb"), atom_json).expect("write atom");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer_json).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    schema
}

fn feed_watch(schema: &OrbitalSchema) -> &TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "FeedWatch" => Some(td),
            _ => None,
        })
        .expect("FeedWatch is inlined")
}

fn effect_event(schema: &OrbitalSchema) -> String {
    let init = feed_watch(schema)
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "INIT")
        .expect("INIT transition");
    let effect = &init.effects.as_ref().expect("INIT effects")[0];
    let SExpression::Array(items) = effect else {
        panic!("effect list: {effect:?}")
    };
    let SExpression::Object(args) = &items[1] else {
        panic!("tab-watch request: {:?}", items[1])
    };
    match args.get("event") {
        Some(SExpression::String(s)) => s.clone(),
        other => panic!("event arg: {other:?}"),
    }
}

fn config_event(schema: &OrbitalSchema) -> String {
    match feed_watch(schema)
        .config
        .as_ref()
        .and_then(|c| c.get("itemEvent"))
        .and_then(|f| f.default.clone())
    {
        Some(SExpression::String(s)) => s,
        other => panic!("itemEvent default: {other:?}"),
    }
}

#[test]
fn own_trait_address_reaches_the_effect_copy_after_a_rename() {
    let schema = inline(&atom("PageWatch.ITEM_FOUND"), &consumer(""));
    assert_eq!(effect_event(&schema), "FeedWatch.ITEM_FOUND");
}

#[test]
fn control_the_config_default_follows_the_rename_too() {
    let schema = inline(&atom("PageWatch.ITEM_FOUND"), &consumer(""));
    assert_eq!(config_event(&schema), "FeedWatch.ITEM_FOUND");
}

#[test]
fn control_an_address_naming_another_trait_is_left_alone() {
    let schema = inline(&atom("OtherTrait.ITEM_FOUND"), &consumer(""));
    assert_eq!(effect_event(&schema), "OtherTrait.ITEM_FOUND");
}

#[test]
fn edge_an_events_rename_of_the_addressed_event_carries_into_the_effect() {
    let schema = inline(
        &atom("PageWatch.ITEM_FOUND"),
        &consumer(r#", "events": { "ITEM_FOUND": "VIDEO_FOUND" }"#),
    );
    assert_eq!(effect_event(&schema), "FeedWatch.VIDEO_FOUND");
}
