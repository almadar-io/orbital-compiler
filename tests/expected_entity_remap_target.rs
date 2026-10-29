//! An orbital import's `entities { Up: Own }` target may be an entity a SIBLING
//! orbital declares (an organism's assistant: its records live as auxiliaries on
//! the identity orbital). Validated as its own file — the Studio's per-orbital
//! rebuild — the importing orbital promises that target with `expects entity Own`
//! (the cross-orbital contract the compose link-check enforces), and the remap
//! applies under that promise instead of failing ORB_O_ENTITY_TARGET_UNKNOWN.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, TraitReference};

/// The atom: its user orbital fetches `Rec`, which a sibling orbital owns.
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
    }
  ]
}"#;

/// The importing orbital alone, as its own file: the target `OwnRec` is declared nowhere here.
fn slice(expects: bool) -> String {
    let expects = if expects {
        r#", "expects": [ { "kind": "entity", "name": "OwnRec" } ]"#
    } else {
        ""
    };
    format!(
        r#"{{
  "name": "slice",
  "version": "1.0.0",
  "orbitals": [{{
    "name": "HelperOrbital",
    "uses": [ {{ "from": "./atom.orb", "as": "At" }} ],
    "entity": "At.orbitals.UserOrbital.entity",
    "traits": [],
    "reference": {{ "ref": "At.orbitals.UserOrbital", "entities": {{ "Rec": "OwnRec" }} }}{expects}
  }}]
}}"#
    )
}

fn inline(expects: bool) -> (OrbitalSchema, Vec<(String, String)>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let path = dir.path().join("slice.orb");
    std::fs::write(&path, slice(expects)).expect("write slice");
    let mut schema = parse_file(&path).expect("slice parses");
    let (mut orbital_ref, mut uses_cfg) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&path),
        &mut orbital_ref,
        &mut uses_cfg,
    )
    .expect("inline succeeds");
    let unknown = orbital_ref
        .into_iter()
        .flat_map(|d| d.entity_target_unknown)
        .collect();
    (schema, unknown)
}

fn rec_log_linked_entity(schema: &OrbitalSchema) -> Option<String> {
    schema.orbitals[0].traits.iter().find_map(|t| match t {
        TraitReference::Inline(def) if def.name.ends_with("RecLog") => def.linked_entity.clone(),
        _ => None,
    })
}

#[test]
fn a_target_the_importing_orbital_expects_is_remapped_not_unknown() {
    let (schema, unknown) = inline(true);
    assert_eq!(unknown, Vec::<(String, String)>::new());
    assert_eq!(rec_log_linked_entity(&schema).as_deref(), Some("OwnRec"));
}

#[test]
fn control_without_the_expectation_the_target_is_still_unknown() {
    let (_, unknown) = inline(false);
    assert_eq!(unknown, vec![("Rec".to_string(), "OwnRec".to_string())]);
}
