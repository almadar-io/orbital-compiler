//! An import swaps the upstream VIEW for a host sibling while keeping the
//! upstream RULES (`omit { View }` + a sibling view + `listens {}` routes).
//! Three things must follow the omit for that to be a clean program:
//! (a) traits only the omitted view embedded (its generated render children)
//!     leave with it, even though the emitter mounts every child on the page;
//! (b) entity fields merged in (`mergedFrom`) that only removed traits
//!     referenced leave with them;
//! (c) a sibling's `listens` naming an imported trait follows the import
//!     prefix, as `listens {}` routes into the import already do.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{EntityRef, ListenSource, OrbitalSchema, TraitReference};

const UPSTREAM: &str = r#"{
  "name": "test-view-swap-upstream",
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
          { "name": "stepLabel", "type": "string", "mergedFrom": "StepView" },
          { "name": "ruleNote", "type": "string", "mergedFrom": "RuleView" }
        ]
      },
      "traits": [
        {
          "name": "ItemView",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" }, { "key": "ACT" }, { "key": "Done" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.ItemViewChild"] }] ] },
              { "from": "idle", "to": "idle", "event": "ACT", "effects": [ ["emit", "ACT"] ] },
              { "from": "idle", "to": "idle", "event": "Done" }
            ]
          },
          "emits": [ { "event": "ACT" } ],
          "listens": [ { "event": "Done", "triggers": "Done", "source": { "kind": "trait", "trait": "ItemRules" } } ]
        },
        {
          "name": "ItemViewChild",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", { "type": "typography", "content": "@entity.stepLabel" }] ] }
            ]
          }
        },
        {
          "name": "ItemRules",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "ready", "isInitial": true } ],
            "events": [ { "key": "ACT" } ],
            "transitions": [
              { "from": "ready", "to": "ready", "event": "ACT",
                "effects": [ ["set", "@entity.ruleNote", "acted"], ["emit", "Done"] ] }
            ]
          },
          "emits": [ { "event": "Done" } ],
          "listens": [ { "event": "ACT", "triggers": "ACT", "source": { "kind": "trait", "trait": "ItemView" } } ]
        }
      ],
      "pages": [
        { "name": "ItemPage", "path": "/items", "linkedEntity": "Item", "traits": [ { "ref": "ItemView" }, { "ref": "ItemViewChild" }, { "ref": "ItemRules" } ] }
      ]
    }
  ]
}"#;

fn consumer(omit: &str) -> String {
    format!(
        r#"{{
  "name": "test-view-swap-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Swap",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.ItemOrbital.entity",
      "traits": [
        {{
          "name": "MemberView",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }}, {{ "key": "TAP" }}, {{ "key": "Done" }} ],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "stack" }}] ] }},
              {{ "from": "idle", "to": "idle", "event": "TAP", "effects": [ ["emit", "TAP"] ] }},
              {{ "from": "idle", "to": "idle", "event": "Done" }}
            ]
          }},
          "emits": [ {{ "event": "TAP" }} ],
          "listens": [
            {{ "event": "Done", "triggers": "Done", "source": {{ "kind": "trait", "trait": "ItemRules" }} }},
            {{ "event": "TAP", "triggers": "TAP", "source": {{ "kind": "trait", "trait": "MemberView" }} }}
          ]
        }},
        {{
          "name": "Shell",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", {{ "type": "stack", "children": ["@trait.ItemRules", "@trait.MemberView"] }}] ] }}
            ]
          }}
        }}
      ],
      "reference": {{
        "ref": "Up.orbitals.ItemOrbital",
        "entity": "Item",
        "omit": [{omit}],
        "pages": {{ "/items": "/mine" }},
        "listens": [ {{ "trait": "ItemRules", "event": "TAP", "triggers": "ACT", "source": {{ "kind": "trait", "trait": "MemberView" }} }} ]
      }}
    }}
  ]
}}"#
    )
}

fn inline(omit: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(omit)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn trait_names(schema: &OrbitalSchema) -> Vec<String> {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            _ => None,
        })
        .collect()
}

fn field_names(schema: &OrbitalSchema) -> Vec<String> {
    match &schema.orbitals[0].entity {
        EntityRef::Inline(e) => e.fields.iter().filter_map(|f| f.name.clone()).collect(),
        other => panic!("expected an inline entity, got {other:?}"),
    }
}

fn listen_sources(schema: &OrbitalSchema, trait_name: &str) -> Vec<String> {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == trait_name => Some(td),
            _ => None,
        })
        .unwrap_or_else(|| panic!("trait {trait_name} missing"))
        .listens
        .iter()
        .flatten()
        .filter_map(|l| match &l.source {
            Some(ListenSource::Trait { trait_name, .. }) => Some(trait_name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_omitted_views_embedded_children_leave_with_it() {
    let names = trait_names(&inline(r#""ItemView""#));
    assert!(
        !names.iter().any(|n| n.ends_with("ItemViewChild")),
        "{names:?}"
    );
    assert!(names.contains(&"SwapItemRules".to_string()), "{names:?}");
    assert!(names.contains(&"MemberView".to_string()), "{names:?}");
}

#[test]
fn control_without_the_omit_the_embedded_child_stays() {
    let names = trait_names(&inline(""));
    assert!(
        names.contains(&"SwapItemViewChild".to_string()),
        "{names:?}"
    );
    assert!(names.contains(&"SwapItemView".to_string()), "{names:?}");
}

#[test]
fn merged_fields_only_removed_traits_read_leave_with_them() {
    let fields = field_names(&inline(r#""ItemView""#));
    assert!(!fields.contains(&"stepLabel".to_string()), "{fields:?}");
    // a merged field a kept trait still writes stays
    assert!(fields.contains(&"ruleNote".to_string()), "{fields:?}");
    // an authored (non-merged) field is never dropped, even when unread
    assert!(fields.contains(&"status".to_string()), "{fields:?}");
}

#[test]
fn control_without_the_omit_merged_fields_stay() {
    let fields = field_names(&inline(""));
    assert!(fields.contains(&"stepLabel".to_string()), "{fields:?}");
}

#[test]
fn a_sibling_listen_naming_an_imported_trait_follows_the_prefix() {
    let schema = inline(r#""ItemView""#);
    let sources = listen_sources(&schema, "MemberView");
    assert!(
        sources.contains(&"SwapItemRules".to_string()),
        "{sources:?}"
    );
    // a sibling naming another sibling keeps the name
    assert!(sources.contains(&"MemberView".to_string()), "{sources:?}");
    // and the route into the import still lands on the prefixed rules
    let rules = listen_sources(&schema, "SwapItemRules");
    assert!(rules.contains(&"MemberView".to_string()), "{rules:?}");
}

#[test]
fn a_sibling_embed_of_an_imported_trait_follows_the_prefix() {
    let schema = inline(r#""ItemView""#);
    let shell = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Shell" => Some(td),
            _ => None,
        })
        .expect("Shell sibling");
    let rendered =
        serde_json::to_string(&shell.state_machine.transitions[0].effects).expect("json");
    assert!(rendered.contains("@trait.SwapItemRules"), "{rendered}");
    // control: an embed of a host sibling keeps its name
    assert!(rendered.contains("@trait.MemberView"), "{rendered}");
    assert!(!rendered.contains("@trait.ItemRules\""), "{rendered}");
}

const TWO_IMPORTS: &str = r#"{
  "name": "test-two-imports",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "First",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ItemOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ItemOrbital", "entity": "FirstItem", "pages": { "/items": "/first" } }
    },
    {
      "name": "Second",
      "uses": [ { "from": "./upstream2.orb", "as": "Up2" } ],
      "entity": "Up2.orbitals.ItemOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up2.orbitals.ItemOrbital",
        "entity": "SecondItem",
        "pages": { "/items": "/second" },
        "listens": [
          { "trait": "ItemRules", "event": "Done", "triggers": "ACT", "source": { "kind": "orbital", "orbital": "First", "trait": "ItemRules" } }
        ]
      }
    }
  ]
}"#;

#[test]
fn an_import_route_from_another_alias_imports_trait_follows_that_imports_prefix() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    std::fs::write(
        dir.path().join("upstream2.orb"),
        UPSTREAM.replace("test-view-swap-upstream", "test-view-swap-upstream-two"),
    )
    .expect("write upstream2");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, TWO_IMPORTS).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    let second_rules = schema.orbitals[1]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "SecondItemRules" => Some(td),
            _ => None,
        })
        .expect("SecondItemRules");
    let sources: Vec<(String, String)> = second_rules
        .listens
        .iter()
        .flatten()
        .filter_map(|l| match &l.source {
            Some(ListenSource::Orbital {
                orbital,
                trait_name,
                ..
            }) => Some((orbital.clone(), trait_name.clone())),
            _ => None,
        })
        .collect();
    assert!(
        sources.contains(&("First".to_string(), "FirstItemRules".to_string())),
        "{sources:?}"
    );
}

const MID: &str = r#"{
  "name": "test-view-swap-mid",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Mid",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ItemOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ItemOrbital", "entity": "Item" }
    }
  ]
}"#;

const NESTED: &str = r#"{
  "name": "test-nested-imports",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "First",
      "uses": [ { "from": "./mid.orb", "as": "M" } ],
      "entity": "M.orbitals.Mid.entity",
      "traits": [],
      "reference": { "ref": "M.orbitals.Mid", "entity": "FirstItem", "pages": { "/items": "/first" } }
    },
    {
      "name": "Second",
      "uses": [ { "from": "./upstream2.orb", "as": "Up2" } ],
      "entity": "Up2.orbitals.ItemOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up2.orbitals.ItemOrbital",
        "entity": "SecondItem",
        "pages": { "/items": "/second" },
        "listens": [
          { "trait": "ItemRules", "event": "Done", "triggers": "ACT", "source": { "kind": "orbital", "orbital": "First", "trait": "MidItemRules" } }
        ]
      }
    }
  ]
}"#;

#[test]
fn a_route_from_an_import_of_an_import_follows_both_prefixes() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    std::fs::write(
        dir.path().join("upstream2.orb"),
        UPSTREAM.replace("test-view-swap-upstream", "test-view-swap-upstream-two"),
    )
    .expect("write upstream2");
    std::fs::write(dir.path().join("mid.orb"), MID).expect("write mid");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, NESTED).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    let second_rules = schema.orbitals[1]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "SecondItemRules" => Some(td),
            _ => None,
        })
        .expect("SecondItemRules");
    let sources: Vec<(String, String)> = second_rules
        .listens
        .iter()
        .flatten()
        .filter_map(|l| match &l.source {
            Some(ListenSource::Orbital {
                orbital,
                trait_name,
                ..
            }) => Some((orbital.clone(), trait_name.clone())),
            _ => None,
        })
        .collect();
    assert!(
        sources.contains(&("First".to_string(), "FirstMidItemRules".to_string())),
        "{sources:?}"
    );
    let target_id = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "FirstMidItemRules" => td.id.clone(),
            _ => None,
        })
        .expect("FirstMidItemRules carries an id");
    let route_id = second_rules
        .listens
        .iter()
        .flatten()
        .find_map(|l| match &l.source {
            Some(ListenSource::Orbital { trait_id, .. }) => trait_id.clone(),
            _ => None,
        })
        .expect("the route carries the source trait id");
    assert_eq!(route_id, target_id);
}

#[test]
fn only_keeps_what_its_kept_traits_embed() {
    // `only { ItemView, ItemRules }` keeps ItemView's embedded render child.
    let names = trait_names(&inline_with_only(r#""ItemView", "ItemRules""#));
    assert!(names.contains(&"SwapItemView".to_string()), "{names:?}");
    assert!(
        names.contains(&"SwapItemViewChild".to_string()),
        "{names:?}"
    );
}

#[test]
fn control_only_still_drops_what_nothing_kept_embeds() {
    let names = trait_names(&inline_with_only(r#""ItemRules""#));
    assert!(
        !names.iter().any(|n| n.ends_with("ItemViewChild")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n == "SwapItemView"), "{names:?}");
}

fn inline_with_only(only: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    let consumer = consumer("").replace(r#""omit": [],"#, &format!(r#""only": [{only}],"#));
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn page_paths(schema: &OrbitalSchema) -> Vec<String> {
    schema.orbitals[0]
        .pages
        .iter()
        .flatten()
        .filter_map(|p| match p {
            orbital_core_base::schema::PageRef::Inline(pd) => Some(pd.path.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn omit_drops_a_page_named_by_its_path() {
    let paths = page_paths(&inline(r#""/items""#));
    assert!(
        !paths.iter().any(|p| p == "/mine" || p == "/items"),
        "{paths:?}"
    );
}

#[test]
fn control_without_the_page_omit_the_page_stays() {
    let paths = page_paths(&inline(""));
    assert!(paths.iter().any(|p| p == "/mine"), "{paths:?}");
}

#[test]
fn edge_omitting_an_unknown_page_path_is_reported() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(r#""/nope""#)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let (mut trait_ref, mut orbital_ref, mut uses_config) = (Vec::new(), Vec::new(), Vec::new());
    orbital_compiler::phases::inline::inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut trait_ref,
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let result =
        orbital_compiler::phases::validation::orbital_ref_config::validate_orbital_ref_diagnostics(
            &orbital_ref,
            &schema,
        );
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code.as_str() == "ORB_O_PAGE_UNKNOWN_PATH" && e.message.contains("/nope")),
        "{:#?}",
        result.errors
    );
}

fn page_trait_refs(schema: &OrbitalSchema) -> Vec<String> {
    schema.orbitals[0]
        .pages
        .iter()
        .flatten()
        .filter_map(|p| match p {
            orbital_core_base::schema::PageRef::Inline(pd) => Some(pd),
            _ => None,
        })
        .flat_map(|pd| pd.traits.iter().flatten())
        .filter_map(|t| match t {
            TraitReference::Reference {
                reference, name, ..
            } => reference.clone().or_else(|| name.clone()),
            TraitReference::Inline(td) => Some(td.name.clone()),
        })
        .collect()
}

#[test]
fn a_mount_naming_an_imported_trait_follows_the_prefix_once() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    let consumer = consumer(r#""ItemView""#).replace(
        r#""pages": { "/items": "/mine" },"#,
        r#""pages": { "/items": "/mine" }, "mounts": { "/items": ["ItemRules", "MemberView"] },"#,
    );
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    let refs = page_trait_refs(&schema);
    assert!(!refs.iter().any(|r| r == "ItemRules"), "{refs:?}");
    assert_eq!(
        refs.iter().filter(|r| *r == "SwapItemRules").count(),
        1,
        "{refs:?}"
    );
    // control: a sibling mount keeps its name
    assert!(refs.iter().any(|r| r == "MemberView"), "{refs:?}");
}
