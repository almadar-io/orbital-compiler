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
use orbital_core::schema::{EntityRef, OrbitalSchema};
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
                    "category": "interaction",
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
