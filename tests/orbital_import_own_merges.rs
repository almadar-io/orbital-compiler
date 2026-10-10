//! G-ORB-162: an orbital import carries only the merged fields (`mergedFrom`)
//! its own kept traits reference. A field a SIBLING orbital's trait merged into
//! the imported record leaves with the sibling; importing the sibling too brings
//! it back through that sibling's trait.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::schema::{EntityRef, OrbitalSchema};

const UPSTREAM: &str = r#"{
  "name": "test-own-merges-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ItemOrbital",
      "entity": {
        "name": "Item",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "status", "type": "string" },
          { "name": "idleNote", "type": "string" },
          { "name": "ownNote", "type": "string", "mergedFrom": "Item" },
          { "name": "siblingNote", "type": "string", "mergedFrom": "Item" }
        ]
      },
      "traits": [
        {
          "name": "ItemView",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [
                  ["set", "@entity.ownNote", "seen"],
                  ["render-ui", "main", { "type": "typography", "content": "@entity.status" }]
                ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "ItemPage", "path": "/items", "linkedEntity": "Item", "traits": [ { "ref": "ItemView" } ] }
      ]
    },
    {
      "name": "VisitOrbital",
      "entity": {
        "name": "Visit",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ItemEditor",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.siblingNote" }] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "VisitPage", "path": "/visit", "linkedEntity": "Item", "traits": [ { "ref": "ItemEditor" } ] }
      ]
    }
  ]
}"#;

const ITEMS: &str = r#"{
  "name": "Items",
  "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
  "entity": "Up.orbitals.ItemOrbital.entity",
  "traits": [],
  "pages": [],
  "reference": { "ref": "Up.orbitals.ItemOrbital", "entity": "Item", "entities": {} }
}"#;

const VISITS: &str = r#"{
  "name": "Visits",
  "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
  "entity": "Up.orbitals.VisitOrbital.entity",
  "traits": [],
  "pages": [],
  "reference": { "ref": "Up.orbitals.VisitOrbital", "entity": "Visit", "entities": { "Item": "Item" } }
}"#;

// A sibling's composed trait rebinds onto another orbital's record: the merge lands
// there after every orbital of the upstream has inlined (a deferred merge).
const ATOM: &str = r#"{
  "name": "test-own-merges-atom",
  "version": "1.0.0",
  "orbitals": [{
    "name": "NoteOrbital",
    "entity": {
      "name": "Note",
      "persistence": "runtime",
      "fields": [
        { "name": "id", "type": "string", "required": true },
        { "name": "noteText", "type": "string" }
      ]
    },
    "traits": [{
      "name": "NoteEditor",
      "entityRebindable": true,
      "linkedEntity": "Note",
      "category": "interaction",
      "stateMachine": {
        "states": [ { "name": "idle", "isInitial": true } ],
        "events": [ { "key": "INIT" } ],
        "transitions": [
          { "from": "idle", "to": "idle", "event": "INIT",
            "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.noteText" }] ] }
        ]
      }
    }]
  }]
}"#;

const REBINDING: &str = r#"{
  "name": "test-own-merges-rebinding",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TaskOrbital",
      "entity": {
        "name": "Task",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "title", "type": "string" }
        ]
      },
      "traits": [{
        "name": "TaskView",
        "linkedEntity": "Task",
        "category": "interaction",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [
            { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.title" }] ] }
          ]
        }
      }],
      "pages": [ { "name": "TaskPage", "path": "/tasks", "linkedEntity": "Task", "traits": [ { "ref": "TaskView" } ] } ]
    },
    {
      "name": "NotesView",
      "uses": [ { "from": "./atom.orb", "as": "Atom" } ],
      "entity": {
        "name": "NotesVisit",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [ { "ref": "Atom.traits.NoteEditor", "name": "TaskNotes", "linkedEntity": "Task" } ],
      "pages": [ { "name": "NotesPage", "path": "/notes", "linkedEntity": "Task", "traits": [ { "ref": "TaskNotes" } ] } ]
    },
    {
      "name": "BadgeView",
      "entity": {
        "name": "BadgeVisit",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [{
        "name": "TaskBadge",
        "linkedEntity": "Task",
        "category": "interaction",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [
            { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["render-ui", "main", { "type": "badge", "label": "@entity.title" }] ] }
          ]
        }
      }],
      "pages": [ { "name": "BadgePage", "path": "/badge", "linkedEntity": "Task", "traits": [ { "ref": "TaskBadge" } ] } ]
    }
  ]
}"#;

const BADGE: &str = r#"{
  "name": "Badge",
  "uses": [ { "from": "./rebinding.orb", "as": "Up" } ],
  "entity": "Up.orbitals.BadgeView.entity",
  "traits": [],
  "pages": [],
  "reference": { "ref": "Up.orbitals.BadgeView", "entity": "BadgeVisit", "entities": { "Task": "Task" } }
}"#;

const TASKS: &str = r#"{
  "name": "Tasks",
  "uses": [ { "from": "./rebinding.orb", "as": "Up" } ],
  "entity": "Up.orbitals.TaskOrbital.entity",
  "traits": [],
  "pages": [],
  "reference": { "ref": "Up.orbitals.TaskOrbital", "entity": "Task", "entities": {} }
}"#;

const NOTES: &str = r#"{
  "name": "Notes",
  "uses": [ { "from": "./rebinding.orb", "as": "Up" } ],
  "entity": "Up.orbitals.NotesView.entity",
  "traits": [],
  "pages": [],
  "reference": { "ref": "Up.orbitals.NotesView", "entity": "NotesVisit", "entities": { "Task": "Task" } }
}"#;

// The same upstream with an earlier orbital holding its own copy of `Task`: the
// sibling's merge lands on that copy, not on the owner's record.
fn shadowed() -> String {
    let mut v: serde_json::Value = serde_json::from_str(REBINDING).expect("rebinding json");
    let shadow = serde_json::json!({
      "name": "ShadowView",
      "entity": { "name": "ShadowVisit", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "auxiliaryEntities": [ {
        "name": "Task", "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ]
      } ],
      "traits": [ {
        "name": "TaskPeek", "linkedEntity": "Task", "category": "interaction",
        "stateMachine": {
          "states": [ { "name": "idle", "isInitial": true } ],
          "events": [ { "key": "INIT" } ],
          "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
            "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.title" }] ] } ]
        }
      } ],
      "pages": [ { "name": "ShadowPage", "path": "/shadow", "linkedEntity": "Task", "traits": [ { "ref": "TaskPeek" } ] } ]
    });
    v["orbitals"]
        .as_array_mut()
        .expect("orbitals")
        .insert(0, shadow);
    v.to_string()
}

// The rebinding view is declared before the record's owner and carries its own copy of
// the record, so inside the upstream the merge lands on the view's copy first.
fn view_first() -> String {
    let mut v: serde_json::Value = serde_json::from_str(REBINDING).expect("rebinding json");
    let orbitals = v["orbitals"].as_array_mut().expect("orbitals");
    let notes = orbitals.remove(1);
    orbitals.insert(0, notes);
    orbitals[0]["auxiliaryEntities"] = serde_json::json!([{
        "name": "Task", "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "title", "type": "string" } ]
    }]);
    v.to_string()
}

fn inline(orbitals: &[&str]) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    std::fs::write(dir.path().join("rebinding.orb"), REBINDING).expect("write rebinding");
    std::fs::write(dir.path().join("shadowed.orb"), shadowed()).expect("write shadowed");
    std::fs::write(dir.path().join("view_first.orb"), view_first()).expect("write view_first");
    let consumer = format!(
        r#"{{ "name": "test-own-merges-consumer", "version": "1.0.0", "orbitals": [ {} ] }}"#,
        orbitals.join(", ")
    );
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn item_fields(schema: &OrbitalSchema) -> Vec<String> {
    let items = schema
        .orbitals
        .iter()
        .find(|o| o.name == "Items" || o.name == "Tasks")
        .expect("Items or Tasks orbital");
    match &items.entity {
        EntityRef::Inline(e) => e.fields.iter().filter_map(|f| f.name.clone()).collect(),
        other => panic!("expected an inline entity, got {other:?}"),
    }
}

#[test]
fn a_field_only_a_sibling_merged_leaves_with_the_sibling() {
    let fields = item_fields(&inline(&[ITEMS]));
    assert!(!fields.contains(&"siblingNote".to_string()), "{fields:?}");
}

#[test]
fn a_merged_field_the_orbitals_own_trait_references_stays() {
    let fields = item_fields(&inline(&[ITEMS]));
    assert!(fields.contains(&"ownNote".to_string()), "{fields:?}");
}

#[test]
fn an_authored_field_nothing_references_stays() {
    let fields = item_fields(&inline(&[ITEMS]));
    assert!(fields.contains(&"idleNote".to_string()), "{fields:?}");
    assert!(fields.contains(&"status".to_string()), "{fields:?}");
}

#[test]
fn control_importing_the_sibling_too_brings_the_field_back() {
    let fields = item_fields(&inline(&[ITEMS, VISITS]));
    assert!(fields.contains(&"siblingNote".to_string()), "{fields:?}");
    assert!(fields.contains(&"ownNote".to_string()), "{fields:?}");
}

#[test]
fn a_siblings_deferred_merge_does_not_travel_into_an_import_without_it() {
    let fields = item_fields(&inline(&[TASKS]));
    assert!(!fields.contains(&"noteText".to_string()), "{fields:?}");
    assert!(fields.contains(&"title".to_string()), "{fields:?}");
}

#[test]
fn control_importing_the_rebinding_sibling_too_carries_its_merge() {
    let fields = item_fields(&inline(&[TASKS, NOTES]));
    assert!(fields.contains(&"noteText".to_string()), "{fields:?}");
}

#[test]
fn a_retarget_carries_only_the_merged_fields_its_traits_use() {
    let fields = item_fields(&inline(&[TASKS, BADGE]));
    assert!(!fields.contains(&"noteText".to_string()), "{fields:?}");
    assert!(fields.contains(&"title".to_string()), "{fields:?}");
}

#[test]
fn control_a_retarget_finds_the_merge_on_whichever_upstream_copy_holds_it() {
    let tasks = TASKS.replace("./rebinding.orb", "./shadowed.orb");
    let notes = NOTES.replace("./rebinding.orb", "./shadowed.orb");
    let fields = item_fields(&inline(&[&tasks, &notes]));
    assert!(fields.contains(&"noteText".to_string()), "{fields:?}");
}

#[test]
fn control_a_merge_landing_on_a_views_copy_inside_the_upstream_reaches_the_owner() {
    let tasks = TASKS.replace("./rebinding.orb", "./view_first.orb");
    let notes = NOTES.replace("./rebinding.orb", "./view_first.orb");
    let fields = item_fields(&inline(&[&notes, &tasks]));
    assert!(fields.contains(&"noteText".to_string()), "{fields:?}");
}
