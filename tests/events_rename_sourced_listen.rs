//! Repro for the call-site `events {}` rename rewriting the SOURCE side of a
//! sourced listen (B-MOD-QUEUE-CROSS-WIRE-CYCLE's L1/L2 half).
//!
//! std-mod-queue's review trait carries an internal sourced listen
//! `CloseButton.CLOSE -> CLOSE`: the EVENT belongs to the source trait's
//! vocabulary, the TRIGGER to the composed trait's own. A call-site rename
//! `events { CLOSE: DASHBOARD_MOD_CLOSE }` on the composed trait must rewrite
//! only the own-vocabulary side (the trigger, plus the trait's emits and
//! transitions) — never the source-side event, whose source trait
//! (`CloseButton`) still declares `CLOSE`. Pre-fix the rename hit both sides
//! and the composed schema failed `ORB_X_LISTEN_SOURCE_UNRESOLVED`.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{validate_comprehensive, ErrorCode};
use orbital_core::schema::{ListenSource, OrbitalSchema, TraitReference};

/// Minimal stand-in for std-mod-queue: a CloseButton atom plus a review trait
/// with the internal sourced listen `CloseButton.CLOSE -> CLOSE`.
const ATOM_MODQUEUE: &str = r#"{
  "name": "test-atom-modqueue",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ModQueueOrbital",
      "entity": {
        "name": "ModItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "CloseButton",
          "linkedEntity": "ModItem",
          "category": "interaction",
          "emits": [ { "event": "CLOSE" } ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "CLOSE", "name": "Close" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "button", "label": "Close", "action": "CLOSE" }]
                ]
              }
            ]
          }
        },
        {
          "name": "ModQueueItemReview",
          "linkedEntity": "ModItem",
          "category": "interaction",
          "emits": [ { "event": "CLOSE" } ],
          "listens": [
            { "event": "CLOSE", "triggers": "CLOSE", "source": { "kind": "trait", "trait": "CloseButton" } }
          ],
          "stateMachine": {
            "states": [ { "name": "open", "isInitial": true }, { "name": "closed" } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "CLOSE", "name": "Close" }
            ],
            "transitions": [
              {
                "from": "open",
                "to": "open",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "stack", "children": [ "@trait.CloseButton" ] }]
                ]
              },
              { "from": "open", "to": "closed", "event": "CLOSE" }
            ]
          }
        }
      ],
      "pages": [
        { "name": "ModPage", "path": "/mod", "linkedEntity": "ModItem", "traits": [ { "ref": "ModQueueItemReview" } ] }
      ]
    }
  ]
}"#;

/// The consumer (std-embedded-dashboard's shape): mounts the review trait
/// with a call-site `events` rename of `CLOSE`.
const CONSUMER: &str = r#"{
  "name": "test-sourced-listen-rename",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "DashboardOrbital",
      "uses": [ { "from": "./atom-modqueue.orb", "as": "ModQueue" } ],
      "entity": {
        "name": "DashboardItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "ModQueue.traits.ModQueueItemReview",
          "name": "DashboardModeration",
          "linkedEntity": "DashboardItem",
          "events": { "CLOSE": "DASHBOARD_MOD_CLOSE" }
        }
      ],
      "pages": [
        { "name": "DashPage", "path": "/dash", "linkedEntity": "DashboardItem", "traits": [ { "ref": "DashboardModeration" } ] }
      ]
    }
  ]
}"#;

fn inline_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom-modqueue.orb"), ATOM_MODQUEUE).expect("write dep");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

#[test]
fn events_rename_keeps_sourced_listen_source_side() {
    let schema = inline_consumer();
    let traits: Vec<_> = schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .collect();

    let review = traits
        .iter()
        .find(|td| td.name == "DashboardModeration")
        .expect("DashboardModeration must be inlined");

    // The sourced listen becomes `CloseButton.CLOSE -> DASHBOARD_MOD_CLOSE`:
    // source side intact (CloseButton's vocabulary), trigger side renamed
    // (the composed trait's own vocabulary).
    let listens = review.listens.as_ref().expect("listens must survive inline");
    let sourced = listens
        .iter()
        .find(|l| {
            matches!(
                &l.source,
                Some(ListenSource::Trait { trait_name, .. }) if trait_name == "CloseButton"
            )
        })
        .expect("the CloseButton-sourced listen must survive inline");
    assert_eq!(
        sourced.event, "CLOSE",
        "source-side event belongs to CloseButton and must not be renamed"
    );
    assert_eq!(
        sourced.triggers.as_deref(),
        Some("DASHBOARD_MOD_CLOSE"),
        "trigger side is the composed trait's own vocabulary and must be renamed"
    );

    // The trait's own transition triggers followed the rename.
    let trigger_events: Vec<&str> = review
        .state_machine
        .transitions
        .iter()
        .map(|t| t.event.as_str())
        .collect();
    assert!(
        trigger_events.contains(&"DASHBOARD_MOD_CLOSE"),
        "own transitions must be renamed, got {trigger_events:?}"
    );

    // The pulled CloseButton sibling still declares the original emit.
    let close_button = traits
        .iter()
        .find(|td| td.name == "CloseButton")
        .expect("CloseButton sibling must be pulled");
    let button_emits: Vec<&str> = close_button
        .emits
        .as_ref()
        .map(|emits| emits.iter().map(|e| e.event.as_str()).collect())
        .unwrap_or_default();
    assert!(
        button_emits.contains(&"CLOSE"),
        "CloseButton emits must stay CLOSE, got {button_emits:?}"
    );

    // The composed schema validates: no ORB_X_LISTEN_SOURCE_UNRESOLVED.
    let result = validate_comprehensive(&schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalListenSourceUnresolved),
        "sourced listen must resolve post-rename; got {:?}",
        result.errors
    );
}
