//! `[identity]` entity rules — the declared shape of `@user`.
//!
//! The Rust checks left here are the `@user.<field>` existence check and the
//! config-held role lists; role-literal comparisons and page nav roles are the
//! `orb lint user-identity` rules.

use orbital_compiler::phases::validation::{user_identity, ValidationContext};
use orbital_core_base::schema::OrbitalSchema;
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

/// A field nothing can ever carry (not on the `Viewer`, and no [identity]
/// entity to extend it) is an error even in an undeclared program.
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

// ---------------------------------------------------------------------------
// Unknown field
// ---------------------------------------------------------------------------

#[test]
fn reading_an_undeclared_identity_field_is_an_error() {
    let trait_def = json!({
        "name": "Shell", "linkedEntity": "Person", "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["=", "@user.clearance", "write"]
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
// Config-held role lists (allowedRoles / nav-item roles)
// ---------------------------------------------------------------------------

fn gated_layout(allowed: Value, nav: Value, compare_to: &str) -> Value {
    let item_includes = |p: &str| {
        json!([
            "array/includes",
            ["object/get", format!("@{p}"), "roles", []],
            "@config.viewerRole"
        ])
    };
    json!({
        "name": "Layout",
        "linkedEntity": "Person",
        "category": "interaction",
        "config": {
            "allowedRoles": { "type": "unknown", "default": allowed },
            "viewerRole": { "type": "unknown", "default": compare_to },
            "navItems": { "type": "unknown", "default": nav }
        },
        "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ {
                "from": "idle", "to": "idle", "event": "INIT",
                "guard": ["array/includes", "@config.allowedRoles", "@config.viewerRole"],
                "effects": [ ["render-ui", "main", {
                    "type": "app-layout",
                    "navItems": ["array/filter", "@config.navItems",
                        ["fn", "item", ["or", item_includes("item"),
                            ["array/some",
                                ["array/filter", ["object/get", "@item", "children", []],
                                    ["fn", "child", item_includes("child")]],
                                ["fn", "c", true]]]]]
                }] ]
            } ]
        }
    })
}

fn role_list_errors(allowed: Value, nav: Value, compare_to: &str) -> usize {
    run(schema_with(
        json!([role_field(&["clinician", "frontdesk"])]),
        json!([gated_layout(allowed, nav, compare_to)]),
        true,
    ))
    .iter()
    .filter(|c| c.as_str() == "ORB_S_ROLE_LITERAL_NOT_MEMBER")
    .count()
}

#[test]
fn a_bogus_literal_in_allowed_roles_errors() {
    assert_eq!(
        role_list_errors(json!(["list", "clinician", "zzz"]), json!([]), "@user.role"),
        1
    );
}

#[test]
fn a_bogus_literal_in_nav_item_roles_errors() {
    let nav = json!([{ "label": "A", "roles": ["zzz"] }]);
    assert_eq!(role_list_errors(json!([]), nav, "@user.role"), 1);
}

#[test]
fn declared_literals_in_role_lists_pass() {
    let nav = json!([{ "label": "A", "roles": ["clinician"],
        "children": [{ "label": "A1", "roles": ["frontdesk"] }] }]);
    assert_eq!(
        role_list_errors(json!(["clinician", "frontdesk"]), nav, "@user.role"),
        0
    );
}

#[test]
fn a_bogus_literal_in_nested_child_roles_errors() {
    let nav = json!([{ "label": "A", "roles": ["clinician"],
        "children": [{ "label": "A1", "roles": ["clinician", "ghost"] }] }]);
    assert_eq!(role_list_errors(json!([]), nav, "@user.role"), 1);
}

/// Paired control: lists never compared against a role source are not checked.
#[test]
fn role_lists_not_compared_to_a_role_source_are_not_checked() {
    let nav = json!([{ "label": "A", "roles": ["zzz"] }]);
    assert_eq!(role_list_errors(json!(["zzz"]), nav, "not-a-user-read"), 0);
}
