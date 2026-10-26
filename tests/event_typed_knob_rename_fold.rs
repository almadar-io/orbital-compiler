//! The `event`-TYPED twin of `import_event_rename_carry.rs` (gap #30/C1
//! root fix, SCAN-EVENT-KNOB-1): a knob declared `: event` is a rename-safe
//! event-name REFERENCE, so a call-site `events { OLD: NEW }` must rewrite
//! its settled value — the baked default AND an explicit override spelling
//! the canonical (pre-rename) name — while an override already spelling the
//! NEW name passes through untouched. Before the fold, only the NEW-name
//! spelling validated; the other two hard-failed
//! ORB_X_RENDER_UI_EVENT_LITERAL_STALE (probe 2026-08-30, variants A/B/C).

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};

/// Consumer-knob atom: the event-typed knob feeds a render prop only —
/// nothing in `emits` or the event keys references it (NOT a definer).
fn atom_picker(knob_type: &str) -> String {
    ATOM_PICKER.replace("\"type\": \"event\"", &format!("\"type\": \"{knob_type}\""))
}

const ATOM_PICKER: &str = r#"{
  "name": "test-atom-picker",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "PickerOrbital",
      "entity": {
        "name": "Item",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "PickerRender",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": {
            "clickEvent": { "type": "event", "default": "PICK", "label": "Click event" }
          },
          "emits": [
            { "event": "PICK", "scope": "internal" }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "PICK", "name": "Pick" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "button", "label": "Pick", "action": "@config.clickEvent" }]
                ]
              },
              { "from": "idle", "to": "idle", "event": "PICK", "effects": [] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "ItemPage", "path": "/item", "linkedEntity": "Item", "traits": [ { "ref": "PickerRender" } ] }
      ]
    }
  ]
}"#;

fn consumer(config_block: &str) -> String {
    format!(
        r#"{{
  "name": "test-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "HostOrbital",
      "uses": [ {{ "from": "./atom-picker.orb", "as": "Picker" }} ],
      "entity": {{
        "name": "Thing",
        "persistence": "persistent",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "Picker.traits.PickerRender",
          "name": "ThingPicker",
          "events": {{ "PICK": "CHOSE" }}{config_block}
        }}
      ],
      "pages": [
        {{ "name": "ThingPage", "path": "/thing", "linkedEntity": "Thing", "traits": [ {{ "ref": "ThingPicker" }} ] }}
      ]
    }}
  ]
}}"#
    )
}

fn inline_fixture_with_atom(atom_json: &str, consumer_json: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom-picker.orb"), atom_json).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer_json).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn inline_fixture(consumer_json: &str) -> OrbitalSchema {
    inline_fixture_with_atom(ATOM_PICKER, consumer_json)
}

fn find_picker<'a>(schema: &'a OrbitalSchema) -> &'a TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .find(|td| td.name == "ThingPicker")
        .expect("ThingPicker must be inlined")
}

fn click_event_value(td: &TraitDefinition) -> String {
    match td
        .config
        .as_ref()
        .and_then(|c| c.get("clickEvent"))
        .and_then(|cf| cf.default.as_ref())
    {
        Some(SExpression::String(s)) => s.clone().into_inner(),
        other => panic!("clickEvent default must be a string, got {other:?}"),
    }
}

/// Variant C: no override — the baked canonical default is rename-folded.
#[test]
fn baked_default_is_rename_folded() {
    let schema = inline_fixture(&consumer(""));
    assert_eq!(click_event_value(find_picker(&schema)), "CHOSE");
}

/// Variant B: override spells the canonical OLD name — rename-folded.
#[test]
fn old_name_override_is_rename_folded() {
    let schema = inline_fixture(&consumer(
        r#",
          "config": { "clickEvent": { "type": "event", "default": "PICK" } }"#,
    ));
    assert_eq!(click_event_value(find_picker(&schema)), "CHOSE");
}

/// Variant A: override already spells the NEW name — untouched.
#[test]
fn new_name_override_passes_through() {
    let schema = inline_fixture(&consumer(
        r#",
          "config": { "clickEvent": { "type": "event", "default": "CHOSE" } }"#,
    ));
    assert_eq!(click_event_value(find_picker(&schema)), "CHOSE");
}

/// A knob DECLARED `: string` at the atom keeps today's verbatim behavior —
/// the fold is gated on the atom's declared type (a call-site type tag never
/// retypes a declaration), so untyped knobs are not silently rewritten.
#[test]
fn string_typed_knob_is_not_folded() {
    let schema = inline_fixture_with_atom(
        &atom_picker("string"),
        &consumer(
            r#",
          "config": { "clickEvent": { "type": "string", "default": "PICK" } }"#,
        ),
    );
    assert_eq!(click_event_value(find_picker(&schema)), "PICK");
}
