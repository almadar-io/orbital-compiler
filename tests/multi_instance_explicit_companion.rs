//! Gap 21 regression: one orbital rebinds the SAME source atom (sim +
//! embedded transport companion) onto MULTIPLE entities. An explicit
//! Reference to the companion trait, rebound to the same entity as its sim,
//! must satisfy the sim's `@trait.<Companion>` pull REGARDLESS of the local
//! name the consumer chose — no duplicate companion is materialised, and the
//! sim's embed token is rewritten to the explicit instance's final name.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::TraitReference;

const ATOM: &str = r#"{
  "name": "test-trials-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TrialsOrbital",
      "entity": {
        "name": "TrialsScene",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "running", "type": "boolean" }
        ]
      },
      "traits": [
        {
          "name": "TrialsTransport",
          "linkedEntity": "TrialsScene",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "STEP" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "STEP",
                "effects": [
                  ["set", "@entity.running", true],
                  ["render-ui", "strip", { "type": "stack" }]
                ]
              }
            ]
          },
          "emits": [ { "event": "LEARN_STEP" } ]
        },
        {
          "name": "TrialsSim",
          "linkedEntity": "TrialsScene",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LEARN_STEP" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "LEARN_STEP",
                "effects": [
                  ["render-ui", "main", {
                    "type": "stack",
                    "children": [ "@trait.TrialsTransport" ]
                  }]
                ]
              }
            ]
          },
          "listens": [ { "event": "LEARN_STEP", "triggers": "LEARN_STEP" } ]
        }
      ],
      "pages": [
        { "name": "TrialsPage", "path": "/trials", "linkedEntity": "TrialsScene", "traits": [ { "ref": "TrialsTransport" }, { "ref": "TrialsSim" } ] }
      ]
    }
  ]
}"#;

const CONSUMER: &str = r#"{
  "name": "test-trials-lab",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "LabOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Trials" } ],
      "entity": {
        "name": "DiceScene",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "running", "type": "boolean" }
        ]
      },
      "entities": [
        {
          "name": "CoinsScene",
          "persistence": "runtime",
          "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "running", "type": "boolean" }
          ]
        }
      ],
      "traits": [
        { "ref": "Trials.traits.TrialsTransport", "linkedEntity": "DiceScene", "name": "DiceTransport" },
        { "ref": "Trials.traits.TrialsSim", "linkedEntity": "DiceScene", "name": "DiceSim" },
        { "ref": "Trials.traits.TrialsTransport", "linkedEntity": "CoinsScene", "name": "CoinsTransport" },
        { "ref": "Trials.traits.TrialsSim", "linkedEntity": "CoinsScene", "name": "CoinsSim" }
      ],
      "pages": [
        { "name": "LabPage", "path": "/lab", "linkedEntity": "DiceScene", "traits": [ { "ref": "DiceTransport" }, { "ref": "DiceSim" }, { "ref": "CoinsTransport" }, { "ref": "CoinsSim" } ] }
      ]
    }
  ]
}"#;

fn embed_tokens(td: &orbital_core_base::schema::TraitDefinition) -> Vec<String> {
    let json = serde_json::to_string(td).expect("trait serialises");
    let mut tokens = Vec::new();
    let mut rest = json.as_str();
    while let Some(pos) = rest.find("@trait.") {
        let tail = &rest[pos + "@trait.".len()..];
        let end = tail
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(tail.len());
        tokens.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    tokens
}

#[test]
fn renamed_explicit_companion_suppresses_pull_per_entity() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds — renamed companions must not collide");

    let traits: Vec<&orbital_core_base::schema::TraitDefinition> = schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(&**td),
            _ => None,
        })
        .collect();
    let names: Vec<&str> = traits.iter().map(|t| t.name.as_str()).collect();

    // Exactly the four explicit instances — no pulled duplicate companion
    // (neither bare "TrialsTransport" nor an owner-prefixed copy).
    assert_eq!(
        traits.len(),
        4,
        "explicit companions must suppress the sibling pull; got {names:?}"
    );
    for expected in ["DiceTransport", "DiceSim", "CoinsTransport", "CoinsSim"] {
        assert!(
            names.contains(&expected),
            "missing {expected}; got {names:?}"
        );
    }
    assert!(
        !names.iter().any(|n| n.contains("TrialsTransport")),
        "no pulled companion may be materialised; got {names:?}"
    );

    // Each sim's embed token must point at ITS entity's explicit transport.
    for (sim, want) in [("DiceSim", "DiceTransport"), ("CoinsSim", "CoinsTransport")] {
        let td = traits
            .iter()
            .find(|t| t.name == sim)
            .unwrap_or_else(|| panic!("{sim} present"));
        let tokens = embed_tokens(td);
        assert_eq!(
            tokens,
            vec![want.to_string()],
            "{sim} must embed {want}; got {tokens:?}"
        );
    }
}

fn same_entity_consumer(transport_rebind: &str, sim_rebind: &str) -> String {
    format!(
        r#"{{
  "name": "test-trials-same-entity",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "LabOrbital",
      "uses": [ {{ "from": "./atom.orb", "as": "Trials" }} ],
      "entity": {{
        "name": "TrialsScene",
        "persistence": "runtime",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "running", "type": "boolean" }}
        ]
      }},
      "traits": [
        {{ "ref": "Trials.traits.TrialsTransport"{transport_rebind}, "name": "MyTransport" }},
        {{ "ref": "Trials.traits.TrialsSim"{sim_rebind}, "name": "MySim" }}
      ],
      "pages": [
        {{ "name": "LabPage", "path": "/lab", "linkedEntity": "TrialsScene", "traits": [ {{ "ref": "MyTransport" }}, {{ "ref": "MySim" }} ] }}
      ]
    }}
  ]
}}"#
    )
}

fn inline_same_entity(transport_rebind: &str, sim_rebind: &str) -> Vec<(String, Vec<String>)> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, same_entity_consumer(transport_rebind, sim_rebind))
        .expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("inline succeeds");
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some((td.name.clone(), embed_tokens(td))),
            _ => None,
        })
        .collect()
}

const REBIND_NATIVE: &str = r#", "linkedEntity": "TrialsScene""#;

#[test]
fn a_rebind_to_the_entity_the_source_already_binds_is_the_same_instance_as_no_rebind() {
    let traits = inline_same_entity(REBIND_NATIVE, "");
    let names: Vec<&str> = traits.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(traits.len(), 2, "one instance per source trait, got {names:?}");
    let sim = traits.iter().find(|(n, _)| n == "MySim").expect("MySim");
    assert_eq!(sim.1, vec!["MyTransport".to_string()]);
}

#[test]
fn a_no_rebind_explicit_instance_satisfies_a_pull_made_with_the_native_rebind() {
    let traits = inline_same_entity("", REBIND_NATIVE);
    let names: Vec<&str> = traits.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(traits.len(), 2, "one instance per source trait, got {names:?}");
    let sim = traits.iter().find(|(n, _)| n == "MySim").expect("MySim");
    assert_eq!(sim.1, vec!["MyTransport".to_string()]);
}

#[test]
fn control_neither_rebound_still_one_instance_per_source_trait() {
    let traits = inline_same_entity("", "");
    assert_eq!(traits.len(), 2, "{traits:?}");
}
