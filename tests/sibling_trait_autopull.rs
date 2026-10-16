//! Regression: when an organism composes ONLY an app atom's main trait via a
//! trait ref, the inline phase must auto-instantiate any `@trait.<Sibling>`
//! the main trait's render-ui references from the SAME source atom. App atoms
//! keep a hand-rolled main trait and embed a curated variation as a sibling
//! trait, addressing it inside the main trait's render-ui as the literal
//! `"@trait.SiblingView"`. Cloning only the main trait left that reference
//! dangling → ORB_BINDING_TRAIT_UNKNOWN. The sibling is now pulled with the
//! SAME linkedEntity rebind and KEEPS its original name so the reference
//! resolves.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::TraitReference;

const ATOM: &str = r#"{
  "name": "test-sibling-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "WidgetOrbital",
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "WidgetMain",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "OPEN",
                "effects": [
                  ["render-ui", "main", {
                    "type": "stack",
                    "children": [ "@trait.WidgetView" ]
                  }]
                ]
              }
            ]
          }
        },
        {
          "name": "WidgetView",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "VIEW" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "VIEW",
                "effects": [
                  ["render-ui", "main", {
                    "type": "data-list",
                    "entity": "Widget"
                  }]
                ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "WidgetPage", "path": "/widget", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetMain" }, { "ref": "WidgetView" } ] }
      ]
    }
  ]
}"#;

const CONSUMER: &str = r#"{
  "name": "test-sibling-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "PanelOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Widget" } ],
      "entity": {
        "name": "Gadget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Widget.traits.WidgetMain", "linkedEntity": "Gadget", "name": "GadgetPanel" }
      ],
      "pages": [
        { "name": "PanelPage", "path": "/panel", "linkedEntity": "Gadget", "traits": [ { "ref": "GadgetPanel" } ] }
      ]
    }
  ]
}"#;

#[test]
fn sibling_trait_referenced_by_main_is_auto_pulled() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");

    let names: Vec<String> = schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect();

    // The main trait was renamed at the call site.
    assert!(
        names.iter().any(|n| n == "GadgetPanel"),
        "consumer should carry the renamed main trait; got {names:?}"
    );
    // The sibling was auto-pulled under its ORIGINAL name so `@trait.WidgetView`
    // still resolves inside GadgetPanel's render tree.
    assert!(
        names.iter().any(|n| n == "WidgetView"),
        "consumer should auto-instantiate the `WidgetView` sibling referenced by \
         the main trait's render-ui; got {names:?}"
    );

    // The dangling `@trait.WidgetView` is gone: exactly two traits, the renamed
    // main + the pulled sibling. No double-instantiation.
    let sibling_count = names.iter().filter(|n| *n == "WidgetView").count();
    assert_eq!(sibling_count, 1, "sibling must not be double-instantiated");
    assert_eq!(
        schema.orbitals[0].traits.len(),
        2,
        "consumer should have exactly the renamed main + one pulled sibling; got {names:?}"
    );

    // The pulled sibling rebinds its store-addressing `entity` prop to the
    // consumer's entity, exactly as a hand-written sibling ref would.
    let sibling = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "WidgetView" => Some(td),
            _ => None,
        })
        .expect("pulled WidgetView present");
    assert_eq!(
        sibling.linked_entity.as_deref(),
        Some("Gadget"),
        "pulled sibling's linkedEntity must be rebound to the consumer entity"
    );
}
