//! The four entity access directives — `@read`/`@create`/`@update`/`@delete`.
//!
//! Each is a per-row predicate: `@entity` binds the candidate row (or, for
//! `@create`, the incoming data), `@user` binds the viewer. No other binding
//! root is legal there. The Rust check covers `@user.<field>` reads only; the
//! other rules are the `orb lint entity-access` rules.

use orbital_compiler::phases::validation::{entity_access, ValidationContext};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn run(schema: Value) -> Vec<String> {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    let mut ctx = ValidationContext::new();
    entity_access::validate_entity_access(&schema, &mut ctx);
    ctx.result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// A `Ticket` entity carrying all four directives, an identity roster, and an
/// `assignee` reference field — the exact shape `std-helpdesk.lolo` uses.
fn schema_with_ticket_policies(read: Value, create: Value, update: Value, delete: Value) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "TicketOrbital",
            "entity": {
                "name": "Ticket",
                "persistence": "persistent",
                "collection": "tickets",
                "fields": [
                    { "name": "id", "type": "string" },
                    // A RELATION to the identity entity, which is what
                    // std-helpdesk actually declares — a bare string here
                    // would (correctly) trip ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED.
                    { "name": "assignee", "type": "relation",
                      "relation": { "entity": "Person", "cardinality": "one" } }
                ],
                "read_policy": read,
                "create_policy": create,
                "update_policy": update,
                "delete_policy": delete
            },
            "auxiliaryEntities": [{
                "name": "Person",
                "identity": true,
                "persistence": "persistent",
                "collection": "people",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "role", "type": "string", "values": ["supervisor", "agent"] }
                ]
            }],
            "traits": [],
            "pages": []
        }]
    })
}

#[test]
fn an_unknown_user_field_reuses_the_identity_error_code() {
    let codes = run(schema_with_ticket_policies(
        Value::Null,
        json!(["=", "@user.clearance", "write"]),
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()),
        "expected the identity error code to be reused, got {codes:?}"
    );
}

#[test]
fn an_entity_with_no_directives_produces_no_errors() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "PlainOrbital",
            "entity": {
                "name": "Plain",
                "persistence": "persistent",
                "collection": "plains",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [],
            "pages": []
        }]
    });
    assert!(run(schema).is_empty());
}
