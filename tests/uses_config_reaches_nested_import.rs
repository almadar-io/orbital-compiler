//! A `uses Desk { config { appName: … } }` override reaches an orbital Desk itself
//! IMPORTS (`DeskAssistant = Agent.orbitals.AgentOrbital { config { appName:
//! @config.appName } }`), not only Desk's own traits: the solution's app name and
//! menu must reach the assistant page a back office carries. Two consumers of one
//! Desk with different overrides each get their own value.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};

const AGENT: &str = r#"{
  "name": "agent",
  "version": "1.0.0",
  "orbitals": [{
    "name": "AgentOrbital",
    "config": { "appName": { "type": "string", "default": "Assistant" } },
    "entity": { "name": "AgentState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
    "traits": [{
      "name": "AgentLayout",
      "linkedEntity": "AgentState",
      "category": "interaction",
      "config": { "appName": { "type": "string", "default": "@config.appName" } },
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [ { "key": "INIT" } ],
        "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "@config.appName" }] ] } ]
      }
    }],
    "pages": [ { "name": "AgentPage", "path": "/assistant", "traits": [ { "ref": "AgentLayout" } ] } ]
  }]
}"#;

const DESK: &str = r#"{
  "name": "desk",
  "version": "1.0.0",
  "config": { "appName": { "type": "string", "default": "Desk" } },
  "orbitals": [{
    "name": "DeskAssistant",
    "uses": [ { "from": "./agent.orb", "as": "Agent" } ],
    "entity": "Agent.orbitals.AgentOrbital.entity",
    "traits": [],
    "reference": { "ref": "Agent.orbitals.AgentOrbital", "config": { "appName": { "type": "string", "default": "@config.appName" } } }
  }]
}"#;

fn consumer(overrides: &[(&str, Option<&str>)]) -> String {
    let orbitals: Vec<String> = overrides
        .iter()
        .map(|(name, app)| {
            let config = app.map_or(String::new(), |a| format!(r#", "config": {{ "appName": {{ "type": "string", "default": "{a}" }} }}"#));
            format!(
                r#"{{ "name": "{name}", "uses": [ {{ "from": "./desk.orb", "as": "Desk"{config} }} ],
                  "entity": "Desk.orbitals.DeskAssistant.entity", "traits": [],
                  "reference": {{ "ref": "Desk.orbitals.DeskAssistant", "pages": {{ "/assistant": "/{route}" }} }} }}"#,
                route = name.to_lowercase()
            )
        })
        .collect();
    format!(r#"{{ "name": "consumer", "version": "1.0.0", "orbitals": [ {} ] }}"#, orbitals.join(", "))
}

fn app_names(overrides: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("agent.orb"), AGENT).expect("write agent");
    std::fs::write(dir.path().join("desk.orb"), DESK).expect("write desk");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(overrides)).expect("write consumer");
    let mut schema: OrbitalSchema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("inline succeeds");
    schema
        .orbitals
        .iter()
        .flat_map(|o| {
            o.traits.iter().filter_map(move |t| match t {
                TraitReference::Inline(def) if def.name.ends_with("AgentLayout") => {
                    let value = def.config.as_ref().and_then(|c| c.get("appName")).and_then(|f| f.default.clone());
                    match value {
                        Some(SExpression::String(s)) => Some((o.name.clone(), s.to_string())),
                        other => Some((o.name.clone(), format!("{other:?}"))),
                    }
                }
                _ => None,
            })
        })
        .collect()
}

#[test]
fn the_uses_override_reaches_the_orbital_the_alias_imports() {
    assert_eq!(app_names(&[("Shop", Some("Atelier"))]), vec![("Shop".to_string(), "Atelier".to_string())]);
}

#[test]
fn control_without_an_override_the_alias_default_applies() {
    assert_eq!(app_names(&[("Shop", None)]), vec![("Shop".to_string(), "Desk".to_string())]);
}

#[test]
fn two_consumers_of_one_alias_keep_their_own_override() {
    let mut names = app_names(&[("Shop", Some("Atelier")), ("Clinic", Some("Visit")), ("Plain", None)]);
    names.sort();
    assert_eq!(
        names,
        vec![
            ("Clinic".to_string(), "Visit".to_string()),
            ("Plain".to_string(), "Desk".to_string()),
            ("Shop".to_string(), "Atelier".to_string()),
        ]
    );
}
