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

/// POSITIVE CONTROL for `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED`: an owner column
/// compared to `@user.id` that is a bare scalar rather than a relation. It
/// type-checks and reads like ownership, but viewer ids are only ever stamped
/// onto declared relations — so the predicate matches no row and the directive
/// denies everyone. This is the shipped `std-helpdesk` `customerEmail` defect.
#[test]
fn an_owner_column_compared_to_the_viewer_must_be_identity_typed() {
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
                    { "name": "customerEmail", "type": "email" }
                ],
                "read_policy": ["=", ["object/get", "@entity", "customerEmail"], "@user.id"]
            },
            "auxiliaryEntities": [{
                "name": "Person",
                "identity": true,
                "persistence": "persistent",
                "collection": "people",
                "fields": [{ "name": "id", "type": "string" }]
            }],
            "traits": [],
            "pages": []
        }]
    });
    assert_eq!(run(schema), vec!["ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED"]);
}

/// NEGATIVE CONTROL: the identity entity reading its OWN row. `@entity.id` is
/// the row's own key, not a mistyped owner column — the self-read shape S3b
/// added across the corpus must stay silent.
#[test]
fn an_entity_reading_its_own_key_is_not_a_mistyped_owner_column() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "StaffOrbital",
            "entity": {
                "name": "Staff",
                "persistence": "persistent",
                "collection": "staff",
                "fields": [{ "name": "id", "type": "string" }],
                "read_policy": ["=", ["object/get", "@entity", "id"], "@user.id"]
            },
            "auxiliaryEntities": [{
                "name": "Person",
                "identity": true,
                "persistence": "persistent",
                "collection": "people",
                "fields": [{ "name": "id", "type": "string" }]
            }],
            "traits": [],
            "pages": []
        }]
    });
    assert!(run(schema).is_empty(), "reading @entity.id must not warn");
}
