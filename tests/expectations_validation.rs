//! `expects` — consumer-side requirement declarations (E3, proposal §5/§5.1).
//!
//! Standalone (single-orbital slice) expectations are TRUSTED: they satisfy
//! the identity rules and serve as the local type environment for
//! self-consistency checks. Multi-orbital schemas are the link step: every
//! expectation must be satisfied by exactly one provider, with shape
//! compatibility (width / `type_compat` / literal-union subset).

use orbital_compiler::phases::validation::{
    entity_access, expectations, user_identity, validate_comprehensive_without_services,
    ValidationContext,
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

fn composed(consumer: Value, provider: Value) -> Value {
    json!({ "name": "App", "orbitals": [consumer, provider] })
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

#[test]
fn record_contract_expectations_keep_presence_and_nullability_independent() {
    for (expected_nullable, actual_nullable, accepted) in [
        (false, true, false),
        (true, false, true),
        (true, true, true),
        (false, false, true),
    ] {
        let mut schema = composed(
            consumer_expecting(
                json!({ "kind": "entity", "name": "OrderRecord", "shape": [{ "name": "totalAmount", "type": "number", "required": true, "nullable": expected_nullable }] }),
            ),
            provider_with(
                json!([{ "name": "id", "type": "string" }, { "name": "totalAmount", "type": "number", "required": true, "nullable": actual_nullable }]),
                false,
            ),
        );
        schema["typeContractVersion"] = json!(1);
        let codes = run_scoped(schema);
        assert_eq!(
            !codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".into()),
            accepted,
            "expected nullable={expected_nullable}, actual nullable={actual_nullable}: {codes:?}"
        );
    }
}

#[test]
fn record_contract_expectations_check_typed_extras_after_fixed_fields() {
    let wanted = json!({ "name": "metadata", "type": "object", "nullable": false, "properties": { "id": { "type": "string", "required": true, "nullable": false } }, "additionalProperties": { "type": "number", "nullable": false } });
    for (extra_type, accepted) in [("number", true), ("boolean", false)] {
        let mut provided = wanted.clone();
        provided["properties"]["extra"] = json!({ "type": extra_type, "nullable": false });
        let mut schema = composed(
            consumer_expecting(
                json!({ "kind": "entity", "name": "OrderRecord", "shape": [wanted.clone()] }),
            ),
            provider_with(json!([{ "name": "id", "type": "string" }, provided]), false),
        );
        schema["typeContractVersion"] = json!(1);
        let codes = run_scoped(schema);
        assert_eq!(!codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".into()), accepted, "known fixed id:string takes precedence over numeric extras; extra={extra_type}: {codes:?}");
    }
}

#[test]
fn record_contract_expectations_preserve_nested_literal_and_catchall_constraints() {
    for catchall in [false, true] {
        let literal = json!({ "type": "enum", "values": ["ready"], "nullable": false });
        let mut wanted =
            json!({ "name": "metadata", "type": "object", "nullable": true, "properties": {} });
        if catchall {
            wanted["additionalProperties"] = literal;
        } else {
            wanted["properties"]["kind"] = literal;
        }
        for (member, accepted) in [("ready", true), ("wrong", false)] {
            let mut provided = wanted.clone();
            if catchall {
                provided["additionalProperties"]["values"] = json!([member]);
            } else {
                provided["properties"]["kind"]["values"] = json!([member]);
            }
            let mut schema = composed(
                consumer_expecting(
                    json!({ "kind": "entity", "name": "OrderRecord", "shape": [wanted.clone()] }),
                ),
                provider_with(json!([{ "name": "id", "type": "string" }, provided]), false),
            );
            schema["typeContractVersion"] = json!(1);
            let codes = run_scoped(schema);
            assert_eq!(
                !codes.contains(&"ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".into()),
                accepted,
                "nested vocabulary catchall={catchall}, member={member}: {codes:?}"
            );
        }
    }
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

fn emits_placed(payload: Value) -> Value {
    provider_trait(json!([{ "event": "ORDER_PLACED", "payloadSchema": payload }]))
}

fn expecting_payload(payload: Value) -> Value {
    consumer_expecting(
        json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED", "payload": payload }),
    )
}

fn codes(schema: Value) -> Vec<String> {
    run_scoped(schema)
}

#[test]
fn an_event_payload_the_provider_carries_is_satisfied() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "string", "required": true }])),
        emits_placed(
            json!([{ "name": "orderId", "type": "string", "required": true }, { "name": "total", "type": "number" }]),
        ),
    );
    assert_eq!(codes(schema), Vec::<String>::new());
}

#[test]
fn an_event_payload_field_of_another_type_is_a_mismatch() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "number" }])),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(
        codes(schema),
        vec!["ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()]
    );
}

#[test]
fn a_required_event_payload_field_the_provider_makes_optional_is_a_mismatch() {
    let schema = composed(
        expecting_payload(json!([{ "name": "orderId", "type": "string", "required": true }])),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(
        codes(schema),
        vec!["ORB_S_EXPECTED_FIELD_TYPE_MISMATCH".to_string()]
    );
}

#[test]
fn control_an_event_expectation_without_a_payload_ignores_the_providers_payload() {
    let schema = composed(
        consumer_expecting(
            json!({ "kind": "event", "traitName": "OrderFeed", "event": "ORDER_PLACED" }),
        ),
        emits_placed(json!([{ "name": "orderId", "type": "string" }])),
    );
    assert_eq!(codes(schema), Vec::<String>::new());
}

#[test]
fn record_contract_source_specs_keep_nullable_proofs_through_import_and_field_renames() {
    use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
    use orbital_core_eval::proof::{prove, Verdict};
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.lolo");
    std::fs::write(
        &base,
        r#"app nullable-writer [version: "1.0.0"]
orbital Base {
 entity Source [runtime] {
  id : string!
  value : number?!
 }
 trait Writer -> @rebindable Source {
  initial: ready
  state ready {
   ASK -> ready when (or (= ?value null) (>= ?value 0))
   (persist upsert Source { id: "row", value: ?value })
  }
  listens { ASK { value : number?! } @description "Write a nullable number." }
 }
}
"#,
    )
    .unwrap();
    let spec = directory.path().join("spec.lolo");
    std::fs::write(
        &spec,
        r#"app nullable-spec [version: "1.0.0"]
orbital Contract {
 expects entity Final {
  renamed : number?!
  @invariant (or (= @entity.renamed null) (>= @entity.renamed 0))
 }
}
"#,
    )
    .unwrap();
    for path in [&base, &spec] {
        let schema = orbital_test_support::lower_lolo_file(path).expect("source contract lowers");
        std::fs::write(
            path.with_extension("orb"),
            serde_json::to_vec(&schema).unwrap(),
        )
        .unwrap();
    }
    let app = directory.path().join("app.lolo");
    std::fs::write(
        &app,
        r#"app nullable-consumer [version: "1.0.0"]
uses Base from "./base.orb"
uses Contract from "./spec.orb"
orbital App {
 entity Final [runtime] {
  id : string!
  renamed : number?!
 }
 trait Bound = Base.traits.Writer -> Final { fields { value: renamed } events { ASK: GO } }
}
"#,
    )
    .unwrap();
    let mut schema = orbital_test_support::lower_lolo_file(&app).expect("consumer lowers");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&app)).expect("imports compose");
    let wire = serde_json::to_value(&schema).unwrap();
    let expects = wire["orbitals"][0]["expects"].as_array().unwrap();
    let expected = expects.iter().find(|e| e["name"] == "Final").unwrap();
    let field = expected["shape"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "renamed")
        .unwrap();
    assert_eq!(field["required"], true);
    assert_eq!(field["nullable"], true);
    let verdict = validate_comprehensive_without_services(&schema);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let findings = prove(&schema);
    assert!(!findings.is_empty(), "imported spec must owe a proof");
    assert!(
        findings
            .iter()
            .all(|finding| finding.verdict == Verdict::Proved),
        "{findings:?}"
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

// ---------------------------------------------------------------------------
// Authored surface vs imported bodies (G-EXPECT-DERIVE-IMPORTS)
// ---------------------------------------------------------------------------

/// Slice whose single trait carries a guard reading `@user.id` — a field the
/// declared shape does NOT carry. `imported` sets the inline phase's
/// `sourceBehavior` provenance marker.
fn slice_with_user_team_read(imported: bool) -> Value {
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
                    ["array/filter", "?rows", ["=", ["object/get", "@entity", "ownerId"], "@user.teamId"]]]]
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
/// internal `@user.teamId` read must NOT be checked against the consumer's
/// derived declaration (derivation can only see the authored surface, so the
/// check would manufacture a mismatch on every BrowseList consumer).
#[test]
fn an_imported_bodys_user_reads_are_not_checked_against_the_declaration() {
    let codes = run_scoped(slice_with_user_team_read(true));
    assert_eq!(
        codes,
        Vec::<String>::new(),
        "imported body must be skipped, got {codes:?}"
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
                                       { "name": "role", "type": "string" },
                                       { "name": "teamId", "type": "string" }] },
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
fn identity_expectation_covering_the_read_passes() {
    let codes = run_scoped(orbital_reading(
        "@user.teamId",
        json!({ "kind": "identity", "name": "Person",
                "shape": [{ "name": "teamId", "type": "string" }] }),
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
fn final_validation_now_reports_an_unprovided_event_and_page() {
    let event = composed(
        consumer_expecting(json!({ "kind": "event", "traitName": "Elsewhere", "event": "PLACED" })),
        provider_trait(json!([{ "event": "PLACED" }])),
    );
    assert_eq!(
        expectations::validate_final_expectations(&parse(event))
            .errors
            .len(),
        1
    );
    let page = composed(
        consumer_expecting(json!({ "kind": "page", "path": "/orders/:id" })),
        provider_page("/other"),
    );
    assert_eq!(
        expectations::validate_final_expectations(&parse(page))
            .errors
            .len(),
        1
    );
    let ok = composed(
        consumer_expecting(json!({ "kind": "page", "path": "/orders/:id" })),
        provider_page("/orders/:id"),
    );
    assert!(expectations::validate_final_expectations(&parse(ok))
        .errors
        .is_empty());
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
    ctx.result
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .filter(|c| c.starts_with("ORB_S_INVARIANT"))
        .collect()
}

#[test]
fn a_program_that_keeps_its_spec_propositions_is_clean() {
    let guard = json!([
        "<=",
        ["+", "@entity.paid", "@payload.amount"],
        "@entity.total"
    ]);
    assert_eq!(
        proposition_codes(paying_program(
            Some(guard),
            json!(["<=", "@entity.paid", "@entity.total"])
        )),
        Vec::<String>::new()
    );
}

#[test]
fn a_write_that_breaks_an_invariant_is_violated_with_a_counterexample() {
    let schema = parse(paying_program(
        None,
        json!(["<=", "@entity.paid", "@entity.total"]),
    ));
    let mut ctx = ValidationContext::new();
    expectations::validate_expectations(&schema, &mut ctx);
    let violated: Vec<_> = ctx
        .result
        .errors
        .iter()
        .filter(|e| e.code.as_str() == "ORB_S_INVARIANT_VIOLATED")
        .collect();
    assert_eq!(violated.len(), 1, "{:#?}", ctx.result.errors);
    assert!(
        violated[0].message.contains("counterexample")
            && violated[0].message.contains("Checkout: open --PAY--> paid"),
        "{}",
        violated[0].message
    );
}

#[test]
fn an_invariant_outside_the_fragment_is_unproved_never_passed() {
    let guard = json!([
        "<=",
        ["+", "@entity.paid", "@payload.amount"],
        "@entity.total"
    ]);
    let codes = proposition_codes(paying_program(
        Some(guard),
        json!([">=", ["math/sqrt", "@entity.paid"], 0]),
    ));
    assert_eq!(codes, vec!["ORB_S_INVARIANT_UNPROVED".to_string()]);
}

fn self_marked(name: &str, own: bool) -> Value {
    let mut decl = json!({ "kind": "entity", "name": name });
    if own {
        decl["self"] = json!(true);
    }
    decl
}

/// `expects entity N [self]` is checked in every validate, standalone or composed: the orbital's own body must declare N.
#[test]
fn a_self_expectation_needs_its_own_body_to_declare_the_entity() {
    let missing = run_scoped(
        json!({ "name": "App", "orbitals": [consumer_expecting(self_marked("Ghost", true))] }),
    );
    assert!(
        missing.contains(&"ORB_S_EXPECTATION_UNSATISFIED".to_string()),
        "expected the unsatisfied finding, got {missing:?}"
    );
    let named_by_sibling_only = run_scoped(composed(
        consumer_expecting(self_marked("OrderRecord", true)),
        provider_with(json!([{ "name": "id", "type": "string" }]), false),
    ));
    assert!(
        named_by_sibling_only.contains(&"ORB_S_EXPECTATION_UNSATISFIED".to_string()),
        "a sibling's entity does not satisfy [self]: {named_by_sibling_only:?}"
    );
}

#[test]
fn a_self_expectation_the_body_satisfies_is_clean_and_an_unmarked_one_stays_deferred() {
    let own = run_scoped(
        json!({ "name": "App", "orbitals": [consumer_expecting(self_marked("Cart", true))] }),
    );
    assert!(
        !own.contains(&"ORB_S_EXPECTATION_UNSATISFIED".to_string()),
        "control: the body declares Cart: {own:?}"
    );
    let deferred = run_scoped(
        json!({ "name": "App", "orbitals": [consumer_expecting(self_marked("Ghost", false))] }),
    );
    assert!(
        !deferred.contains(&"ORB_S_EXPECTATION_UNSATISFIED".to_string()),
        "control: an unmarked expectation stays deferred to --final: {deferred:?}"
    );
}
