//! `@pages` lists only the root pages that declare their nav entry (`@label`),
//! in declaration order, carrying `@icon` and `@roles`.

use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::resolve::resolve_to_oir;

const SCHEMA: &str = r#"{
  "name": "nav-entries",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ShellOrbital",
      "entity": { "name": "Shell", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "Nav",
          "linkedEntity": "Shell",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", { "type": "navigation", "items": "@pages" }] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "HomePage", "path": "/", "label": "Home", "icon": "house", "traits": [ { "ref": "Nav" } ] },
        { "name": "HiddenPage", "path": "/hidden", "traits": [ { "ref": "Nav" } ] },
        { "name": "DetailPage", "path": "/items/:id", "label": "Item", "traits": [ { "ref": "Nav" } ] }
      ]
    },
    {
      "name": "StaffOrbital",
      "entity": { "name": "Staff", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [],
      "pages": [
        { "name": "StaffPage", "path": "/staff", "label": "Staff", "roles": ["manager"] },
        { "name": "AboutPage", "path": "/about", "label": "About" }
      ]
    }
  ]
}"#;

fn nav_items() -> serde_json::Value {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("nav.orb");
    std::fs::write(&path, SCHEMA).expect("write schema");
    let schema = parse_file(&path).expect("parses");
    let module = resolve_to_oir(&schema).expect("resolves");
    let effects = serde_json::to_value(&module.traits[0].transitions[0].effects).expect("json");
    fn find_items(v: &serde_json::Value) -> Option<serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m
                .get("items")
                .filter(|i| i.is_array())
                .cloned()
                .or_else(|| m.values().find_map(find_items)),
            serde_json::Value::Array(a) => a.iter().find_map(find_items),
            _ => None,
        }
    }
    find_items(&effects).unwrap_or_else(|| panic!("no items in {effects}"))
}

fn hrefs(items: &serde_json::Value) -> Vec<String> {
    items
        .as_array()
        .expect("array")
        .iter()
        .map(|i| i["href"].as_str().expect("href").to_string())
        .collect()
}

#[test]
fn only_pages_declaring_a_label_are_listed_in_declaration_order() {
    let items = nav_items();
    assert_eq!(hrefs(&items), vec!["/", "/staff", "/about"], "{items}");
}

#[test]
fn an_entry_carries_its_icon_and_roles() {
    let items = nav_items();
    assert_eq!(items[0]["label"], "Home", "{items}");
    assert_eq!(items[0]["icon"], "house", "{items}");
    assert!(items[0].get("roles").is_none(), "{items}");
    assert_eq!(items[1]["roles"], serde_json::json!(["manager"]), "{items}");
}

#[test]
fn edge_a_param_route_is_never_a_nav_entry_even_with_a_label() {
    let items = nav_items();
    assert!(
        !hrefs(&items).contains(&"/items/:id".to_string()),
        "{items}"
    );
}
