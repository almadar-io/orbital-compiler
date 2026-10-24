//! One app importing the same upstream orbital more than once: each import's
//! config may address its OWN traits by its local name (`First.Workflow.PROPOSE`).
//! Every import's address must land on that import's materialized trait — not
//! only the last import's.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;

const UPSTREAM: &str = r#"{
  "name": "proposal-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ProposalOrbital",
      "config": {
        "tools": {
          "type": "[AgentTool]",
          "items": { "type": "union", "values": ["AgentToolInput", "AgentToolRead"], "properties": {
            "AgentToolInput": { "name": "AgentToolInput", "type": "object", "properties": {
              "event": { "name": "event", "type": "EventAddress", "required": true } } },
            "AgentToolRead": { "name": "AgentToolRead", "type": "object", "properties": {
              "read": { "name": "read", "type": "string", "required": true } } } } },
          "default": [ { "event": "Workflow.PROPOSE" } ]
        }
      },
      "entity": { "name": "Run", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "Workflow",
          "linkedEntity": "Run",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" }, { "key": "PROPOSE", "input": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT" }, { "from": "idle", "to": "idle", "event": "PROPOSE" } ]
          }
        },
        {
          "name": "Loop",
          "linkedEntity": "Run",
          "category": "interaction",
          "config": { "tools": { "type": "[AgentTool]", "default": "@config.tools" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["call-service", "test-svc", "run", { "tools": "@config.tools" }] ] } ]
          }
        }
      ],
      "pages": [ { "name": "ProposalPage", "path": "/proposal", "traits": [ { "ref": "Workflow" }, { "ref": "Loop" } ] } ]
    }
  ]
}"#;

fn import(local: &str) -> String {
    format!(
        r#"{{
      "name": "{local}",
      "uses": [ {{ "from": "./upstream.orb", "as": "Proposal" }} ],
      "entity": "Proposal.orbitals.ProposalOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Proposal.orbitals.ProposalOrbital",
        "pages": {{ "/proposal": "/{path}" }},
        "config": {{ "tools": {{ "type": "unknown", "default": [ {{ "read": "Run" }}, {{ "event": "{local}.Workflow.PROPOSE" }} ] }} }}
      }}
    }}"#,
        path = local.to_lowercase()
    )
}

fn inlined(locals: &[&str]) -> String {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer = format!(
        r#"{{ "name": "consumer", "version": "1.0.0", "orbitals": [ {} ] }}"#,
        locals
            .iter()
            .map(|l| import(l))
            .collect::<Vec<_>>()
            .join(",")
    );
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    serde_json::to_string(&schema).expect("serializes")
}

#[test]
fn every_import_of_one_upstream_addresses_its_own_trait() {
    let out = inlined(&["First", "Second"]);
    for local in ["First", "Second"] {
        assert!(
            out.contains(&format!("{local}.{local}Workflow.PROPOSE")),
            "{local} not re-addressed"
        );
        assert!(
            !out.contains(&format!("\"{local}.Workflow.PROPOSE\"")),
            "{local} left unaddressed"
        );
    }
}

#[test]
fn control_a_single_import_addresses_its_own_trait() {
    let out = inlined(&["Only"]);
    assert!(out.contains("Only.OnlyWorkflow.PROPOSE"));
    assert!(!out.contains("\"Only.Workflow.PROPOSE\""));
}
