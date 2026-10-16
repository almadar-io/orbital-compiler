//! G-ORB-031: a whole-orbital import's `pages { "/up": "/local" }` remap must
//! follow `(navigate "/up" …)` targets (and exact config page-path strings) in
//! SIBLING imports from the same upstream alias, not just in the orbitals that
//! own the remapped page. Two imports remapping the same upstream path to
//! DIFFERENT local paths leave the sibling's target untouched (ambiguous — no
//! guess), so the existing unreachable-navigate check reports it.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, TraitReference};

const UPSTREAM: &str = r#"{
  "name": "sibling-remap-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ChannelOrbital",
      "entity": {
        "name": "Channel",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ChannelList",
          "linkedEntity": "Channel",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "OPEN", "effects": [ ["navigate", "/channels/:id"] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "ChannelsPage", "path": "/channels", "linkedEntity": "Channel", "traits": [ { "ref": "ChannelList" } ] },
        { "name": "ChannelPage", "path": "/channels/:id", "linkedEntity": "Channel", "traits": [ { "ref": "ChannelList" } ] }
      ]
    },
    {
      "name": "RoomOrbital",
      "entity": {
        "name": "Room",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "RoomView",
          "linkedEntity": "Room",
          "category": "interaction",
          "config": {
            "navItems": { "type": "array", "default": [ { "href": "/channels", "label": "Channels" } ] }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "BACK" }, { "key": "OPEN" }, { "key": "ELSE" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "BACK", "effects": [ ["navigate", "/channels"] ] },
              { "from": "idle", "to": "idle", "event": "OPEN", "effects": [ ["navigate", ["str/concat", "/channels/", "@entity.id"]] ] },
              { "from": "idle", "to": "idle", "event": "ELSE", "effects": [ ["navigate", "/elsewhere"] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "RoomPage", "path": "/room", "linkedEntity": "Room", "traits": [ { "ref": "RoomView" } ] }
      ]
    }
  ]
}"#;

fn channel_import(name: &str, prefix: &str) -> String {
    format!(
        r#"{{
      "name": "{name}",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.ChannelOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Up.orbitals.ChannelOrbital",
        "pages": {{ "/channels": "{prefix}/channels", "/channels/:id": "{prefix}/channels/:id" }}
      }}
    }}"#
    )
}

const ROOM_IMPORT: &str = r#"{
      "name": "RoomImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.RoomOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.RoomOrbital" }
    }"#;

fn inline_consumer(orbitals: &[String]) -> OrbitalSchema {
    let consumer = format!(
        r#"{{ "name": "sibling-remap-consumer", "version": "1.0.0", "orbitals": [ {} ] }}"#,
        orbitals.join(",")
    );
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline must succeed");
    schema
}

fn room_view_json(schema: &OrbitalSchema) -> String {
    let orbital = schema
        .orbitals
        .iter()
        .find(|o| o.name == "RoomImport")
        .expect("RoomImport");
    let td = orbital
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "RoomImportRoomView" => Some(td),
            _ => None,
        })
        .expect("RoomImportRoomView");
    serde_json::to_string(td).unwrap()
}

#[test]
fn sibling_import_navigate_follows_the_remap_of_another_import_from_the_same_upstream() {
    let schema = inline_consumer(&[channel_import("ChannelImport", "/chat"), ROOM_IMPORT.into()]);
    let json = room_view_json(&schema);
    assert!(json.contains(r#"["navigate","/chat/channels"]"#), "{json}");
    assert!(
        json.contains(r#""/chat/channels/""#),
        "param route prefix: {json}"
    );
    assert!(!json.contains(r#"["navigate","/channels"]"#), "{json}");
    // A config string is data, not a page reference: the remap never rewrites it.
    assert!(
        json.contains(r#""href":"/channels""#),
        "config href: {json}"
    );
}

#[test]
fn sibling_remap_is_declaration_order_independent() {
    let schema = inline_consumer(&[ROOM_IMPORT.into(), channel_import("ChannelImport", "/chat")]);
    assert!(room_view_json(&schema).contains(r#"["navigate","/chat/channels"]"#));
}

#[test]
fn control_navigate_to_a_path_no_import_remaps_is_unchanged() {
    let schema = inline_consumer(&[channel_import("ChannelImport", "/chat"), ROOM_IMPORT.into()]);
    assert!(room_view_json(&schema).contains(r#"["navigate","/elsewhere"]"#));
}

#[test]
fn control_without_any_remap_the_sibling_keeps_the_upstream_path() {
    let plain = r#"{
      "name": "ChannelImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ChannelOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ChannelOrbital" }
    }"#;
    let schema = inline_consumer(&[plain.into(), ROOM_IMPORT.into()]);
    assert!(room_view_json(&schema).contains(r#"["navigate","/channels"]"#));
}

#[test]
fn own_import_remap_wins_over_a_sibling_remap_of_the_same_path() {
    let schema = inline_consumer(&[
        channel_import("ChannelImport", "/chat"),
        r#"{
      "name": "RoomImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.RoomOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.RoomOrbital" }
    }"#
        .into(),
    ]);
    let channel = schema
        .orbitals
        .iter()
        .find(|o| o.name == "ChannelImport")
        .unwrap();
    let json = serde_json::to_string(&channel.traits).unwrap();
    assert!(json.contains("/chat/channels/:id"), "{json}");
}

#[test]
fn identical_remaps_from_two_imports_still_apply_to_the_sibling() {
    let schema = inline_consumer(&[
        channel_import("ChannelA", "/chat"),
        channel_import("ChannelB", "/chat"),
        ROOM_IMPORT.into(),
    ]);
    assert!(room_view_json(&schema).contains(r#"["navigate","/chat/channels"]"#));
}

#[test]
fn conflicting_remaps_of_one_path_leave_the_sibling_target_unrewritten() {
    let schema = inline_consumer(&[
        channel_import("ChannelA", "/a"),
        channel_import("ChannelB", "/b"),
        ROOM_IMPORT.into(),
    ]);
    let json = room_view_json(&schema);
    assert!(json.contains(r#"["navigate","/channels"]"#), "{json}");
    assert!(
        !json.contains("/a/channels") && !json.contains("/b/channels"),
        "{json}"
    );
}
