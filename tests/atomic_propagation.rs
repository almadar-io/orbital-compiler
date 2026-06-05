//! Phase 3.5.G.D.2 acid test: atomic propagation.
//!
//! The single definitive proof that editing an atom's `.orb` propagates to
//! a molecule's lowered output without touching any TS source. This is the
//! end-to-end claim Phase F has been chasing: atoms are the source of truth
//! for state-machine topology + content; molecules carry override arguments
//! at the reference site; the compiler walks the resolved trait clone and
//! substitutes; the lowered OIR reflects the atom's current state.
//!
//! How the test works:
//!
//! 1. Create a temporary directory with an atom `.orb` (`atom-v1`) and a
//!    molecule `.orb` that imports the atom via a relative `uses[]` path.
//!    Both files live in the same temp dir so the molecule's relative
//!    import resolves cleanly to the atom on disk. The relative path
//!    deliberately AVOIDS the `std/behaviors/` prefix so the loader's
//!    embedded-registry shortcut does NOT fire — the test exercises the
//!    on-disk filesystem path the same way a future external std lib
//!    would.
//!
//! 2. Compile the molecule. Capture the OIR trait's effect count and the
//!    text of the render-ui content for a chosen transition.
//!
//! 3. OVERWRITE the atom file in place with `atom-v2`, which adds a new
//!    state to the state machine and changes the render-ui content of the
//!    chosen transition. Crucially, no TS file is touched.
//!
//! 4. Re-compile the molecule. Capture the OIR trait again.
//!
//! 5. Assert the v1 and v2 captures DIFFER — the new state appears in the
//!    lowered states list, and the render-ui content text reflects the v2
//!    edit. This is the acid test passing.
//!
//! 6. As a sanity check, REVERT the atom to v1 and re-compile a third
//!    time. Assert the trait matches the v1 capture. This rules out
//!    "compile_file is non-deterministic" as a confounder.
//!
//! Notes:
//!  - `compile_file` constructs a fresh `ExternalOrbitalLoader` per call,
//!    so there is no cross-call cache that could mask the change.
//!  - The test uses `tempfile::TempDir` so the temp directory + its files
//!    are cleaned up after the test exits, even on failure.

use orbital_compiler::{compile_file, CompilerOptions};
use orbital_core::ir::OirModule;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const ATOM_V1: &str = r#"{
  "name": "test-atom-v1",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TestAtomOrbital",
      "entity": {
        "name": "TestRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "name", "type": "string" }
        ]
      },
      "traits": [
        {
          "name": "TestAtomTrait",
          "linkedEntity": "TestRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [
              { "name": "idle", "isInitial": true }
            ],
            "events": [
              { "key": "INIT", "name": "Initialize" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["ref", "TestRecord"],
                  ["render-ui", "main", {
                    "type": "typography",
                    "content": "ATOM_V1_CONTENT",
                    "variant": "h2"
                  }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}
"#;

const ATOM_V2: &str = r#"{
  "name": "test-atom-v2",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TestAtomOrbital",
      "entity": {
        "name": "TestRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "name", "type": "string" }
        ]
      },
      "traits": [
        {
          "name": "TestAtomTrait",
          "linkedEntity": "TestRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [
              { "name": "idle", "isInitial": true },
              { "name": "alerted" }
            ],
            "events": [
              { "key": "INIT", "name": "Initialize" },
              { "key": "ALERT", "name": "Alert" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["ref", "TestRecord"],
                  ["render-ui", "main", {
                    "type": "typography",
                    "content": "ATOM_V2_CONTENT_REWRITTEN",
                    "variant": "h2"
                  }]
                ]
              },
              {
                "from": "idle",
                "to": "alerted",
                "event": "ALERT",
                "effects": [
                  ["render-ui", "main", { "type": "alert", "variant": "danger", "message": "alerted" }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}
"#;

const MOLECULE_FIXTURE: &str = r#"{
  "name": "test-molecule",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TestMoleculeOrbital",
      "uses": [
        { "from": "./test-atom", "as": "Atom" }
      ],
      "entity": {
        "name": "ConsumerRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "name", "type": "string" }
        ]
      },
      "traits": [
        {
          "ref": "Atom.traits.TestAtomTrait",
          "name": "ConsumerInstance",
          "linkedEntity": "ConsumerRecord"
        }
      ]
    }
  ]
}
"#;

fn lowered_oir_options() -> CompilerOptions {
    CompilerOptions {
        skip_validation: true,
        comprehensive_validation: false,
        ..Default::default()
    }
}

/// Snapshot of the fields the acid test cares about. Captured AFTER
/// compilation so we can compare across atom edits without keeping the
/// full OIR around.
///
/// `init_effects_value` is a `serde_json::Value` rather than a `String` so
/// equality is structural. Some intermediate types in the OIR back their
/// children with `HashMap`, which serialize to JSON with non-deterministic
/// key order — meaning two semantically-identical compilations can produce
/// different `to_string` outputs. Comparing as `Value` ignores that.
#[derive(Debug, Clone, PartialEq)]
struct TraitSnapshot {
    state_count: usize,
    state_names: Vec<String>,
    init_effects_value: serde_json::Value,
}

impl TraitSnapshot {
    /// Concatenated text of every string leaf inside the INIT effects tree.
    /// Used to assert that specific content text (like "ATOM_V1_CONTENT")
    /// is present without depending on the exact JSON key order.
    fn init_text_blob(&self) -> String {
        let mut buf = String::new();
        collect_strings(&self.init_effects_value, &mut buf);
        buf
    }
}

fn collect_strings(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, val) in map.iter() {
                collect_strings(val, out);
            }
        }
        _ => {}
    }
}

fn snapshot_consumer_trait(oir: &OirModule) -> TraitSnapshot {
    let trait_def = oir
        .find_trait("ConsumerInstance")
        .expect("lowered OIR must contain the renamed ConsumerInstance trait");

    let init_transition = trait_def
        .transitions
        .iter()
        .find(|t| t.event == "INIT")
        .expect("must find an INIT transition");
    let init_effects_value = serde_json::to_value(&init_transition.effects)
        .expect("INIT effects must convert to serde_json::Value");

    TraitSnapshot {
        state_count: trait_def.states.len(),
        state_names: trait_def.states.iter().map(|s| s.name.clone()).collect(),
        init_effects_value,
    }
}

fn write_files(dir: &PathBuf, atom_content: &str) -> PathBuf {
    fs::write(dir.join("test-atom.orb"), atom_content).expect("write atom");
    let molecule_path = dir.join("test-molecule.orb");
    fs::write(&molecule_path, MOLECULE_FIXTURE).expect("write molecule");
    molecule_path
}

#[test]
fn editing_atom_orb_propagates_to_molecule_lowered_output() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path().to_path_buf();

    // ---- Step 1: write atom v1 + molecule, compile, snapshot ----
    let molecule_path = write_files(&dir, ATOM_V1);
    let oir_v1 = compile_file(&molecule_path, &lowered_oir_options())
        .expect("v1 compile must succeed");
    let snap_v1 = snapshot_consumer_trait(&oir_v1);

    // Sanity: the v1 atom has exactly 1 state and the v1 content text.
    assert_eq!(snap_v1.state_count, 1, "v1 atom must have 1 state");
    assert_eq!(snap_v1.state_names, vec!["idle".to_string()]);
    assert!(
        snap_v1.init_text_blob().contains("ATOM_V1_CONTENT"),
        "v1 INIT effects must carry the v1 content text; got {}",
        snap_v1.init_text_blob()
    );

    // ---- Step 2: overwrite atom with v2, recompile, snapshot ----
    fs::write(dir.join("test-atom.orb"), ATOM_V2).expect("overwrite atom with v2");
    let oir_v2 = compile_file(&molecule_path, &lowered_oir_options())
        .expect("v2 compile must succeed");
    let snap_v2 = snapshot_consumer_trait(&oir_v2);

    // ---- Step 3: assert the change propagated ----
    assert_ne!(
        snap_v1, snap_v2,
        "molecule's lowered OIR must reflect the atom edit"
    );
    assert_eq!(
        snap_v2.state_count, 2,
        "v2 atom adds an `alerted` state; the molecule must inherit it"
    );
    assert!(
        snap_v2.state_names.contains(&"alerted".to_string()),
        "lowered states must include the v2-added state; got {:?}",
        snap_v2.state_names
    );
    assert!(
        snap_v2.init_text_blob().contains("ATOM_V2_CONTENT_REWRITTEN"),
        "v2 INIT effects must carry the v2 content text; got {}",
        snap_v2.init_text_blob()
    );
    assert!(
        !snap_v2.init_text_blob().contains("ATOM_V1_CONTENT"),
        "v2 INIT effects must NOT carry the v1 content text"
    );

    // ---- Step 4: revert to v1, recompile, snapshot, assert determinism ----
    // Guards against a non-deterministic compile_file (which would make
    // the v1→v2 difference unreliable evidence).
    fs::write(dir.join("test-atom.orb"), ATOM_V1).expect("revert atom to v1");
    let oir_v1_again = compile_file(&molecule_path, &lowered_oir_options())
        .expect("revert compile must succeed");
    let snap_v1_again = snapshot_consumer_trait(&oir_v1_again);
    assert_eq!(
        snap_v1, snap_v1_again,
        "reverting the atom must produce the same lowered output as v1"
    );
}

// ============================================================================
// Recursive composition: organism → molecule → atom
// ============================================================================
//
// The Phase F.10 refactor of std-list proves the molecule tier participates
// in the same composition pipeline atoms do. The test below validates that
// claim end-to-end at the compiler level: a 3-tier composition where an
// organism imports a molecule that imports an atom. Editing the ATOM must
// propagate through the molecule and into the organism's lowered output —
// without touching the molecule's source.
//
// This is the strongest possible statement of atomic propagation: the
// rewrite walker traverses arbitrarily deep composition chains, and the
// loader resolves uses[] declarations in molecules just as readily as in
// organisms. The compiler does not distinguish tiers.

const RECURSIVE_ATOM_V1: &str = r#"{
  "name": "recursive-atom-v1",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "RecursiveAtomOrbital",
      "entity": {
        "name": "AtomRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "name": "AtomTrait",
          "linkedEntity": "AtomRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT", "name": "Initialize" }],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "typography", "content": "ATOM_V1_DEEP" }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}
"#;

const RECURSIVE_ATOM_V2: &str = r#"{
  "name": "recursive-atom-v2",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "RecursiveAtomOrbital",
      "entity": {
        "name": "AtomRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "name": "AtomTrait",
          "linkedEntity": "AtomRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT", "name": "Initialize" }],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["render-ui", "main", { "type": "typography", "content": "ATOM_V2_DEEP_REWRITTEN" }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}
"#;

const RECURSIVE_MOLECULE: &str = r#"{
  "name": "recursive-molecule",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MoleculeOrbital",
      "uses": [
        { "from": "./recursive-atom", "as": "Atom" }
      ],
      "entity": {
        "name": "MoleculeRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "ref": "Atom.traits.AtomTrait",
          "name": "MoleculeIntermediate",
          "linkedEntity": "MoleculeRecord"
        }
      ]
    }
  ]
}
"#;

const RECURSIVE_ORGANISM: &str = r#"{
  "name": "recursive-organism",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "OrganismOrbital",
      "uses": [
        { "from": "./recursive-molecule", "as": "Molecule" }
      ],
      "entity": {
        "name": "OrganismRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "ref": "Molecule.traits.MoleculeIntermediate",
          "name": "OrganismFinal",
          "linkedEntity": "OrganismRecord"
        }
      ]
    }
  ]
}
"#;

fn snapshot_organism_trait(oir: &OirModule) -> TraitSnapshot {
    let trait_def = oir
        .find_trait("OrganismFinal")
        .expect("lowered OIR must contain the renamed OrganismFinal trait");
    let init_transition = trait_def
        .transitions
        .iter()
        .find(|t| t.event == "INIT")
        .expect("must find an INIT transition");
    let init_effects_value = serde_json::to_value(&init_transition.effects)
        .expect("INIT effects must convert to serde_json::Value");
    TraitSnapshot {
        state_count: trait_def.states.len(),
        state_names: trait_def.states.iter().map(|s| s.name.clone()).collect(),
        init_effects_value,
    }
}

#[test]
fn editing_atom_orb_propagates_through_molecule_into_organism() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path().to_path_buf();

    fs::write(dir.join("recursive-atom.orb"), RECURSIVE_ATOM_V1).expect("write atom v1");
    fs::write(dir.join("recursive-molecule.orb"), RECURSIVE_MOLECULE).expect("write molecule");
    let organism_path = dir.join("recursive-organism.orb");
    fs::write(&organism_path, RECURSIVE_ORGANISM).expect("write organism");

    // ---- v1 compile ----
    let oir_v1 = compile_file(&organism_path, &lowered_oir_options())
        .expect("v1 organism compile must succeed");
    let snap_v1 = snapshot_organism_trait(&oir_v1);

    assert!(
        snap_v1.init_effects_value.to_string().contains("ATOM_V1_DEEP"),
        "organism's lowered OIR must carry the v1 atom content (deep propagation); got {}",
        snap_v1.init_effects_value
    );

    // The organism rebound the linkedEntity to OrganismRecord. Both the
    // molecule's intermediate rebind (MoleculeRecord) and the organism's
    // rebind chain through the substitution walker. The final lowered
    // trait should be linked to OrganismRecord, NOT AtomRecord and NOT
    // MoleculeRecord.
    let trait_def = oir_v1
        .find_trait("OrganismFinal")
        .expect("lowered OIR must contain OrganismFinal");
    assert_eq!(
        trait_def.linked_entity,
        Some("OrganismRecord".to_string()),
        "linkedEntity rebind must propagate through 2 layers of composition"
    );

    // ---- v2 compile ----
    fs::write(dir.join("recursive-atom.orb"), RECURSIVE_ATOM_V2)
        .expect("overwrite atom with v2");
    let oir_v2 = compile_file(&organism_path, &lowered_oir_options())
        .expect("v2 organism compile must succeed");
    let snap_v2 = snapshot_organism_trait(&oir_v2);

    assert_ne!(snap_v1, snap_v2, "atom edit must propagate through both layers");
    assert!(
        snap_v2
            .init_effects_value
            .to_string()
            .contains("ATOM_V2_DEEP_REWRITTEN"),
        "organism's lowered OIR must carry the v2 atom content; got {}",
        snap_v2.init_effects_value
    );
    assert!(
        !snap_v2.init_effects_value.to_string().contains("ATOM_V1_DEEP"),
        "v2 organism must NOT carry the v1 content"
    );

    // ---- revert sanity ----
    fs::write(dir.join("recursive-atom.orb"), RECURSIVE_ATOM_V1).expect("revert atom");
    let oir_v1_again = compile_file(&organism_path, &lowered_oir_options())
        .expect("revert compile must succeed");
    let snap_v1_again = snapshot_organism_trait(&oir_v1_again);
    assert_eq!(
        snap_v1, snap_v1_again,
        "reverting the atom must produce the same organism lowered output as v1"
    );
}

// ============================================================================
// Entity-valued pattern prop rewrite
// ============================================================================
//
// Regression for the `rewrite_identifiers` Object-arm gap: render-ui pattern
// configs address the entity store by name via props like `entity:` on
// `data-grid` / `data-list`. When a consumer rebinds `linkedEntity`, the
// generic recursive walk used to drop the atom-era string literal ("From")
// verbatim, so the compiled component read from the wrong store key and the
// UI rendered "No items" even though the store was populated.
//
// This test snapshots an atom that hard-codes `entity: "AtomEntity"` inside
// its render-ui, has a consumer rebind `linkedEntity` to `ConsumerEntity`,
// and asserts the rewritten effects tree contains only the consumer entity
// name — never the atom's.

const ENTITY_PROP_ATOM: &str = r#"{
  "name": "entity-prop-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "EntityPropAtomOrbital",
      "entity": {
        "name": "AtomEntity",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "title", "type": "string" }
        ]
      },
      "traits": [
        {
          "name": "EntityPropAtomTrait",
          "linkedEntity": "AtomEntity",
          "category": "interaction",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT", "name": "Initialize" }],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [
                  ["ref", "AtomEntity"],
                  ["render-ui", "main", {
                    "type": "data-grid",
                    "entity": "AtomEntity",
                    "source": "AtomEntity",
                    "entityType": "AtomEntity"
                  }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}
"#;

const ENTITY_PROP_MOLECULE: &str = r#"{
  "name": "entity-prop-molecule",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "EntityPropMoleculeOrbital",
      "uses": [
        { "from": "./entity-prop-atom", "as": "Atom" }
      ],
      "entity": {
        "name": "ConsumerEntity",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "title", "type": "string" }
        ]
      },
      "traits": [
        {
          "ref": "Atom.traits.EntityPropAtomTrait",
          "name": "ConsumerEntityPropTrait",
          "linkedEntity": "ConsumerEntity"
        }
      ]
    }
  ]
}
"#;

#[test]
fn linked_entity_rebind_rewrites_pattern_entity_props() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path().to_path_buf();

    fs::write(dir.join("entity-prop-atom.orb"), ENTITY_PROP_ATOM).expect("write atom");
    let molecule_path = dir.join("entity-prop-molecule.orb");
    fs::write(&molecule_path, ENTITY_PROP_MOLECULE).expect("write molecule");

    let oir = compile_file(&molecule_path, &lowered_oir_options())
        .expect("molecule compile must succeed");

    let trait_def = oir
        .find_trait("ConsumerEntityPropTrait")
        .expect("lowered OIR must contain the renamed ConsumerEntityPropTrait");
    let init_transition = trait_def
        .transitions
        .iter()
        .find(|t| t.event == "INIT")
        .expect("must find an INIT transition");
    let effects_json = serde_json::to_string(&init_transition.effects)
        .expect("effects must serialize");

    // Positive: the consumer entity name lands in the pattern props.
    assert!(
        effects_json.contains("ConsumerEntity"),
        "pattern entity prop must be rewritten to the consumer's entity; got {}",
        effects_json
    );
    // Negative: no occurrence of the atom-era entity name survives anywhere
    // in the effects tree — `entity:`, `source:`, `entityType:`, or the
    // `["ref", X]` form. This is the whole point of the fix.
    assert!(
        !effects_json.contains("AtomEntity"),
        "no occurrence of the atom-era entity name may survive the rebind; got {}",
        effects_json
    );
}
