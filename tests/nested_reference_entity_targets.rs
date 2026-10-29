//! A building block that itself imports an orbital with `entities { Up: Own }`
//! (std-agent-assistant's records retargeted onto the block's owner-scoped
//! entities) must resolve those targets against the BLOCK's entities, not the
//! app importing one of the block's other orbitals. Otherwise every app that
//! imports any orbital of an assistant-carrying block fails with
//! ORB_O_ENTITY_TARGET_UNKNOWN on the block's own assistant orbital.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;

/// The generic atom: an orbital whose trait binds an entity owned by a sibling orbital.
const ATOM: &str = r#"{
  "name": "atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "RecordsOrbital",
      "entity": { "name": "Rec", "persistence": "persistent", "collection": "recs",
        "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [],
      "pages": []
    },
    {
      "name": "UserOrbital",
      "entity": { "name": "UserState", "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [{
        "name": "RecLog",
        "linkedEntity": "Rec",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
            "effects": [ ["fetch", "Rec", {}] ] } ]
        }
      }],
      "pages": [ { "name": "UserPage", "path": "/user", "traits": [ { "ref": "RecLog" } ] } ]
    },
    {
      "name": "StoreOrbital",
      "entity": { "name": "StoreState", "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "auxiliaryEntities": [ { "name": "Ledger", "persistence": "persistent", "collection": "ledgers",
        "fields": [ { "name": "id", "type": "string", "required": true } ] } ],
      "traits": [{
        "name": "LedgerLog",
        "linkedEntity": "Ledger",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
            "effects": [ ["fetch", "Ledger", {}] ] } ]
        }
      }],
      "pages": [ { "name": "StorePage", "path": "/store", "traits": [ { "ref": "LedgerLog" } ] } ]
    }
  ]
}"#;

/// The block: its own records entity, and an import of the atom retargeted onto it.
fn block(target: &str) -> String {
    format!(
        r#"{{
  "name": "block",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "ThingOrbital",
      "entity": {{ "name": "Thing", "persistence": "persistent", "collection": "things",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "auxiliaryEntities": [ {{ "name": "BlockRec", "id": "ent_BLOCKRECID0000000000000000", "persistence": "persistent", "collection": "blockRecs",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }} ],
      "traits": [],
      "pages": [ {{ "name": "ThingsPage", "path": "/things", "traits": [] }} ]
    }},
    {{
      "name": "Sib",
      "uses": [ {{ "from": "./atom.orb", "as": "At" }} ],
      "entity": "At.orbitals.StoreOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "At.orbitals.StoreOrbital" }}
    }},
    {{
      "name": "HelperOrbital",
      "uses": [ {{ "from": "./atom.orb", "as": "At" }} ],
      "entity": "At.orbitals.UserOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "At.orbitals.UserOrbital", "entities": {{ "Rec": "{target}" }} }}
    }}
  ]
}}"#
    )
}

const APP: &str = r#"{
  "name": "app",
  "version": "1.0.0",
  "orbitals": [{
    "name": "Things",
    "uses": [ { "from": "./block.orb", "as": "Blk" } ],
    "entity": "Blk.orbitals.ThingOrbital.entity",
    "traits": [],
    "reference": { "ref": "Blk.orbitals.ThingOrbital" }
  }]
}"#;

fn entity_target_unknown(target: &str) -> Vec<(String, String)> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    std::fs::write(dir.path().join("block.orb"), block(target)).expect("write block");
    let path = dir.path().join("app.orb");
    std::fs::write(&path, APP).expect("write app");
    let mut schema = parse_file(&path).expect("app parses");
    let (mut orbital_ref, mut uses_cfg) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&path),
        &mut orbital_ref,
        &mut uses_cfg,
    )
    .expect("inline succeeds");
    orbital_ref
        .into_iter()
        .flat_map(|d| d.entity_target_unknown)
        .collect()
}

#[test]
fn a_block_import_target_resolves_against_the_block_not_the_app() {
    assert_eq!(
        entity_target_unknown("BlockRec"),
        Vec::<(String, String)>::new()
    );
}

#[test]
fn control_a_target_the_block_does_not_declare_is_still_reported() {
    assert_eq!(
        entity_target_unknown("Nowhere"),
        vec![("Rec".to_string(), "Nowhere".to_string())]
    );
}

#[test]
fn a_block_import_target_a_sibling_import_materializes_resolves() {
    assert_eq!(
        entity_target_unknown("SibLedger"),
        Vec::<(String, String)>::new(),
        "an auxiliary entity an earlier import of the block materializes is a target its later imports can name"
    );
}

/// The app re-imports the block's orbitals under their own names; the helper's retarget names the block's
/// `Thing`, the primary of a sibling orbital the app imports by its own name too.
const APP_SAME_NAMES: &str = r#"{
  "name": "app",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ThingOrbital",
      "uses": [ { "from": "./block.orb", "as": "Blk" } ],
      "entity": "Blk.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": { "ref": "Blk.orbitals.ThingOrbital" }
    },
    {
      "name": "HelperOrbital",
      "uses": [ { "from": "./block.orb", "as": "Blk" } ],
      "entity": "Blk.orbitals.HelperOrbital.entity",
      "traits": [],
      "reference": { "ref": "Blk.orbitals.HelperOrbital" }
    }
  ]
}"#;

fn entity_unmapped_same_names(target: &str, mapped: bool) -> Vec<(String, String)> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    std::fs::write(dir.path().join("block.orb"), block(target)).expect("write block");
    let path = dir.path().join("app.orb");
    let app = if mapped {
        APP_SAME_NAMES.replace(
            r#""reference": { "ref": "Blk.orbitals.HelperOrbital" }"#,
            r#""reference": { "ref": "Blk.orbitals.HelperOrbital", "entities": { "Thing": "Thing" } }"#,
        )
    } else {
        APP_SAME_NAMES.to_string()
    };
    std::fs::write(&path, app).expect("write app");
    let mut schema = parse_file(&path).expect("app parses");
    let (mut orbital_ref, mut uses_cfg) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&path),
        &mut orbital_ref,
        &mut uses_cfg,
    )
    .expect("inline succeeds");
    orbital_ref.into_iter().flat_map(|d| d.entity_unmapped).collect()
}

/// Owner ruling 2026-10-24: a sibling's entity is shared only through a written `entities {}` mapping, never by name.
#[test]
fn a_same_name_reimport_shares_a_sibling_entity_only_through_its_mapping() {
    let unmapped = entity_unmapped_same_names("Thing", false);
    assert!(
        unmapped.iter().any(|(_, e)| e == "Thing"),
        "an unmapped sibling entity is diagnosed even when the app re-imports its orbital by name: {unmapped:?}"
    );
    assert_eq!(
        entity_unmapped_same_names("Thing", true),
        Vec::<(String, String)>::new(),
        "control: the written mapping shares it"
    );
}
