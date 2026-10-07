//! A trait rebound onto an entity a SIBLING orbital owns (`-> OrderRecord`) binds that entity.
//! Its source orbital's same-named entity must not travel into the consumer as an auxiliary copy:
//! the copy shadows the owner's entity (by name) and drops the fields only the owner carries.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;

const ATOM: &str = r#"{
  "name": "orders-view-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "OrdersViewOrbital",
      "entity": { "name": "OrdersVisit", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "auxiliaryEntities": [
        { "name": "OrderRecord", "persistence": "persistent", "collection": "orders",
          "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ] }
      ],
      "traits": [
        {
          "name": "MyOrders",
          "linkedEntity": "OrderRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.title" }] ] } ]
          }
        }
      ],
      "pages": [ { "name": "OrdersPage", "path": "/orders", "traits": [ { "ref": "MyOrders" } ] } ]
    }
  ]
}"#;

fn consumer(rebind: bool, owner: bool) -> String {
    let linked = if rebind {
        r#", "linkedEntity": "OrderRecord""#
    } else {
        ""
    };
    let owner_orbital = if owner {
        r#"{
      "name": "OrderDesk",
      "entity": { "name": "OrderRecord", "persistence": "persistent", "collection": "orders",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" }, { "name": "stage", "type": "string" } ] },
      "traits": [],
      "pages": []
    },"#
    } else {
        ""
    };
    format!(
        r#"{{
  "name": "store",
  "version": "1.0.0",
  "orbitals": [
    {owner_orbital}
    {{
      "name": "MyOrdersPage",
      "uses": [ {{ "from": "./atom.orb", "as": "Orders" }} ],
      "entity": {{ "name": "PageVisit", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [ {{ "ref": "Orders.traits.MyOrders"{linked} }} ],
      "pages": [ {{ "name": "MyOrders", "path": "/my-orders", "traits": [ {{ "ref": "MyOrders" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn aux_names(rebind: bool, owner: bool) -> Vec<String> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let path = dir.path().join("store.orb");
    std::fs::write(&path, consumer(rebind, owner)).expect("write consumer");
    let mut schema = parse_file(&path).expect("parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    let page = schema
        .orbitals
        .iter()
        .find(|o| o.name == "MyOrdersPage")
        .expect("page orbital");
    page.auxiliary_entities
        .iter()
        .flatten()
        .filter_map(|e| match e {
            orbital_core::schema::EntityRef::Inline(def) => Some(def.name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_trait_rebound_onto_a_sibling_entity_carries_no_shadow_copy() {
    assert!(
        !aux_names(true, true).contains(&"OrderRecord".to_string()),
        "{:?}",
        aux_names(true, true)
    );
}

#[test]
fn an_unrebound_trait_brings_no_copy_of_an_entity_a_sibling_owns() {
    assert!(
        !aux_names(false, true).contains(&"OrderRecord".to_string()),
        "{:?}",
        aux_names(false, true)
    );
}

#[test]
fn control_an_unrebound_trait_brings_its_own_entity_when_nobody_owns_it() {
    assert!(
        aux_names(false, false).contains(&"OrderRecord".to_string()),
        "{:?}",
        aux_names(false, false)
    );
}
