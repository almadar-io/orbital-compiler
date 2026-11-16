//! `[identity]` must survive Schema → OIR lowering.
//!
//! The flag exists on `EntityDefinition` and drives `@user`'s type, the persona
//! roster and ownership seeding — but `resolve_entity` used to copy `shared` and
//! the four access policies while silently dropping `identity`, so nothing
//! downstream of OIR could tell which entity is the viewer. Codegen needs it to
//! name the collection a generated app reads its persona roster from.

use orbital_compiler::phases::resolve_to_oir;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::json;

fn lower(schema: serde_json::Value) -> orbital_core_base::ir::OirModule {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    resolve_to_oir(&schema).expect("fixture must lower")
}

fn schema_with_identity(identity: bool) -> serde_json::Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "PersonOrbital",
            "entity": {
                "name": "Person",
                "identity": identity,
                "persistence": "persistent",
                "collection": "people",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "role", "type": "string", "values": ["admin", "member"] }
                ]
            },
            "traits": [],
            "pages": []
        }]
    })
}

#[test]
fn identity_entity_carries_the_flag_into_oir() {
    let module = lower(schema_with_identity(true));
    let person = module
        .entities
        .iter()
        .find(|e| e.name == "Person")
        .expect("Person must lower");

    assert!(
        person.identity,
        "[identity] must survive lowering — codegen resolves the persona roster's collection from it"
    );
    assert_eq!(
        person.collection, "people",
        "the roster is read from the collection, not the entity name"
    );
}

#[test]
fn a_plain_entity_is_not_the_identity() {
    let module = lower(schema_with_identity(false));
    let person = module.entities.iter().find(|e| e.name == "Person").unwrap();
    assert!(!person.identity);
}

#[test]
fn identity_is_omitted_from_oir_json_when_false() {
    let module = lower(schema_with_identity(false));
    let person = module.entities.iter().find(|e| e.name == "Person").unwrap();
    let json = serde_json::to_value(person).expect("OirEntity serializes");

    assert!(
        json.get("identity").is_none(),
        "a non-identity entity must gain no OIR key — same invariant the AST side pins"
    );
}

#[test]
fn identity_is_orthogonal_to_shared() {
    let module = lower(schema_with_identity(true));
    let person = module.entities.iter().find(|e| e.name == "Person").unwrap();
    assert!(
        !person.shared,
        "[identity] must not imply [shared] — nothing writes @user, it is an ambient read-only viewer"
    );
}
