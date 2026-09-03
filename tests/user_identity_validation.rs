//! `[identity]` entity rules — the declared shape of `@user`.
//!
//! The acceptance property for the whole feature: a role literal that no
//! declared persona can ever hold must FAIL the build. std-healthcare's
//! `"patient"` and std-lms's `"student"` pass `orb validate` today and have
//! always silently rendered the staff branch.

use orbital_compiler::phases::validation::{user_identity, ValidationContext};
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

fn run(schema: Value) -> Vec<String> {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    let mut ctx = ValidationContext::new();
    user_identity::validate_user_identity(&schema, &mut ctx);
    ctx.result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// A trait whose config knob default branches on a role literal — the shape
/// std-healthcare actually uses (`navItems : [NavItem] = (if ["=", …] …)`),
/// which is a CONFIG DEFAULT and not a guard.
fn trait_with_role_branch(literal: &str) -> Value {
    json!({
        "name": "Shell",
        "linkedEntity": "Person",
        "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [] },
        "config": {
            "navItems": {
                "type": "json",
                "default": ["if", ["=", "@user.role", literal], ["a"], ["b"]]
            }
        }
    })
}

fn schema_with(identity_fields: Value, traits: Value, identity: bool) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "PersonOrbital",
            "entity": {
                "name": "Person",
                "identity": identity,
                "persistence": "persistent",
                "collection": "people",
                "fields": identity_fields
            },
            "traits": traits,
            "pages": []
        }]
    })
}

fn role_field(values: &[&str]) -> Value {
    json!({ "name": "role", "type": "string", "values": values })
}

// ---------------------------------------------------------------------------
// The headline rule
// ---------------------------------------------------------------------------

#[test]
fn a_role_literal_outside_the_declared_vocabulary_is_an_error() {
    let codes = run(schema_with(
        json!([role_field(&["clinician", "frontdesk"])]),
        json!([trait_with_role_branch("patient")]),
        true,
    ));
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "expected the dead branch to fail the build, got {codes:?}"
    );
}

#[test]
fn a_declared_role_literal_is_accepted() {
    let codes = run(schema_with(
        json!([role_field(&["clinician", "frontdesk", "patient"])]),
        json!([trait_with_role_branch("patient")]),
        true,
    ));
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

/// The backward-compatibility guarantee, updated for I.3: an undeclared program
/// gets the implicit DEFAULT identity shape, whose `role` carries no members —
/// so any literal comparison stays legal. Without this, every committed `.orb`
/// and all six std atoms that read `@user` would fail instantly.
#[test]
fn without_a_declared_identity_entity_role_literals_stay_legal() {
    let codes = run(schema_with(
        json!([role_field(&["clinician", "frontdesk"])]),
        json!([trait_with_role_branch("patient")]),
        false,
    ));
    assert!(
        codes.is_empty(),
        "the default identity's unconstrained role must accept any literal, got {codes:?}"
    );
}

/// The I.3 default-shape arm: a field nothing can ever carry (not on the
/// default id/uid/email/name/role shape, and no [identity] entity to extend it)
/// is an error even in an undeclared program.
#[test]
fn without_a_declared_identity_entity_an_unknown_field_is_still_an_error() {
    let trait_def = json!({
        "name": "Shell",
        "linkedEntity": "Person",
        "category": "interaction",
        "stateMachine": {
            "events": [],
            "states": [],
            "transitions": [{
                "from": "idle", "event": "GO", "to": "idle",
                "guard": ["=", "@user.department", "ops"]
            }]
        }
    });
    let codes = run(schema_with(json!([]), json!([trait_def]), false));
    assert_eq!(codes, vec!["ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()]);

    // The default shape's own fields stay readable.
    let ok_trait = json!({
        "name": "Shell",
        "linkedEntity": "Person",
        "category": "interaction",
        "stateMachine": {
            "events": [],
            "states": [],
            "transitions": [{
                "from": "idle", "event": "GO", "to": "idle",
                "guard": ["=", "@user.role", "anything"]
            }]
        }
    });
    let codes = run(schema_with(json!([]), json!([ok_trait]), false));
    assert!(
        codes.is_empty(),
        "default-shape fields must stay readable, got {codes:?}"
    );
}

/// `=` and `==` both occur in committed `.orb` files.
#[test]
fn both_equality_spellings_are_checked() {
    for op in ["=", "==", "!="] {
        let trait_def = json!({
            "name": "Shell",
            "linkedEntity": "Person",
            "category": "interaction",
            "stateMachine": {
                "events": [],
                "states": [],
                "transitions": [{
                    "from": "idle", "event": "GO", "to": "idle",
                    "guard": [op, "@user.role", "ghost"]
                }]
            }
        });
        let codes = run(schema_with(
            json!([role_field(&["clinician"])]),
            json!([trait_def]),
            true,
        ));
        assert!(
            codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
            "operator {op} was not checked, got {codes:?}"
        );
    }
}

/// Multi-valued roles keep the union on the ELEMENT, and `array/includes` is
/// directional — the vocabulary side is the haystack.
#[test]
fn array_includes_checks_the_element_vocabulary() {
    let roles = json!({
        "name": "roles",
        "type": "array",
        "items": { "type": "string", "values": ["clinician", "admin"] }
    });
    let bad = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["array/includes", "@user.roles", "ghost"]
        }]}
    });
    let codes = run(schema_with(json!([roles.clone()]), json!([bad]), true));
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "array/includes was not checked, got {codes:?}"
    );

    let good = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["array/includes", "@user.roles", "admin"]
        }]}
    });
    let codes = run(schema_with(json!([roles]), json!([good]), true));
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

/// A field with no declared vocabulary accepts any literal — which is exactly
/// what keeps the implicit DEFAULT identity's unconstrained `role : string`
/// from breaking the 38 existing `@user.role` sites.
#[test]
fn an_unconstrained_field_accepts_any_literal() {
    let codes = run(schema_with(
        json!([{ "name": "role", "type": "string" }]),
        json!([trait_with_role_branch("anything-at-all")]),
        true,
    ));
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

// ---------------------------------------------------------------------------
// Unknown field
// ---------------------------------------------------------------------------

#[test]
fn reading_an_undeclared_identity_field_is_an_error() {
    let trait_def = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["=", "@user.permissions", "write"]
        }]}
    });
    let codes = run(schema_with(
        json!([role_field(&["clinician"])]),
        json!([trait_def]),
        true,
    ));
    assert!(
        codes.contains(&"ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()),
        "expected the phantom field to be caught, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Cardinality
// ---------------------------------------------------------------------------

#[test]
fn two_differently_named_identity_entities_are_an_error() {
    let schema = json!({
        "name": "App",
        "orbitals": [
            {
                "name": "A",
                "entity": { "name": "Person", "identity": true, "fields": [] },
                "traits": [], "pages": []
            },
            {
                "name": "B",
                "entity": { "name": "Account", "identity": true, "fields": [] },
                "traits": [], "pages": []
            }
        ]
    });
    let codes = run(schema);
    assert!(
        codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "expected ambiguity to be rejected, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// An imported roster yields to the host's own
//
// A behavior declares its own `[identity]` roster so it runs standalone. That
// roster orbital is never imported — composition is per-trait — but a trait
// bound to one of its siblings drags the roster in as an auxiliary copy, tag
// and all. The composing app is the one that says who `@user` is, so a PRIMARY
// roster shadows every imported copy.
// ---------------------------------------------------------------------------

/// `host_roster`: does the composing orbital declare its own `[identity]`?
fn schema_with_imported_roster(host_roster: bool) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "HostOrbital",
            "entity": {
                "name": "Staff",
                "identity": host_roster,
                "persistence": "persistent",
                "collection": "staff",
                "fields": [
                    { "name": "id", "type": "string" },
                    role_field(&["owner", "member"])
                ]
            },
            "traits": [],
            "pages": [],
            "auxiliaryEntities": [{
                "name": "Employee",
                "identity": true,
                "persistence": "persistent",
                "collection": "employees",
                "fields": [
                    { "name": "id", "type": "string" },
                    role_field(&["employee", "approver"])
                ]
            }]
        }]
    })
}

#[test]
fn a_host_roster_shadows_an_imported_roster_copy() {
    let codes = run(schema_with_imported_roster(true));
    assert!(
        !codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "the host's own roster decides the viewer; the imported copy yields, got {codes:?}"
    );
}

#[test]
fn an_imported_roster_still_counts_when_the_host_declares_none() {
    // A thin app that wraps one behavior and declares no roster inherits that
    // behavior's — otherwise `@user` would silently fall back to the default
    // shape and every role literal would stop being checked.
    let codes = run(schema_with_imported_roster(false));
    assert!(
        !codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "one inherited roster is not ambiguous, got {codes:?}"
    );
}

#[test]
fn two_imported_rosters_with_no_host_roster_are_still_ambiguous() {
    let mut schema = schema_with_imported_roster(false);
    schema["orbitals"][0]["auxiliaryEntities"]
        .as_array_mut()
        .expect("fixture declares auxiliaryEntities")
        .push(json!({
            "name": "Contributor",
            "identity": true,
            "persistence": "persistent",
            "collection": "contributors",
            "fields": [{ "name": "id", "type": "string" }, role_field(&["author"])]
        }));
    let codes = run(schema);
    assert!(
        codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "nothing in the program says which imported roster is the viewer, got {codes:?}"
    );
}

#[test]
fn two_host_rosters_are_still_ambiguous() {
    let mut schema = schema_with_imported_roster(true);
    schema["orbitals"]
        .as_array_mut()
        .expect("fixture declares orbitals")
        .push(json!({
            "name": "SecondOrbital",
            "entity": {
                "name": "Member",
                "identity": true,
                "persistence": "persistent",
                "collection": "members",
                "fields": [{ "name": "id", "type": "string" }, role_field(&["member"])]
            },
            "traits": [],
            "pages": []
        }));
    let codes = run(schema);
    assert!(
        codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "shadowing never applies between two PRIMARY rosters, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Provenance is not policy
//
// `sourceEntityDefinition` records the atom's ORIGINAL entity so tooling can
// trace an imported trait back to its source. Once a consumer rebinds the trait
// (`-> PfTimesheet`), that entity is not in the composed program at all and its
// access policies govern nothing — but its `@user.role` literals are the ATOM's
// vocabulary. Judging them against the CONSUMER's roster reports the organism's
// own roles as dead branches: composing one std-time-tracking trait into an app
// rostered on `Person` produced 17 such errors for `"approver"`/`"employee"`,
// literals of an entity nowhere in the app.
//
// Nothing is lost by skipping it: an un-rebound trait's entity IS materialised
// (primary or auxiliary) and its policies are validated there — see
// `a_dead_role_branch_on_a_real_entity_is_still_caught`.
// ---------------------------------------------------------------------------

#[test]
fn a_role_literal_inside_source_entity_provenance_is_not_judged() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "HostOrbital",
            "entity": {
                "name": "Person",
                "identity": true,
                "persistence": "persistent",
                "collection": "people",
                "fields": [{ "name": "id", "type": "string" }, role_field(&["owner", "member"])]
            },
            "traits": [{
                "name": "Imported",
                "linkedEntity": "PfThing",
                "category": "interaction",
                "sourceBehavior": { "behavior": "std-time-tracking", "alias": "TimeTracking",
                                    "originalName": "TimesheetBrowseList" },
                // The atom's own entity — NOT part of this program — carrying the
                // atom's own role vocabulary.
                "sourceEntityDefinition": {
                    "name": "Timesheet",
                    "persistence": "persistent",
                    "collection": "timesheets",
                    "fields": [{ "name": "id", "type": "string" }],
                    "read_policy": ["=", "@user.role", "approver"]
                },
                "stateMachine": { "events": [], "states": [], "transitions": [] }
            }],
            "pages": []
        }]
    });
    let codes = run(schema);
    assert!(
        !codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "the atom's own role vocabulary must not be judged against the consumer's roster, got {codes:?}"
    );
}

#[test]
fn a_dead_role_branch_on_a_real_entity_is_still_caught() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "HostOrbital",
            "entity": {
                "name": "Person",
                "identity": true,
                "persistence": "persistent",
                "collection": "people",
                "fields": [{ "name": "id", "type": "string" }, role_field(&["owner", "member"])]
            },
            "traits": [trait_with_role_branch("admin")],
            "pages": []
        }]
    });
    let codes = run(schema);
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "a dead branch on an entity that IS in the program must still fail, got {codes:?}"
    );
}
