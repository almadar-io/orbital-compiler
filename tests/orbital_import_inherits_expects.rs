//! An orbital import carries its upstream's `expects`: the imported traits still read what the
//! upstream declared it reads (`@user` under an access policy, a sibling entity), so whatever
//! composes the import owes the same. Without the upstream expectation, the policy is reported.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::validate_comprehensive;

fn upstream(expects: &str) -> String {
    format!(
        r#"{{
  "name": "test-expects-upstream",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "ProductOrbital",
      {expects}
      "entity": {{
        "name": "Product",
        "persistence": "persistent",
        "create_policy": ["=", ["object/get", "@user", "role"], "manager"],
        "fields": [ {{ "name": "id", "type": "string", "required": true }}, {{ "name": "name", "type": "string" }} ]
      }},
      "traits": [
        {{
          "name": "ProductRules",
          "linkedEntity": "Product",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "ready", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [ {{ "from": "ready", "to": "ready", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "stack" }}] ] }} ]
          }}
        }}
      ],
      "pages": [ {{ "name": "ProductPage", "path": "/products", "traits": [ {{ "ref": "ProductRules" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn consumer(own_expects: &str) -> String {
    CONSUMER.replace(r#""traits": [],"#, &format!(r#"{own_expects} "traits": [],"#))
}

const CONSUMER: &str = r#"{
  "name": "test-expects-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Shop",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ProductOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ProductOrbital", "entity": "Product", "pages": { "/products": "/shop" } }
    }
  ]
}"#;

fn identity_policy_errors(expects: &str) -> Vec<String> {
    identity_policy_errors_with(expects, "")
}

fn identity_policy_errors_with(expects: &str, own_expects: &str) -> Vec<String> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream(expects)).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(own_expects)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    validate_comprehensive(&schema)
        .errors
        .iter()
        .filter(|e| e.code.as_str() == "ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY")
        .map(|e| e.message.clone())
        .collect()
}

#[test]
fn an_import_inherits_the_upstream_identity_expectation() {
    let errors = identity_policy_errors(
        r#""expects": [ { "kind": "identity", "name": "Customer", "shape": [ { "name": "role", "type": "string", "values": ["customer", "manager"] } ] } ],"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn control_an_upstream_without_the_expectation_still_reports_the_policy() {
    let errors = identity_policy_errors("");
    assert!(!errors.is_empty());
}

const NAMED_CUSTOMER: &str = r#""expects": [ { "kind": "identity", "name": "Customer", "shape": [ { "name": "role", "type": "string", "values": ["customer", "manager"] } ] } ],"#;

#[test]
fn a_named_inherited_identity_supersedes_a_bare_local_one() {
    let errors = identity_policy_errors_with(NAMED_CUSTOMER, r#""expects": [ { "kind": "identity" } ],"#);
    assert!(errors.is_empty(), "{errors:?}");
}

fn inlined_identities(expects: &str, own_expects: &str) -> Vec<(Option<String>, Vec<String>)> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream(expects)).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(own_expects)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema.orbitals[0]
        .expects
        .iter()
        .flatten()
        .filter_map(|e| match e {
            orbital_core::schema::ExpectDeclaration::Identity { name, shape } => Some((
                name.clone(),
                shape
                    .iter()
                    .flatten()
                    .filter_map(|f| f.name.clone())
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn an_inherited_identity_merges_into_the_local_one() {
    let identities = inlined_identities(
        NAMED_CUSTOMER,
        r#""expects": [ { "kind": "identity", "name": "Customer", "shape": [ { "name": "name", "type": "string" } ] } ],"#,
    );
    assert_eq!(
        identities,
        vec![(Some("Customer".to_string()), vec!["name".to_string(), "role".to_string()])]
    );
}

#[test]
fn a_bare_local_identity_takes_the_inherited_name() {
    let identities = inlined_identities(NAMED_CUSTOMER, r#""expects": [ { "kind": "identity" } ],"#);
    assert_eq!(identities, vec![(Some("Customer".to_string()), vec!["role".to_string()])]);
}

#[test]
fn control_a_local_name_is_never_replaced_by_the_upstream_one() {
    let identities = inlined_identities(
        NAMED_CUSTOMER,
        r#""expects": [ { "kind": "identity", "name": "Member" } ],"#,
    );
    assert_eq!(identities, vec![(Some("Member".to_string()), vec!["role".to_string()])]);
}

fn inherited_role_values(role_values: &str, roles: &str) -> Vec<String> {
    let expects = format!(
        r#""expects": [ {{ "kind": "identity", "name": "Customer", "shape": [ {{ "name": "role", "type": "string", "values": [{role_values}] }} ] }} ],"#
    );
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream(&expects)).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    let consumer = CONSUMER.replace(
        r#""pages": { "/products": "/shop" } }"#,
        &format!(r#""pages": {{ "/products": "/shop" }}{roles} }}"#),
    );
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema.orbitals[0]
        .expects
        .iter()
        .flatten()
        .find_map(|e| match e {
            orbital_core::schema::ExpectDeclaration::Identity { shape, .. } => shape
                .iter()
                .flatten()
                .find(|f| f.name.as_deref() == Some("role"))
                .and_then(|f| f.values.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn an_inherited_role_vocabulary_follows_the_import_role_remap() {
    let values = inherited_role_values(
        r#""customer", "manager""#,
        r#", "roles": { "manager": ["owner", "project_manager"], "customer": ["client"] }"#,
    );
    assert_eq!(values, vec!["client", "owner", "project_manager"]);
}

#[test]
fn an_unmapped_role_literal_is_kept() {
    let values = inherited_role_values(
        r#""customer", "manager", "director""#,
        r#", "roles": { "manager": ["owner"] }"#,
    );
    assert_eq!(values, vec!["customer", "owner", "director"]);
}

#[test]
fn control_without_a_role_remap_the_vocabulary_is_inherited_as_is() {
    let values = inherited_role_values(r#""customer", "manager""#, "");
    assert_eq!(values, vec!["customer", "manager"]);
}

fn reconciled_consumer() -> serde_json::Value {
    serde_json::json!({
        "name": "Reconciled", "version": "1.0.0",
        "orbitals": [{
            "name": "Shop", "uses": [{"from": "./upstream.orb", "as": "Up"}],
            "entity": {"name": "Product", "collection": "products", "persistence": "persistent", "fields": [{"name": "id", "type": "string"}]},
            "expects": [
                {"kind": "entity", "name": "Customer", "shape": [{"name": "email", "type": "email"}]},
                {"kind": "identity", "name": "OnlineUser", "shape": [
                    {"name": "role", "type": "string", "values": ["customer", "manager"]},
                    {"name": "name", "type": "string"}
                ]}
            ],
            "traits": [{"ref": "Up.traits.ProductRules"}], "pages": [{"name":"ShopPage","path":"/shop","traits":[{"ref":"ProductRules"}]}]
        }, {
            "name": "Customers", "entity": {"name": "Customer", "collection": "customers", "persistence": "persistent", "fields": [{"name": "id", "type": "string"}, {"name": "email", "type": "email"}]},
            "traits": [], "pages": [{"name":"CustomersPage","path":"/customers","traits":[]}]
        }, {
            "name": "Viewers", "entity": {"name": "OnlineUser", "collection": "users", "identity": true, "persistence": "persistent", "fields": [
                {"name": "id", "type": "string"},
                {"name": "role", "type": "string", "values": ["customer", "manager", "moderator"]},
                {"name": "name", "type": "string", "required": true}
             ]}, "traits": [], "pages": [{"name":"ViewersPage","path":"/viewers","traits":[]}]
        }]
    })
}

fn inline_reconciled_consumer(
    mut input: serde_json::Value,
    whole_orbital: bool,
) -> orbital_core::schema::OrbitalSchema {
    if whole_orbital {
        input["orbitals"][0]["traits"] = serde_json::json!([]);
        input["orbitals"][0]["reference"] =
            serde_json::json!({"ref": "Up.orbitals.ProductOrbital", "entity": "Product"});
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let expectations = r#""expects": [{"kind":"identity","name":"Customer","shape":[{"name":"role","type":"string","values":["customer","manager"]},{"name":"name","type":"string","required":true}]}],"#;
    let mut source: serde_json::Value =
        serde_json::from_str(&upstream(expectations)).expect("upstream JSON");
    source["orbitals"][0]["entity"]["collection"] = serde_json::json!("products");
    std::fs::write(dir.path().join("upstream.orb"), source.to_string()).expect("write upstream");
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, input.to_string()).expect("write consumer");
    let mut schema = parse_file(&path).expect("parse");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline");
    schema
}

fn validate_reconciled_contracts(
    schema: &orbital_core::schema::OrbitalSchema,
) -> orbital_compiler::phases::validation::ValidationResult {
    use orbital_compiler::phases::validation::{
        entity_access, expectations, user_identity, ValidationContext,
    };
    let mut context = ValidationContext::new();
    user_identity::validate_user_identity(schema, &mut context);
    entity_access::validate_entity_access(schema, &mut context);
    expectations::validate_expectations(schema, &mut context);
    context.result
}

#[test]
fn explicit_reconciled_viewer_and_record_expectations_survive_both_import_paths() {
    use orbital_core::schema::ExpectDeclaration;
    for whole_orbital in [false, true] {
        let schema = inline_reconciled_consumer(reconciled_consumer(), whole_orbital);
        let declarations = schema.orbitals[0]
            .expects
            .as_ref()
            .expect("expects retained");
        assert_eq!(declarations.len(), 2);
        assert!(declarations.iter().any(|e| matches!(e, ExpectDeclaration::Entity { name, shape: Some(fields) } if name == "Customer" && fields.len() == 1 && fields[0].name.as_deref() == Some("email"))));
        let identity = declarations
            .iter()
            .find_map(|e| match e {
                ExpectDeclaration::Identity { name, shape } => Some((name, shape)),
                _ => None,
            })
            .expect("viewer expectation retained");
        assert_eq!(identity.0.as_deref(), Some("OnlineUser"));
        assert!(identity
            .1
            .as_ref()
            .expect("viewer shape")
            .iter()
            .any(|f| f.name.as_deref() == Some("name") && f.required == Some(true)));
        let result = validate_reconciled_contracts(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
}

#[test]
fn reconciliation_does_not_erase_viewer_or_record_contract_failures() {
    for whole_orbital in [false, true] {
        for mutation in [
            "identity-tag",
            "missing-field",
            "wrong-type",
            "wrong-role",
            "optional-required",
            "record-type",
            "genuine-named",
            "unrelated-record",
        ] {
            let mut input = reconciled_consumer();
            match mutation {
                "identity-tag" => {
                    input["orbitals"][2]["entity"]["identity"] = serde_json::json!(false)
                }
                "missing-field" => {
                    input["orbitals"][2]["entity"]["fields"][2]["name"] = serde_json::json!("other")
                }
                "wrong-type" => {
                    input["orbitals"][2]["entity"]["fields"][2]["type"] =
                        serde_json::json!("number")
                }
                "wrong-role" => {
                    input["orbitals"][2]["entity"]["fields"][1]["values"] =
                        serde_json::json!(["moderator"])
                }
                "optional-required" => {
                    input["orbitals"][2]["entity"]["fields"][2]["required"] =
                        serde_json::json!(false)
                }
                "record-type" => {
                    input["orbitals"][1]["entity"]["fields"][1]["type"] =
                        serde_json::json!("number")
                }
                "genuine-named" => {
                    input["orbitals"][0]["expects"][1]["name"] = serde_json::json!("Customer")
                }
                "unrelated-record" => {
                    input["orbitals"][0]["expects"] =
                        serde_json::json!([{"kind":"entity","name":"Other"}])
                }
                _ => unreachable!(),
            }
            let schema = inline_reconciled_consumer(input, whole_orbital);
            let result = validate_reconciled_contracts(&schema);
            assert!(
                result.errors.iter().any(|e| e.path.contains(".expects[")),
                "{whole_orbital}/{mutation}: {:?}",
                result.errors
            );
        }
    }
}

#[test]
fn an_absent_explicit_viewer_remains_a_deferred_library_requirement() {
    for whole_orbital in [false, true] {
        let mut input = reconciled_consumer();
        input["orbitals"].as_array_mut().expect("orbitals").pop();
        let schema = inline_reconciled_consumer(input, whole_orbital);
        let result = validate_reconciled_contracts(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
}
