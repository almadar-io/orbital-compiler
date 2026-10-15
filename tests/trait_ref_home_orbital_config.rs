//! §B4-R5: a §8 trait-form reference (`Alias.traits.X`) resolves the
//! REFERENCED trait's own `@config.<knob>` forwards, not just its call-site
//! overrides. When the knob is native to the orbital that DECLARES the
//! trait (an orbital-level knob, e.g. `TimesheetPanelOrbital.config.
//! browseColumns` forwarded by its inline `TimesheetBrowseList.config.
//! columns`), the consumer's own rungs (embedders, `orbital.config`,
//! `schema.config`) never see it — before this fix the forward survived
//! inline unresolved and `ORB_CONFIG_DANGLING_REF` fired naming a knob no
//! trait in the consumer's referrer chain declares.
//!
//! Fixture: `home.orb` declares one app-level knob (`appName`) and one
//! orbital-level knob (`browseColumns`) on `HomeOrbital`; its inline trait
//! `BrowseList` forwards both (`columns: @config.browseColumns`, `subtitle:
//! @config.appName`). Three consumers import ONLY the trait
//! (`Home.traits.BrowseList`), never the orbital:
//!   1. `consumer_baseline_source` — no consumer-side declaration of either
//!      knob anywhere: both forwards must resolve through the upstream
//!      chain (home-orbital config, then that alias's own app config).
//!   2. `consumer_orbital_override_source` — the CONSUMER's own orbital
//!      declares `browseColumns` too: the consumer's value must win over
//!      the upstream orbital's.
//!   3. `consumer_uses_config_override_source` — the consumer's `uses {
//!      config { appName } }` overrides the upstream's own app-level
//!      default: the override must win.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core::schema::{OrbitalSchema, SExpression, TraitReference};

/// `HomeOrbital` declares the orbital-level knob `browseColumns` (default
/// `["upstreamCol"]`) and the app declares `appName` (default `"Upstream
/// App"`). The inline trait `BrowseList` forwards both under DIFFERENT
/// config keys (`columns`/`subtitle`) — the general JSX-hoisted shape, not a
/// same-name coincidence.
const HOME: &str = r#"{
  "name": "test-trait-ref-home-orbital-upstream",
  "version": "1.0.0",
  "config": {
    "appName": { "type": "string", "default": "Upstream App" }
  },
  "orbitals": [
    {
      "name": "HomeOrbital",
      "config": {
        "browseColumns": { "type": "unknown", "default": ["upstreamCol"] }
      },
      "entity": {
        "name": "HomeItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "BrowseList",
          "linkedEntity": "HomeItem",
          "category": "interaction",
          "config": {
            "columns": { "type": "unknown", "default": "@config.browseColumns" },
            "subtitle": { "type": "string", "default": "@config.appName" }
          },
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }
      ]
    }
  ]
}"#;

fn consumer_baseline_source() -> &'static str {
    r#"{
  "name": "test-trait-ref-home-orbital-consumer-baseline",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ConsumerOrbital",
      "uses": [ { "from": "./home.orb", "as": "Home" } ],
      "entity": {
        "name": "ConsumerItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Home.traits.BrowseList", "name": "Local", "linkedEntity": "ConsumerItem" }
      ]
    }
  ]
}"#
}

fn consumer_orbital_override_source() -> &'static str {
    r#"{
  "name": "test-trait-ref-home-orbital-consumer-orbital-override",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ConsumerOrbital",
      "uses": [ { "from": "./home.orb", "as": "Home" } ],
      "config": {
        "browseColumns": { "type": "unknown", "default": ["consumerCol"] }
      },
      "entity": {
        "name": "ConsumerItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Home.traits.BrowseList", "name": "Local", "linkedEntity": "ConsumerItem" }
      ]
    }
  ]
}"#
}

fn consumer_uses_config_override_source() -> &'static str {
    r#"{
  "name": "test-trait-ref-home-orbital-consumer-uses-override",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ConsumerOrbital",
      "uses": [ { "from": "./home.orb", "as": "Home", "config": {
        "appName": { "type": "string", "default": "Override App" }
      } } ],
      "entity": {
        "name": "ConsumerItem",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Home.traits.BrowseList", "name": "Local", "linkedEntity": "ConsumerItem" }
      ]
    }
  ]
}"#
}

/// Write `home.orb` + the given consumer source into a fresh temp dir,
/// inline the consumer, and return the fully-inlined schema.
fn materialize(consumer_source: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("home.orb"), HOME).expect("write home.orb");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer_source).expect("write consumer.orb");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn local_trait(schema: &OrbitalSchema) -> &orbital_core::schema::TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Local" => Some(td),
            _ => None,
        })
        .expect("expected inlined trait Local")
}

fn config_default(td: &orbital_core::schema::TraitDefinition, key: &str) -> Option<SExpression> {
    td.config.as_ref()?.get(key)?.default.clone()
}

fn config_forwarded_from(td: &orbital_core::schema::TraitDefinition, key: &str) -> Option<String> {
    td.config.as_ref()?.get(key)?.forwarded_from.clone()
}

#[test]
fn trait_form_reference_resolves_forward_against_the_home_orbital_when_the_consumer_declares_nothing(
) {
    let schema = materialize(consumer_baseline_source());
    let local = local_trait(&schema);

    assert_eq!(
        config_default(local, "columns"),
        Some(SExpression::Array(vec![SExpression::String(
            "upstreamCol".to_string()
        )])),
        "columns must resolve to HomeOrbital's declared browseColumns default, \
         not stay an unresolved @config.browseColumns forward"
    );
    assert_eq!(
        config_forwarded_from(local, "columns").as_deref(),
        Some("@config.browseColumns"),
        "provenance must name the ORIGINAL forward token"
    );

    assert_eq!(
        config_default(local, "subtitle"),
        Some(SExpression::String("Upstream App".to_string())),
        "subtitle must resolve to the upstream file's own app-level appName default"
    );
    assert_eq!(
        config_forwarded_from(local, "subtitle").as_deref(),
        Some("@config.appName")
    );

    let result = validate_comprehensive_without_services(&schema);
    let dangling: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
        .collect();
    assert!(
        dangling.is_empty(),
        "expected 0 ORB_CONFIG_DANGLING_REF, got: {dangling:#?}"
    );
}

#[test]
fn trait_form_reference_prefers_the_consumers_own_orbital_declaration_over_the_home_orbital() {
    let schema = materialize(consumer_orbital_override_source());
    let local = local_trait(&schema);

    assert_eq!(
        config_default(local, "columns"),
        Some(SExpression::Array(vec![SExpression::String(
            "consumerCol".to_string()
        )])),
        "the consumer's own orbital-level browseColumns must win over HomeOrbital's"
    );

    let result = validate_comprehensive_without_services(&schema);
    let dangling: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
        .collect();
    assert!(
        dangling.is_empty(),
        "expected 0 ORB_CONFIG_DANGLING_REF, got: {dangling:#?}"
    );
}

#[test]
fn trait_form_reference_honors_the_uses_config_override_of_an_upstream_app_knob() {
    let schema = materialize(consumer_uses_config_override_source());
    let local = local_trait(&schema);

    assert_eq!(
        config_default(local, "subtitle"),
        Some(SExpression::String("Override App".to_string())),
        "the uses-config override of appName must win over \
         the upstream file's own appName default"
    );

    // browseColumns has no consumer-side declaration at all in this fixture,
    // so it must still fall through to HomeOrbital's default — the override
    // rung is additive, not a replacement for the whole upstream chain.
    assert_eq!(
        config_default(local, "columns"),
        Some(SExpression::Array(vec![SExpression::String(
            "upstreamCol".to_string()
        )])),
    );

    let result = validate_comprehensive_without_services(&schema);
    let dangling: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
        .collect();
    assert!(
        dangling.is_empty(),
        "expected 0 ORB_CONFIG_DANGLING_REF, got: {dangling:#?}"
    );
}
