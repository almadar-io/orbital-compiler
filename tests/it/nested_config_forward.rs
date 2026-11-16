//! G-CROSS-024: an app knob read NESTED in a composed `{ref}` trait's call-site
//! config value (`metrics[].target`) folds to its value through inline, keeps
//! its provenance on the inlined trait's declared field (the override fold in
//! `apply_overrides_to_trait`), and so publishes the app knob to the std lint
//! `std-orb-lint-orbital-config-dead-knob`.

use orbital_compiler::phases::inline::inline_schema;
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::InlineConfig;
use orbital_core_base::schema::{SExpression, TraitReference};

const ATOM_TEMPLATE: &str = r#"{
  "name": "stats-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "StatsOrbital",
      "entity": { "name": "StatsItem", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "StatsRender",
          "config": { "metrics": METRICS_DECL, "title": { "type": "string", "default": "Stats" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD", "name": "Load" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD", "effects": [] } ]
          }
        }
      ],
      "pages": []
    }
  ]
}"#;

const LIST_METRICS: &str = r#"{ "type": "[object]", "items": { "type": "object", "properties": { "label": { "type": "string" }, "target": { "type": "number" } } }, "default": [] }"#;
const MAP_METRICS: &str = r#"{ "type": "object", "properties": { "w": { "type": "number" }, "x": { "type": "number" }, "y": { "type": "number" }, "z": { "type": "number" } }, "default": {} }"#;

fn atom(metrics_decl: &str) -> String {
    ATOM_TEMPLATE.replace("METRICS_DECL", metrics_decl)
}

fn composer(metrics: &str) -> String {
    format!(
        r#"{{
  "name": "dashboard",
  "version": "1.0.0",
  "config": {{ "weeklyWinTarget": {{ "type": "number", "default": 2 }} }},
  "orbitals": [
    {{
      "name": "DashboardOrbital",
      "uses": [ {{ "from": "./atom.orb", "as": "Stats" }} ],
      "entity": {{ "name": "Deal", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [
        {{ "ref": "Stats.traits.StatsRender", "name": "WeeklyStats", "config": {{ "metrics": {{ "type": "unknown", "default": {metrics} }} }} }}
      ]
    }}
  ]
}}"#
    )
}

fn inlined(metrics: &str) -> orbital_core_base::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom(LIST_METRICS)).expect("atom");
    let path = dir.path().join("composer.orb");
    std::fs::write(&path, composer(metrics)).expect("composer");
    let mut schema = parse_file(&path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    schema
}

fn metrics_field(schema: &orbital_core_base::schema::OrbitalSchema) -> orbital_core_base::schema::ConfigField {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "WeeklyStats" => td.config.as_ref()?.get("metrics").cloned(),
            _ => None,
        })
        .expect("WeeklyStats.metrics")
}

#[test]
fn nested_app_knob_folds_and_publishes() {
    let schema = inlined(r#"[ { "label": "Wins", "target": "@config.weeklyWinTarget" } ]"#);
    let metrics = metrics_field(&schema);
    assert_eq!(
        metrics.default,
        Some(serde_json::from_value::<SExpression>(serde_json::json!([{ "label": "Wins", "target": 2 }])).expect("sexpr")),
    );
    assert_eq!(metrics.nested_forwarded_from, vec!["@config.weeklyWinTarget".to_string()]);
}


fn inlined_with_knobs(metrics: &str) -> orbital_core_base::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom(MAP_METRICS)).expect("atom");
    let path = dir.path().join("composer.orb");
    let body = composer(metrics).replace(
        r#""config": { "weeklyWinTarget": { "type": "number", "default": 2 } }"#,
        r#""config": { "alpha": { "type": "number", "default": 1 }, "bravo": { "type": "number", "default": 2 }, "charlie": { "type": "number", "default": 3 }, "delta": { "type": "number", "default": 4 } }"#,
    );
    std::fs::write(&path, body).expect("composer");
    let mut schema = parse_file(&path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    schema
}

/// Resolve output must be reproducible: the provenance of several nested knobs
/// comes out in one fixed order, not the order a hash map happened to walk.
#[test]
fn several_nested_knobs_are_recorded_in_a_stable_order() {
    let metrics = r#"{ "w": "@config.delta", "x": "@config.charlie", "y": "@config.bravo", "z": "@config.alpha" }"#;
    let expected: Vec<String> = ["@config.alpha", "@config.bravo", "@config.charlie", "@config.delta"].iter().map(|s| s.to_string()).collect();
    for _ in 0..8 {
        assert_eq!(metrics_field(&inlined_with_knobs(metrics)).nested_forwarded_from, expected);
    }
}

const KNOB_ATOM: &str = r#"{
  "name": "knob-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "BaseOrbital",
      "config": { "mix": { "type": "number", "default": 3 } },
      "entity": { "name": "Item", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "Ranker",
          "linkedEntity": "Item",
          "config": { "rank": { "type": "number", "default": "@config.mix" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD", "name": "Load" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD", "effects": [] } ]
          }
        }
      ],
      "pages": []
    }
  ]
}"#;

const KNOB_MID: &str = r#"{
  "name": "knob-mid",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MidOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Base" } ],
      "entity": "Base.orbitals.BaseOrbital.entity",
      "traits": [],
      "reference": { "ref": "Base.orbitals.BaseOrbital" }
    }
  ]
}"#;

fn knob_orbital_config(levels: u8) -> Vec<String> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), KNOB_ATOM).expect("atom");
    std::fs::write(dir.path().join("mid.orb"), KNOB_MID).expect("mid");
    let top = if levels == 2 {
        r#"{ "name": "knob-top", "version": "1.0.0", "orbitals": [{
          "name": "TopOrbital", "uses": [ { "from": "./mid.orb", "as": "Mid" } ],
          "entity": "Mid.orbitals.MidOrbital.entity", "traits": [],
          "reference": { "ref": "Mid.orbitals.MidOrbital" } }] }"#
    } else {
        r#"{ "name": "knob-top", "version": "1.0.0", "orbitals": [{
          "name": "TopOrbital", "uses": [ { "from": "./atom.orb", "as": "Base" } ],
          "entity": "Base.orbitals.BaseOrbital.entity", "traits": [],
          "reference": { "ref": "Base.orbitals.BaseOrbital" } }] }"#
    };
    let path = dir.path().join("top.orb");
    std::fs::write(&path, top).expect("top");
    let mut schema = parse_file(&path).expect("top parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    let mut keys: Vec<String> = schema.orbitals[0]
        .config
        .iter()
        .flat_map(|c| c.keys().cloned())
        .collect();
    keys.sort();
    keys
}

#[test]
fn control_an_orbital_knob_a_trait_forwards_survives_one_import() {
    assert_eq!(knob_orbital_config(1), vec!["mix".to_string()]);
}

#[test]
fn an_orbital_knob_a_trait_forwarded_survives_a_nested_import() {
    assert_eq!(knob_orbital_config(2), vec!["mix".to_string()]);
}
