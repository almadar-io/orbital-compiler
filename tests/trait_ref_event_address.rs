//! A pulled sibling's `Trait.EVENT` config value names its host by the atom's
//! ORIGINAL trait name; when the consumer renames the host (and its events), the
//! address follows exactly as the sibling's `listens` source does.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{SExpression, TraitReference};

const ATOM: &str = r#"{
  "name": "atom",
  "version": "1.0.0",
  "orbitals": [{
    "name": "SrcOrbital",
    "entity": { "name": "Src", "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ] },
    "traits": [
      {
        "name": "HostMain",
        "linkedEntity": "Src",
        "category": "interaction",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "OPEN" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
            "effects": [ ["render-ui", "main", { "type": "stack", "children": [ "@trait.GridView" ] }] ] } ]
        },
        "emits": [ { "event": "OPEN" } ]
      },
      {
        "name": "GridView",
        "linkedEntity": "Src",
        "category": "interaction",
        "config": {
          "target": { "type": "event", "default": "HostMain.OPEN" },
          "tools": { "type": "array", "default": [ { "event": "HostMain.OPEN" } ],
                     "items": { "type": "object", "properties": { "event": { "type": "event" } } } }
        },
        "listens": [ { "event": "OPEN", "triggers": "REFRESH", "source": { "kind": "trait", "trait": "HostMain" } } ],
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "REFRESH" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "REFRESH" } ]
        }
      }
    ],
    "pages": [ { "name": "SrcPage", "path": "/src", "linkedEntity": "Src", "traits": [ { "ref": "HostMain" } ] } ]
  }]
}"#;

fn consumer(events: &str) -> String {
    format!(
        r#"{{
  "name": "consumer",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "AppOrbital",
    "uses": [ {{ "from": "./atom.orb", "as": "Host" }} ],
    "entity": {{ "name": "Thing", "persistence": "persistent",
                "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
    "traits": [ {{ "ref": "Host.traits.HostMain", "linkedEntity": "Thing", "name": "ThingHost"{events} }} ],
    "pages": [ {{ "name": "AppPage", "path": "/app", "linkedEntity": "Thing", "traits": [ {{ "ref": "ThingHost" }} ] }} ]
  }}]
}}"#
    )
}

fn grid_values(events: &str) -> (String, String) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer(events)).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    let grid = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "GridView" => Some(td),
            _ => None,
        })
        .expect("GridView pulled");
    let cfg = grid.config.clone().unwrap_or_default();
    let target = match cfg.get("target").and_then(|f| f.default.clone()) {
        Some(SExpression::String(s)) => s,
        other => panic!("target: {other:?}"),
    };
    let tool = cfg
        .get("tools")
        .and_then(|f| f.default.clone())
        .and_then(|d| serde_json::to_value(d).ok())
        .and_then(|v| v[0]["event"].as_str().map(str::to_string))
        .expect("tools[0].event");
    (target.into_inner(), tool)
}

#[test]
fn a_trait_address_follows_the_host_rename() {
    assert_eq!(
        grid_values(""),
        ("ThingHost.OPEN".to_string(), "ThingHost.OPEN".to_string())
    );
}

#[test]
fn a_trait_address_follows_the_host_events_rename() {
    assert_eq!(
        grid_values(r#", "events": { "OPEN": "OPEN_THING" }"#),
        (
            "ThingHost.OPEN_THING".to_string(),
            "ThingHost.OPEN_THING".to_string()
        )
    );
}
