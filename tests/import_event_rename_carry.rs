//! Repro for S-EVENTS-RENAME-LOST-WHEN-AN-ATOM-IS-IMPORTED.
//!
//! An atom declares an `events {}` rename on a reference to a trait whose
//! emit name is a `@config.<knob>` ref (ButtonRender's `@config.action`).
//! Resolved alone, the rename applies (control test). Imported by a consumer,
//! the atom's inline pass runs deferred (`defer_config_subst = true`): the
//! emit-name resolution at trait.rs:164 is gated OFF while the rename pass at
//! trait.rs:233 still runs, matches nothing against the raw `@config.action`
//! token, and no carrier survives the import boundary — the consumer's own
//! pass then resolves the untouched declared default and the button emits the
//! raw slot `ACTION` instead of the renamed event. Measured live: every
//! confirm/cancel button of std-invoice collapses to `ACTION` inside
//! std-accounting, so confirm and cancel fire the SAME event and the Void
//! confirmation modal cannot be confirmed in any consumer.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, TraitDefinition, TraitReference};

/// A minimal stand-in for `ui-button`: emit name is the `@config.action`
/// config ref, declared default "ACTION" — the exact shape whose rename is
/// lost on import.
const ATOM_BUTTON: &str = r#"{
  "name": "test-atom-button",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "BtnOrbital",
      "entity": {
        "name": "Src",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ButtonRender",
          "linkedEntity": "Src",
          "category": "interaction",
          "config": {
            "action": { "type": "string", "default": "ACTION", "label": "Action" }
          },
          "emits": [
            { "event": "@config.action", "scope": "external" }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "@config.action", "name": "@config.action" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "button", "label": "Go", "action": "@config.action" }]
                ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "SrcPage", "path": "/src", "linkedEntity": "Src", "traits": [ { "ref": "ButtonRender" } ] }
      ]
    }
  ]
}"#;

/// The importable atom (std-invoice's shape): references ButtonRender with an
/// `events` rename against the RESOLVED emit name, and a host trait that
/// embeds the button so consumers pull it as a sibling.
const ATOM_INVOICE: &str = r#"{
  "name": "test-atom-invoice",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "InvoiceOrbital",
      "uses": [ { "from": "./atom-button.orb", "as": "UiButton" } ],
      "entity": {
        "name": "Invoice",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "UiButton.traits.ButtonRender",
          "name": "ConfirmBtn",
          "linkedEntity": "Invoice",
          "events": { "ACTION": "CONFIRM_CLICKED" }
        },
        {
          "name": "InvoiceManage",
          "linkedEntity": "Invoice",
          "category": "interaction",
          "emits": [ { "event": "CONFIRM_CLICKED", "scope": "internal" } ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "CONFIRM_CLICKED", "name": "Confirmed" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "stack", "children": [ "@trait.ConfirmBtn" ] }]
                ]
              },
              { "from": "idle", "to": "idle", "event": "CONFIRM_CLICKED" }
            ]
          }
        }
      ],
      "pages": [
        { "name": "InvoicePage", "path": "/invoices", "linkedEntity": "Invoice", "traits": [ { "ref": "InvoiceManage" } ] }
      ]
    }
  ]
}"#;

/// The consumer (std-accounting's shape): imports the atom and mounts its
/// host trait; the button arrives via the sibling auto-pull.
const CONSUMER: &str = r#"{
  "name": "test-rename-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AppOrbital",
      "uses": [ { "from": "./atom-invoice.orb", "as": "Invoice" } ],
      "entity": {
        "name": "Ledger",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Invoice.traits.InvoiceManage", "name": "LedgerManage" }
      ],
      "pages": [
        { "name": "AppPage", "path": "/app", "linkedEntity": "Ledger", "traits": [ { "ref": "LedgerManage" } ] }
      ]
    }
  ]
}"#;

/// Negative control: a consumer that explicitly overrides the knob at its
/// own call site must win over the atom's folded rename.
const CONSUMER_WITH_OVERRIDE: &str = r#"{
  "name": "test-rename-consumer-override",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AppOrbital",
      "uses": [ { "from": "./atom-invoice.orb", "as": "Invoice" } ],
      "entity": {
        "name": "Ledger",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "Invoice.traits.ConfirmBtn",
          "name": "LedgerConfirm",
          "config": { "action": { "type": "string", "default": "LEDGER_CONFIRM" } }
        }
      ],
      "pages": [
        { "name": "AppPage", "path": "/app", "linkedEntity": "Ledger", "traits": [ { "ref": "LedgerConfirm" } ] }
      ]
    }
  ]
}"#;

fn inline_fixture(files: &[(&str, &str)], consumer: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (name, content) in files {
        std::fs::write(dir.path().join(name), content).expect("write dep");
    }
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn find_button<'a>(schema: &'a OrbitalSchema) -> &'a TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .find(|td| {
            td.name == "ConfirmBtn"
                || td
                    .source_behavior
                    .as_ref()
                    .is_some_and(|sb| sb.original_name == "ConfirmBtn")
        })
        .expect("ConfirmBtn (or its pulled copy) must be inlined")
}

fn emit_events(td: &TraitDefinition) -> Vec<String> {
    td.emits
        .as_ref()
        .map(|emits| emits.iter().map(|e| e.event.clone()).collect())
        .unwrap_or_default()
}

/// Control: the atom resolved as the top-level file applies the rename.
/// This passes today and pins the working half of the contract.
#[test]
fn atom_alone_applies_config_ref_event_rename() {
    let schema = inline_fixture(&[("atom-button.orb", ATOM_BUTTON)], ATOM_INVOICE);
    let btn = find_button(&schema);
    let events = emit_events(btn);
    assert!(
        events.contains(&"CONFIRM_CLICKED".to_string()),
        "atom-alone ButtonRender must emit the renamed event, got {events:?}"
    );
    assert!(
        !events.contains(&"ACTION".to_string()),
        "raw pattern slot ACTION must be renamed away, got {events:?}"
    );
}

/// The bug (S-EVENTS-RENAME-LOST-WHEN-AN-ATOM-IS-IMPORTED): the same atom
/// imported by a consumer lost the rename — the button emitted the raw slot
/// `ACTION`. The deferred-import fold of the rename into the knob's declared
/// default is the carrier that keeps this green.
#[test]
fn imported_atom_carries_config_ref_event_rename() {
    let schema = inline_fixture(
        &[
            ("atom-button.orb", ATOM_BUTTON),
            ("atom-invoice.orb", ATOM_INVOICE),
        ],
        CONSUMER,
    );
    let btn = find_button(&schema);
    let events = emit_events(btn);
    assert!(
        events.contains(&"CONFIRM_CLICKED".to_string()),
        "imported ButtonRender must keep the atom's rename, got {events:?}"
    );
    assert!(
        !events.contains(&"ACTION".to_string()),
        "imported ButtonRender must not fall back to the raw pattern slot, got {events:?}"
    );
}

/// Negative control: an explicit call-site config override on the same knob
/// beats the atom's folded rename — the fold changes only the DEFAULT.
#[test]
fn consumer_config_override_still_wins_over_folded_rename() {
    let schema = inline_fixture(
        &[
            ("atom-button.orb", ATOM_BUTTON),
            ("atom-invoice.orb", ATOM_INVOICE),
        ],
        CONSUMER_WITH_OVERRIDE,
    );
    let btn = schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .find(|td| td.name == "LedgerConfirm")
        .expect("LedgerConfirm must be inlined");
    let events = emit_events(btn);
    assert!(
        events.contains(&"LEDGER_CONFIRM".to_string()),
        "explicit call-site override must win, got {events:?}"
    );
}
