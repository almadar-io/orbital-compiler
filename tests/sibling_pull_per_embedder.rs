//! Regression for C-SIBLING-PULL-SHARED-ACROSS-REBINDS.
//!
//! Binding ONE atom twice inside one orbital — the `std-realtime-chat` shape,
//! `ChannelRail = Browse.BrowseItemBrowse -> Channel` alongside
//! `ChatThread = Browse.BrowseItemBrowse -> ChatMessage` — used to materialise a
//! SINGLE copy of the atom's `@trait.X` sub-view. The dedup key was
//! (alias, sibling name), so the second rebind silently shared the first's copy:
//! one `linkedEntity`, one host in `listens[].source`, one call-site config. The
//! rail rendered the thread's rows.
//!
//! The pull is now keyed per EMBEDDER, so each rebind gets its own sub-view bound
//! to its own entity and listening to its own host.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{ListenSource, TraitDefinition, TraitReference};

/// One atom whose main trait embeds a listening sub-view via `@trait.RowView`.
const ATOM: &str = r#"{
  "name": "test-browse-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SrcOrbital",
      "entity": {
        "name": "Item",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ItemBrowse",
          "linkedEntity": "Item",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" }, { "key": "ItemsLoaded" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "OPEN",
                "effects": [
                  ["render-ui", "main", {
                    "type": "stack",
                    "children": [ "@trait.RowView" ]
                  }]
                ]
              }
            ]
          }
        },
        {
          "name": "RowView",
          "linkedEntity": "Item",
          "category": "interaction",
          "listens": [
            { "event": "ItemsLoaded", "triggers": "REFRESH", "source": { "kind": "trait", "trait": "ItemBrowse" } }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "REFRESH" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "REFRESH",
                "effects": [
                  ["render-ui", "main", { "type": "data-list", "entity": "Item" }]
                ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "SrcPage", "path": "/src", "linkedEntity": "Item", "traits": [ { "ref": "ItemBrowse" } ] }
      ]
    }
  ]
}"#;

/// The chat shape: one atom, two rebinds, two entities.
const TWO_REBIND_CONSUMER: &str = r#"{
  "name": "test-two-rebind-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AppOrbital",
      "uses": [ { "from": "./atom-browse.orb", "as": "Browse" } ],
      "entity": {
        "name": "Message",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "auxiliaryEntities": [
        {
          "name": "Channel",
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true } ]
        }
      ],
      "traits": [
        { "ref": "Browse.traits.ItemBrowse", "linkedEntity": "Channel", "name": "ChannelRail" },
        { "ref": "Browse.traits.ItemBrowse", "linkedEntity": "Message", "name": "MessageThread" }
      ],
      "pages": [
        { "name": "AppPage", "path": "/app", "linkedEntity": "Message", "traits": [ { "ref": "ChannelRail" }, { "ref": "MessageThread" } ] }
      ]
    }
  ]
}"#;

fn inline_fixture(files: &[(&str, String)], consumer: &str) -> orbital_core_base::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (name, content) in files {
        std::fs::write(dir.path().join(name), content).expect("write dep");
    }
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn inline_traits_of(schema: &orbital_core_base::schema::OrbitalSchema) -> Vec<&TraitDefinition> {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(&**td),
            _ => None,
        })
        .collect()
}

/// The `@trait.<X>` token a parent's render tree carries (first one found).
fn embed_ref_of(td: &TraitDefinition) -> String {
    let json = serde_json::to_string(td).expect("serialize");
    let idx = json.find("@trait.").expect("parent should embed a sibling");
    let rest = &json[idx + "@trait.".len()..];
    let end = rest.find(['"', '.', '[']).unwrap_or(rest.len());
    rest[..end].to_string()
}

fn listen_source_of(td: &TraitDefinition) -> String {
    let listens = td
        .listens
        .as_ref()
        .unwrap_or_else(|| panic!("{} must keep its listen route", td.name));
    assert_eq!(listens.len(), 1, "exactly the one authored route survives");
    match listens[0].source.as_ref() {
        Some(ListenSource::Trait { trait_name, .. }) => trait_name.clone(),
        other => panic!("expected a trait-scoped source, got {other:?}"),
    }
}

#[test]
fn two_rebinds_of_one_atom_each_get_their_own_sub_view() {
    let schema = inline_fixture(
        &[("atom-browse.orb", ATOM.to_string())],
        TWO_REBIND_CONSUMER,
    );
    let traits = inline_traits_of(&schema);
    let names: Vec<&str> = traits.iter().map(|t| t.name.as_str()).collect();

    let views: Vec<&&TraitDefinition> = traits
        .iter()
        .filter(|t| t.name.contains("RowView"))
        .collect();
    assert_eq!(
        views.len(),
        2,
        "each rebind must materialise its OWN sub-view; got {names:?}"
    );

    for (host, entity) in [("ChannelRail", "Channel"), ("MessageThread", "Message")] {
        let parent = traits
            .iter()
            .find(|t| t.name == host)
            .unwrap_or_else(|| panic!("{host} present; got {names:?}"));
        let referenced = embed_ref_of(parent);
        let view = traits
            .iter()
            .find(|t| t.name == referenced)
            .unwrap_or_else(|| panic!("{host} refs {referenced} which must exist; got {names:?}"));

        assert_eq!(
            view.linked_entity.as_deref(),
            Some(entity),
            "{host}'s sub-view must be bound to {entity}, not the other rebind's entity"
        );
        assert_eq!(
            listen_source_of(view),
            host,
            "{host}'s sub-view must listen to {host}, not the other rebind"
        );
    }

    // The two parents must not converge on one declaration.
    let a = embed_ref_of(
        traits
            .iter()
            .find(|t| t.name == "ChannelRail")
            .expect("ChannelRail"),
    );
    let b = embed_ref_of(
        traits
            .iter()
            .find(|t| t.name == "MessageThread")
            .expect("MessageThread"),
    );
    assert_ne!(a, b, "the two rebinds must not share one sub-view ({a})");
}

/// The per-embedder copies are distinct declarations, so they must carry distinct
/// trait ids: listens match sources by id, and a shared id makes each copy's
/// listeners hear the other copy (G-ORB-281).
#[test]
fn per_embedder_copies_of_one_sibling_get_distinct_ids() {
    let schema = inline_fixture(
        &[("atom-browse.orb", ATOM.to_string())],
        TWO_REBIND_CONSUMER,
    );
    let traits = inline_traits_of(&schema);
    let ids: Vec<(&str, Option<&str>)> = traits
        .iter()
        .filter(|t| t.name.contains("RowView"))
        .map(|t| (t.name.as_str(), t.id.as_deref()))
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.iter().all(|(_, id)| id.is_some()), "{ids:?}");
    assert_ne!(ids[0].1, ids[1].1, "two copies share one trait id: {ids:?}");
}

#[test]
fn every_inlined_trait_has_its_own_id() {
    let schema = inline_fixture(
        &[("atom-browse.orb", ATOM.to_string())],
        TWO_REBIND_CONSUMER,
    );
    let mut ids: Vec<&str> = inline_traits_of(&schema)
        .iter()
        .filter_map(|t| t.id.as_deref())
        .collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "every trait of the inlined orbital has its own id");
}
