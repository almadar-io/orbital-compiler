//! An entity name keeps the id the schema established first. A later orbital rebinding onto
//! `OrderRecord` must bind the owner's id, even when an orbital in between pulled an atom's own
//! same-named `OrderRecord` (carrying the atom's id) through a composed trait with no rebind.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{EntityRef, TraitReference};

const ORDERS_ATOM: &str = r#"{
  "name": "orders-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "OrdersViewOrbital",
      "entity": { "name": "OrdersVisit", "id": "ent_ORDERS_VISIT", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "auxiliaryEntities": [
        { "name": "OrderRecord", "id": "ent_ATOM", "persistence": "persistent", "collection": "orders",
          "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ] }
      ],
      "traits": [
        {
          "name": "MyOrders",
          "linkedEntity": "OrderRecord",
          "linkedEntityId": "ent_ATOM",
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

const LIST_ATOM: &str = r#"{
  "name": "list-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ItemOrbital",
      "entity": { "name": "Item", "id": "ent_ITEM", "persistence": "persistent", "collection": "items",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ] },
      "traits": [
        {
          "name": "ItemList",
          "linkedEntity": "Item",
          "linkedEntityId": "ent_ITEM",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.title" }] ] } ]
          }
        }
      ],
      "pages": [ { "name": "ItemsPage", "path": "/items", "traits": [ { "ref": "ItemList" } ] } ]
    }
  ]
}"#;

fn store(owner: bool) -> String {
    let owner_orbital = if owner {
        r#"{
      "name": "OrderDesk",
      "entity": { "name": "OrderRecord", "id": "ent_OWNER", "persistence": "persistent", "collection": "orders",
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
      "uses": [ {{ "from": "./orders.orb", "as": "Orders" }} ],
      "entity": {{ "name": "PageVisit", "id": "ent_PAGE_VISIT", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [ {{ "ref": "Orders.traits.MyOrders" }} ],
      "pages": [ {{ "name": "MyOrders", "path": "/my-orders", "traits": [ {{ "ref": "MyOrders" }} ] }} ]
    }},
    {{
      "name": "OrderBoardPage",
      "uses": [ {{ "from": "./list.orb", "as": "List" }} ],
      "entity": {{ "name": "BoardVisit", "id": "ent_BOARD_VISIT", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [ {{ "ref": "List.traits.ItemList", "name": "OrderBoard", "linkedEntity": "OrderRecord" }} ],
      "pages": [ {{ "name": "OrderBoard", "path": "/order-board", "traits": [ {{ "ref": "OrderBoard" }} ] }} ]
    }}
  ]
}}"#
    )
}

struct Ids {
    board: Option<String>,
    owner: Option<String>,
    pulled: Option<String>,
}

fn entity_id(e: &EntityRef) -> Option<String> {
    match e {
        EntityRef::Inline(e) if e.name == "OrderRecord" => e.id.clone(),
        _ => None,
    }
}

fn ids(owner: bool) -> Ids {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("orders.orb"), ORDERS_ATOM).expect("write orders atom");
    std::fs::write(dir.path().join("list.orb"), LIST_ATOM).expect("write list atom");
    let path = dir.path().join("store.orb");
    std::fs::write(&path, store(owner)).expect("write store");
    let mut schema = parse_file(&path).expect("parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    let orbital = |name: &str| schema.orbitals.iter().find(|o| o.name == name);
    let board = orbital("OrderBoardPage")
        .expect("board orbital")
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "OrderBoard" => td.linked_entity_id.clone(),
            _ => None,
        });
    let owner = orbital("OrderDesk").and_then(|o| entity_id(&o.entity));
    let pulled = orbital("MyOrdersPage")
        .and_then(|o| o.auxiliary_entities.as_ref())
        .and_then(|aux| aux.iter().find_map(entity_id));
    Ids {
        board,
        owner,
        pulled,
    }
}

#[test]
fn a_later_rebind_binds_the_owners_id_not_a_pulled_copys() {
    let ids = ids(true);
    assert!(
        ids.owner.is_some(),
        "the owner declares OrderRecord with an id"
    );
    assert_eq!(ids.board, ids.owner);
}

#[test]
fn control_with_no_owner_the_first_pulled_copy_establishes_the_id() {
    let ids = ids(false);
    assert!(
        ids.pulled.is_some(),
        "the no-rebind pull carries OrderRecord in"
    );
    assert_eq!(ids.board, ids.pulled);
}
