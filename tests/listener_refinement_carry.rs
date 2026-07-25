//! The listen refinement surface (`when` guard + `with{}` payload mapping)
//! must survive schema → OIR resolution. Both were silently dropped by
//! `resolve.rs`'s listener construction — `OirListener.guard` existed but was
//! never populated, `payload_mapping` did not exist — so every compiled shell
//! lost what the interpreter honored (C-OIR-LISTENER-DROPS-WITH-AND-GUARD).

use orbital_compiler::phases::resolve_to_oir;
use orbital_core::ir::OirExpr;
use orbital_core::schema::OrbitalSchema;

fn schema_with_refined_listen() -> OrbitalSchema {
    let json = r#"{
      "name": "carry-probe",
      "orbitals": [
        {
          "name": "ProbeOrbital",
          "entity": {
            "name": "Probe",
            "persistence": "runtime",
            "fields": [{ "name": "id", "type": "string", "required": true }]
          },
          "traits": [
            {
              "name": "ProbeRun",
              "linkedEntity": "Probe",
              "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "LOOKUP" }]
              },
              "listens": [
                {
                  "event": "EXACT_UNMATCHED",
                  "triggers": "LOOKUP",
                  "source": { "kind": "trait", "trait": "ExactRung" },
                  "guard": [">", "@payload.score", 0],
                  "payloadMapping": {
                    "candidate": "@payload.candidate",
                    "accepted": ["object/get", "@payload.request", "accepted"]
                  }
                }
              ]
            }
          ],
          "pages": []
        }
      ]
    }"#;
    serde_json::from_str(json).expect("probe schema parses")
}

#[test]
fn resolve_carries_listen_guard_and_payload_mapping_into_oir() {
    let module = resolve_to_oir(&schema_with_refined_listen()).expect("resolves");
    let tr = module
        .traits
        .iter()
        .find(|t| t.name == "ProbeRun")
        .expect("ProbeRun resolved");
    let listener = tr.listeners.first().expect("listener resolved");

    assert_eq!(listener.event_pattern, "EXACT_UNMATCHED");
    assert_eq!(listener.triggers, "LOOKUP");
    assert!(listener.guard.is_some(), "when-guard must be carried");

    let mapping = listener
        .payload_mapping
        .as_ref()
        .expect("with{} mapping must be carried");
    assert_eq!(mapping.len(), 2);
    assert!(
        matches!(mapping.get("candidate"), Some(OirExpr::Binding { .. })),
        "@payload.candidate must resolve to a binding, got {:?}",
        mapping.get("candidate")
    );
    assert!(
        matches!(mapping.get("accepted"), Some(OirExpr::Call { function, .. }) if function == "object/get"),
        "expression values must resolve as calls, got {:?}",
        mapping.get("accepted")
    );
}
