//! The four entity access directives — `@read`/`@create`/`@update`/`@delete`.
//!
//! Each is a per-row predicate: `@entity` binds the candidate row (or, for
//! `@create`, the incoming data), `@user` binds the viewer. No other binding
//! root is legal there, and both halves must resolve against a real field.

use orbital_compiler::phases::validation::{entity_access, ValidationContext};
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

fn run(schema: Value) -> Vec<String> {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    let mut ctx = ValidationContext::new();
    entity_access::validate_entity_access(&schema, &mut ctx);
    ctx.result.errors.iter().map(|e| e.code.as_str().to_string()).collect()
}

/// A `Ticket` entity carrying all four directives, an identity roster, and an
/// `assignee` reference field — the exact shape `std-helpdesk.lolo` uses.
fn schema_with_ticket_policies(
    read: Value,
    create: Value,
    update: Value,
    delete: Value,
) -> Value {
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
                    { "name": "assignee", "type": "string" }
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
fn a_known_entity_field_is_accepted() {
    let codes = run(schema_with_ticket_policies(
        json!(["=", ["object/get", "@entity", "assignee"], ["object/get", "@user", "id"]]),
        Value::Null,
        Value::Null,
        Value::Null,
    ));
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

#[test]
fn an_unknown_entity_field_is_an_error() {
    let codes = run(schema_with_ticket_policies(
        json!(["=", ["object/get", "@entity", "ownerId"], ["object/get", "@user", "id"]]),
        Value::Null,
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_ACCESS_POLICY_UNKNOWN_ENTITY_FIELD".to_string()),
        "expected the unknown field to be caught, got {codes:?}"
    );
}

#[test]
fn the_bare_dotted_string_form_is_also_checked() {
    let codes = run(schema_with_ticket_policies(
        json!(["=", "@entity.ownerId", "@user.id"]),
        Value::Null,
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_ACCESS_POLICY_UNKNOWN_ENTITY_FIELD".to_string()),
        "expected the bare-string form to be checked too, got {codes:?}"
    );
}

#[test]
fn an_unknown_user_field_reuses_the_identity_error_code() {
    let codes = run(schema_with_ticket_policies(
        Value::Null,
        json!(["=", "@user.permissions", "write"]),
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()),
        "expected the identity error code to be reused, got {codes:?}"
    );
}

#[test]
fn a_role_literal_outside_the_declared_vocabulary_is_an_error() {
    let codes = run(schema_with_ticket_policies(
        json!(["=", "@user.role", "manager"]),
        Value::Null,
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "expected the dead branch to fail, got {codes:?}"
    );
}

#[test]
fn a_binding_root_other_than_entity_or_user_is_rejected() {
    let codes = run(schema_with_ticket_policies(
        Value::Null,
        json!(["=", "@payload.status", "open"]),
        Value::Null,
        Value::Null,
    ));
    assert!(
        codes.contains(&"ORB_S_ACCESS_POLICY_INVALID_BINDING".to_string()),
        "expected @payload to be rejected inside an access directive, got {codes:?}"
    );
}

#[test]
fn reading_user_with_no_declared_identity_entity_is_an_error() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "TicketOrbital",
            "entity": {
                "name": "Ticket",
                "persistence": "persistent",
                "collection": "tickets",
                "fields": [{ "name": "id", "type": "string" }],
                "read_policy": ["=", "@user.role", "supervisor"]
            },
            "traits": [],
            "pages": []
        }]
    });
    let codes = run(schema);
    assert!(
        codes.contains(&"ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY".to_string()),
        "expected the missing-roster error, got {codes:?}"
    );
}

#[test]
fn a_directive_reading_only_entity_needs_no_identity_entity() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "TicketOrbital",
            "entity": {
                "name": "Ticket",
                "persistence": "persistent",
                "collection": "tickets",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "quantity", "type": "number" }
                ],
                "create_policy": ["<=", "@entity.quantity", 2]
            },
            "traits": [],
            "pages": []
        }]
    });
    let codes = run(schema);
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
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
