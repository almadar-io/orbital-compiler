//! W4-R + B3-R: the orbital-import validation codes
//! (`docs/Almadar_Orbital_Import.md` §4.5) — one positive + one passing
//! negative per code, plus the page-path collision regression and the
//! backwards-compatibility (no-config) sweep.
//!
//! - `ORB_U_CONFIG_UNKNOWN_KEY`: converted from
//!   [`orbital_compiler::phases::inline::UsesConfigDiagnostic`]s collected in
//!   the `uses` loop.
//! - `ORB_O_ROLE_TARGET_NOT_MEMBER` / `ORB_O_ROLE_UNKNOWN_UPSTREAM` /
//!   `ORB_O_ENTITY_UNMAPPED` / `ORB_O_ENTITY_UNKNOWN_KEY` /
//!   `ORB_O_ENTITY_TARGET_UNKNOWN` (B3-R): the
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
use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

// ============================================================================
// ORB_U_CONFIG_UNKNOWN_KEY
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
    let mut orbital_ref = Vec::new();
    let mut uses_config = Vec::new();
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
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
        if let orbital_core_base::schema::TraitReference::Inline(td) = t {
            if td.name == "LocalShellHeader" {
                return td.config.as_ref().and_then(|c| c.get("title")).cloned();
            }
        }
        None
    });
    let title = title.expect("LocalShellHeader.config.title present");
    assert_eq!(
        title.default,
        Some(orbital_core_base::schema::SExpression::String(("Probe".to_string()).into())),
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
        if let orbital_core_base::schema::TraitReference::Inline(td) = t {
            if td.name == "LocalShellHeader" {
                return td.config.as_ref().and_then(|c| c.get("title")).cloned();
            }
        }
        None
    });
    let title = title.expect("LocalShellHeader.config.title present");
    assert_eq!(
        title.default,
        Some(orbital_core_base::schema::SExpression::String(("App".to_string()).into())),
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

    let orbital_core_base::schema::EntityRef::Inline(b_entity) = &import_b.entity else {
        panic!("ImportB.entity must be inline");
    };
    assert_eq!(b_entity.name, "LocalB");
    let b_id = b_entity.id.clone().expect("LocalB carries a derived id");

    // The relation field on ImportA's materialized primary must retarget to
    // "LocalB" with LocalB's OWN id — not a fresh/foreign id, and not left
    // unmapped.
    let orbital_core_base::schema::EntityRef::Inline(a_entity) = &import_a.entity else {
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
        if let orbital_core_base::schema::TraitReference::Inline(td) = t {
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
// Orbital config codes — single-file, no `uses`; hand-built schemas (ORB_O_CONFIG_DEAD_KNOB is the std lint
// `std-orb-lint-orbital-config-dead-knob`).
// ============================================================================

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
fn no_config_anywhere_produces_no_config_code() {
    let schema = schema_with(None, None, None);
    let cs = codes(&schema);
    assert!(
        !cs.contains(&"ORB_U_CONFIG_UNKNOWN_KEY".to_string()),
        "ORB_U_CONFIG_UNKNOWN_KEY must not fire on a schema declaring no config anywhere: {cs:?}"
    );
}

// ============================================================================
// Ref-entry call-site forward (§B4-R2; the dead-knob half is the std lint `std-orb-lint-orbital-config-dead-knob`): the ref-entry call-site forward
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
/// naming `Member.role` specifically.
#[test]
fn role_target_that_is_a_different_fields_vocabulary_member_fires_org_o() {
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
}

/// A `roles {}` list mixing a genuine roster member (`owner`) with a
/// different field's vocabulary value (`active`) fires
/// ORB_O_ROLE_TARGET_NOT_MEMBER for the invalid member only.
#[test]
fn role_target_list_with_one_status_value_fires_for_that_member_only() {
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
}

/// The valid Project-Friday-shaped mapping (`approver -> owner`, with a
/// `status` field also present on the roster) stays silent.
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

fn orbital_references_of(consumer_source: &str) -> Vec<Value> {
    let (_dir, consumer_path) = materialize(consumer_source);
    let session = orbital_compiler::phases::inline::InlineSession::default();
    session.collect_references();
    let config = InlineConfig { session: Some(session.clone()), ..InlineConfig::default() };
    let mut schema = parse_file(&consumer_path).expect("consumer must parse");
    inline_schema(&mut schema, &config, Some(&consumer_path)).expect("inline must succeed");
    serde_json::to_value(session.take_references())
        .expect("serializes")
        .as_array()
        .cloned()
        .expect("a list")
}

#[test]
fn an_orbital_reference_with_extend_is_paired_with_its_upstream_orbital() {
    let references = orbital_references_of(&orbital_ref_consumer(
        r#", "extend": [ { "name": "tier", "type": "string" } ]"#,
    ));
    assert_eq!(references.len(), 1, "{references:#?}");
    let pair = &references[0];
    assert_eq!(pair["kind"], "orbital");
    assert_eq!(pair["alias"], "Up");
    assert_eq!(pair["name"], "WidgetOrbital");
    assert_eq!(pair["path"], "orbitals[0]");
    assert_eq!(pair["callSite"]["ref"], "Up.orbitals.WidgetOrbital");
    assert_eq!(pair["callSite"]["extend"][0]["name"], "tier");
    assert_eq!(pair["declaration"]["name"], "WidgetOrbital");
    assert_eq!(pair["declaration"]["entity"]["name"], "Widget");
}

#[test]
fn control_an_orbital_reference_without_extend_carries_none_in_its_call_site() {
    let references = orbital_references_of(&orbital_ref_consumer(""));
    assert_eq!(references.len(), 1, "{references:#?}");
    assert!(references[0]["callSite"].get("extend").is_none(), "{}", references[0]["callSite"]);
}
