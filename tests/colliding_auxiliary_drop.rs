//! An imported auxiliary entity never displaces a consumer's own entity.
//!
//! `uses` imports individual TRAITS, but auxiliary entities travel per ORBITAL —
//! the inliner carries the source orbital's whole closure, because a trait body
//! can fetch or persist a sibling entity by name. That granularity mismatch means
//! asking for one browse trait can hand you an unrelated subsystem: importing
//! `std-hr-portal`'s `TimeOffBrowseList` carries seven auxiliaries, among them
//! `CalendarEvent` from the `std-calendar-sync` that orbital also composes.
//!
//! When the consumer declares its own `CalendarEvent`, that is a hard
//! `DuplicateDefinition`, and unlike a trait's bound entity there is no call site
//! to rebind — no imported trait ever named it. So an unreferenced auxiliary that
//! collides with a consumer primary is dropped; a REFERENCED one still collides,
//! because two live shapes under one name is a real conflict.

use orbital_compiler::phases::{inline_schema, InlineConfig};
use orbital_core_base::schema::{EntityRef, OrbitalSchema};
use serde_json::{json, Value};

/// Two orbitals: one owns `Shared` as its primary, the other carries an imported
/// auxiliary of the same name. `referenced` decides whether a trait names it.
fn schema_with_colliding_aux(referenced: bool) -> Value {
    let effects: Value = if referenced {
        json!([["fetch", "Shared", {}]])
    } else {
        json!([["fetch", "Local", {}]])
    };
    json!({
        "name": "App",
        "orbitals": [
            {
                "name": "OwnerOrbital",
                "entity": {
                    "name": "Shared",
                    "persistence": "persistent",
                    "collection": "shared",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                "traits": [], "pages": []
            },
            {
                "name": "ConsumerOrbital",
                "entity": {
                    "name": "Local",
                    "persistence": "persistent",
                    "collection": "locals",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                // The imported closure's leftover — a DIFFERENT shape that
                // happens to share the name.
                "auxiliaryEntities": [{
                    "name": "Shared",
                    "persistence": "persistent",
                    "collection": "imported_shared",
                    "fields": [{ "name": "id", "type": "string" },
                               { "name": "importedOnly", "type": "string" }]
                }],
                "traits": [{
                    "name": "LocalView",
                    "linkedEntity": "Local",
                    "stateMachine": {
                        "events": [],
                        "states": [{ "name": "idle" }],
                        "transitions": [{ "from": "idle", "event": "INIT", "to": "idle",
                                          "effects": effects }]
                    }
                }],
                "pages": []
            }
        ]
    })
}

fn aux_names(schema: &OrbitalSchema, orbital: &str) -> Vec<String> {
    schema
        .orbitals
        .iter()
        .find(|o| o.name == orbital)
        .and_then(|o| o.auxiliary_entities.as_ref())
        .map(|aux| {
            aux.iter()
                .filter_map(|e| match e {
                    EntityRef::Inline(d) => Some(d.name.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn inline(schema: Value) -> OrbitalSchema {
    let mut s: OrbitalSchema = serde_json::from_value(schema).expect("fixture must deserialize");
    inline_schema(&mut s, &InlineConfig::default(), None).expect("inline must succeed");
    s
}

#[test]
fn an_unreferenced_colliding_auxiliary_is_dropped() {
    let s = inline(schema_with_colliding_aux(false));
    assert!(
        !aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()),
        "nothing names it and it collides with OwnerOrbital's primary — dropping strands nothing"
    );
}

#[test]
fn a_referenced_colliding_auxiliary_is_kept() {
    let s = inline(schema_with_colliding_aux(true));
    assert!(
        aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()),
        "a trait still fetches it — keep it so the real conflict is reported, not hidden"
    );
}

#[test]
fn a_non_colliding_auxiliary_is_always_kept() {
    let mut fixture = schema_with_colliding_aux(false);
    fixture["orbitals"][1]["auxiliaryEntities"][0]["name"] = json!("Unrelated");
    let s = inline(fixture);
    assert!(
        aux_names(&s, "ConsumerOrbital").contains(&"Unrelated".to_string()),
        "the rule only fires on a collision with a consumer primary"
    );
}

const OWNER_ID: &str = "ent_5PW23VF2P6HB39FGKJ0EPBBPJ4";
const AUX_ID: &str = "ent_QWXD45Q8JGM987ZTT21E124DEN";

fn ledger_row(id: &str) -> Value {
    json!({ "id": id, "kind": "entity", "bakedName": "Shared", "curName": "Shared", "renames": [], "owner": "workspace" })
}

/// `schema_with_colliding_aux` with ids on both `Shared` declarations, their ledger rows, and a third orbital whose
/// trait carries an entity-typed config value naming `Shared` by the AUXILIARY's id.
fn schema_with_id_edge_to_aux() -> Value {
    let mut fixture = schema_with_colliding_aux(false);
    fixture["orbitals"][0]["entity"]["id"] = json!(OWNER_ID);
    fixture["orbitals"][1]["auxiliaryEntities"][0]["id"] = json!(AUX_ID);
    fixture["ledger"] = json!({
        "schemaVersion": 1,
        "entries": { OWNER_ID: ledger_row(OWNER_ID), AUX_ID: ledger_row(AUX_ID) }
    });
    fixture["orbitals"].as_array_mut().unwrap().push(json!({
        "name": "ViewOrbital",
        "entity": {
            "name": "ViewRow",
            "persistence": "persistent",
            "collection": "view_rows",
            "fields": [{ "name": "id", "type": "string" }]
        },
        "traits": [{
            "name": "ViewList",
            "linkedEntity": "ViewRow",
            "config": { "relatedEntity": { "type": "entity", "default": "Shared", "refId": AUX_ID } },
            "stateMachine": {
                "events": [],
                "states": [{ "name": "idle" }],
                "transitions": [{ "from": "idle", "event": "INIT", "to": "idle", "effects": [] }]
            }
        }],
        "pages": []
    }));
    fixture
}

fn view_ref_id(schema: &OrbitalSchema) -> Option<String> {
    use orbital_core_base::schema::TraitReference;
    let orbital = schema.orbitals.iter().find(|o| o.name == "ViewOrbital")?;
    orbital.traits.iter().find_map(|t| match t {
        TraitReference::Inline(td) => td.config.as_ref()?.get("relatedEntity")?.ref_id.clone(),
        TraitReference::Reference { config, .. } => config.as_ref()?.get("relatedEntity")?.ref_id.clone(),
    })
}

#[test]
fn a_dropped_auxiliary_leaves_no_ledger_row_and_no_dangling_id_edge() {
    let s = inline(schema_with_id_edge_to_aux());
    assert!(!aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()));
    assert_eq!(
        view_ref_id(&s).as_deref(),
        Some(OWNER_ID),
        "the config value names `Shared`, which the owner's primary now carries alone"
    );
    let entries = &s.ledger.as_ref().expect("ledger survives").entries;
    assert!(!entries.contains_key(AUX_ID), "the dropped declaration's ledger row is stale");
    assert!(entries.contains_key(OWNER_ID), "the owner's row stays");
}

#[test]
fn a_kept_auxiliary_keeps_its_ledger_row_and_its_id_edge() {
    let mut fixture = schema_with_id_edge_to_aux();
    fixture["orbitals"][1]["traits"][0]["stateMachine"]["transitions"][0]["effects"] = json!([["fetch", "Shared", {}]]);
    let s = inline(fixture);
    assert!(aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()));
    assert_eq!(view_ref_id(&s).as_deref(), Some(AUX_ID));
    assert!(s.ledger.as_ref().expect("ledger survives").entries.contains_key(AUX_ID));
}

#[test]
fn a_shadowed_narrower_view_leaves_no_ledger_row_and_no_dangling_id_edge() {
    let mut fixture = schema_with_id_edge_to_aux();
    fixture["orbitals"][1]["auxiliaryEntities"][0]["collection"] = json!("shared");
    fixture["orbitals"][1]["auxiliaryEntities"][0]["fields"] = json!([{ "name": "id", "type": "string" }]);
    fixture["orbitals"][1]["traits"][0]["stateMachine"]["transitions"][0]["effects"] = json!([["fetch", "Shared", {}]]);
    let s = inline(fixture);
    assert!(!aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()), "a view of the owner's own record is shadowed by it");
    assert_eq!(view_ref_id(&s).as_deref(), Some(OWNER_ID));
    assert!(!s.ledger.as_ref().expect("ledger survives").entries.contains_key(AUX_ID));
}

#[test]
fn a_wider_copy_is_not_shadowed_and_keeps_its_row() {
    let mut fixture = schema_with_id_edge_to_aux();
    fixture["orbitals"][1]["auxiliaryEntities"][0]["collection"] = json!("shared");
    fixture["orbitals"][1]["traits"][0]["stateMachine"]["transitions"][0]["effects"] = json!([["fetch", "Shared", {}]]);
    let s = inline(fixture);
    assert!(aux_names(&s, "ConsumerOrbital").contains(&"Shared".to_string()), "it carries a field the owner lacks");
    assert_eq!(view_ref_id(&s).as_deref(), Some(AUX_ID));
    assert!(s.ledger.as_ref().expect("ledger survives").entries.contains_key(AUX_ID));
}
