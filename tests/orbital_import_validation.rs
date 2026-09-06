//! W4-R: the four new orbital-import validation codes
//! (`docs/Almadar_Orbital_Import.md` §4.5) — one positive + one passing
//! negative per code, plus the page-path collision regression and the
//! backwards-compatibility (no-config) sweep.
//!
//! - `ORB_O_CONFIG_UNKNOWN_KEY` / `ORB_O_OMIT_EMBEDDED_TRAIT`: converted from
//!   [`orbital_compiler::phases::inline::OrbitalRefDiagnostic`]s collected
//!   while resolving an `orbital X = Alias.orbitals.Y { … }` reference.
//! - `ORB_U_CONFIG_UNKNOWN_KEY`: converted from
//!   [`orbital_compiler::phases::inline::UsesConfigDiagnostic`]s collected in
//!   the `uses` loop.
//! - `ORB_O_CONFIG_DEAD_KNOB`: a post-inline whole-schema walk
//!   (`validate_orbital_config_forwarding`), exercised directly via
//!   hand-built schemas (no loader needed — single-file, no `uses`).
//! - Page-path collision: no new code — `validate_page_path_uniqueness`
//!   already covers it; this is the regression proof for the orbital-import
//!   case (two bare imports of one upstream, no `pages {}` remap).

use orbital_compiler::phases::inline::{
    inline_schema, inline_schema_with_orbital_diagnostics, InlineConfig, OrbitalRefDiagnostic,
    UsesConfigDiagnostic,
};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::error::ErrorCode;
use orbital_compiler::phases::validation::orbital_ref_config::{
    validate_orbital_ref_diagnostics, validate_uses_config_diagnostics,
};
use orbital_compiler::phases::validation::page::validate_page_path_uniqueness;
use orbital_compiler::phases::validation::validate_comprehensive;
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

// ============================================================================
// ORB_O_CONFIG_UNKNOWN_KEY / ORB_O_OMIT_EMBEDDED_TRAIT / ORB_U_CONFIG_UNKNOWN_KEY
// (loader-backed: these arise from real `uses`/`orbital = Alias.orbitals.X`
// resolution, so they need the real inline pipeline, not a hand-built schema.)
// ============================================================================

/// One orbital: app-level `config { appName }`, orbital-level `config {
/// pageSize }` forwarded by `WidgetList` as `limit`. `WidgetList` embeds
/// `@trait.WidgetDetail`; `WidgetExtra` is a plain sibling nothing embeds —
/// the safe thing to `omit`.
const UPSTREAM: &str = r#"{
  "name": "test-orbital-ref-validation-upstream",
  "version": "1.0.0",
  "config": { "appName": { "type": "string", "default": "App" } },
  "orbitals": [
    {
      "name": "WidgetOrbital",
      "id": "orb_UPSTREAMREFVALID0000000000",
      "config": { "pageSize": { "type": "number", "default": 10 } },
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "WidgetList",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": { "limit": { "type": "number", "default": "@config.pageSize" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "OPEN",
                "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.WidgetDetail"] }] ]
              }
            ]
          }
        },
        {
          "name": "WidgetDetail",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "GO" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "GO", "effects": [] } ]
          }
        },
        {
          "name": "WidgetExtra",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "PING" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "PING", "effects": [] } ]
          }
        }
      ],
      "pages": [
        { "name": "WidgetListPage", "path": "/widgets", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetList" } ] },
        { "name": "WidgetDetailPage", "path": "/widgets/:id", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetDetail" } ] },
        { "name": "WidgetExtraPage", "path": "/widgets/extra", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetExtra" } ] }
      ]
    }
  ]
}"#;

fn materialize(consumer_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer_source).expect("write consumer");
    (dir, consumer_path)
}

struct Collected {
    orbital_ref: Vec<OrbitalRefDiagnostic>,
    uses_config: Vec<UsesConfigDiagnostic>,
}

fn collect(consumer_source: &str) -> Collected {
    let (_dir, consumer_path) = materialize(consumer_source);
    let mut schema = parse_file(&consumer_path).expect("consumer must parse");
    let mut trait_ref_config_diagnostics = Vec::new();
    let mut orbital_ref = Vec::new();
    let mut uses_config = Vec::new();
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut trait_ref_config_diagnostics,
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("orbital-import inline must succeed");
    Collected { orbital_ref, uses_config }
}

/// `orbital Local = Up.orbitals.WidgetOrbital { <reference_extra> }`.
fn orbital_ref_consumer(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-orbital-ref-validation-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.WidgetOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Up.orbitals.WidgetOrbital"{extra}
      }}
    }}
  ]
}}"#,
        extra = reference_extra
    )
}

#[test]
fn orbital_ref_unknown_config_key_fires() {
    let collected =
        collect(&orbital_ref_consumer(r#", "config": { "bogus": { "type": "number", "default": 1 } }"#));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref);
    assert!(
        result.errors.iter().any(|e| e.code == ErrorCode::OrbitalRefConfigUnknownKey
            && e.message.contains("bogus")
            && e.message.contains("WidgetOrbital")),
        "expected ORB_O_CONFIG_UNKNOWN_KEY for 'bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn orbital_ref_declared_config_key_does_not_fire_unknown_key() {
    let collected = collect(&orbital_ref_consumer(
        r#", "config": { "pageSize": { "type": "number", "default": 25 } }"#,
    ));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref);
    assert!(
        !result.errors.iter().any(|e| e.code == ErrorCode::OrbitalRefConfigUnknownKey),
        "declared knob 'pageSize' must not fire ORB_O_CONFIG_UNKNOWN_KEY: {:#?}",
        result.errors
    );
}

#[test]
fn orbital_ref_omit_of_embedded_trait_fires() {
    let collected = collect(&orbital_ref_consumer(r#", "omit": ["WidgetDetail"]"#));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref);
    assert!(
        result.errors.iter().any(|e| e.code == ErrorCode::OrbitalRefOmitEmbeddedTrait
            && e.message.contains("WidgetDetail")
            && e.message.contains("WidgetList")),
        "expected ORB_O_OMIT_EMBEDDED_TRAIT for omitted 'WidgetDetail' still embedded by 'WidgetList', got: {:#?}",
        result.errors
    );
}

#[test]
fn orbital_ref_omit_of_unembedded_trait_does_not_fire() {
    let collected = collect(&orbital_ref_consumer(r#", "omit": ["WidgetExtra"]"#));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref);
    assert!(
        !result.errors.iter().any(|e| e.code == ErrorCode::OrbitalRefOmitEmbeddedTrait),
        "'WidgetExtra' is embedded by no one — omitting it must not fire ORB_O_OMIT_EMBEDDED_TRAIT: {:#?}",
        result.errors
    );
}

/// `uses Up from "…" { <use_config_extra> }`, importing one plain trait
/// (`WidgetExtra`) — the `uses … { config }` diagnostic is collected
/// regardless of whether the orbital-reference form is used.
fn uses_config_consumer(use_config_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-uses-config-validation-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up"{extra} }} ],
      "entity": {{
        "name": "LocalWidget",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{ "ref": "Up.traits.WidgetExtra", "linkedEntity": "LocalWidget" }}
      ]
    }}
  ]
}}"#,
        extra = use_config_extra
    )
}

#[test]
fn uses_config_unknown_key_fires() {
    let collected = collect(&uses_config_consumer(
        r#", "config": { "bogus": { "type": "string", "default": "X" } }"#,
    ));
    let result = validate_uses_config_diagnostics(&collected.uses_config);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::UsesConfigUnknownKey && e.message.contains("bogus")),
        "expected ORB_U_CONFIG_UNKNOWN_KEY for 'bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn uses_config_declared_key_does_not_fire_unknown_key() {
    let collected = collect(&uses_config_consumer(
        r#", "config": { "appName": { "type": "string", "default": "X" } }"#,
    ));
    let result = validate_uses_config_diagnostics(&collected.uses_config);
    assert!(
        !result.errors.iter().any(|e| e.code == ErrorCode::UsesConfigUnknownKey),
        "declared app-level knob 'appName' must not fire ORB_U_CONFIG_UNKNOWN_KEY: {:#?}",
        result.errors
    );
}

// ============================================================================
// Page-path collision regression (no new code — validate_page_path_uniqueness
// already covers it post-flatten; two bare imports of one upstream, neither
// remapping `pages {}`, collide on every one of the upstream's paths since
// only NAMES get the unconditional prefix — paths are untouched unless
// `pages {}` remaps them).
// ============================================================================

const DOUBLE_IMPORT_CONSUMER: &str = r#"{
  "name": "test-orbital-import-page-collision",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "FirstImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.WidgetOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.WidgetOrbital" }
    },
    {
      "name": "SecondImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.WidgetOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.WidgetOrbital" }
    }
  ]
}"#;

#[test]
fn double_import_with_no_page_remap_collides_on_every_path() {
    let (_dir, consumer_path) = materialize(DOUBLE_IMPORT_CONSUMER);
    let mut schema = parse_file(&consumer_path).expect("consumer must parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("orbital-import inline must succeed");

    let result = validate_page_path_uniqueness(&schema);
    assert_eq!(
        result.errors.len(),
        3,
        "all three upstream page paths (/widgets, /widgets/:id, /widgets/extra) must collide: {:#?}",
        result.errors
    );
    assert!(
        result.errors.iter().all(|e| e.code == ErrorCode::PageDuplicatePath),
        "{:#?}",
        result.errors
    );
}

// ============================================================================
// ORB_O_CONFIG_DEAD_KNOB — single-file, no `uses`; hand-built schemas.
// ============================================================================

const DEAD_KNOB: &str = "ORB_O_CONFIG_DEAD_KNOB";

fn codes(schema: &OrbitalSchema) -> Vec<String> {
    validate_comprehensive(schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// One orbital, one trait (`WidgetView`), one page. `app_config` /
/// `orbital_config` / `trait_config` splice in the declared/forwarding
/// blocks each test varies.
fn schema_with(app_config: Option<Value>, orbital_config: Option<Value>, trait_config: Option<Value>) -> OrbitalSchema {
    let mut value = json!({
        "name": "test-dead-knob",
        "version": "1.0.0",
        "orbitals": [
            {
                "name": "WidgetOrbital",
                "entity": {
                    "name": "Widget",
                    "persistence": "persistent",
                    "fields": [ { "name": "id", "type": "string", "required": true } ]
                },
                "traits": [
                    {
                        "name": "WidgetView",
                        "linkedEntity": "Widget",
                        "category": "interaction",
                        "stateMachine": {
                            "states": [ { "name": "idle", "isInitial": true } ],
                            "events": [ { "key": "OPEN" } ],
                            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] } ]
                        }
                    }
                ],
                "pages": [
                    { "name": "WidgetPage", "path": "/widgets", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetView" } ] }
                ]
            }
        ]
    });
    if let Some(cfg) = app_config {
        value["config"] = cfg;
    }
    if let Some(cfg) = orbital_config {
        value["orbitals"][0]["config"] = cfg;
    }
    if let Some(cfg) = trait_config {
        value["orbitals"][0]["traits"][0]["config"] = cfg;
    }
    serde_json::from_value(value).expect("schema fixture must deserialize")
}

#[test]
fn app_level_unforwarded_knob_fires_dead_knob() {
    let schema = schema_with(Some(json!({ "appName": { "type": "string", "default": "App" } })), None, None);
    assert!(
        codes(&schema).contains(&DEAD_KNOB.to_string()),
        "declared app-level 'appName' with no forwarder must fire {DEAD_KNOB}"
    );
}

#[test]
fn app_level_forwarded_knob_does_not_fire_dead_knob() {
    let schema = schema_with(
        Some(json!({ "appName": { "type": "string", "default": "App" } })),
        None,
        Some(json!({ "title": { "type": "string", "default": "@config.appName" } })),
    );
    assert!(
        !codes(&schema).contains(&DEAD_KNOB.to_string()),
        "'appName' is forwarded by WidgetView's 'title' — must not fire {DEAD_KNOB}"
    );
}

#[test]
fn orbital_level_unforwarded_knob_fires_dead_knob() {
    let schema = schema_with(None, Some(json!({ "pageSize": { "type": "number", "default": 10 } })), None);
    assert!(
        codes(&schema).contains(&DEAD_KNOB.to_string()),
        "declared orbital-level 'pageSize' with no forwarder must fire {DEAD_KNOB}"
    );
}

#[test]
fn orbital_level_forwarded_knob_does_not_fire_dead_knob() {
    let schema = schema_with(
        None,
        Some(json!({ "pageSize": { "type": "number", "default": 10 } })),
        Some(json!({ "limit": { "type": "number", "default": "@config.pageSize" } })),
    );
    assert!(
        !codes(&schema).contains(&DEAD_KNOB.to_string()),
        "'pageSize' is forwarded by WidgetView's 'limit' — must not fire {DEAD_KNOB}"
    );
}

#[test]
fn dotted_tail_forward_does_not_count_and_still_fires_dead_knob() {
    let schema = schema_with(
        Some(json!({ "appName": { "type": "string", "default": "App" } })),
        None,
        Some(json!({ "title": { "type": "string", "default": "@config.appName.sub" } })),
    );
    assert!(
        codes(&schema).contains(&DEAD_KNOB.to_string()),
        "'@config.appName.sub' has a dotted tail — must NOT count as forwarding 'appName', so {DEAD_KNOB} still fires"
    );
}

#[test]
fn no_config_anywhere_produces_none_of_the_four_new_codes() {
    let schema = schema_with(None, None, None);
    let cs = codes(&schema);
    for code in [
        DEAD_KNOB,
        "ORB_O_CONFIG_UNKNOWN_KEY",
        "ORB_O_OMIT_EMBEDDED_TRAIT",
        "ORB_U_CONFIG_UNKNOWN_KEY",
    ] {
        assert!(
            !cs.contains(&code.to_string()),
            "{code} must not fire on a schema declaring no config anywhere: {cs:?}"
        );
    }
}
