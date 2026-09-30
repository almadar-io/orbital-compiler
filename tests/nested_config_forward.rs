//! G-CROSS-024: an app knob read NESTED in a composed `{ref}` trait's call-site
//! config value (`metrics[].target`) folds to its value through inline, keeps
//! its provenance on the inlined trait's declared field (the override fold in
//! `apply_overrides_to_trait`), and so publishes the app knob to
//! `ORB_O_CONFIG_DEAD_KNOB`.

use orbital_compiler::phases::inline::inline_schema;
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::orbital_config::validate_orbital_config_forwarding;
use orbital_compiler::phases::InlineConfig;
use orbital_core::schema::{SExpression, TraitReference};

const ATOM: &str = r#"{
  "name": "stats-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "StatsOrbital",
      "entity": { "name": "StatsItem", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "StatsRender",
          "category": "interaction",
          "config": { "metrics": { "type": "unknown", "default": [] }, "title": { "type": "string", "default": "Stats" } },
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

fn inlined(metrics: &str) -> orbital_core::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("atom");
    let path = dir.path().join("composer.orb");
    std::fs::write(&path, composer(metrics)).expect("composer");
    let mut schema = parse_file(&path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    schema
}

fn metrics_field(schema: &orbital_core::schema::OrbitalSchema) -> orbital_core::schema::ConfigField {
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
    let codes: Vec<String> = validate_orbital_config_forwarding(&schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    assert!(!codes.contains(&"ORB_O_CONFIG_DEAD_KNOB".to_string()), "{codes:?}");
}

#[test]
fn control_an_override_that_never_reads_the_app_knob_leaves_it_dead() {
    let schema = inlined(r#"[ { "label": "Wins", "target": 5 } ]"#);
    assert!(metrics_field(&schema).nested_forwarded_from.is_empty());
    let codes: Vec<String> = validate_orbital_config_forwarding(&schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    assert!(codes.contains(&"ORB_O_CONFIG_DEAD_KNOB".to_string()), "{codes:?}");
}
