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
          "category": "interaction",
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
