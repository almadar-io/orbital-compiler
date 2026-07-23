//! Regressions for the sibling auto-pull pass (phases/inline/trait.rs).
//!
//! 1. C-SIBLING-PULL-CROSS-ATOM-NAME-CAPTURE: inline chrome names are numbered
//!    per source atom (`InlineButtonRender1` exists in many atoms). A name-only
//!    dedup made the second atom's parent tree bind to the FIRST atom's trait
//!    (std-finance-tracker's Charge button firing REPRINT_RECEIPT). Cross-alias
//!    collisions must pull BOTH siblings, disambiguating the later one and
//!    rewriting only its own parent's `@trait.X` tokens.
//!
//! 2. C-INLINE-PHASE95-LISTEN-SOURCE-RENAME-BLIND: a pulled sibling's
//!    source-scoped listen names its host by the atom's ORIGINAL trait name;
//!    when the consumer renames the host (the universal case), Phase 9.5 must
//!    rewrite the listen source through the rename map instead of dropping the
//!    route as "source not imported".

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::schema::{ListenSource, TraitDefinition, TraitReference};

fn atom_with_button(atom_name: &str, main: &str, fire_event: &str) -> String {
    format!(
        r#"{{
  "name": "{atom_name}",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "SrcOrbital",
      "entity": {{
        "name": "Src",
        "persistence": "persistent",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "name": "{main}",
          "linkedEntity": "Src",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "OPEN" }} ],
            "transitions": [
              {{
                "from": "idle",
                "to": "idle",
                "event": "OPEN",
                "effects": [
                  ["render-ui", "main", {{
                    "type": "stack",
                    "children": [ "@trait.InlineButtonRender1" ]
                  }}]
                ]
              }}
            ]
          }}
        }},
        {{
          "name": "InlineButtonRender1",
          "linkedEntity": "Src",
          "category": "interaction",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "{fire_event}" }} ],
            "transitions": [
              {{
                "from": "idle",
                "to": "idle",
                "event": "{fire_event}",
                "effects": [
                  ["render-ui", "main", {{ "type": "button", "label": "{fire_event}" }}]
                ]
              }}
            ]
          }}
        }}
      ],
      "pages": [
        {{ "name": "SrcPage", "path": "/src", "linkedEntity": "Src", "traits": [ {{ "ref": "{main}" }} ] }}
      ]
    }}
  ]
}}"#
    )
}

const CROSS_ATOM_CONSUMER: &str = r#"{
  "name": "test-cross-atom-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AppOrbital",
      "uses": [
        { "from": "./atom-alpha.orb", "as": "Alpha" },
        { "from": "./atom-beta.orb", "as": "Beta" }
      ],
      "entity": {
        "name": "Thing",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Alpha.traits.AlphaMain", "linkedEntity": "Thing", "name": "ThingAlpha" },
        { "ref": "Beta.traits.BetaMain", "linkedEntity": "Thing", "name": "ThingBeta" }
      ],
      "pages": [
        { "name": "AppPage", "path": "/app", "linkedEntity": "Thing", "traits": [ { "ref": "ThingAlpha" }, { "ref": "ThingBeta" } ] }
      ]
    }
  ]
}"#;

fn inline_fixture(files: &[(&str, String)], consumer: &str) -> orbital_core::schema::OrbitalSchema {
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

fn inline_traits_of(schema: &orbital_core::schema::OrbitalSchema) -> Vec<&TraitDefinition> {
    schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
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

#[test]
fn cross_atom_same_named_siblings_both_pulled_and_correctly_wired() {
    let schema = inline_fixture(
        &[
            ("atom-alpha.orb", atom_with_button("atom-alpha", "AlphaMain", "ALPHA_FIRE")),
            ("atom-beta.orb", atom_with_button("atom-beta", "BetaMain", "BETA_FIRE")),
        ],
        CROSS_ATOM_CONSUMER,
    );

    let traits = inline_traits_of(&schema);
    let names: Vec<&str> = traits.iter().map(|t| t.name.as_str()).collect();

    // BOTH atoms' buttons exist as distinct traits — no silent capture.
    let buttons: Vec<&&TraitDefinition> = traits
        .iter()
        .filter(|t| t.name.contains("InlineButtonRender1"))
        .collect();
    assert_eq!(
        buttons.len(),
        2,
        "both atoms' same-named siblings must be pulled as distinct traits; got {names:?}"
    );

    // Each parent's tree references the trait carrying ITS atom's event.
    for (parent_name, marker) in [("ThingAlpha", "ALPHA_FIRE"), ("ThingBeta", "BETA_FIRE")] {
        let parent = traits
            .iter()
            .find(|t| t.name == parent_name)
            .unwrap_or_else(|| panic!("{parent_name} present; got {names:?}"));
        let referenced = embed_ref_of(parent);
        let target = traits
            .iter()
            .find(|t| t.name == referenced)
            .unwrap_or_else(|| panic!("{parent_name} refs {referenced} which must exist; got {names:?}"));
        let target_json = serde_json::to_string(target).expect("serialize");
        assert!(
            target_json.contains(marker),
            "{parent_name} must reference its OWN atom's button (marker {marker}); \
             refs {referenced} whose body lacks it"
        );
    }
}

fn atom_with_listening_sibling() -> String {
    r#"{
  "name": "test-listen-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SrcOrbital",
      "entity": {
        "name": "Src",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "HostMain",
          "linkedEntity": "Src",
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
                    "children": [ "@trait.GridView" ]
                  }]
                ]
              }
            ]
          }
        },
        {
          "name": "GridView",
          "linkedEntity": "Src",
          "category": "interaction",
          "listens": [
            { "event": "OPEN", "triggers": "REFRESH", "source": { "kind": "trait", "trait": "HostMain" } }
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
                  ["render-ui", "main", { "type": "data-list", "entity": "Src" }]
                ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "SrcPage", "path": "/src", "linkedEntity": "Src", "traits": [ { "ref": "HostMain" } ] }
      ]
    }
  ]
}"#
    .to_string()
}

const RENAMED_HOST_CONSUMER: &str = r#"{
  "name": "test-renamed-host-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AppOrbital",
      "uses": [ { "from": "./atom-listen.orb", "as": "Host" } ],
      "entity": {
        "name": "Thing",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Host.traits.HostMain", "linkedEntity": "Thing", "name": "ThingHost" }
      ],
      "pages": [
        { "name": "AppPage", "path": "/app", "linkedEntity": "Thing", "traits": [ { "ref": "ThingHost" } ] }
      ]
    }
  ]
}"#;

#[test]
fn pulled_sibling_listen_source_follows_host_rename() {
    let schema = inline_fixture(
        &[("atom-listen.orb", atom_with_listening_sibling())],
        RENAMED_HOST_CONSUMER,
    );

    let traits = inline_traits_of(&schema);
    let names: Vec<&str> = traits.iter().map(|t| t.name.as_str()).collect();
    let grid = traits
        .iter()
        .find(|t| t.name == "GridView")
        .unwrap_or_else(|| panic!("GridView pulled; got {names:?}"));

    let listens = grid
        .listens
        .as_ref()
        .expect("pulled sibling must KEEP its listen route (was dropped pre-fix)");
    assert_eq!(listens.len(), 1, "exactly the one authored route survives");
    match listens[0].source.as_ref() {
        Some(ListenSource::Trait { trait_name, .. }) => {
            assert_eq!(
                trait_name, "ThingHost",
                "listen source must be rewritten through the consumer's rename map"
            );
        }
        other => panic!("expected a trait-scoped source, got {other:?}"),
    }
}
