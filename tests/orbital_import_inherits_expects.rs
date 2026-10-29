//! An orbital import carries its upstream's `expects`: the imported traits still read what the
//! upstream declared it reads (`@user` under an access policy, a sibling entity), so whatever
//! composes the import owes the same.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::validate_comprehensive_without_services;

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

const NAMED_CUSTOMER: &str = r#""expects": [ { "kind": "identity", "name": "Customer", "shape": [ { "name": "role", "type": "string", "values": ["customer", "manager"] } ] } ],"#;

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
            orbital_core_base::schema::ExpectDeclaration::Identity { name, shape, .. } => Some((
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
            orbital_core_base::schema::ExpectDeclaration::Identity { shape, .. } => shape
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
) -> orbital_core_base::schema::OrbitalSchema {
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

const SPEC: &str = r#"{
  "name": "shop-spec",
  "version": "1.0.0",
  "orbitals": [{
    "name": "ShopSpec",
    "spec": true,
    "expects": [__EXPECTS__],
    "entity": { "name": "ShopSpec", "fields": [] },
    "traits": []
  }]
}"#;

const SPEC_CONSUMER: &str = r#"{
  "name": "shop",
  "version": "1.0.0",
  "orbitals": [{
    "name": "Shop",
    "uses": [ { "from": "./spec.orb", "as": "Spec" } ],
    "entity": {
      "name": "Order", "persistence": "persistent", "collection": "orders",
      "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "total", "type": "number" } ]
    },
    "traits": [{
      "name": "OrderView", "linkedEntity": "Order", 
      "stateMachine": {
        "states": [ { "name": "ready", "isInitial": true } ],
        "events": [ { "key": "INIT" } ],
        "transitions": [ { "from": "ready", "to": "ready", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "stack" }] ] } ]
      }
    }],
    "pages": [ { "name": "OrderPage", "path": "/orders", "traits": [ { "ref": "OrderView" } ] } ]
  }]
}"#;

fn spec_import_codes(spec_expects: &str) -> Vec<String> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("spec.orb"), SPEC.replace("__EXPECTS__", spec_expects)).expect("write spec");
    let path = dir.path().join("shop.orb");
    std::fs::write(&path, SPEC_CONSUMER).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    validate_comprehensive_without_services(&schema)
        .errors
        .iter()
        .map(|e| format!("{} {}", e.code.as_str(), e.message))
        .collect()
}

#[test]
fn an_imported_spec_obligation_with_the_wrong_field_type_is_a_mismatch() {
    let codes = spec_import_codes(
        r#"{ "kind": "entity", "name": "Order", "strict": true, "shape": [ { "name": "total", "type": "string" } ] }"#,
    );
    assert!(codes.iter().any(|c| c.starts_with("ORB_S_EXPECTED_FIELD_TYPE_MISMATCH")), "{codes:?}");
}

const NODE_PROVIDER: &str = r#"{
  "name": "node-provider",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NodeStoreOrbital",
    "entity": { "name": "Node", "persistence": "persistent", "collection": "nodes",
      "fields": [ { "name": "id", "type": "string", "required": true } ] },
    "traits": [],
    "pages": []
  }]
}"#;

const NODE_READER: &str = r#"{
  "name": "node-reader",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NodeReaderOrbital",
    "expects": [ { "kind": "entity", "name": "Node" } ],
    "entity": { "name": "Report", "persistence": "runtime",
      "fields": [ { "name": "id", "type": "string", "required": true } ] },
    "traits": [{
      "name": "NodeReader",
      "linkedEntity": "Report",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [ { "key": "INIT" } ],
        "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
          "effects": [ ["fetch", "Node", {}] ] } ]
      }
    }],
    "pages": [ { "name": "ReaderPage", "path": "/nodes", "traits": [ { "ref": "NodeReader" } ] } ]
  }]
}"#;

fn graph_import(name: &str) -> String {
    format!(
        r#"{{ "name": "{name}", "uses": [ {{ "from": "./provider.orb", "as": "Store" }} ],
      "entity": "Store.orbitals.NodeStoreOrbital.entity", "traits": [],
      "reference": {{ "ref": "Store.orbitals.NodeStoreOrbital" }} }}"#
    )
}

const READER_IMPORT: &str = r#"{ "name": "Reader", "uses": [ { "from": "./reader.orb", "as": "Rd" } ],
      "entity": "Rd.orbitals.NodeReaderOrbital.entity", "traits": [],
      "reference": { "ref": "Rd.orbitals.NodeReaderOrbital", "pages": { "/nodes": "/reader-nodes" } } }"#;

fn reader_expected_entity(schema: &orbital_core_base::schema::OrbitalSchema) -> String {
    let reader = schema.orbitals.iter().find(|o| o.name == "Reader").expect("Reader orbital");
    reader
        .expects
        .iter()
        .flatten()
        .find_map(|e| match e {
            orbital_core_base::schema::ExpectDeclaration::Entity { name, .. } => Some(name.clone()),
            _ => None,
        })
        .expect("an entity expectation")
}

fn reader_fetches(schema: &orbital_core_base::schema::OrbitalSchema) -> Vec<String> {
    let reader = schema.orbitals.iter().find(|o| o.name == "Reader").expect("Reader orbital");
    let text = serde_json::to_string(&reader.traits).expect("traits serialize");
    ["GraphNode", "Graph2Node", "Node"]
        .iter()
        .filter(|name| text.contains(&format!(r#"["fetch","{name}""#)))
        .map(|name| name.to_string())
        .collect()
}
