//! `expects` — consumer-side requirement declarations (E3, proposal §5/§5.1).
//!
//! Standalone (single-orbital slice) expectations are TRUSTED: they satisfy
//! the identity rules and serve as the local type environment for
//! self-consistency checks. Multi-orbital schemas are the link step: every
//! expectation must be satisfied by exactly one provider, with shape
//! compatibility (width / `type_compat` / literal-union subset).

use orbital_compiler::phases::validation::{
    entity_access, expectations, user_identity, validate_comprehensive_without_services, ValidationContext,
};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn parse(schema: Value) -> OrbitalSchema {
    serde_json::from_value(schema).expect("schema fixture must deserialize")
}

/// The validators `expects` touches: identity environment (user_identity,
/// entity_access) plus the expectation checks themselves.
fn run_scoped(schema: Value) -> Vec<String> {
    let schema = parse(schema);
    let mut ctx = ValidationContext::new();
    user_identity::validate_user_identity(&schema, &mut ctx);
    entity_access::validate_entity_access(&schema, &mut ctx);
    expectations::validate_expectations(&schema, &mut ctx);
    ctx.result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// The full pipeline — for the licensing arms (relation targets, persist /
/// fetch targets, `linkedEntity`) whose entity-name table is built by
/// `collect_names`.
fn run_full(schema: Value) -> Vec<String> {
    validate_comprehensive_without_services(&parse(schema))
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

fn expects_identity_customer() -> Value {
    json!([{
        "kind": "identity",
        "name": "Customer",
        "shape": [
            { "name": "id", "type": "string" },
            { "name": "role", "type": "string",
              "values": ["store-manager", "operations-director"] }
        ]
    }])
}

/// Single-orbital slice: `Product` with a `@create` policy reading
/// `@user.role`, and the `expects entity Customer [identity]` declaration carrying the
/// role vocabulary the policy reads.
fn slice_with_policy(policy: Value) -> Value {
    json!({
        "name": "App",
        "orbitals": [{
            "name": "ProductOrbital",
            "expects": expects_identity_customer(),
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [{ "name": "id", "type": "string" }],
                "create_policy": policy
            },
            "traits": [],
            "pages": []
        }]
    })
}

fn role_policy(literal: &str) -> Value {
    json!([
        "or",
        ["=", "@user.role", "store-manager"],
        ["=", "@user.role", literal]
    ])
}

// ---------------------------------------------------------------------------
// Standalone slice: expectations are trusted and self-consistent
// ---------------------------------------------------------------------------

/// The acceptance property for the slice path: a policy-bearing orbital that
/// declares what it reads validates with NO identity errors and no new codes.
#[test]
fn slice_with_expects_identity_validates_clean() {
    let codes = run_scoped(slice_with_policy(role_policy("operations-director")));
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "expected a clean slice, got {codes:?}"
    );
}

/// The same slice WITHOUT `expects` errors exactly as today — the escape
/// hatch must be declared, never inferred.
#[test]
fn without_expects_the_same_slice_still_requires_identity() {
    let mut schema = slice_with_policy(role_policy("operations-director"));
    schema["orbitals"][0]
        .as_object_mut()
        .unwrap()
        .remove("expects");
    let codes = run_scoped(schema);
    assert!(
        codes.contains(&"ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY".to_string()),
        "removing expects must restore today's error, got {codes:?}"
    );
}

/// `@user.<field>` outside the declared shape fails standalone against the
/// consumer's OWN declaration — with the new code, not the identity-entity one.
#[test]
fn a_user_field_outside_the_declared_shape_fails_standalone() {
    let codes = run_scoped(slice_with_policy(json!(["=", "@user.avatar", "x"])));
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "expected the self-consistency error, got {codes:?}"
    );
    assert!(
        !codes.contains(&"ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()),
        "the declaration replaces the default shape, got {codes:?}"
    );
    assert!(
        !codes.contains(&"ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY".to_string()),
        "the expectation still satisfies the roster requirement, got {codes:?}"
    );
}

/// Role literals check against the DECLARED closed vocabulary standalone.
#[test]
fn a_role_literal_outside_the_declared_vocabulary_fails_standalone() {
    let codes = run_scoped(slice_with_policy(role_policy("mnager")));
    assert!(
        codes.contains(&"ORB_S_ROLE_LITERAL_NOT_MEMBER".to_string()),
        "expected the typo'd literal to fail against the declared vocabulary, got {codes:?}"
    );
}

/// A trait-body `@user` read (not a policy) is checked against the declared
/// shape too — user_identity's walk resolves through the same environment.
#[test]
fn a_trait_guard_outside_the_declared_shape_fails_standalone() {
    let mut schema = slice_with_policy(json!(["=", "@user.role", "store-manager"]));
    schema["orbitals"][0]["traits"] = json!([{
        "name": "Shell",
        "linkedEntity": "Product",
        "category": "interaction",
        "stateMachine": { "events": [], "states": [], "transitions": [{
            "from": "idle", "event": "GO", "to": "idle",
            "guard": ["=", "@user.avatar", "x"]
        }]}
    }]);
    let codes = run_scoped(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "expected the guard read to fail against the declaration, got {codes:?}"
    );
}

/// A NAMED `expects entity Customer [identity]` satisfies the owner-field check for a
/// relation typed to Customer.
#[test]
fn a_named_expects_identity_satisfies_owner_field_typing() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "TicketOrbital",
            "expects": expects_identity_customer(),
            "entity": {
                "name": "Ticket",
                "persistence": "persistent",
                "collection": "tickets",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "owner", "type": "relation",
                      "relation": { "entity": "Customer", "cardinality": "one" } }
                ],
                "read_policy": ["=", "@entity.owner", "@user.id"]
            },
            "traits": [],
            "pages": []
        }]
    });
    let codes = run_scoped(schema);
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "expected a clean slice, got {codes:?}"
    );
}

/// A BARE `expects entity [identity]` satisfies the roster requirement but is NOT a
/// name the owner-field check can resolve to.
#[test]
fn a_bare_expects_identity_does_not_name_an_owner_type() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "TicketOrbital",
            "expects": [{ "kind": "identity" }],
            "entity": {
                "name": "Ticket",
                "persistence": "persistent",
                "collection": "tickets",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "owner", "type": "relation",
                      "relation": { "entity": "Customer", "cardinality": "one" } }
                ],
                "read_policy": ["=", "@entity.owner", "@user.id"]
            },
            "traits": [],
            "pages": []
        }]
    });
    let codes = run_scoped(schema);
    assert!(
        codes.contains(&"ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED".to_string()),
        "a nameless expectation cannot type the owner field, got {codes:?}"
    );
    assert!(
        !codes.contains(&"ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY".to_string()),
        "the bare expectation still satisfies the roster requirement, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Standalone slice: `expects entity` licensing + persist self-consistency
// ---------------------------------------------------------------------------

/// Relation fields, persist/fetch targets, and `linkedEntity` rebinds naming
/// an `expects entity` resolve standalone (full pipeline, so `collect_names`
/// is what licenses them).
#[test]
fn expects_entity_licenses_relation_persist_and_linked_entity_targets() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "ProductOrbital",
            "expects": [{ "kind": "entity", "name": "Customer" }],
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "customer", "type": "relation",
                      "relation": { "entity": "Customer", "cardinality": "one" } }
                ]
            },
            "traits": [{
                "name": "ProductList",
                "linkedEntity": "Customer",
                "category": "interaction",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "SAVE", "name": "Save" }],
                    "transitions": [{
                        "from": "idle", "event": "SAVE", "to": "idle",
                        "effects": [
                            ["persist", "create", "Customer", { "anything": "goes" }],
                            ["fetch", "Customer"]
                        ]
                    }]
                }
            }],
            "pages": []
        }]
    });
    let codes = run_full(schema);
    for unexpected in [
        "ORB_E_INVALID_RELATION",
        "ORB_X_UNRESOLVED_RELATION_TARGET",
        "ORB_EFF_FETCH_INVALID_ENTITY",
        "ORB_S_EXPECTATION_FIELD_UNDECLARED",
    ] {
        assert!(
            !codes.contains(&unexpected.to_string()),
            "existence-only expects entity must license the target and leave payloads unchecked, \
             found {unexpected} in {codes:?}"
        );
    }
}

/// L-10: an `@entity` payload sentinel on a trait `linkedEntity`-bound to an
/// expects-licensed sibling DEFERS to compose (the slice cannot carry the
/// provider's shape by design) — while the same trait without the expects
/// declaration still blocks, so the sweep's backstop is intact.
#[test]
fn an_expects_licensed_linked_entity_defers_the_entity_sentinel() {
    let orbital = |expects: serde_json::Value| {
        json!({
            "name": "App",
            "orbitals": [{
                "name": "AccountOrbital",
                "expects": expects,
                "entity": {
                    "name": "CsAccount",
                    "persistence": "persistent",
                    "collection": "accounts",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                "traits": [{
                    "name": "HealthList",
                    "linkedEntity": "HealthScoreSearch",
                    "category": "interaction",
                    "emits": [{
                        "event": "VIEW",
                        "payloadSchema": [{ "name": "row", "type": "@entity" }]
                    }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "VIEW", "name": "View" }],
                        "transitions": [{ "from": "idle", "event": "VIEW", "to": "idle" }]
                    }
                }],
                "pages": []
            }]
        })
    };
    let licensed = run_full(orbital(
        json!([{ "kind": "entity", "name": "HealthScoreSearch" }]),
    ));
    assert!(
        !licensed.contains(&"ORB_T_UNRESOLVED_TYPE_PARAM".to_string()),
        "expects-licensed linkedEntity must defer the @entity sentinel to compose, got {licensed:?}"
    );
    let unlicensed = run_full(orbital(json!([])));
    assert!(
        unlicensed.contains(&"ORB_T_UNRESOLVED_TYPE_PARAM".to_string())
            || unlicensed
                .iter()
                .any(|c| c.contains("LINKED_ENTITY") || c.contains("INVALID")),
        "without the expects declaration the slice must still be rejected, got {unlicensed:?}"
    );
}

/// A NAMED `expects entity [identity]` licenses the same targets: the dual form
/// (`expects entity X` beside `expects entity X [identity]`) is
/// ELOLO_DUPLICATE_EXPECTATION, so a derived slice whose entity holds a
/// relation to the organism's identity entity carries ONLY the identity
/// declaration — it must resolve standalone (measured: std-cms edit-demote,
/// `author : Contributor` under `expects entity Contributor [identity]`).
#[test]
fn a_named_expects_identity_licenses_relation_targets() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "ArticleOrbital",
            "expects": [{
                "kind": "identity",
                "name": "Contributor",
                "shape": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "role", "type": "string", "values": ["author", "editor"] }
                ]
            }],
            "entity": {
                "name": "Article",
                "persistence": "persistent",
                "collection": "articles",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "author", "type": "relation",
                      "relation": { "entity": "Contributor", "cardinality": "one" } }
                ]
            },
            "traits": [{
                "name": "ArticleList",
                "category": "interaction",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "SAVE", "name": "Save" }],
                    "transitions": [{ "from": "idle", "event": "SAVE", "to": "idle" }]
                }
            }],
            "pages": []
        }]
    });
    let codes = run_full(schema);
    for unexpected in ["ORB_E_INVALID_RELATION", "ORB_X_UNRESOLVED_RELATION_TARGET"] {
        assert!(
            !codes.contains(&unexpected.to_string()),
            "a named `expects entity X [identity]` must license relation targets naming it, \
             found {unexpected} in {codes:?}"
        );
    }
}

/// With a declared shape, a persist payload field outside it fails — the
/// declaration is the opting-in (payload fields are checked nowhere else).
#[test]
fn a_persist_payload_outside_the_declared_shape_fails() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "CheckoutOrbital",
            "expects": [{
                "kind": "entity",
                "name": "OrderRecord",
                "shape": [
                    { "name": "status", "type": "string",
                      "values": ["placed", "shipped", "delivered"] },
                    { "name": "totalAmount", "type": "number" }
                ]
            }],
            "entity": {
                "name": "Cart",
                "persistence": "persistent",
                "collection": "carts",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [{
                "name": "Checkout",
                "linkedEntity": "Cart",
                "category": "interaction",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "PLACE", "name": "Place" }],
                    "transitions": [{
                        "from": "idle", "event": "PLACE", "to": "idle",
                        "effects": [
                            ["persist", "create", "OrderRecord",
                             { "status": "placed", "total": 5 }]
                        ]
                    }]
                }
            }],
            "pages": []
        }]
    });
    let codes = run_scoped(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "the undeclared payload field must fail self-consistency, got {codes:?}"
    );
}

/// The same persist with a payload INSIDE the declared shape is clean.
#[test]
fn a_persist_payload_inside_the_declared_shape_is_clean() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "CheckoutOrbital",
            "expects": [{
                "kind": "entity",
                "name": "OrderRecord",
                "shape": [
                    { "name": "status", "type": "string",
                      "values": ["placed", "shipped", "delivered"] },
                    { "name": "totalAmount", "type": "number" }
                ]
            }],
            "entity": {
                "name": "Cart",
                "persistence": "persistent",
                "collection": "carts",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [{
                "name": "Checkout",
                "linkedEntity": "Cart",
                "category": "interaction",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "PLACE", "name": "Place" }],
                    "transitions": [{
                        "from": "idle", "event": "PLACE", "to": "idle",
                        "effects": [
                            ["persist", "create", "OrderRecord",
                             { "status": "placed", "totalAmount": 5 }]
                        ]
                    }]
                }
            }],
            "pages": []
        }]
    });
    let codes = run_scoped(schema);
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "expected a clean slice, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Compose link-checks (multi-orbital schemas)
// ---------------------------------------------------------------------------

fn consumer_expecting(expectation: Value) -> Value {
    json!({
        "name": "ConsumerOrbital",
        "expects": [expectation],
        "entity": {
            "name": "Cart",
            "persistence": "persistent",
            "collection": "carts",
            "fields": [{ "name": "id", "type": "string" }]
        },
        "traits": [],
        "pages": []
    })
}

fn provider_with(fields: Value, identity: bool) -> Value {
    json!({
        "name": "OrderOrbital",
        "entity": {
            "name": "OrderRecord",
            "identity": identity,
            "persistence": "persistent",
            "collection": "orders",
            "fields": fields
        },
        "traits": [],
        "pages": []
    })
}

fn composed(consumer: Value, provider: Value) -> Value {
    json!({ "name": "App", "orbitals": [consumer, provider] })
}

/// A single-orbital schema is a slice: expectations are trusted and NEVER
/// reported unsatisfied.
#[test]
fn a_single_orbital_schema_never_reports_unsatisfied() {
    let schema = json!({
        "name": "App",
        "orbitals": [consumer_expecting(json!({ "kind": "entity", "name": "Nobody" }))]
    });
    let codes = run_scoped(schema);
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "a slice trusts its expectations, got {codes:?}"
    );
}

/// An expectation whose provider is ABSENT is deferred to the importer, not an
/// error — in a multi-orbital program exactly as in a single-orbital slice.
/// This is what lets a LIBRARY organism drop its own `[identity]`, declare what
/// it expects, and still validate standalone; the composed program is where the
/// provider is checked. It also keeps this module consistent with
/// `entity_access`, which already trusts any `expects entity [identity]` to satisfy
/// `ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY`.
#[test]
fn an_absent_provider_is_deferred_not_unsatisfied() {
    let schema = composed(
        consumer_expecting(json!({ "kind": "entity", "name": "OrderRecord" })),
        json!({
            "name": "ProductOrbital",
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [],
            "pages": []
        }),
    );
    let result = validate_comprehensive_without_services(&parse(schema));
    let hits: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code.as_str() == "ORB_S_EXPECTATION_UNSATISFIED")
        .collect();
    assert!(
        hits.is_empty(),
        "an absent provider must defer, not error: {:?}",
        hits
    );
}

/// The deferral must NOT extend to a provider that is present but wrong — that
/// is still a hard error, which is the whole point of keeping the link check.
#[test]
fn a_present_but_incompatible_provider_still_errors() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "Product",
            "shape": [{ "name": "totalAmount", "type": "number" }]
        })),
        json!({
            "name": "ProductOrbital",
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [],
            "pages": []
        }),
    );
    let result = validate_comprehensive_without_services(&parse(schema));
    let hits: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code.as_str() == "ORB_S_EXPECTED_FIELD_NOT_PROVIDED")
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "a provider missing an expected field must still error: {:?}",
        result.errors
    );
}

/// `expects entity Customer [identity]` where Customer exists but carries no
/// [identity] tag.
#[test]
fn a_provider_without_the_identity_tag_is_a_kind_mismatch() {
    let schema = json!({
        "name": "App",
        "orbitals": [
            {
                "name": "ProductOrbital",
                "expects": [{ "kind": "identity", "name": "Customer" }],
                "entity": {
                    "name": "Product",
                    "persistence": "persistent",
                    "collection": "products",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                "traits": [],
                "pages": []
            },
            {
                "name": "CustomerOrbital",
                "entity": {
                    "name": "Customer",
                    "persistence": "persistent",
                    "collection": "customers",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                "traits": [],
                "pages": []
            }
        ]
    });
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_PROVIDER_MISMATCH".to_string()),
        "expected the kind mismatch, got {codes:?}"
    );
    assert!(
        !codes.contains(&"ORB_S_EXPECTATION_UNSATISFIED".to_string()),
        "the provider exists — only its kind is wrong, got {codes:?}"
    );
}

/// The provider exists but declares no field with an expected name.
#[test]
fn an_expected_field_the_provider_lacks_is_reported() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [{ "name": "totalAmount", "type": "number" }]
        })),
        provider_with(json!([{ "name": "id", "type": "string" }]), false),
    );
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTED_FIELD_NOT_PROVIDED".to_string()),
        "expected the missing field to be reported, got {codes:?}"
    );
}

/// Name matches, type does not (per `type_compat`).
#[test]
fn an_incompatible_field_type_is_reported() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [{ "name": "totalAmount", "type": "number" }]
        })),
        provider_with(
            json!([
                { "name": "id", "type": "string" },
                { "name": "totalAmount", "type": "string" }
            ]),
            false,
        ),
    );
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()),
        "expected the type mismatch, got {codes:?}"
    );
}

/// Optionality is one-directional: an optional expectation is satisfied by a
/// required provider field (the provider's guarantee is stronger), while a
/// required expectation against an optional provider field is a mismatch.
#[test]
fn an_optional_expectation_accepts_a_required_provider_field() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [{ "name": "totalAmount", "type": "number" }]
        })),
        provider_with(
            json!([
                { "name": "id", "type": "string" },
                { "name": "totalAmount", "type": "number", "required": true }
            ]),
            false,
        ),
    );
    let codes = run_full(schema);
    assert!(
        !codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()),
        "a stronger provider guarantee must satisfy an optional expectation, got {codes:?}"
    );
}

#[test]
fn a_required_expectation_rejects_an_optional_provider_field() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [{ "name": "totalAmount", "type": "number", "required": true }]
        })),
        provider_with(
            json!([
                { "name": "id", "type": "string" },
                { "name": "totalAmount", "type": "number" }
            ]),
            false,
        ),
    );
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()),
        "expected the optionality mismatch, got {codes:?}"
    );
}

/// Variance: the provider may know MORE — extra fields (width) and extra
/// literal-union members (subset) both pass.
#[test]
fn a_provider_superset_satisfies_the_expectation() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [
                { "name": "status", "type": "string", "values": ["placed", "shipped"] },
                { "name": "totalAmount", "type": "number" }
            ]
        })),
        provider_with(
            json!([
                { "name": "id", "type": "string" },
                { "name": "status", "type": "string",
                  "values": ["placed", "shipped", "delivered"] },
                { "name": "totalAmount", "type": "number" },
                { "name": "internalNotes", "type": "string" }
            ]),
            false,
        ),
    );
    let codes = run_full(schema);
    assert!(
        !codes.iter().any(|c| c.starts_with("ORB_S_EXPECT")),
        "width + literal-union superset must satisfy, got {codes:?}"
    );
}

/// The subset rule's cutting edge: every value the consumer knows must be
/// real — an expected member the provider lacks is a mismatch.
#[test]
fn an_expected_union_member_the_provider_lacks_is_reported() {
    let schema = composed(
        consumer_expecting(json!({
            "kind": "entity",
            "name": "OrderRecord",
            "shape": [
                { "name": "status", "type": "string", "values": ["placed", "shipped"] }
            ]
        })),
        provider_with(
            json!([
                { "name": "id", "type": "string" },
                { "name": "status", "type": "string", "values": ["placed"] }
            ]),
            false,
        ),
    );
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()),
        "the missing union member must fail the subset rule, got {codes:?}"
    );
}

/// A fully satisfied composed program — identity expectation included —
/// produces no expectation codes at all.
#[test]
fn a_satisfied_composed_program_is_clean() {
    let schema = json!({
        "name": "App",
        "orbitals": [
            {
                "name": "ProductOrbital",
                "expects": [{
                    "kind": "identity",
                    "name": "Customer",
                    "shape": [
                        { "name": "role", "type": "string",
                          "values": ["store-manager", "operations-director"] }
                    ]
                }],
                "entity": {
                    "name": "Product",
                    "persistence": "persistent",
                    "collection": "products",
                    "fields": [{ "name": "id", "type": "string" }],
                    "create_policy": ["=", "@user.role", "store-manager"]
                },
                "traits": [],
                "pages": []
            },
            {
                "name": "CustomerOrbital",
                "entity": {
                    "name": "Customer",
                    "identity": true,
                    "persistence": "persistent",
                    "collection": "customers",
                    "fields": [
                        { "name": "id", "type": "string" },
                        { "name": "role", "type": "string",
                          "values": ["store-manager", "operations-director", "viewer"] }
                    ]
                },
                "traits": [],
                "pages": []
            }
        ]
    });
    let codes = run_full(schema);
    assert!(
        !codes
            .iter()
            .any(|c| c.starts_with("ORB_S_EXPECT") || c.starts_with("ORB_S_ACCESS_POLICY")),
        "a satisfied program must be clean of expectation codes, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// `expects` never declares
// ---------------------------------------------------------------------------

/// `expects entity [identity]` is not an identity declaration: a real [identity]
/// entity alongside expectations must not trip the uniqueness rule.
#[test]
fn expects_identity_never_counts_as_an_identity_declaration() {
    let schema = json!({
        "name": "App",
        "orbitals": [
            {
                "name": "CustomerOrbital",
                "entity": {
                    "name": "Customer",
                    "identity": true,
                    "persistence": "persistent",
                    "collection": "customers",
                    "fields": [
                        { "name": "id", "type": "string" },
                        { "name": "role", "type": "string", "values": ["admin"] }
                    ]
                },
                "traits": [],
                "pages": []
            },
            {
                "name": "ProductOrbital",
                "expects": [
                    { "kind": "identity", "name": "Customer",
                      "shape": [{ "name": "role", "type": "string", "values": ["admin"] }] },
                    { "kind": "identity" }
                ],
                "entity": {
                    "name": "Product",
                    "persistence": "persistent",
                    "collection": "products",
                    "fields": [{ "name": "id", "type": "string" }]
                },
                "traits": [],
                "pages": []
            }
        ]
    });
    let codes = run_full(schema);
    assert!(
        !codes.contains(&"ORB_S_IDENTITY_NOT_UNIQUE".to_string()),
        "expects never declares identity, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Regression: no-expects behavior is bit-for-bit today's
// ---------------------------------------------------------------------------

/// Without `expects`, an unknown `@user` field in a trait guard keeps today's
/// verdict (an error against the implicit default shape).
#[test]
fn without_expects_an_unknown_user_field_keeps_todays_verdict() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "ProductOrbital",
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [{
                "name": "Shell",
                "linkedEntity": "Product",
                "category": "interaction",
                "stateMachine": { "events": [], "states": [], "transitions": [{
                    "from": "idle", "event": "GO", "to": "idle",
                    "guard": ["=", "@user.avatar", "x"]
                }]}
            }],
            "pages": []
        }]
    });
    let codes = run_scoped(schema);
    assert!(
        codes.contains(&"ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()),
        "today's default-shape verdict must be unchanged, got {codes:?}"
    );
    assert!(
        !codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "no declaration, no self-consistency code, got {codes:?}"
    );
}

/// Without `expects`, a dangling relation target errors exactly as today.
#[test]
fn without_expects_a_dangling_relation_still_errors() {
    let schema = json!({
        "name": "App",
        "orbitals": [{
            "name": "ProductOrbital",
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [
                    { "name": "id", "type": "string" },
                    { "name": "customer", "type": "relation",
                      "relation": { "entity": "Customer", "cardinality": "one" } }
                ]
            },
            "traits": [],
            "pages": []
        }]
    });
    let codes = run_full(schema);
    assert!(
        codes.contains(&"ORB_E_INVALID_RELATION".to_string()),
        "removing expects must restore today's relation error, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Authored surface vs imported bodies (G-EXPECT-DERIVE-IMPORTS)
// ---------------------------------------------------------------------------

/// Slice whose single trait carries a guard reading `@user.id` — a field the
/// declared shape does NOT carry. `imported` sets the inline phase's
/// `sourceBehavior` provenance marker.
fn slice_with_user_id_read(imported: bool) -> Value {
    let mut trait_def = json!({
        "name": "ProductBrowseList",
        "linkedEntity": "Product",
        "category": "interaction",
        "stateMachine": {
            "events": [],
            "states": [],
            "transitions": [{
                "from": "idle", "event": "LOAD", "to": "idle",
                "effects": [["set", "rows",
                    ["filter", "?rows", ["=", ["object/get", "@entity", "ownerId"], "@user.id"]]]]
            }]
        }
    });
    if imported {
        trait_def.as_object_mut().unwrap().insert(
            "sourceBehavior".to_string(),
            json!({ "behavior": "std-browse", "alias": "Gallery", "originalName": "BrowseItemBrowse" }),
        );
    }
    json!({
        "name": "App",
        "orbitals": [{
            "name": "ProductOrbital",
            "expects": [{
                "kind": "identity",
                "name": "Customer",
                "shape": [{ "name": "role", "type": "string" }]
            }],
            "entity": {
                "name": "Product",
                "persistence": "persistent",
                "collection": "products",
                "fields": [{ "name": "id", "type": "string" }]
            },
            "traits": [trait_def],
            "pages": []
        }]
    })
}

/// A `uses[]`-inlined body is the imported behavior's own contract: its
/// internal `@user.id` read must NOT be checked against the consumer's
/// derived declaration (derivation can only see the authored surface, so the
/// check would manufacture a mismatch on every BrowseList consumer).
#[test]
fn an_imported_bodys_user_reads_are_not_checked_against_the_declaration() {
    let codes = run_scoped(slice_with_user_id_read(true));
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "imported body must be skipped, got {codes:?}"
    );
}

/// The SAME read in a trait authored directly on the orbital is the declared
/// surface — still checked, still failing outside the shape.
#[test]
fn an_authored_traits_user_reads_are_checked_against_the_declaration() {
    let codes = run_scoped(slice_with_user_id_read(false));
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "authored reads stay self-consistent, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Imported expectations travel with the trait
//
// `uses` inlines individual TRAITS, never orbitals, so an organism's
// `expects entity [identity] { … }` is invisible to the app composing it unless the
// inline phase merges it onto the consuming orbital. Without that merge the
// contract is only ever checked when the organism is validated STANDALONE —
// where its own roster satisfies it trivially — and a composing app whose
// roster lacks a field the organism reads off `@user` validates clean.
//
// These fixtures assert the POST-inline shape (what the merge produces), which
// is what `validate_expectation_links` actually sees.
// ---------------------------------------------------------------------------

/// A composing app: its own roster, plus an expectation propagated from an
/// imported organism's orbital. `roster_has_id` toggles the only field the
/// imported organism needs.
fn composed_with_propagated_expectation(roster_has_id: bool) -> Value {
    let mut fields = vec![json!({ "name": "name", "type": "string", "required": true })];
    if roster_has_id {
        fields.insert(
            0,
            json!({ "name": "id", "type": "string", "required": true }),
        );
    }
    json!({
        "name": "App",
        "orbitals": [{
            "name": "HostOrbital",
            "entity": {
                "name": "Staff",
                "identity": true,
                "persistence": "persistent",
                "collection": "staff",
                "fields": fields
            },
            // Unnamed + shaped: resolves to whatever the program tagged
            // [identity], i.e. the HOST's roster — which is what lets one app
            // satisfy imports from organisms whose own rosters are named
            // differently.
            "expects": [{
                "kind": "identity",
                "shape": [{ "name": "id", "type": "string", "required": true }]
            }],
            "traits": [],
            "pages": []
        }]
    })
}

#[test]
fn a_propagated_expectation_checks_the_composing_apps_roster() {
    let codes = run_scoped(composed_with_propagated_expectation(false));
    assert!(
        codes.contains(&"ORB_S_EXPECTED_FIELD_NOT_PROVIDED".to_string()),
        "a host roster missing a field the imported organism reads must fail, got {codes:?}"
    );
}

#[test]
fn a_satisfied_propagated_expectation_is_clean() {
    let codes = run_scoped(composed_with_propagated_expectation(true));
    assert!(
        !codes.contains(&"ORB_S_EXPECTED_FIELD_NOT_PROVIDED".to_string()),
        "a roster carrying the expected field must pass, got {codes:?}"
    );
}

// =============================================================================
// derived ⊆ authored — an expectation is a LOWER BOUND
//
// `expects entity [identity] { shape }` says "the roster must give me AT LEAST this", so
// omitting fields the provider happens to have is legal and intended. Declaring
// FEWER than the orbital actually reads is not: a roster satisfying the
// declaration exactly would not carry the field, and the slice reads undefined.
//
// The gap these pin: when a real `[identity]` entity exists it WINS the type
// environment, so `@user` reads resolve against the roster — which knows more
// than the declaration — and the declaration is never compared to anything. It
// only becomes the type environment once the orbital is sliced. That is how 219
// blocks declaring `{ role : string }` while reading `@user.email` shipped.
// =============================================================================

fn orbital_reading(user_field: &str, declared: Value) -> Value {
    json!({
        "name": "app",
        "orbitals": [
            {
                "name": "RosterOrbital",
                "entity": { "name": "Person", "identity": true, "persistence": "persistent",
                            "collection": "people",
                            "fields": [{ "name": "id", "type": "string", "required": true },
                                       { "name": "name", "type": "string" },
                                       { "name": "email", "type": "string" },
                                       { "name": "role", "type": "string" }] },
                "traits": [], "pages": []
            },
            {
                "name": "ConsumerOrbital",
                "expects": [declared],
                "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs",
                            "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "DocView", "linkedEntity": "Doc",
                    "category": "interaction",
                    "stateMachine": { "events": [], "states": [], "transitions": [{
                        "from": "idle", "event": "OPEN", "to": "idle",
                        "guard": ["=", user_field, "admin"]
                    }]}
                }],
                "pages": []
            }
        ]
    })
}

#[test]
fn under_declared_identity_expectation_fails_even_when_a_real_roster_exists() {
    let codes = run_scoped(orbital_reading(
        "@user.email",
        json!({ "kind": "identity", "name": "Person",
                "shape": [{ "name": "role", "type": "string" }] }),
    ));
    assert!(
        codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "reading @user.email while declaring only `role` must fail, got: {codes:?}"
    );
}

#[test]
fn identity_expectation_covering_the_read_passes() {
    let codes = run_scoped(orbital_reading(
        "@user.email",
        json!({ "kind": "identity", "name": "Person",
                "shape": [{ "name": "email", "type": "string" }] }),
    ));
    assert!(
        !codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "a declaration covering the read must pass, got: {codes:?}"
    );
}

#[test]
fn over_declaring_is_legal_because_the_shape_is_a_minimum() {
    let codes = run_scoped(orbital_reading(
        "@user.email",
        json!({ "kind": "identity", "name": "Person",
                "shape": [{ "name": "email", "type": "string" },
                          { "name": "role",  "type": "string" },
                          { "name": "name",  "type": "string" }] }),
    ));
    assert!(
        !codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "over-declaring asks MORE of the provider, not less — must pass, got: {codes:?}"
    );
}

#[test]
fn shapeless_identity_expectation_declares_no_minimum_and_opts_out() {
    let codes = run_scoped(orbital_reading(
        "@user.email",
        json!({ "kind": "identity" }),
    ));
    assert!(
        !codes.contains(&"ORB_S_EXPECTATION_FIELD_UNDECLARED".to_string()),
        "an existence-only declaration has no lower bound to under-declare, got: {codes:?}"
    );
}

fn provider_trait(emits: Value) -> Value {
    json!({
        "name": "OrderOrbital",
        "entity": { "name": "OrderRecord", "persistence": "persistent", "collection": "orders", "fields": [{ "name": "id", "type": "string" }] },
        "traits": [{
            "name": "OrderFeed",
            "linkedEntity": "OrderRecord",
            "emits": emits,
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "INIT", "name": "Init" }], "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }] }
        }],
        "pages": []
    })
}

fn unsatisfied(schema: Value) -> Vec<String> {
    run_scoped(schema)
        .into_iter()
        .filter(|c| c == "ORB_S_EXPECTATION_UNSATISFIED")
        .collect()
}

/// `expects trait Trait { emits { EVENT } }`: a present trait of that name must emit the event.
#[test]
fn an_expected_event_the_present_provider_trait_does_not_emit_is_unsatisfied() {
    let consumer = consumer_expecting(
        json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" }),
    );
    assert_eq!(
        unsatisfied(composed(
            consumer,
            provider_trait(json!([{ "event": "ORDER_SHIPPED" }]))
        ))
        .len(),
        1
    );
}

#[test]
fn control_an_expected_event_the_provider_trait_emits_is_satisfied() {
    let consumer = consumer_expecting(
        json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" }),
    );
    assert!(unsatisfied(composed(
        consumer,
        provider_trait(json!([{ "event": "ORDER_PLACED" }]))
    ))
    .is_empty());
}

#[test]
fn control_an_expected_event_with_no_trait_of_that_name_is_deferred() {
    let consumer = consumer_expecting(
        json!({ "kind": "event", "traitName": "Elsewhere", "event": "ORDER_PLACED" }),
    );
    assert!(unsatisfied(composed(
        consumer,
        provider_trait(json!([{ "event": "ORDER_SHIPPED" }]))
    ))
    .is_empty());
}

// ---------------------------------------------------------------------------
// Strict obligations (a spec import): unprovided is an error, not deferred
// ---------------------------------------------------------------------------

fn strict(mut expectation: Value) -> Value {
    expectation["strict"] = json!(true);
    expectation
}

fn provider_page(path: &str) -> Value {
    json!({
        "name": "OrderOrbital",
        "entity": { "name": "OrderRecord", "persistence": "persistent", "collection": "orders", "fields": [{ "name": "id", "type": "string" }] },
        "traits": [{
            "name": "OrderView",
            "linkedEntity": "OrderRecord",
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "INIT", "name": "Init" }], "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }] }
        }],
        "pages": [{ "name": "OrderPage", "path": path, "traits": [{ "ref": "OrderView" }] }]
    })
}

#[test]
fn a_strict_entity_with_no_provider_is_unsatisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "entity", "name": "Ghost" })));
    assert_eq!(unsatisfied(composed(consumer, provider_with(json!([{ "name": "id", "type": "string" }]), false))).len(), 1);
}

#[test]
fn control_the_same_entity_expectation_without_strict_still_defers() {
    let consumer = consumer_expecting(json!({ "kind": "entity", "name": "Ghost" }));
    assert!(unsatisfied(composed(consumer, provider_with(json!([{ "name": "id", "type": "string" }]), false))).is_empty());
}

#[test]
fn a_strict_entity_a_sibling_provides_is_satisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "entity", "name": "OrderRecord" })));
    assert!(unsatisfied(composed(consumer, provider_with(json!([{ "name": "id", "type": "string" }]), false))).is_empty());
}

#[test]
fn a_strict_expectation_in_a_single_orbital_schema_is_not_trusted() {
    let schema = json!({ "name": "App", "orbitals": [consumer_expecting(strict(json!({ "kind": "entity", "name": "Ghost" })))] });
    assert_eq!(unsatisfied(schema).len(), 1);
}

#[test]
fn a_strict_identity_with_no_identity_entity_is_unsatisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "identity", "name": "Customer" })));
    assert_eq!(unsatisfied(composed(consumer, provider_with(json!([{ "name": "id", "type": "string" }]), false))).len(), 1);
}

#[test]
fn a_strict_event_with_no_trait_of_that_name_is_unsatisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "event", "traitName": "Elsewhere", "event": "PLACED" })));
    assert_eq!(unsatisfied(composed(consumer, provider_trait(json!([{ "event": "PLACED" }])))).len(), 1);
}

#[test]
fn a_strict_event_the_named_trait_emits_is_satisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" })));
    assert!(unsatisfied(composed(consumer, provider_trait_firing(true))).is_empty());
}

#[test]
fn a_strict_page_no_orbital_declares_is_unsatisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "page", "path": "/orders/:id" })));
    assert_eq!(unsatisfied(composed(consumer, provider_page("/other"))).len(), 1);
}

#[test]
fn a_strict_page_a_sibling_declares_is_satisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "page", "path": "/orders/:id" })));
    assert!(unsatisfied(composed(consumer, provider_page("/orders/:id"))).is_empty());
}

#[test]
fn control_a_lenient_page_with_no_provider_defers() {
    let consumer = consumer_expecting(json!({ "kind": "page", "path": "/orders/:id" }));
    assert!(unsatisfied(composed(consumer, provider_page("/other"))).is_empty());
}

#[test]
fn final_validation_now_reports_an_unprovided_event_and_page() {
    let event = composed(
        consumer_expecting(json!({ "kind": "event", "traitName": "Elsewhere", "event": "PLACED" })),
        provider_trait(json!([{ "event": "PLACED" }])),
    );
    assert_eq!(expectations::validate_final_expectations(&parse(event)).errors.len(), 1);
    let page = composed(consumer_expecting(json!({ "kind": "page", "path": "/orders/:id" })), provider_page("/other"));
    assert_eq!(expectations::validate_final_expectations(&parse(page)).errors.len(), 1);
    let ok = composed(consumer_expecting(json!({ "kind": "page", "path": "/orders/:id" })), provider_page("/orders/:id"));
    assert!(expectations::validate_final_expectations(&parse(ok)).errors.is_empty());
}

fn spec_orbital() -> Value {
    json!({
        "name": "ShopSpec",
        "spec": true,
        "expects": [
            { "kind": "entity", "name": "Order", "strict": true, "shape": [{ "name": "total", "type": "number" }] },
            { "kind": "event", "traitName": "Checkout", "event": "PLACED", "strict": true },
            { "kind": "page", "path": "/orders", "strict": true }
        ],
        "entity": { "name": "ShopSpec", "fields": [] },
        "traits": []
    })
}

#[test]
fn a_spec_orbital_validates_clean_on_its_own() {
    let schema = json!({ "name": "ShopSpec", "orbitals": [spec_orbital()] });
    assert_eq!(run_full(schema), Vec::<String>::new());
}

#[test]
fn control_an_orbital_with_a_real_entity_and_strict_expectations_is_still_checked() {
    let mut orbital = consumer_expecting(strict(json!({ "kind": "entity", "name": "Ghost" })));
    orbital["traits"] = json!([]);
    let schema = json!({ "name": "App", "orbitals": [orbital] });
    assert_eq!(unsatisfied(schema).len(), 1);
}

// ---------------------------------------------------------------------------
// `expects trait T { emits { E { payload } } }`: the provider's payload is link-checked
// ---------------------------------------------------------------------------

fn emits_placed(payload: Value) -> Value {
    provider_trait(json!([{ "event": "ORDER_PLACED", "payloadSchema": payload }]))
}

fn expecting_payload(payload: Value) -> Value {
    consumer_expecting(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED", "payload": payload }))
}

fn codes(schema: Value) -> Vec<String> {
    run_scoped(schema)
}

#[test]
fn an_event_payload_the_provider_carries_is_satisfied() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "string", "required": true }])),
        emits_placed(json!([{ "name": "orderId", "type": "string", "required": true }, { "name": "total", "type": "number" }])),
    );
    assert_eq!(codes(schema), Vec::<String>::new());
}

#[test]
fn an_event_payload_field_the_provider_lacks_is_not_provided() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "string" }, { "name": "extra", "type": "string" }])),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(codes(schema), vec!["ORB_S_EXPECTED_FIELD_NOT_PROVIDED".to_string()]);
}

#[test]
fn an_event_payload_field_of_another_type_is_a_mismatch() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "number" }])),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(codes(schema), vec!["ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()]);
}

#[test]
fn a_required_event_payload_field_the_provider_makes_optional_is_a_mismatch() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "string", "required": true }])),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(codes(schema), vec!["ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()]);
}

#[test]
fn control_an_event_expectation_without_a_payload_ignores_the_providers_payload() {
    let schema = composed(
        consumer_expecting(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" })),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(codes(schema), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// A strict event obligation must be FIRED, not merely declared
// ---------------------------------------------------------------------------

fn provider_trait_firing(fires: bool) -> Value {
    let effects = if fires { json!([["emit", "ORDER_PLACED"]]) } else { json!([]) };
    json!({
        "name": "OrderOrbital",
        "entity": { "name": "OrderRecord", "persistence": "persistent", "collection": "orders", "fields": [{ "name": "id", "type": "string" }] },
        "traits": [{
            "name": "OrderFeed",
            "linkedEntity": "OrderRecord",
            "emits": [{ "event": "ORDER_PLACED", "scope": "external" }],
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT", "name": "Init" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
            }
        }],
        "pages": []
    })
}

#[test]
fn a_strict_event_the_trait_declares_but_never_fires_is_unsatisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" })));
    assert_eq!(unsatisfied(composed(consumer, provider_trait_firing(false))).len(), 1);
}

#[test]
fn a_strict_event_the_trait_fires_is_satisfied() {
    let consumer = consumer_expecting(strict(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" })));
    assert!(unsatisfied(composed(consumer, provider_trait_firing(true))).is_empty());
}

#[test]
fn control_a_lenient_event_the_trait_never_fires_is_not_reported() {
    let consumer = consumer_expecting(json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" }));
    assert!(unsatisfied(composed(consumer, provider_trait_firing(false))).is_empty());
}

// ---------------------------------------------------------------------------
// Entity storage/marks/access, page traits, trait states and edges
// ---------------------------------------------------------------------------

fn flow_provider(entity_extra: Value, persists: bool) -> Value {
    let mut entity = json!({
        "name": "OrderRecord", "persistence": "persistent", "collection": "orders",
        "fields": [{ "name": "id", "type": "string", "required": true, "primaryKey": true }, { "name": "email", "type": "string", "unique": true }]
    });
    for (k, v) in entity_extra.as_object().cloned().unwrap_or_default() {
        entity[k] = v;
    }
    let effects = if persists { json!([["persist", "create", "OrderRecord", { "id": "@payload.id" }]]) } else { json!([]) };
    json!({
        "name": "OrderOrbital",
        "entity": entity,
        "traits": [{
            "name": "OrderFlow",
            "linkedEntity": "OrderRecord",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }, { "name": "paid" }],
                "events": [{ "key": "INIT", "name": "Init" }, { "key": "PAY", "name": "Pay" }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT" },
                    { "from": "idle", "to": "paid", "event": "PAY", "effects": effects }
                ]
            }
        }],
        "pages": [{ "name": "OrderPage", "path": "/orders", "traits": [{ "ref": "OrderFlow" }] }]
    })
}

fn expecting(expectation: Value, provider: Value) -> Vec<String> {
    run_scoped(composed(consumer_expecting(expectation), provider))
}

const MISMATCH: &str = "ORB_S_EXPECTATION_PROVIDER_MISMATCH";
const UNSATISFIED: &str = "ORB_S_EXPECTATION_UNSATISFIED";

#[test]
fn an_entity_expectation_checks_the_storage_mode_shared_and_local() {
    let ok = json!({ "kind": "entity", "name": "OrderRecord", "persistence": "persistent" });
    assert_eq!(expecting(ok, flow_provider(json!({}), false)), Vec::<String>::new());
    let wrong = json!({ "kind": "entity", "name": "OrderRecord", "persistence": "runtime" });
    assert_eq!(expecting(wrong, flow_provider(json!({}), false)), vec![MISMATCH.to_string()]);
    let shared = json!({ "kind": "entity", "name": "OrderRecord", "shared": true });
    assert_eq!(expecting(shared.clone(), flow_provider(json!({}), false)), vec![MISMATCH.to_string()]);
    assert_eq!(expecting(shared, flow_provider(json!({ "shared": true }), false)), Vec::<String>::new());
    let local = json!({ "kind": "entity", "name": "OrderRecord", "local": true });
    assert_eq!(expecting(local, flow_provider(json!({}), false)), vec![MISMATCH.to_string()]);
}

#[test]
fn control_an_entity_expectation_that_states_no_storage_constrains_none() {
    let bare = json!({ "kind": "entity", "name": "OrderRecord" });
    assert_eq!(expecting(bare, flow_provider(json!({ "persistence": "runtime", "shared": true }), false)), Vec::<String>::new());
}

#[test]
fn an_entity_expectation_checks_key_and_unique_marks() {
    let ok = json!({ "kind": "entity", "name": "OrderRecord", "shape": [{ "name": "id", "type": "string", "primaryKey": true }, { "name": "email", "type": "string", "unique": true }] });
    assert_eq!(expecting(ok, flow_provider(json!({}), false)), Vec::<String>::new());
    let unmarked = json!({ "kind": "entity", "name": "OrderRecord", "shape": [{ "name": "email", "type": "string", "unique": true }] });
    let mut provider = flow_provider(json!({}), false);
    provider["entity"]["fields"][1]["unique"] = json!(false);
    assert_eq!(expecting(unmarked, provider), vec![MISMATCH.to_string()]);
    let keyed = json!({ "kind": "entity", "name": "OrderRecord", "shape": [{ "name": "id", "type": "string", "primaryKey": true }] });
    let mut unkeyed = flow_provider(json!({}), false);
    unkeyed["entity"]["fields"][0]["primaryKey"] = json!(false);
    assert_eq!(expecting(keyed, unkeyed), vec![MISMATCH.to_string()]);
    let control = json!({ "kind": "entity", "name": "OrderRecord", "shape": [{ "name": "email", "type": "string" }] });
    assert_eq!(expecting(control, flow_provider(json!({}), false)), Vec::<String>::new(), "a mark the spec does not state is not constrained");
}

#[test]
fn an_entity_expectation_compares_access_policies_structurally() {
    let policy = json!(["!=", "@entity.id", ""]);
    let ok = json!({ "kind": "entity", "name": "OrderRecord", "read_policy": policy });
    assert_eq!(expecting(ok.clone(), flow_provider(json!({ "read_policy": policy }), false)), Vec::<String>::new());
    assert_eq!(expecting(ok.clone(), flow_provider(json!({ "read_policy": ["==", "@entity.id", "x"] }), false)), vec![MISMATCH.to_string()]);
    assert_eq!(expecting(ok, flow_provider(json!({}), false)), vec![MISMATCH.to_string()], "a stated policy the provider lacks");
    let waived = json!({ "kind": "entity", "name": "OrderRecord", "access_waivers": { "update": "joins another entity" } });
    assert_eq!(expecting(waived.clone(), flow_provider(json!({ "access_waivers": { "update": "a different reason" } }), false)), Vec::<String>::new(), "the reason text is not compared");
    assert_eq!(expecting(waived, flow_provider(json!({}), false)), vec![MISMATCH.to_string()]);
}

#[test]
fn a_strict_persistent_entity_must_be_written_by_the_program() {
    let spec = strict(json!({ "kind": "entity", "name": "OrderRecord", "persistence": "persistent" }));
    assert_eq!(unsatisfied(composed(consumer_expecting(spec.clone()), flow_provider(json!({}), false))).len(), 1);
    assert!(unsatisfied(composed(consumer_expecting(spec), flow_provider(json!({}), true))).is_empty());
    let lenient = json!({ "kind": "entity", "name": "OrderRecord", "persistence": "persistent" });
    assert!(unsatisfied(composed(consumer_expecting(lenient), flow_provider(json!({}), false))).is_empty(), "control: only an imported spec obliges a write");
}

#[test]
fn a_page_expectation_checks_the_traits_the_provider_page_mounts() {
    let ok = json!({ "kind": "page", "path": "/orders", "traits": ["OrderFlow"] });
    assert_eq!(expecting(ok, flow_provider(json!({}), false)), Vec::<String>::new());
    let wrong = json!({ "kind": "page", "path": "/orders", "traits": ["Receipt"] });
    assert_eq!(expecting(wrong, flow_provider(json!({}), false)), vec![MISMATCH.to_string()]);
    let bare = json!({ "kind": "page", "path": "/orders" });
    assert_eq!(expecting(bare, flow_provider(json!({}), false)), Vec::<String>::new(), "control: no traits stated");
}

#[test]
fn a_state_expectation_needs_the_provider_trait_to_declare_it() {
    let ok = json!({ "kind": "state", "traitName": "OrderFlow", "state": "paid" });
    assert_eq!(expecting(ok, flow_provider(json!({}), false)), Vec::<String>::new());
    let missing = json!({ "kind": "state", "traitName": "OrderFlow", "state": "refunded" });
    assert_eq!(expecting(missing, flow_provider(json!({}), false)), vec![UNSATISFIED.to_string()]);
    let absent = json!({ "kind": "state", "traitName": "Elsewhere", "state": "paid" });
    assert_eq!(expecting(absent.clone(), flow_provider(json!({}), false)), Vec::<String>::new(), "a lenient state of an absent trait defers");
    assert_eq!(expecting(strict(absent), flow_provider(json!({}), false)).len(), 1);
}

#[test]
fn a_transition_expectation_needs_that_edge_in_the_provider_trait() {
    let edge = |from: &str, event: &str, to: &str| json!({ "kind": "transition", "traitName": "OrderFlow", "from": from, "event": event, "to": to });
    assert_eq!(expecting(edge("idle", "PAY", "paid"), flow_provider(json!({}), false)), Vec::<String>::new());
    assert_eq!(expecting(edge("idle", "PAY", "idle"), flow_provider(json!({}), false)), vec![UNSATISFIED.to_string()]);
    assert_eq!(expecting(edge("paid", "PAY", "paid"), flow_provider(json!({}), false)), vec![UNSATISFIED.to_string()]);
    assert_eq!(expecting(edge("idle", "NOPE", "paid"), flow_provider(json!({}), false)), vec![UNSATISFIED.to_string()]);
}

// ---------------------------------------------------------------------------
// Spec propositions: `@invariant` / `@holds` are proved by `orb validate`
// ---------------------------------------------------------------------------

fn paying_program(guard: Option<Value>, invariant: Value) -> Value {
    let mut pay = json!({ "from": "open", "to": "paid", "event": "PAY",
        "effects": [["persist", "update", "Order", "@entity.id", { "paid": ["+", "@entity.paid", "@payload.amount"] }],
                    ["set", "@entity.status", "paid"]] });
    if let Some(g) = guard {
        pay["guard"] = g;
    }
    json!({ "name": "App", "orbitals": [
        {
            "name": "ShopSpec", "spec": true, "entity": { "name": "ShopSpec", "fields": [] }, "traits": [],
            "expects": [
                { "kind": "entity", "name": "Order", "strict": true, "invariants": [invariant] },
                { "kind": "state", "traitName": "Checkout", "state": "paid", "strict": true, "holds": ["=", "@entity.status", "paid"] }
            ]
        },
        {
            "name": "Shop",
            "expects": [
                { "kind": "entity", "name": "Order", "strict": true, "invariants": [invariant] },
                { "kind": "state", "traitName": "Checkout", "state": "paid", "strict": true, "holds": ["=", "@entity.status", "paid"] }
            ],
            "entity": { "name": "Order", "persistence": "persistent", "collection": "orders", "fields": [
                { "name": "id", "type": "string", "required": true },
                { "name": "total", "type": "number", "required": true },
                { "name": "paid", "type": "number" },
                { "name": "status", "type": "enum", "values": ["open", "paid"], "default": "open" }
            ] },
            "traits": [{
                "name": "Checkout", "linkedEntity": "Order",
                "stateMachine": {
                    "states": [{ "name": "open", "isInitial": true }, { "name": "paid" }],
                    "events": [{ "key": "INIT", "name": "Init" },
                               { "key": "PAY", "name": "Pay", "payloadSchema": [{ "name": "amount", "type": "number", "required": true }] }],
                    "transitions": [{ "from": "open", "to": "open", "event": "INIT" }, pay]
                }
            }],
            "pages": []
        }
    ] })
}

fn proposition_codes(schema: Value) -> Vec<String> {
    let mut ctx = ValidationContext::new();
    expectations::validate_expectations(&parse(schema), &mut ctx);
    ctx.result.errors.iter().map(|e| e.code.as_str().to_string()).filter(|c| c.starts_with("ORB_S_INVARIANT")).collect()
}

#[test]
fn a_program_that_keeps_its_spec_propositions_is_clean() {
    let guard = json!(["<=", ["+", "@entity.paid", "@payload.amount"], "@entity.total"]);
    assert_eq!(proposition_codes(paying_program(Some(guard), json!(["<=", "@entity.paid", "@entity.total"]))), Vec::<String>::new());
}

#[test]
fn a_write_that_breaks_an_invariant_is_violated_with_a_counterexample() {
    let schema = parse(paying_program(None, json!(["<=", "@entity.paid", "@entity.total"])));
    let mut ctx = ValidationContext::new();
    expectations::validate_expectations(&schema, &mut ctx);
    let violated: Vec<_> = ctx.result.errors.iter().filter(|e| e.code.as_str() == "ORB_S_INVARIANT_VIOLATED").collect();
    assert_eq!(violated.len(), 1, "{:#?}", ctx.result.errors);
    assert!(violated[0].message.contains("counterexample") && violated[0].message.contains("Checkout: open --PAY--> paid"), "{}", violated[0].message);
}

#[test]
fn an_invariant_outside_the_fragment_is_unproved_never_passed() {
    let guard = json!(["<=", ["+", "@entity.paid", "@payload.amount"], "@entity.total"]);
    let codes = proposition_codes(paying_program(Some(guard), json!([">=", ["math/sqrt", "@entity.paid"], 0])));
    assert_eq!(codes, vec!["ORB_S_INVARIANT_UNPROVED".to_string()]);
}
