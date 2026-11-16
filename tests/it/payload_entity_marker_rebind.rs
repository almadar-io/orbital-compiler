//! End-to-end proof that a `linkedEntity` rebind on a composed trait carries
//! its payload fields' `PayloadField.entity` marker along with it (B4-C
//! `entity` marker; `inline/rewrite.rs`'s `rewrite_payload_entity_refs`).
//!
//! Mirrors the real regression: `std-notes.orb`'s `NoteDelete` composes
//! `Confirmation.traits.ConfirmActionConfirmation -> Note` (rebinding the
//! atom's own `ConfirmAction` entity to `Note`); before the fix, the DELETE
//! event's `row` payload field kept `entity: "ConfirmAction"` after the
//! rebind, so a consumer reading `f.entity === entityName` (the crud-delete
//! `wholeRow` check) never recognized `row` as the whole deleted record.
//! Unit coverage for the rewrite function itself (rebind / splice-prefix /
//! entities{}-retarget substitution shapes, scalar + array-form + nested
//! `properties`) lives in `inline/rewrite.rs`'s `payload_entity_marker_tests`
//! module; this file proves the REAL call site (`apply_overrides_to_trait`'s
//! linked-entity rebind, reached through `uses` + `ref: ... -> Entity`) is
//! actually wired to it.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::TraitReference;

const CONFIRMATION_ATOM: &str = r#"{
  "name": "test-confirmation-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ConfirmationOrbital",
      "entity": {
        "name": "ConfirmAction",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ConfirmActionConfirmation",
          "linkedEntity": "ConfirmAction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true }, { "name": "open" } ],
            "events": [
              {
                "key": "DELETE",
                "payloadSchema": [
                  { "name": "id", "type": "string" },
                  { "name": "row", "type": "object", "entity": "ConfirmAction" }
                ]
              }
            ],
            "transitions": [
              { "from": "idle", "to": "open", "event": "DELETE", "effects": [] }
            ]
          }
        }
      ]
    }
  ]
}"#;

const NOTES_CONSUMER: &str = r#"{
  "name": "test-notes-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "NotesOrbital",
      "uses": [ { "from": "./confirmation.orb", "as": "Confirmation" } ],
      "entity": {
        "name": "Note",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "Confirmation.traits.ConfirmActionConfirmation",
          "name": "NoteDelete",
          "linkedEntity": "Note"
        }
      ]
    }
  ]
}"#;

#[test]
fn linked_entity_rebind_carries_the_payload_entity_marker() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("confirmation.orb"), CONFIRMATION_ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, NOTES_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");

    let note_delete = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "NoteDelete" => Some(td),
            _ => None,
        })
        .expect("NoteDelete must be inlined");

    let delete_event = note_delete
        .state_machine
        .events
        .as_ref()
        .expect("events carried")
        .iter()
        .find(|e| e.key == "DELETE")
        .expect("DELETE event carried");
    let payload = delete_event
        .payload_schema
        .as_ref()
        .expect("payloadSchema carried");

    let id_field = payload
        .iter()
        .find(|f| f.name == "id")
        .expect("id field carried");
    assert_eq!(id_field.entity, None, "non-entity field untouched");

    let row_field = payload
        .iter()
        .find(|f| f.name == "row")
        .expect("row field carried");
    assert_eq!(
        row_field.entity.as_deref(),
        Some("Note"),
        "row's entity marker must follow the ConfirmAction -> Note rebind, \
         not stay the atom's original entity: {row_field:?}"
    );
}

// G-CROSS-575: an `Event @entity` payload on a trait rebound onto an entity ANOTHER orbital owns is the row of
// that entity, exactly as when the entity is the trait's own orbital's.
const ROW_EVENT_ATOM: &str = r#"{
  "name": "test-row-event-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "BoardOrbital",
      "entity": {
        "name": "BoardItem",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "label", "type": "string" } ]
      },
      "traits": [
        {
          "name": "ItemBoard",
          "linkedEntity": "BoardItem",
          "entityRebindable": true,
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "SAVED", "payloadEntity": "@entity", "payloadSchema": [ { "name": "value", "type": "@entity" } ] }
            ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "SAVED", "effects": [] } ]
          }
        }
      ]
    }
  ]
}"#;

fn row_event_consumer(bound_in_own_orbital: bool) -> String {
    let task = serde_json::json!({
        "name": "Task", "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ]
    });
    let screen = serde_json::json!({
        "name": "Screen", "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
    });
    let board = serde_json::json!([ { "ref": "Board.traits.ItemBoard", "name": "TaskBoard", "linkedEntity": "Task" } ]);
    let uses = serde_json::json!([ { "from": "./board.orb", "as": "Board" } ]);
    let orbitals = if bound_in_own_orbital {
        serde_json::json!([ { "name": "TaskOrbital", "uses": uses, "entity": task, "traits": board } ])
    } else {
        serde_json::json!([
            { "name": "ProjectOrbital", "uses": uses, "entity": screen, "traits": board },
            { "name": "TaskOrbital", "entity": task, "traits": [] }
        ])
    };
    serde_json::json!({ "name": "test-row-event-consumer", "version": "1.0.0", "orbitals": orbitals }).to_string()
}

fn saved_payload_names(bound_in_own_orbital: bool) -> Vec<String> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("board.orb"), ROW_EVENT_ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, row_event_consumer(bound_in_own_orbital)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("inline succeeds");
    let board = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "TaskBoard" => Some(td),
            _ => None,
        })
        .expect("TaskBoard inlined");
    let saved = board
        .state_machine
        .events
        .as_ref()
        .expect("events carried")
        .iter()
        .find(|e| e.key == "SAVED")
        .expect("SAVED carried");
    saved.payload_schema.as_ref().expect("payload").iter().map(|f| f.name.clone()).collect()
}

#[test]
fn a_row_event_on_a_trait_bound_to_another_orbitals_entity_carries_that_entitys_row() {
    let names = saved_payload_names(false);
    assert!(names.contains(&"title".to_string()), "SAVED must carry Task's row: {names:?}");
    assert!(!names.contains(&"value".to_string()), "the atom's placeholder field must not survive: {names:?}");
}

#[test]
fn control_a_row_event_on_a_trait_bound_to_its_own_orbitals_entity_carries_that_entitys_row() {
    let names = saved_payload_names(true);
    assert!(names.contains(&"title".to_string()), "SAVED must carry Task's row: {names:?}");
}
