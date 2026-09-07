//! `[identity]` entity rules — the declared shape of `@user`.
//!
//! The acceptance property for the whole feature: a role literal that no
//! declared persona can ever hold must FAIL the build. std-healthcare's
//! `"patient"` and std-lms's `"student"` pass `orb validate` today and have
//! always silently rendered the staff branch.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{entity_access, user_identity, ValidationContext};
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

/// The Stage-B orbital-import role rewrite (`roles {}` remapped to a list of
/// consumer targets) produces the REVERSE `array/includes` shape from the
/// test above — a literal array haystack with the `@user.<f>` sigil as the
/// needle (`(array/includes ["owner","project_manager"] @user.role)`),
/// rather than the sigil-as-haystack shape. Every haystack literal must be
/// checked individually against the sigil's field vocabulary — a policy
/// authored this way directly must fire once per non-member, not zero times.
#[test]
fn array_includes_with_a_literal_haystack_checks_every_member() {
    let role = json!({ "name": "role", "type": "string", "values": ["approver", "owner"] });
    let bad = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["array/includes", ["approver", "ghost"], "@user.role"]
        }]}
    });
    let codes = run(schema_with(json!([role.clone()]), json!([bad]), true));
    let ghost_errors: Vec<&String> = codes
        .iter()
        .filter(|c| c.as_str() == "ORB_S_ROLE_LITERAL_NOT_MEMBER")
        .collect();
    assert_eq!(
        ghost_errors.len(),
        1,
        "expected exactly one ORB_S_ROLE_LITERAL_NOT_MEMBER (for 'ghost' only, not 'approver'), got {codes:?}"
    );

    let good = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["array/includes", ["approver", "owner"], "@user.role"]
        }]}
    });
    let codes = run(schema_with(json!([role]), json!([good]), true));
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

// ---------------------------------------------------------------------------
// B3-R: an imported entity's role literal, judged post-inline through the
// REAL orbital-import pipeline (not `sourceEntityDefinition` — the entity
// here is fully materialized, splice-in-place, same as any other primary).
// ---------------------------------------------------------------------------

/// One orbital: primary `Task` carries `read_policy: ["=", "@user.role",
/// "approver"]`; auxiliary `[identity]` `Approver` is the UPSTREAM roster
/// (`role : "employee" | "approver"`) a `roles {}` remap resolves against.
const IMPORTED_POLICY_UPSTREAM: &str = r#"{
  "name": "test-user-identity-imported-policy-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TaskOrbital",
      "entity": {
        "name": "Task",
        "persistence": "persistent",
        "read_policy": ["=", "@user.role", "approver"],
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "auxiliaryEntities": [
        {
          "name": "Approver",
          "identity": true,
          "persistence": "persistent",
          "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "role", "type": "string", "values": ["employee", "approver"] }
          ]
        }
      ],
      "traits": [
        {
          "name": "TaskView",
          "linkedEntity": "Task",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] } ]
          }
        }
      ],
      "pages": [
        { "name": "TaskPage", "path": "/tasks", "primaryEntity": "Task", "traits": [ { "ref": "TaskView" } ] }
      ]
    }
  ]
}"#;

/// `orbital Local = Up.orbitals.TaskOrbital { <reference_extra> }`, alongside
/// the CONSUMER's own `[identity]` roster (`Person`, `role : "owner" |
/// "member"`) — `Task` (imported) is not identity-tagged, so no ambiguity.
fn imported_policy_consumer(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-user-identity-imported-policy-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Local",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.TaskOrbital.entity",
      "traits": [],
      "reference": {{
        "ref": "Up.orbitals.TaskOrbital"{extra}
      }}
    }},
    {{
      "name": "PersonOrbital",
      "entity": {{
        "name": "Person",
        "identity": true,
        "persistence": "persistent",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "role", "type": "string", "values": ["owner", "member"] }}
        ]
      }},
      "traits": []
    }}
  ]
}}"#,
        extra = reference_extra
    )
}

fn inline_imported_policy_consumer(reference_extra: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), IMPORTED_POLICY_UPSTREAM)
        .expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, imported_policy_consumer(reference_extra))
        .expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer must parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("orbital-import inline must succeed");
    schema
}

/// `read_policy` lives on the ENTITY, not inside any trait body — it is
/// `entity_access::validate_entity_access` (not `user_identity::validate_user_identity`,
/// which only walks trait JSON) that runs `check_comparison` over it.
fn run_schema(schema: &OrbitalSchema) -> Vec<String> {
    let mut ctx = ValidationContext::new();
    entity_access::validate_entity_access(schema, &mut ctx);
    ctx.result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

#[test]
fn an_imported_policy_remapped_to_the_consumer_roster_passes() {
    let schema = inline_imported_policy_consumer(r#", "roles": { "approver": ["owner"] }"#);
    let codes = run_schema(&schema);
    assert!(
        !codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "remapped to 'owner' (a consumer roster member) — must not fire, got {codes:?}"
    );
}

#[test]
fn an_imported_policy_left_unmapped_fires_role_literal_not_member() {
    let schema = inline_imported_policy_consumer("");
    let codes = run_schema(&schema);
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "left as upstream's own 'approver', which the consumer roster doesn't declare — must fire, got {codes:?}"
    );
}
