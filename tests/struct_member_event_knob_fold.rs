//! The STRUCT-MEMBER twin of `event_typed_knob_rename_fold.rs`
//! (SCAN-DETAIL-PANEL-ACTIONS-UNTAGGED-1).
//!
//! A knob declared `actions : [RecordAction]` whose element schema types a
//! member `event` holds event-name REFERENCES in exactly the same sense as a
//! scalar `: event` knob. The step-4 effect walk cannot reach them: a render-ui
//! node forwards the prop as the STRING `"@config.actions"` and the literal
//! array lives on the knob, which no walk touched. So an authored
//! `events { EDIT: … }` renamed the trait's transitions and emits while its own
//! action buttons kept firing the pre-rename keys, and nothing handled them —
//! 8 `CIRCUIT_ORPHAN_EVENT` on std-finance-tracker's `FinanceReportViewer`.
//!
//! The fold is TYPE-directed, never name-directed: a member named `event` that
//! is not declared `event`-typed must be left alone.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};

const ATOM_DETAIL: &str = r#"{
  "name": "test-atom-detail",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "DetailOrbital",
      "entity": {
        "name": "Record",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "RecordItemDetail",
          "linkedEntity": "Record",
          "category": "interaction",
          "config": {
            "actions": {
              "type": "[RecordAction]",
              "items": {
                "type": "object",
                "properties": {
                  "label": { "name": "label", "type": "string", "required": true },
                  "event": { "name": "event", "type": "event", "required": true }
                }
              },
              "default": [
                { "event": "EDIT", "label": "Edit" },
                { "event": "DELETE", "label": "Delete" },
                { "event": "CLOSE_VIEW", "label": "Close" }
              ]
            },
            "legend": {
              "type": "[LegendRow]",
              "items": {
                "type": "object",
                "properties": {
                  "event": { "name": "event", "type": "string", "required": true }
                }
              },
              "default": [ { "event": "EDIT" } ]
            }
          },
          "emits": [
            { "event": "EDIT", "scope": "internal" },
            { "event": "DELETE", "scope": "internal" }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "EDIT", "name": "Edit" },
              { "key": "DELETE", "name": "Delete" },
              { "key": "CLOSE_VIEW", "name": "Close" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "detail-panel", "actions": "@config.actions" }]
                ]
              },
              { "from": "idle", "to": "idle", "event": "EDIT", "effects": [] },
              { "from": "idle", "to": "idle", "event": "DELETE", "effects": [] },
              { "from": "idle", "to": "idle", "event": "CLOSE_VIEW", "effects": [] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "RecordPage", "path": "/record", "linkedEntity": "Record", "traits": [ { "ref": "RecordItemDetail" } ] }
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
      "uses": [ {{ "from": "./atom-detail.orb", "as": "Detail" }} ],
      "entity": {{
        "name": "Report",
        "persistence": "persistent",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "Detail.traits.RecordItemDetail",
          "name": "ReportViewer",
          "events": {{ "EDIT": "PDF_EXPORT_REQUESTED", "DELETE": "CSV_EXPORT_REQUESTED" }}{config_block}
        }}
      ],
      "pages": [
        {{ "name": "ReportPage", "path": "/report", "linkedEntity": "Report", "traits": [ {{ "ref": "ReportViewer" }} ] }}
      ]
    }}
  ]
}}"#
    )
}

fn inline_fixture(consumer_json: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom-detail.orb"), ATOM_DETAIL).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer_json).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn find_viewer(schema: &OrbitalSchema) -> &TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .find(|td| td.name == "ReportViewer")
        .expect("ReportViewer must be inlined")
}

/// Event names of a descriptor-array knob, in declaration order.
fn member_events(td: &TraitDefinition, knob: &str) -> Vec<String> {
    let default = td
        .config
        .as_ref()
        .and_then(|c| c.get(knob))
        .and_then(|cf| cf.default.as_ref())
        .unwrap_or_else(|| panic!("{knob} must have a default"));
    let SExpression::Array(entries) = default else {
        panic!("{knob} default must be an array, got {default:?}");
    };
    entries
        .iter()
        .map(|e| match e {
            SExpression::Object(map) => match map.get("event") {
                Some(SExpression::String(s)) => s.clone(),
                other => panic!("event member must be a string, got {other:?}"),
            },
            other => panic!("descriptor must be an object, got {other:?}"),
        })
        .collect()
}

#[test]
fn baked_descriptor_default_is_rename_folded() {
    let schema = inline_fixture(&consumer(""));
    assert_eq!(
        member_events(find_viewer(&schema), "actions"),
        vec![
            "PDF_EXPORT_REQUESTED".to_string(),
            "CSV_EXPORT_REQUESTED".to_string(),
            // Not in the rename map — untouched.
            "CLOSE_VIEW".to_string(),
        ],
    );
}

#[test]
fn a_call_site_override_is_rename_folded_too() {
    // Overriding the knob must not silently opt out of the rename.
    let schema = inline_fixture(&consumer(
        r#",
          "config": { "actions": { "type": "[RecordAction]", "default": [ { "event": "EDIT", "label": "Download PDF" } ] } }"#,
    ));
    assert_eq!(
        member_events(find_viewer(&schema), "actions"),
        vec!["PDF_EXPORT_REQUESTED".to_string()],
    );
}

#[test]
fn a_member_named_event_that_is_not_event_typed_is_left_alone() {
    // The fold keys off the DECLARED member type, never the member name.
    let schema = inline_fixture(&consumer(""));
    assert_eq!(
        member_events(find_viewer(&schema), "legend"),
        vec!["EDIT".to_string()],
    );
}

#[test]
fn an_at_binding_member_is_never_rewritten() {
    let schema = inline_fixture(&consumer(
        r#",
          "config": { "actions": { "type": "[RecordAction]", "default": [ { "event": "@config.someKnob", "label": "Dynamic" } ] } }"#,
    ));
    assert_eq!(
        member_events(find_viewer(&schema), "actions"),
        vec!["@config.someKnob".to_string()],
    );
}
