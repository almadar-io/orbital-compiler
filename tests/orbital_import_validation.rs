//! W4-R + B3-R: the orbital-import validation codes
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
//! - `ORB_O_ROLE_TARGET_NOT_MEMBER` / `ORB_O_ROLE_UNKNOWN_UPSTREAM` /
//!   `ORB_O_ENTITY_UNMAPPED` / `ORB_O_ENTITY_UNKNOWN_KEY` /
//!   `ORB_O_ENTITY_TARGET_UNKNOWN` / `ORB_O_PAGE_UNKNOWN_PATH` (B3-R): the
//!   Stage-B `roles {}` / `entities {}` / `pages {}` remap diagnostics, at
//!   the bottom of this file.

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
use orbital_compiler::phases::validation::validate_comprehensive_without_services;
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

fn materialize_with(
    upstream_source: &str,
    consumer_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream_source).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer_source).expect("write consumer");
    (dir, consumer_path)
}

fn materialize(consumer_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    materialize_with(UPSTREAM, consumer_source)
}

struct Collected {
    orbital_ref: Vec<OrbitalRefDiagnostic>,
    uses_config: Vec<UsesConfigDiagnostic>,
    schema: OrbitalSchema,
}

fn collect_with(upstream_source: &str, consumer_source: &str) -> Collected {
    let (_dir, consumer_path) = materialize_with(upstream_source, consumer_source);
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
    Collected {
        orbital_ref,
        uses_config,
        schema,
    }
}

fn collect(consumer_source: &str) -> Collected {
    collect_with(UPSTREAM, consumer_source)
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
    let collected = collect(&orbital_ref_consumer(
        r#", "config": { "bogus": { "type": "number", "default": 1 } }"#,
    ));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefConfigUnknownKey
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
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefConfigUnknownKey),
        "declared knob 'pageSize' must not fire ORB_O_CONFIG_UNKNOWN_KEY: {:#?}",
        result.errors
    );
}

#[test]
fn orbital_ref_omit_of_embedded_trait_fires() {
    let collected = collect(&orbital_ref_consumer(r#", "omit": ["WidgetDetail"]"#));
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
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
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
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
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::UsesConfigUnknownKey),
        "declared app-level knob 'appName' must not fire ORB_U_CONFIG_UNKNOWN_KEY: {:#?}",
        result.errors
    );
}

// ============================================================================
// (A) app-level `@config.<knob>` forward on an ORBITAL IMPORT must resolve
// against the ALIAS's OWN schema config (its app-level `config {}`, folded
// with the `uses Alias { config {…} }` call-site override) — never the
// CONSUMER's own top-level `schema.config`, a different file's app knobs
// entirely. Root cause: `inline_orbital` (mod.rs) used to pass the
// CONSUMER's `schema_config` straight into `resolve_orbital_reference`;
// fixed by keying a per-alias fold (`alias_schema_configs`, built beside
// `ctx.add_alias_multi`) and having `resolve_orbital_reference` look it up
// by the reference's own alias.
// ============================================================================

/// A SEPARATE small upstream (not the shared `UPSTREAM` const above, so this
/// doesn't perturb the omit/embed/page-count assertions the other tests pin)
/// whose app-level `appName` is forwarded by an orbital trait — the exact
/// shape `std-time-tracking`'s `*AppLayout`s use for `appName`/`navItems`.
const APP_FORWARD_UPSTREAM: &str = r#"{
  "name": "test-orbital-ref-app-forward-upstream",
  "version": "1.0.0",
  "config": { "appName": { "type": "string", "default": "App" } },
  "orbitals": [
    {
      "name": "ShellOrbital",
      "id": "orb_APPFORWARDUPSTREAM000000",
      "entity": { "name": "ShellItem", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "ShellHeader",
          "linkedEntity": "ShellItem",
          "category": "interaction",
          "config": { "title": { "type": "string", "default": "@config.appName" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [] } ]
          }
        }
      ],
      "pages": [ { "name": "ShellPage", "path": "/shell", "linkedEntity": "ShellItem", "traits": [ { "ref": "ShellHeader" } ] } ]
    }
  ]
}"#;

/// `orbital Local = Up.orbitals.ShellOrbital { … }`, with a `uses … { config
/// { appName } }` override at the FILE-SCOPE `uses` declaration (§4.5's
/// SCHEMA/alias rung) — distinct from the reference body's own `config {}`
/// (§4.5's ORBITAL rung).
fn app_forward_consumer(uses_config_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-orbital-ref-app-forward-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up"{uses_extra} }} ],
      "entity": "Up.orbitals.ShellOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.ShellOrbital" }}
    }}
  ]
}}"#,
        uses_extra = uses_config_extra
    )
}

#[test]
fn orbital_ref_app_level_forward_resolves_from_the_alias_schema_not_the_consumer() {
    let consumer = app_forward_consumer(
        r#", "config": { "appName": { "type": "string", "default": "Probe" } }"#,
    );
    let collected = collect_with(APP_FORWARD_UPSTREAM, &consumer);
    let orbital = collected
        .schema
        .orbitals
        .iter()
        .find(|o| o.name == "Local")
        .expect("Local orbital");
    let title = orbital.traits.iter().find_map(|t| {
        if let orbital_core::schema::TraitReference::Inline(td) = t {
            if td.name == "LocalShellHeader" {
                return td.config.as_ref().and_then(|c| c.get("title")).cloned();
            }
        }
        None
    });
    let title = title.expect("LocalShellHeader.config.title present");
    assert_eq!(
        title.default,
        Some(orbital_core::schema::SExpression::String("Probe".to_string())),
        "app-level forward must resolve to the `uses … {{ config }}` override, from the ALIAS's OWN schema (folded), not the consumer's — got: {:#?}",
        title
    );
    assert_eq!(
        title.forwarded_from.as_deref(),
        Some("@config.appName"),
        "resolved forward must carry provenance: {:#?}",
        title
    );
}

#[test]
fn orbital_ref_app_level_forward_without_uses_override_keeps_upstream_default() {
    // No `uses … { config }` override at all — the alias's OWN declared
    // app default ("App") must still resolve (proves the alias-schema
    // lookup itself, independent of any override fold).
    let collected = collect_with(APP_FORWARD_UPSTREAM, &app_forward_consumer(""));
    let orbital = collected
        .schema
        .orbitals
        .iter()
        .find(|o| o.name == "Local")
        .expect("Local orbital");
    let title = orbital.traits.iter().find_map(|t| {
        if let orbital_core::schema::TraitReference::Inline(td) = t {
            if td.name == "LocalShellHeader" {
                return td.config.as_ref().and_then(|c| c.get("title")).cloned();
            }
        }
        None
    });
    let title = title.expect("LocalShellHeader.config.title present");
    assert_eq!(
        title.default,
        Some(orbital_core::schema::SExpression::String("App".to_string())),
        "with no `uses … {{ config }}` override, the alias's own declared app default must resolve: {:#?}",
        title
    );
}

#[test]
fn uses_config_unknown_key_still_fires_alongside_a_resolving_orbital_ref() {
    // The (A) fix must not weaken `ORB_U_CONFIG_UNKNOWN_KEY`: a `uses …
    // { config }` key the upstream app never declares stays flagged even
    // though the reference itself resolves fine.
    let consumer =
        app_forward_consumer(r#", "config": { "bogus": { "type": "string", "default": "X" } }"#);
    let collected = collect_with(APP_FORWARD_UPSTREAM, &consumer);
    let result = validate_uses_config_diagnostics(&collected.uses_config);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::UsesConfigUnknownKey && e.message.contains("bogus")),
        "expected ORB_U_CONFIG_UNKNOWN_KEY for 'bogus' even though the orbital reference itself resolves: {:#?}",
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
        result
            .errors
            .iter()
            .all(|e| e.code == ErrorCode::PageDuplicatePath),
        "{:#?}",
        result.errors
    );
}

// ============================================================================
// (B) order-free `entities {}` mapping across TWO sibling reference-form
// orbitals in the SAME consumer file: import 1's `entities {}` names an
// out-of-orbital primary onto import 2's OWN materialized entity — legal
// regardless of FILE ORDER, since `consumer_entity_ids` must include every
// reference-form orbital's materialized primary BEFORE the per-orbital
// resolve loop runs, not just already-inline ones. The approved Project
// Friday shape (`TimesheetOrbital`'s `entities { ApprovalRequest:
// ApprovalRequest }` targeting `ApprovalRequestOrbital`'s own materialized
// entity, declared LATER in the file).
// ============================================================================

/// Two orbitals: `MainA`'s entity `AItem` relates to `BItem`, an
/// OUT-OF-ORBITAL entity that is `MainB`'s own PRIMARY (so mapping it is
/// REQUIRED — `ORB_O_ENTITY_UNMAPPED` if left unmapped, not an aux-only
/// auto-clone).
const ORDER_FREE_UPSTREAM: &str = r#"{
  "name": "test-order-free-entities-map-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MainA",
      "id": "orb_MAINAORDERFREE00000000000",
      "entity": {
        "name": "AItem",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "bId", "type": "relation", "relation": { "entity": "BItem", "cardinality": "one" } }
        ]
      },
      "traits": [
        {
          "name": "AItemFetcher",
          "linkedEntity": "AItem",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD", "effects": [ ["fetch", "BItem", {}] ] } ]
          }
        }
      ]
    },
    {
      "name": "MainB",
      "id": "orb_MAINBORDERFREE00000000000",
      "entity": {
        "name": "BItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": []
    }
  ]
}"#;

/// `ImportA` (processed FIRST, since orbitals resolve in file order) maps
/// out-of-orbital `BItem` onto `LocalB` — `ImportB`'s OWN materialized
/// primary, declared SECOND. The order-free fix under test.
const ORDER_FREE_CONSUMER: &str = r#"{
  "name": "test-order-free-entities-map-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ImportA",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.MainA.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.MainA",
        "entities": { "BItem": "LocalB" }
      }
    },
    {
      "name": "ImportB",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.MainB.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.MainB",
        "entity": "LocalB"
      }
    }
  ]
}"#;

#[test]
fn entities_map_onto_a_sibling_reference_form_orbitals_materialized_entity_is_order_free() {
    let (_dir, consumer_path) = materialize_with(ORDER_FREE_UPSTREAM, ORDER_FREE_CONSUMER);
    let mut schema = parse_file(&consumer_path).expect("consumer must parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("orbital-import inline must succeed — ImportA's `entities {}` must resolve against ImportB's materialized entity even though ImportB resolves SECOND");

    let import_a = schema
        .orbitals
        .iter()
        .find(|o| o.name == "ImportA")
        .expect("ImportA present");
    let import_b = schema
        .orbitals
        .iter()
        .find(|o| o.name == "ImportB")
        .expect("ImportB present");

    let orbital_core::schema::EntityRef::Inline(b_entity) = &import_b.entity else {
        panic!("ImportB.entity must be inline");
    };
    assert_eq!(b_entity.name, "LocalB");
    let b_id = b_entity.id.clone().expect("LocalB carries a derived id");

    // The relation field on ImportA's materialized primary must retarget to
    // "LocalB" with LocalB's OWN id — not a fresh/foreign id, and not left
    // unmapped.
    let orbital_core::schema::EntityRef::Inline(a_entity) = &import_a.entity else {
        panic!("ImportA.entity must be inline");
    };
    let rel_field = a_entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("bId"))
        .expect("bId field carried");
    let rel = rel_field.relation.as_ref().expect("relation carried");
    assert_eq!(rel.entity, "LocalB");
    assert_eq!(
        rel.entity_id.as_deref(),
        Some(b_id.as_str()),
        "the relation's dual-carry id must equal ImportB's OWN materialized entity id, computed order-free"
    );

    // The `["fetch", "BItem", …]` positional token in the fetcher trait must
    // also retarget.
    let fetcher = import_a.traits.iter().find_map(|t| {
        if let orbital_core::schema::TraitReference::Inline(td) = t {
            if td.name == "ImportAAItemFetcher" {
                return Some(td);
            }
        }
        None
    });
    let fetcher = fetcher.expect("ImportAAItemFetcher trait present");
    let blob = serde_json::to_string(&fetcher.state_machine.transitions).unwrap();
    assert!(blob.contains("\"LocalB\""), "{blob}");
    assert!(!blob.contains("\"BItem\""), "{blob}");
}

// ============================================================================
// ORB_O_CONFIG_DEAD_KNOB — single-file, no `uses`; hand-built schemas.
// ============================================================================

const DEAD_KNOB: &str = "ORB_O_CONFIG_DEAD_KNOB";

fn codes(schema: &OrbitalSchema) -> Vec<String> {
    validate_comprehensive_without_services(schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// One orbital, one trait (`WidgetView`), one page. `app_config` /
/// `orbital_config` / `trait_config` splice in the declared/forwarding
/// blocks each test varies.
fn schema_with(
    app_config: Option<Value>,
    orbital_config: Option<Value>,
    trait_config: Option<Value>,
) -> OrbitalSchema {
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
    let schema = schema_with(
        Some(json!({ "appName": { "type": "string", "default": "App" } })),
        None,
        None,
    );
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

/// G-CROSS-024: an app knob read NESTED in a trait's config value
/// (`metrics[].target`) publishes the knob — both once the inline phase has
/// folded it (provenance in `nestedForwardedFrom`) and while the token is
/// still in place.
#[test]
fn app_knob_read_nested_in_a_trait_config_value_does_not_fire_dead_knob() {
    let app = json!({ "weeklyWinTarget": { "type": "number", "default": 2 } });
    let resolved = schema_with(
        Some(app.clone()),
        None,
        Some(json!({ "metrics": {
            "type": "array",
            "default": [ { "label": "Wins", "target": 2 } ],
            "nestedForwardedFrom": [ "@config.weeklyWinTarget" ]
        } })),
    );
    assert!(
        !codes(&resolved).contains(&DEAD_KNOB.to_string()),
        "the folded nested read must publish 'weeklyWinTarget'"
    );
    let unresolved = schema_with(
        Some(app),
        None,
        Some(json!({ "metrics": {
            "type": "array",
            "default": [ { "label": "Wins", "target": "@config.weeklyWinTarget" } ]
        } })),
    );
    assert!(
        !codes(&unresolved).contains(&DEAD_KNOB.to_string()),
        "an unfolded nested token must publish 'weeklyWinTarget' too"
    );
}

/// Control: nested provenance naming a DIFFERENT knob publishes only that one.
#[test]
fn nested_provenance_for_another_knob_still_fires_dead_knob() {
    let schema = schema_with(
        Some(json!({ "weeklyWinTarget": { "type": "number", "default": 2 } })),
        None,
        Some(json!({ "metrics": {
            "type": "array",
            "default": [ { "target": 5 } ],
            "nestedForwardedFrom": [ "@config.weeklyLeadTarget" ]
        } })),
    );
    assert!(
        codes(&schema).contains(&DEAD_KNOB.to_string()),
        "'weeklyWinTarget' is read nowhere — must fire {DEAD_KNOB}"
    );
}

#[test]
fn app_knob_forwarded_through_an_imported_orbital_knob_does_not_fire_dead_knob() {
    // §107: the consumer folded its app knob into the imported orbital's knob
    // (`forwarded_from` on the ORBITAL field) and a trait forwards THAT knob —
    // the app knob is published through the chain, never dead.
    let schema = schema_with(
        Some(json!({ "googleCalendarId": { "type": "string", "default": "" } })),
        Some(
            json!({ "calendarId": { "type": "string", "default": "", "forwardedFrom": "@config.googleCalendarId" } }),
        ),
        Some(
            json!({ "calendarId": { "type": "string", "default": "", "forwardedFrom": "@config.calendarId" } }),
        ),
    );
    assert!(
        !codes(&schema).contains(&DEAD_KNOB.to_string()),
        "'googleCalendarId' reaches WidgetView through the orbital knob 'calendarId' — must not fire {DEAD_KNOB}"
    );
}

#[test]
fn app_knob_chained_to_an_orbital_knob_no_trait_forwards_fires_dead_knob_twice() {
    let schema = schema_with(
        Some(json!({ "googleCalendarId": { "type": "string", "default": "" } })),
        Some(
            json!({ "calendarId": { "type": "string", "default": "", "forwardedFrom": "@config.googleCalendarId" } }),
        ),
        None,
    );
    assert_eq!(
        codes(&schema)
            .iter()
            .filter(|c| c.as_str() == DEAD_KNOB)
            .count(),
        2,
        "a chain nobody forwards leaves BOTH knobs dead"
    );
}

#[test]
fn orbital_level_unforwarded_knob_fires_dead_knob() {
    let schema = schema_with(
        None,
        Some(json!({ "pageSize": { "type": "number", "default": 10 } })),
        None,
    );
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

// ============================================================================
// ORB_O_CONFIG_DEAD_KNOB regression (§B4-R2): the ref-entry call-site forward
// resolver (`inline/trait.rs`'s `TraitReference::Reference` arm, via
// `EmbedGraph`) COLLAPSES a `@config.<knob>` forward supplied as a
// `ref:` entry's own call-site config override into its resolved literal —
// so the string-survival check `validate_orbital_config_forwarding` used to
// rely on can no longer see it. The fix lives in the IR itself
// (`ConfigField::forwarded_from`, stamped by `forwarded_sibling_config_from`
// at the moment of collapse) rather than a side channel, so the ONE
// `validate_orbital_config_forwarding(schema)` entry point — no extra
// parameter — sees it on the resolved schema directly.
// ============================================================================

/// A plain trait declaring two ALREADY-DECLARED config knobs (`title`,
/// `limit`) with concrete placeholder defaults — a call-site override on an
/// undeclared knob is dropped as non-declaration-form wiring
/// (`ConfigField::is_declaration_form`, `apply_overrides_to_trait`'s
/// GAP-AG-VALUE-DRIFT fold only overwrites an EXISTING declared field's
/// default), so the forward has to land on a key the atom already declares —
/// exactly the real corpus shape (`InlineTypographyRender4.config.content`
/// overrides `TypographyRender`'s own declared `content`).
const REF_FORWARD_UPSTREAM: &str = r#"{
  "name": "test-ref-forward-dead-knob-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "PlainOrbital",
      "entity": {
        "name": "PlainWidget",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "PlainView",
          "linkedEntity": "PlainWidget",
          "category": "interaction",
          "config": {
            "title": { "type": "string", "default": "Untitled" },
            "limit": { "type": "number", "default": 5 }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] } ]
          }
        }
      ]
    }
  ]
}"#;

/// One orbital importing `PlainView` via a `ref:` entry whose call-site
/// `config` forwards BOTH an app-level knob (`appName`) and an orbital-level
/// knob (`pageSize`) — the exact JSX-hoisted shape (`std-time-tracking.lolo`'s
/// `TimesheetAppLayout`) that regressed.
const REF_FORWARD_CONSUMER: &str = r#"{
  "name": "test-ref-forward-dead-knob-consumer",
  "version": "1.0.0",
  "config": { "appName": { "type": "string", "default": "Time Tracking" } },
  "orbitals": [
    {
      "name": "Local",
      "config": { "pageSize": { "type": "number", "default": 25 } },
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": {
        "name": "LocalWidget",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "Up.traits.PlainView",
          "name": "TitleView",
          "linkedEntity": "LocalWidget",
          "config": {
            "title": { "type": "unknown", "default": "@config.appName" },
            "limit": { "type": "unknown", "default": "@config.pageSize" }
          }
        }
      ]
    }
  ]
}"#;

#[test]
fn ref_entry_forwarded_app_and_orbital_knobs_do_not_fire_dead_knob() {
    let collected = collect_with(REF_FORWARD_UPSTREAM, REF_FORWARD_CONSUMER);
    let result = validate_comprehensive_without_services(&collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalConfigDeadKnob),
        "'appName' and 'pageSize' are both forwarded through TitleView's \
         call-site config — must not fire {DEAD_KNOB}: {:#?}",
        result.errors
    );

    // "orb resolve shows the collapsed literal": the ref entry's own
    // override values must be the resolved literals, not the raw tokens,
    // AND each carries `forwarded_from` naming the original token — the
    // provenance the dead-knob check reads once the literal itself is gone.
    let title_view = collected
        .schema
        .orbitals
        .iter()
        .find_map(|o| {
            o.traits.iter().find_map(|t| match t {
                orbital_core::schema::TraitReference::Inline(td) if td.name == "TitleView" => {
                    Some(td)
                }
                _ => None,
            })
        })
        .expect("TitleView must be inlined");
    let cfg = title_view.config.as_ref().expect("config carried");
    let title_field = cfg.get("title").expect("title carried");
    assert_eq!(
        title_field.default,
        Some(orbital_core::schema::SExpression::String(
            "Time Tracking".to_string()
        )),
        "title must resolve to the app-level appName literal"
    );
    assert_eq!(
        title_field.forwarded_from.as_deref(),
        Some("@config.appName"),
        "title's forwarded_from must name the original app-level token"
    );
    let limit_field = cfg.get("limit").expect("limit carried");
    assert_eq!(
        limit_field.default,
        Some(orbital_core::schema::SExpression::Number(25.0)),
        "limit must resolve to the orbital-level pageSize literal"
    );
    assert_eq!(
        limit_field.forwarded_from.as_deref(),
        Some("@config.pageSize"),
        "limit's forwarded_from must name the original orbital-level token"
    );
}

#[test]
fn ref_entry_with_an_unforwarded_orbital_knob_still_fires_dead_knob() {
    // Same shape, but `Local` declares a SECOND orbital-level knob
    // (`unused`) nothing forwards — the fix must not blanket-suppress
    // ORB_O_CONFIG_DEAD_KNOB, only silence it for knobs actually forwarded.
    let consumer = REF_FORWARD_CONSUMER.replacen(
        r#""config": { "pageSize": { "type": "number", "default": 25 } },"#,
        r#""config": { "pageSize": { "type": "number", "default": 25 }, "unused": { "type": "string", "default": "x" } },"#,
        1,
    );
    let collected = collect_with(REF_FORWARD_UPSTREAM, &consumer);
    let result = validate_comprehensive_without_services(&collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalConfigDeadKnob && e.message.contains("unused")),
        "'unused' is declared but forwarded by nothing — must still fire {DEAD_KNOB}: {:#?}",
        result.errors
    );
}

// ============================================================================
// B3-R: the six Stage-B `ORB_O_*` codes — `roles {}` / `entities {}` /
// `pages {}` remap diagnostics on a real two-orbital upstream + a consumer
// with its OWN `[identity]` roster (`Almadar_Orbital_Import.md` §4.5).
// ============================================================================

/// Two orbitals under one alias. `MainOrbital`'s primary `Widget` carries a
/// `read_policy` role-literal comparison and an out-of-orbital relation
/// (`otherId -> Other`, declared by the alias's second orbital); its
/// auxiliary `ThingOwner` is the upstream `[identity]` roster
/// (`role : "employee" | "approver"`). `OtherFetcher` also reads a bare
/// `@Other.name` token. `OtherOrbital` declares `Other`.
const UPSTREAM_STAGE_B: &str = r#"{
  "name": "test-orbital-stage-b-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MainOrbital",
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "read_policy": ["=", "@user.role", "approver"],
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "otherId", "type": "relation", "relation": { "entity": "Other", "cardinality": "one" } }
        ]
      },
      "auxiliaryEntities": [
        {
          "name": "ThingOwner",
          "identity": true,
          "persistence": "persistent",
          "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "role", "type": "string", "values": ["employee", "approver"] }
          ]
        }
      ],
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
        },
        {
          "name": "OtherFetcher",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "LOAD",
                "guard": ["!=", "@Other.name", ""],
                "effects": [ ["fetch", "Other", {}] ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "MainPage", "path": "/main", "primaryEntity": "Widget", "traits": [ { "ref": "WidgetView" } ] }
      ]
    },
    {
      "name": "OtherOrbital",
      "entity": {
        "name": "Other",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "name", "type": "string" }
        ]
      },
      "traits": []
    }
  ]
}"#;

/// `orbital Local = Up.orbitals.MainOrbital { <reference_extra> }`, alongside
/// the CONSUMER's own `[identity]` roster (`Member`, `role : "owner" |
/// "member"`) and a plain `ConsumerOther` entity — the `entities {}` remap
/// target for `MainOrbital`'s out-of-orbital `Other` relation. Both carry an
/// explicit `id` so `entities {}` can resolve them (Stage B only maps a
/// target that has one).
fn orbital_ref_consumer_stage_b(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-orbital-stage-b-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.MainOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Up.orbitals.MainOrbital"{extra}
      }}
    }},
    {{
      "name": "MemberOrbital",
      "entity": {{
        "id": "ent_MEMBERENTITYID000000000",
        "name": "Member",
        "identity": true,
        "persistence": "persistent",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "role", "type": "string", "values": ["owner", "member"] }}
        ]
      }},
      "traits": []
    }},
    {{
      "name": "ConsumerOtherOrbital",
      "entity": {{
        "id": "ent_CONSUMEROTHERID00000000",
        "name": "ConsumerOther",
        "persistence": "persistent",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": []
    }}
  ]
}}"#,
        extra = reference_extra
    )
}

fn collect_stage_b(reference_extra: &str) -> Collected {
    collect_with(
        UPSTREAM_STAGE_B,
        &orbital_ref_consumer_stage_b(reference_extra),
    )
}

/// Same as `orbital_ref_consumer_stage_b`, except `Member` carries a SECOND
/// vocabulary-bearing field, `status : "active" | "leave" | "terminated"` —
/// Project Friday's `Person` roster shape (a role field alongside an
/// unrelated status field). A `roles {}` target of `"active"` must be judged
/// against `Member.role`'s vocabulary specifically, never pass because it
/// happens to be a member of `Member.status`'s vocabulary instead.
fn orbital_ref_consumer_stage_b_with_status(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-orbital-stage-b-consumer-status",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.MainOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Up.orbitals.MainOrbital"{extra}
      }}
    }},
    {{
      "name": "MemberOrbital",
      "entity": {{
        "id": "ent_MEMBERENTITYID000000000",
        "name": "Member",
        "identity": true,
        "persistence": "persistent",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "role", "type": "string", "values": ["owner", "member"] }},
          {{ "name": "status", "type": "string", "values": ["active", "leave", "terminated"] }}
        ]
      }},
      "traits": []
    }},
    {{
      "name": "ConsumerOtherOrbital",
      "entity": {{
        "id": "ent_CONSUMEROTHERID00000000",
        "name": "ConsumerOther",
        "persistence": "persistent",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": []
    }}
  ]
}}"#,
        extra = reference_extra
    )
}

fn collect_stage_b_with_status(reference_extra: &str) -> Collected {
    collect_with(
        UPSTREAM_STAGE_B,
        &orbital_ref_consumer_stage_b_with_status(reference_extra),
    )
}

// --- ORB_O_ROLE_TARGET_NOT_MEMBER -------------------------------------------

#[test]
fn role_target_not_member_of_consumer_roster_fires() {
    let collected = collect_stage_b(r#", "roles": { "approver": ["nonmember"] }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleTargetNotMember
                && e.message.contains("nonmember")
                && e.message.contains("Member")),
        "expected ORB_O_ROLE_TARGET_NOT_MEMBER for 'nonmember', got: {:#?}",
        result.errors
    );
}

#[test]
fn role_target_that_is_a_consumer_roster_member_does_not_fire() {
    let collected = collect_stage_b(r#", "roles": { "approver": ["owner"] }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleTargetNotMember),
        "'owner' is a member of the consumer roster ('Member') — must not fire: {:#?}",
        result.errors
    );
}

/// B4-R4: a `roles {}` target that is a member of some OTHER field's
/// vocabulary on the consumer roster (`Member.status`'s `"active"`) must not
/// pass by matching the wrong field — `ORB_O_ROLE_TARGET_NOT_MEMBER` fires
/// naming `Member.role` specifically, and the downstream rewritten policy
/// (`@user.role == "active"`) still fires `ORB_S_ROLE_LITERAL_NOT_MEMBER` —
/// both name the same cause, which is fine.
#[test]
fn role_target_that_is_a_different_fields_vocabulary_member_fires_org_o_and_downstream_org_s() {
    let collected = collect_stage_b_with_status(r#", "roles": { "approver": ["active"] }"#);

    let org_o = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        org_o
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleTargetNotMember
                && e.message.contains("active")
                && e.message.contains("role")),
        "'active' is a member of Member.status's vocabulary, not Member.role's — \
         ORB_O_ROLE_TARGET_NOT_MEMBER must still fire, naming the `role` field: {:#?}",
        org_o.errors
    );

    let comprehensive = validate_comprehensive_without_services(&collected.schema);
    assert!(
        comprehensive.errors.iter().any(
            |e| e.code == ErrorCode::SchemaRoleLiteralNotMember && e.message.contains("active")
        ),
        "the rewritten policy compares `@user.role` to \"active\" directly — the downstream \
         ORB_S_ROLE_LITERAL_NOT_MEMBER check must also fire: {:#?}",
        comprehensive.errors
    );
}

/// A `roles {}` list mixing a genuine roster member (`owner`) with a
/// different field's vocabulary value (`active`) fires BOTH codes for the
/// invalid member only — `owner` must stay silent on both checks.
#[test]
fn role_target_list_with_one_status_value_fires_both_codes_for_that_member_only() {
    let collected =
        collect_stage_b_with_status(r#", "roles": { "approver": ["owner", "active"] }"#);

    let org_o = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    let org_o_target_errors: Vec<_> = org_o
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::OrbitalRefRoleTargetNotMember)
        .collect();
    assert_eq!(
        org_o_target_errors.len(),
        1,
        "expected exactly one ORB_O_ROLE_TARGET_NOT_MEMBER (for 'active' only, not 'owner'), got {:#?}",
        org_o.errors
    );
    assert!(org_o_target_errors[0].message.contains("active"));

    let comprehensive = validate_comprehensive_without_services(&collected.schema);
    let role_literal_errors: Vec<_> = comprehensive
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::SchemaRoleLiteralNotMember)
        .collect();
    assert_eq!(
        role_literal_errors.len(),
        1,
        "expected exactly one ORB_S_ROLE_LITERAL_NOT_MEMBER (for 'active' only), got {:#?}",
        comprehensive.errors
    );
    assert!(role_literal_errors[0].message.contains("active"));
}

/// The valid Project-Friday-shaped mapping (`approver -> owner`, with a
/// `status` field also present on the roster) stays silent on both checks.
#[test]
fn role_target_valid_mapping_stays_silent_even_with_a_status_field_present() {
    let collected = collect_stage_b_with_status(r#", "roles": { "approver": ["owner"] }"#);

    let org_o = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !org_o
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleTargetNotMember),
        "{:#?}",
        org_o.errors
    );

    let comprehensive = validate_comprehensive_without_services(&collected.schema);
    assert!(
        !comprehensive
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaRoleLiteralNotMember),
        "{:#?}",
        comprehensive.errors
    );
}

// --- ORB_O_ROLE_UNKNOWN_UPSTREAM ---------------------------------------------

#[test]
fn role_key_unknown_upstream_fires() {
    let collected = collect_stage_b(r#", "roles": { "bogus": ["owner"] }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleUnknownUpstream
                && e.message.contains("bogus")),
        "expected ORB_O_ROLE_UNKNOWN_UPSTREAM for 'bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn role_key_declared_upstream_does_not_fire_unknown_upstream() {
    let collected = collect_stage_b(r#", "roles": { "approver": ["owner"] }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefRoleUnknownUpstream),
        "'approver' is in upstream ThingOwner's vocabulary — must not fire: {:#?}",
        result.errors
    );
}

// --- ORB_O_ENTITY_UNMAPPED ----------------------------------------------------

#[test]
fn out_of_orbital_relation_left_unmapped_fires_entity_unmapped() {
    let collected = collect_stage_b("");
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityUnmapped && e.message.contains("Other")),
        "expected ORB_O_ENTITY_UNMAPPED for the unmapped 'Other' relation, got: {:#?}",
        result.errors
    );
}

#[test]
fn out_of_orbital_relation_mapped_does_not_fire_entity_unmapped() {
    let collected = collect_stage_b(r#", "entities": { "Other": "ConsumerOther" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityUnmapped),
        "'Other' is mapped to 'ConsumerOther' — must not fire: {:#?}",
        result.errors
    );
}

// --- ORB_O_ENTITY_UNKNOWN_KEY -------------------------------------------------

#[test]
fn entities_key_naming_no_upstream_entity_fires_unknown_key() {
    let collected =
        collect_stage_b(r#", "entities": { "Other": "ConsumerOther", "Bogus": "ConsumerOther" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityUnknownKey && e.message.contains("Bogus")),
        "expected ORB_O_ENTITY_UNKNOWN_KEY for 'Bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn entities_key_naming_the_out_of_orbital_entity_does_not_fire_unknown_key() {
    let collected = collect_stage_b(r#", "entities": { "Other": "ConsumerOther" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityUnknownKey),
        "'Other' names the alias's own out-of-orbital entity — must not fire: {:#?}",
        result.errors
    );
}

// --- ORB_O_ENTITY_TARGET_UNKNOWN ----------------------------------------------

#[test]
fn entities_target_naming_no_consumer_entity_fires_target_unknown() {
    let collected = collect_stage_b(r#", "entities": { "Other": "NoSuchConsumerEntity" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityTargetUnknown
                && e.message.contains("NoSuchConsumerEntity")),
        "expected ORB_O_ENTITY_TARGET_UNKNOWN for 'NoSuchConsumerEntity', got: {:#?}",
        result.errors
    );
}

#[test]
fn entities_target_naming_a_declared_consumer_entity_does_not_fire_target_unknown() {
    let collected = collect_stage_b(r#", "entities": { "Other": "ConsumerOther" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefEntityTargetUnknown),
        "'ConsumerOther' is declared by the consumer schema — must not fire: {:#?}",
        result.errors
    );
}

// --- ORB_O_PAGE_UNKNOWN_PATH ---------------------------------------------------

#[test]
fn pages_key_naming_no_upstream_path_fires_page_unknown_path() {
    let collected = collect_stage_b(r#", "pages": { "/bogus": "/x" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefPageUnknownPath && e.message.contains("/bogus")),
        "expected ORB_O_PAGE_UNKNOWN_PATH for '/bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn page_modifiers_key_naming_no_upstream_path_fires_page_unknown_path() {
    let collected =
        collect_stage_b(r#", "pageModifiers": { "/bogus": { "indexing": "noindex" } }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefPageUnknownPath && e.message.contains("/bogus")),
        "expected ORB_O_PAGE_UNKNOWN_PATH for '/bogus', got: {:#?}",
        result.errors
    );
}

#[test]
fn pages_key_naming_a_real_upstream_path_does_not_fire_page_unknown_path() {
    let collected = collect_stage_b(r#", "pages": { "/main": "/widgets" }"#);
    let result = validate_orbital_ref_diagnostics(&collected.orbital_ref, &collected.schema);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::OrbitalRefPageUnknownPath),
        "'/main' is a real upstream page path — must not fire: {:#?}",
        result.errors
    );
}
