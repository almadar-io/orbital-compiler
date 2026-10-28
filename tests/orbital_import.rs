//! `orbital X = Alias.orbitals.Y { … }` (W3-R, `docs/Almadar_Orbital_Import.md`
//! §3-§6) — end-to-end proof that `resolve_orbital_reference` flattens an
//! imported orbital's whole trait/entity/page closure through the SAME
//! `inline_schema` pipeline every other reference form goes through.
//!
//! Pattern mirrors `tests/cross_orbital_pulled_sibling_uniquify.rs`: two JSON
//! `.orb` fixtures written to a temp dir, wired together by a relative `uses`
//! path, then `parse_file` + `inline_schema`. Case-specific white-box tests
//! (three-rung config fallback, omit-of-embedded, unknown-config-key,
//! `ListenSource::Trait` rewrite, all three navigate arities) live in
//! `src/phases/inline/orbital.rs`'s own `#[cfg(test)]` module, which calls
//! `resolve_orbital_reference` directly without the loader.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::{EntityRef, ListenSource, OrbitalSchema, PageRef, TraitReference};

/// One orbital ("ThingOrbital"): two traits (`ThingList` embeds
/// `@trait.ThingDetail` and listens on it via a `ListenSource::Orbital` —
/// 3-part `Orbital.Trait.EVENT` form), two aux entities (`ThingTag`, the
/// `[identity]` `ThingOwner`), a declared `config { pageSize }` forwarded by
/// `ThingList`, two pages, and both navigate-literal arities: a bare-string
/// target on `ThingList` and a `(str/concat "/things/" …)` target on
/// `ThingDetail`. The primary entity additionally carries a relation to the
/// identity aux (`ownerId`, compared to `@user.id` in `read_policy` — the
/// real-organism shape that trips `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED` when
/// its relation target isn't renamed) and a self-relation (`parentId`).
/// `ThingDetail` additionally carries `sourceBehavior`/`sourceEntityDefinition`
/// naming the (pre-rename) `Thing` entity — the atom-clone shape that trips
/// `ORB_T_ENTITY_NOT_REBINDABLE` when the source copy isn't renamed alongside
/// `linkedEntity`. `ThingList`'s `config` also carries an `AppLayout`-style
/// `navItems` default holding a bare upstream page path outside any
/// `(navigate …)` effect.
const UPSTREAM: &str = r#"{
  "name": "test-orbital-import-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ThingOrbital",
      "id": "orb_UPSTREAMORBITALID00000000",
      "config": { "pageSize": { "type": "number", "default": 10 } },
      "entity": {
        "name": "Thing",
        "persistence": "persistent",
        "read_policy": ["=", ["object/get", "@entity", "ownerId"], ["object/get", "@user", "id"]],
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "title", "type": "string" },
          { "name": "ownerId", "type": "relation", "relation": { "entity": "ThingOwner", "cardinality": "one" } },
          { "name": "parentId", "type": "relation", "relation": { "entity": "Thing", "cardinality": "one" } }
        ]
      },
      "auxiliaryEntities": [
        {
          "name": "ThingTag",
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true } ]
        },
        {
          "name": "ThingOwner",
          "identity": true,
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true } ]
        }
      ],
      "traits": [
        {
          "name": "ThingList",
          "linkedEntity": "Thing",
          "category": "interaction",
          "config": {
            "limit": { "type": "number", "default": "@config.pageSize" },
            "navItems": { "type": "array", "default": [ { "href": "/things", "label": "Things" } ] }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "OPEN",
                "effects": [
                  ["render-ui", "main", { "type": "stack", "children": ["@trait.ThingDetail"] }],
                  ["navigate", "/things/:id"]
                ]
              }
            ]
          },
          "listens": [
            { "event": "SAVED", "triggers": "OPEN", "source": { "kind": "orbital", "orbital": "ThingOrbital", "trait": "ThingDetail" } }
          ]
        },
        {
          "name": "ThingDetail",
          "linkedEntity": "Thing",
          "category": "interaction",
          "sourceBehavior": { "behavior": "std-thing-atom", "alias": "PageAtom", "originalName": "ThingDetail" },
          "sourceEntityDefinition": {
            "name": "Thing",
            "persistence": "persistent",
            "fields": [
              { "name": "id", "type": "string", "required": true },
              { "name": "parentId", "type": "relation", "relation": { "entity": "Thing", "cardinality": "one" } }
            ]
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "GO" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "GO",
                "effects": [
                  ["navigate", ["str/concat", "/things/", "@entity.id"]]
                ]
              }
            ]
          },
          "emits": [ { "event": "SAVED" } ]
        }
      ],
      "pages": [
        { "name": "ThingListPage", "path": "/things", "linkedEntity": "Thing", "traits": [ { "ref": "ThingList" } ] },
        { "name": "ThingDetailPage", "path": "/things/:id", "linkedEntity": "Thing", "traits": [ { "ref": "ThingDetail" } ] }
      ]
    }
  ]
}"#;

/// Two consumer orbitals, each importing `Up.orbitals.ThingOrbital` under a
/// different local name — proves two imports of one upstream are disjoint by
/// NAME (unconditional prefix) and by ID (fresh `derive_id` per import).
/// `FirstImport` exercises the full override surface (`fields`, `pages`,
/// `config`); `SecondImport` is a bare import (no overrides) so its trait
/// names/ids can be diffed against `FirstImport`'s without any override
/// muddying which differences come from the prefix vs. the overrides.
/// `FirstImport` also carries its OWN `id` — the id-stamping phase's
/// placeholder-orbital ledger row — which materialization must NOT overwrite
/// with an upstream-derived id (gap C, `ORB_ID_LEDGER_ORPHAN`).
const CONSUMER: &str = r#"{
  "name": "test-orbital-import-consumer",
  "version": "1.0.0",
  "ledger": {
    "schemaVersion": 1,
    "entries": {
      "orb_LOCALFIRSTIMPORTID000000": {
        "id": "orb_LOCALFIRSTIMPORTID000000",
        "kind": "orbital",
        "bakedName": "FirstImport",
        "curName": "FirstImport",
        "renames": [],
        "owner": "workspace"
      }
    }
  },
  "orbitals": [
    {
      "name": "FirstImport",
      "id": "orb_LOCALFIRSTIMPORTID000000",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.ThingOrbital",
        "fields": { "title": "heading" },
        "pages": { "/things": "/local-things", "/things/:id": "/local-things/:id" },
        "config": { "pageSize": { "type": "number", "default": 25 } }
      }
    },
    {
      "name": "SecondImport",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ThingOrbital" }
    }
  ]
}"#;

fn inline_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("orbital-import inline must succeed");
    schema
}

fn trait_names(schema: &OrbitalSchema, orbital_idx: usize) -> Vec<String> {
    schema.orbitals[orbital_idx]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            _ => None,
        })
        .collect()
}

fn find_trait<'a>(
    schema: &'a OrbitalSchema,
    orbital_idx: usize,
    name: &str,
) -> &'a orbital_core_base::schema::TraitDefinition {
    schema.orbitals[orbital_idx]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == name => Some(td),
            _ => None,
        })
        .unwrap_or_else(|| panic!("trait {name} not found in orbital {orbital_idx}"))
}

#[test]
fn after_inline_no_orbital_carries_reference() {
    let schema = inline_consumer();
    assert!(
        schema.orbitals.iter().all(|o| o.reference.is_none()),
        "reference must be cleared by inline: {:?}",
        schema
            .orbitals
            .iter()
            .map(|o| o.reference.is_some())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unconditional_prefix_renames_every_trait_and_entity() {
    let schema = inline_consumer();
    let names = trait_names(&schema, 0);
    assert!(
        names.contains(&"FirstImportThingList".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"FirstImportThingDetail".to_string()),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "ThingList" || n == "ThingDetail"),
        "{names:?}"
    );

    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    assert_eq!(entity.name, "FirstImportThing");
    let aux = schema.orbitals[0]
        .auxiliary_entities
        .as_ref()
        .expect("aux entities carried");
    assert!(aux
        .iter()
        .any(|e| matches!(e, EntityRef::Inline(d) if d.name == "FirstImportThingTag")));
}

#[test]
fn entity_rebind_and_fields_override_rewrite_field_tokens() {
    let schema = inline_consumer();
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    // `fields { title: heading }` renamed the entity's own field key.
    assert!(entity
        .fields
        .iter()
        .any(|f| f.name.as_deref() == Some("heading")));
    assert!(!entity
        .fields
        .iter()
        .any(|f| f.name.as_deref() == Some("title")));
}

#[test]
fn trait_embed_token_follows_the_rename() {
    let schema = inline_consumer();
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let effects = list.state_machine.transitions[0]
        .effects
        .as_ref()
        .expect("effects");
    let blob = serde_json::to_string(effects).unwrap();
    assert!(
        blob.contains("@trait.FirstImportThingDetail"),
        "embed token must follow the rename: {blob}"
    );
    assert!(!blob.contains("@trait.ThingDetail"), "{blob}");
}

#[test]
fn listen_source_orbital_kind_is_rewritten_to_the_local_orbital() {
    let schema = inline_consumer();
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let listens = list.listens.as_ref().expect("listens carried");
    let source = listens[0].source.as_ref().expect("source");
    match source {
        ListenSource::Orbital {
            orbital,
            trait_name,
            ..
        } => {
            assert_eq!(
                orbital, "FirstImport",
                "orbital must be renamed to the LOCAL orbital"
            );
            assert_eq!(trait_name, "FirstImportThingDetail");
        }
        other => panic!("expected ListenSource::Orbital, got {other:?}"),
    }
}

#[test]
fn pages_remap_and_navigate_targets_rewrite_all_forms() {
    let schema = inline_consumer();
    let pages = schema.orbitals[0].pages.as_ref().expect("pages carried");
    let paths: Vec<&str> = pages
        .iter()
        .map(|p| match p {
            PageRef::Inline(pd) => pd.path.as_str(),
            _ => panic!("page must be inline"),
        })
        .collect();
    assert!(paths.contains(&"/local-things"), "{paths:?}");
    assert!(paths.contains(&"/local-things/:id"), "{paths:?}");

    // Page trait mount lists follow the trait rename too.
    for p in pages {
        let PageRef::Inline(pd) = p else { continue };
        let refs: Vec<&str> = pd
            .traits
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|t| match t {
                TraitReference::Reference { reference, .. } => reference.as_deref(),
                _ => None,
            })
            .collect();
        assert!(
            refs.iter().all(|r| r.starts_with("FirstImport")),
            "{refs:?}"
        );
    }

    // Bare-string navigate literal on ThingList: "/things/:id" -> "/local-things/:id".
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let list_effects =
        serde_json::to_string(list.state_machine.transitions[0].effects.as_ref().unwrap()).unwrap();
    assert!(list_effects.contains("/local-things/:id"), "{list_effects}");
    assert!(!list_effects.contains("\"/things/:id\""), "{list_effects}");

    // `(str/concat "/things/" …)` navigate literal on ThingDetail: prefix
    // rewritten against the ":id"-stripped page-path map.
    let detail = find_trait(&schema, 0, "FirstImportThingDetail");
    let detail_effects = serde_json::to_string(
        detail.state_machine.transitions[0]
            .effects
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert!(
        detail_effects.contains("/local-things/"),
        "{detail_effects}"
    );
    assert!(!detail_effects.contains("/things/"), "{detail_effects}");
}

#[test]
fn config_override_folds_and_forwards_to_the_declaring_trait() {
    let schema = inline_consumer();
    assert_eq!(
        schema.orbitals[0]
            .config
            .as_ref()
            .and_then(|c| c.get("pageSize"))
            .and_then(|f| f.default.clone()),
        Some(orbital_core_base::schema::SExpression::Number(25.0)),
        "orbital.config must fold the call-site override"
    );
    let list = find_trait(&schema, 0, "FirstImportThingList");
    assert_eq!(
        list.config
            .as_ref()
            .and_then(|c| c.get("limit"))
            .and_then(|f| f.default.clone()),
        Some(orbital_core_base::schema::SExpression::Number(25.0)),
        "the forwarding trait's own declared default must resolve through the orbital rung"
    );
}

#[test]
fn two_imports_of_one_upstream_are_disjoint_by_name_and_id() {
    let schema = inline_consumer();
    let first_names = trait_names(&schema, 0);
    let second_names = trait_names(&schema, 1);
    assert!(
        second_names.contains(&"SecondImportThingList".to_string()),
        "{second_names:?}"
    );
    assert!(
        second_names.contains(&"SecondImportThingDetail".to_string()),
        "{second_names:?}"
    );
    let overlap: Vec<&String> = first_names
        .iter()
        .filter(|n| second_names.contains(n))
        .collect();
    assert!(
        overlap.is_empty(),
        "trait names must be disjoint: {overlap:?}"
    );

    let first_list = find_trait(&schema, 0, "FirstImportThingList");
    let second_list = find_trait(&schema, 1, "SecondImportThingList");
    assert_ne!(
        first_list.id, second_list.id,
        "two imports of the SAME upstream trait must get disjoint derived ids"
    );
    assert!(first_list.id.is_some() && second_list.id.is_some());

    let EntityRef::Inline(first_entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline")
    };
    let EntityRef::Inline(second_entity) = &schema.orbitals[1].entity else {
        panic!("primary entity must be inline")
    };
    assert_ne!(
        first_entity.id, second_entity.id,
        "entity ids must also be disjoint per import"
    );

    // The bare (non-overridden) second import must ALSO get the pageSize
    // default forwarded — from the UPSTREAM declared config this time
    // (no call-site override), proving the orbital rung works with no `obj.config` too.
    let second_detail_list = find_trait(&schema, 1, "SecondImportThingList");
    assert_eq!(
        second_detail_list
            .config
            .as_ref()
            .and_then(|c| c.get("limit"))
            .and_then(|f| f.default.clone()),
        Some(orbital_core_base::schema::SExpression::Number(10.0)),
        "with no override, the forward must resolve to the upstream's own default"
    );
}

/// Gap (A): relation targets on the primary entity — a relation to an
/// auxiliary entity (`ownerId` -> the identity aux `ThingOwner`) and a
/// SELF-relation (`parentId` -> the primary entity itself) — must be renamed
/// to the materialized (prefixed) entity names, not left naming upstream's
/// un-prefixed entities.
#[test]
fn relation_targets_are_renamed_on_the_primary_entity() {
    let schema = inline_consumer();
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    let owner_id = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("ownerId"))
        .expect("ownerId field carried");
    assert_eq!(
        owner_id.relation.as_ref().map(|r| r.entity.as_str()),
        Some("FirstImportThingOwner"),
        "relation to the identity aux must be renamed"
    );
    let parent_id = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("parentId"))
        .expect("parentId field carried");
    assert_eq!(
        parent_id.relation.as_ref().map(|r| r.entity.as_str()),
        Some("FirstImportThing"),
        "the self-relation must be renamed to the materialized primary entity name"
    );
}

/// Gap (A), auxiliary-entity side: `ThingOwner` itself must be renamed and
/// marked `identity` still — the rename must not touch that flag, only the
/// name (proven separately) and any relation targets it carries (it has
/// none here, but the aux loop runs the same helper unconditionally).
#[test]
fn identity_aux_entity_survives_materialization_as_identity() {
    let schema = inline_consumer();
    let aux = schema.orbitals[0]
        .auxiliary_entities
        .as_ref()
        .expect("aux entities carried");
    let owner = aux
        .iter()
        .find_map(|e| match e {
            EntityRef::Inline(d) if d.name == "FirstImportThingOwner" => Some(d),
            _ => None,
        })
        .expect("FirstImportThingOwner aux entity carried");
    assert!(
        owner.identity,
        "identity flag must survive the rename untouched"
    );
}

/// Gap (B): `ThingDetail`'s `sourceEntityDefinition` names the pre-rename
/// `Thing` entity (the atom-clone provenance copy) — it must be renamed
/// alongside `linkedEntity` so the two agree post-import, exactly as they did
/// pre-import (both named `Thing` before materialization). Left un-renamed,
/// `validate_entity_rebind` reads this as a spurious rebind and fires
/// `ORB_T_ENTITY_NOT_REBINDABLE` on a trait that was never a rebind.
#[test]
fn source_entity_definition_is_renamed_to_match_the_renamed_linked_entity() {
    let schema = inline_consumer();
    let detail = find_trait(&schema, 0, "FirstImportThingDetail");
    assert_eq!(detail.linked_entity.as_deref(), Some("FirstImportThing"));
    let source = detail
        .source_entity_definition
        .as_ref()
        .expect("source_entity_definition carried (provenance must survive)");
    assert_eq!(
        source.name, "FirstImportThing",
        "source_entity_definition.name must track the same rename as linked_entity"
    );
    // The source copy's own relation field (a self-relation, same shape as
    // the primary entity's) must also be renamed.
    let parent_id = source
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("parentId"))
        .expect("source copy carries its own parentId relation field");
    assert_eq!(
        parent_id.relation.as_ref().map(|r| r.entity.as_str()),
        Some("FirstImportThing")
    );
}

/// Gap (C): the LOCAL placeholder orbital's own `id` (stamped by the
/// id-stamping phase, ahead of inline — the ledger already has a row for it)
/// must survive materialization unchanged. Overwriting it with an
/// upstream-derived id orphans that ledger row (`ORB_ID_LEDGER_ORPHAN`).
#[test]
fn local_placeholder_orbital_id_survives_materialization() {
    let schema = inline_consumer();
    assert_eq!(
        schema.orbitals[0].id.as_deref(),
        Some("orb_LOCALFIRSTIMPORTID000000"),
        "materialization must keep the LOCAL orbital's own id, not derive one from upstream"
    );
}

/// The bare (no local `id`) `SecondImport` orbital has no ledger row of its
/// own to orphan, so it still gets a fresh upstream-derived id — the id
/// survival rule in gap (C) is "keep local when present", not "never derive".
#[test]
fn an_import_with_no_local_id_still_gets_an_upstream_derived_id() {
    let schema = inline_consumer();
    assert!(
        schema.orbitals[1].id.is_some(),
        "an import with no pre-stamped local id must still get a derived id"
    );
    assert_ne!(
        schema.orbitals[1].id.as_deref(),
        Some("orb_UPSTREAMORBITALID00000000"),
        "the derived id must not be the upstream orbital's own id verbatim"
    );
}

/// A config-default string equal to an upstream page path
/// (`ThingList.config.navItems[0].href == "/things"`) is data: a `pages`
/// remap never rewrites it. The call site overrides its own links.
#[test]
fn config_default_equal_to_a_remapped_page_path_stays_verbatim() {
    let schema = inline_consumer();
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let nav_items = list
        .config
        .as_ref()
        .and_then(|c| c.get("navItems"))
        .and_then(|f| f.default.clone())
        .expect("navItems default carried");
    let blob = serde_json::to_string(&nav_items).unwrap();
    assert!(blob.contains("\"/things\""), "{blob}");
    assert!(!blob.contains("/local-things"), "{blob}");
}

/// Control: on the same import the `(navigate …)` target IS remapped.
#[test]
fn control_navigate_target_on_the_same_import_is_remapped() {
    let schema = inline_consumer();
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let blob = serde_json::to_string(&list.state_machine.transitions).unwrap();
    assert!(blob.contains("\"/local-things/:id\""), "{blob}");
}

/// The real-organism proof's acceptance bar, reproduced at fixture scale:
/// none of the materialization-gap codes fire on this schema.
/// `validate_comprehensive_without_services` is used (not just `inline_schema`) because these
/// are POST-inline structural checks (`trait_validator`, `entity_access`,
/// `entity`/`cross_orbital::relations`) — inlining alone can't observe them.
#[test]
fn materialized_schema_trips_none_of_the_gap_codes() {
    let schema = inline_consumer();
    let result = validate_comprehensive_without_services(&schema);
    let codes: Vec<&str> = result.errors.iter().map(|e| e.code.as_str()).collect();
    for forbidden in [
        "ORB_E_INVALID_RELATION",
        "ORB_X_UNRESOLVED_RELATION_TARGET",
        "ORB_T_ENTITY_NOT_REBINDABLE",
        "ORB_ID_LEDGER_ORPHAN",
    ] {
        assert!(!codes.contains(&forbidden), "{forbidden} fired: {codes:?}");
    }
}

fn page_names(schema: &OrbitalSchema, orbital_idx: usize) -> Vec<String> {
    schema.orbitals[orbital_idx]
        .pages
        .as_ref()
        .expect("pages carried")
        .iter()
        .map(|p| match p {
            PageRef::Inline(pd) => pd.name.clone(),
            _ => panic!("page must be inline"),
        })
        .collect()
}

/// Gap (E): a materialized page's NAME must get the same unconditional
/// prefix traits/entities already get — `PageDefinition.name` (the `as
/// Name`, or the path-derived name when there is none) was being copied
/// verbatim from upstream while `path` was remapped, so `FirstImport`'s
/// pages still carried upstream's bare `ThingListPage`/`ThingDetailPage`
/// names.
#[test]
fn materialized_page_name_is_prefixed_with_local_orbital_name() {
    let schema = inline_consumer();
    let names = page_names(&schema, 0);
    assert!(
        names.contains(&"FirstImportThingListPage".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"FirstImportThingDetailPage".to_string()),
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n == "ThingListPage" || n == "ThingDetailPage"),
        "{names:?}"
    );
}

/// Gap (E) regression: two imports of one upstream — `FirstImport` (with a
/// `pages {}` path remap) and `SecondImport` (bare, no override) — must not
/// collide on page NAME even though only one of them remaps paths. Checked
/// two ways: (1) the flattened schema's own page-name sets are disjoint;
/// (2) the schema compiled all the way to OIR (`compile_schema` ->
/// `OirValidator`, the exact path `orb validate`'s plain-text output goes
/// through — see `run_full_validation` in
/// `crates/orbital-cli/src/commands/validate.rs`) reports no
/// "Duplicate page name" error. The duplicate-page-name check lives in
/// `orbital_core::ir::validation` (OIR-level), NOT in the compiler-crate
/// `validate_comprehensive_without_services` used by the test above (which only checks page
/// PATH uniqueness, `phases/validation/page.rs`) — so this test exercises
/// the OIR path directly rather than `validate_comprehensive_without_services`.
#[test]
fn two_imports_of_one_upstream_produce_no_duplicate_page_names() {
    let schema = inline_consumer();
    let first_names = page_names(&schema, 0);
    let second_names = page_names(&schema, 1);
    assert!(
        first_names.iter().all(|n| !second_names.contains(n)),
        "page names collide across imports: {first_names:?} / {second_names:?}"
    );

    let mut options = orbital_compiler::CompilerOptions::default();
    options.inline.skip_inline = true; // already inlined above
    let module = orbital_compiler::compile_schema(schema, &options)
        .expect("compile already-inlined schema to OIR");
    let oir_result = orbital_core_base::ir::OirValidator::new(&module).validate();
    let dup_page_name_errors: Vec<&orbital_core_base::ir::ValidationError> = oir_result
        .errors
        .iter()
        .filter(|e| e.message.contains("Duplicate page name"))
        .collect();
    assert!(
        dup_page_name_errors.is_empty(),
        "duplicate-page-name OIR error(s): {dup_page_name_errors:?}"
    );
}

fn page_ids(schema: &OrbitalSchema, orbital_idx: usize) -> Vec<String> {
    schema.orbitals[orbital_idx]
        .pages
        .as_ref()
        .expect("pages carried")
        .iter()
        .map(|p| match p {
            PageRef::Inline(pd) => pd.id.clone().expect("materialized page carries an id"),
            _ => panic!("page must be inline"),
        })
        .collect()
}

/// Two-path contract: a materialized page's ID is re-derived per import
/// exactly like traits/entities (JS `asPageId(deriveMaterializedId(...))`),
/// so two imports of one upstream never share a page id.
#[test]
fn two_imports_of_one_upstream_produce_distinct_page_ids() {
    let schema = inline_consumer();
    let first = page_ids(&schema, 0);
    let second = page_ids(&schema, 1);
    assert_eq!(first.len(), page_names(&schema, 0).len());
    assert!(
        first.iter().all(|id| !second.contains(id)),
        "page ids collide across imports: {first:?} / {second:?}"
    );
    let mut all: Vec<&String> = first.iter().chain(second.iter()).collect();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        first.len() + second.len(),
        "page ids must be unique within an import too"
    );
}

/// Embedder-forward-through-splice fixture (std-browse's `MasterListView`
/// shape): `Embedder` embeds `@trait.Sibling` and declares its OWN
/// `itemClickEvent` config default as the literal `"VIEW"`; `Sibling`
/// forwards `itemClickEvent` (`default "@config.itemClickEvent"`), emits it
/// (`emits: [{event: "@config.itemClickEvent"}]`), and its state machine's
/// only declared event KEY is that same forward string — the Option B
/// definer contract (`config_ref_event_knob`/`resolve_config_ref_event_name`,
/// `orbital-core/src/schema/types.rs`): `resolve_config_ref_emit_names`
/// resolves an `emits[].event` config-ref against the trait's own effective
/// config and applies the SAME resolved literal to any `state_machine.
/// events[].key` that still reads the raw forward string. Before the fix,
/// the orbital-import splice always resolved a pulled sibling's forward with
/// `parent_name: None` (no embedder lookup), so `Sibling`'s forward never
/// saw `Embedder`'s `"VIEW"` and both the config default and the SM event
/// key stayed the unresolved `"@config.itemClickEvent"` string all the way
/// through `inline_schema`.
const CONFIG_FORWARD_UPSTREAM: &str = r#"{
  "name": "test-orbital-import-config-forward-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MLOrbital",
      "entity": {
        "name": "MLWidget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "Embedder",
          "linkedEntity": "MLWidget",
          "category": "interaction",
          "config": { "itemClickEvent": { "type": "string", "default": "VIEW" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "OPEN",
                "effects": [
                  ["render-ui", "main", { "type": "stack", "children": ["@trait.Sibling"] }]
                ]
              }
            ]
          }
        },
        {
          "name": "Sibling",
          "linkedEntity": "MLWidget",
          "category": "interaction",
          "config": { "itemClickEvent": { "type": "string", "default": "@config.itemClickEvent" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "@config.itemClickEvent" } ],
            "transitions": []
          },
          "emits": [ { "event": "@config.itemClickEvent" } ]
        }
      ]
    }
  ]
}"#;

const CONFIG_FORWARD_CONSUMER: &str = r#"{
  "name": "test-orbital-import-config-forward-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "LocalML",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.MLOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.MLOrbital" }
    }
  ]
}"#;

#[test]
fn config_forward_resolves_against_embedder_through_orbital_import() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), CONFIG_FORWARD_UPSTREAM)
        .expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONFIG_FORWARD_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("orbital-import inline must succeed");

    let sibling = find_trait(&schema, 0, "LocalMLSibling");
    assert_eq!(
        sibling
            .config
            .as_ref()
            .and_then(|c| c.get("itemClickEvent"))
            .and_then(|f| f.default.clone()),
        Some(orbital_core_base::schema::SExpression::String(
            ("VIEW".to_string()).into()
        )),
        "config forward must resolve against the embedder (Embedder), not stay the raw \
         @config.itemClickEvent token"
    );
    let events = sibling
        .state_machine
        .events
        .as_ref()
        .expect("Sibling declares one event");
    assert_eq!(
        events.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
        vec!["VIEW"],
        "the state-machine event key (Option B definer) must resolve end-to-end to the \
         embedder-forwarded literal, not stay @config.itemClickEvent"
    );
}

// ============================================================================
// Stage B: roles / entities / mounts+siblings / page-forward-ordering,
// proven through the REAL multi-orbital `uses` loader (not
// `InlineContext::add_alias_multi` directly, as the white-box tests in
// `orbital.rs` use) — a separate upstream/consumer pair (mirrors the
// `CONFIG_FORWARD_UPSTREAM`/`CONFIG_FORWARD_CONSUMER` pattern above) rather
// than growing `UPSTREAM`/`CONSUMER`, so the ~20 pre-existing assertions
// above stay byte-for-byte unaffected by Stage B's new vocabulary.
// ============================================================================

/// Two orbitals under one alias: `MainOrbital` (primary `Widget`, aux
/// identity `WidgetOwner` with a `role` vocabulary) relates to `Other` —
/// declared by the SECOND orbital, `OtherOrbital`, of the SAME upstream
/// file/alias (the "out-of-orbital" shape `entities {}` remaps). `MainView`
/// forwards the orbital's `navItemsForward` knob (page-path-rewrite-after-
/// forward) and its `LOAD` guard carries a role-literal comparison.
const STAGE_B_UPSTREAM: &str = r#"{
  "name": "test-orbital-import-stage-b-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MainOrbital",
      "config": {
        "navItemsForward": { "type": "array", "default": [ { "href": "/main", "label": "Main" } ] }
      },
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "otherId", "type": "relation", "relation": { "entity": "Other", "cardinality": "one" } }
        ]
      },
      "auxiliaryEntities": [
        {
          "name": "WidgetOwner",
          "identity": true,
          "persistence": "persistent",
          "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "role", "type": "string", "values": ["employee", "approver"] }
          ]
        }
      ],
      "traits": [
        {
          "name": "MainView",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": {
            "navItemsViaForward": { "type": "array", "default": "@config.navItemsForward" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD" } ],
            "transitions": [
              {
                "from": "idle", "to": "idle", "event": "LOAD",
                "guard": ["=", "@user.role", "approver"],
                "effects": [ ["fetch", "Other", {}] ]
              }
            ]
          }
        }
      ],
      "pages": [
        { "name": "MainPage", "path": "/main", "primaryEntity": "Widget", "traits": [ { "ref": "MainView" } ] }
      ]
    },
    {
      "name": "OtherOrbital",
      "entity": {
        "name": "Other",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": []
    }
  ]
}"#;

/// `OtherHolder` is a plain (non-reference) orbital declaring `LocalOther` —
/// the consumer entity `entities { Other: LocalOther }` repoints the
/// out-of-orbital relation at. `LocalMain` imports `MainOrbital` with
/// `roles`/`entities`/`pages`/`mounts` all exercised at once, and its
/// `traits` array carries `LocalSidebar` — a sibling authored directly
/// alongside the `reference` (the shape `orbital-lolo`'s lowering produces
/// for a reference-body trait declaration).
const STAGE_B_CONSUMER: &str = r#"{
  "name": "test-orbital-import-stage-b-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "OtherHolder",
      "entity": {
        "name": "LocalOther",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": []
    },
    {
      "name": "LocalMain",
      "uses": [ { "from": "./upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.MainOrbital.entity",
      "traits": [
        {
          "name": "LocalSidebar",
          "linkedEntity": "LocalMainWidget",
          "category": "interaction",
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }
      ],
      "reference": {
        "ref": "Up.orbitals.MainOrbital",
        "roles": { "approver": ["owner"] },
        "entities": { "Other": "LocalOther" },
        "pages": { "/main": "/local-main" },
        "mounts": { "/main": ["LocalSidebar"] }
      }
    }
  ]
}"#;

fn inline_stage_b_consumer() -> (
    OrbitalSchema,
    Vec<orbital_compiler::phases::inline::OrbitalRefDiagnostic>,
) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), STAGE_B_UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, STAGE_B_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let mut orbital_diags = Vec::new();
    let mut uses_diags = Vec::new();
    orbital_compiler::phases::inline::inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut orbital_diags,
        &mut uses_diags,
    )
    .expect("stage-b orbital-import inline must succeed");
    (schema, orbital_diags)
}

/// `LocalMain` is the SECOND orbital in `schema.orbitals` (`OtherHolder`
/// is first).
fn stage_b_local_main(schema: &OrbitalSchema) -> &orbital_core_base::schema::Orbital {
    &schema.orbitals[1]
}

#[test]
fn stage_b_roles_remap_the_upstream_guard_through_the_real_loader() {
    let (schema, diagnostics) = inline_stage_b_consumer();
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].role_unknown_upstream.is_empty(),
        "{diagnostics:?}"
    );
    assert_eq!(
        diagnostics[0].role_targets,
        vec![(
            "role".to_string(),
            "approver".to_string(),
            vec!["owner".to_string()]
        )]
    );
    let main = stage_b_local_main(&schema);
    let view = main
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "LocalMainMainView" => Some(td),
            _ => None,
        })
        .expect("LocalMainMainView carried");
    let blob = serde_json::to_string(&view.state_machine.transitions).unwrap();
    assert!(blob.contains("\"owner\""), "{blob}");
    assert!(!blob.contains("\"approver\""), "{blob}");
}

/// `entities { Other: LocalOther }` rewrites BOTH the relation target on
/// the materialized primary entity and the `["fetch", "Other", …]` token
/// inside `MainView`, through the real loader pipeline.
#[test]
fn stage_b_entities_remap_through_the_real_loader() {
    let (schema, diagnostics) = inline_stage_b_consumer();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].entity_unmapped.is_empty());
    assert!(diagnostics[0].entity_unknown_key.is_empty());
    assert!(diagnostics[0].entity_target_unknown.is_empty());
    let main = stage_b_local_main(&schema);
    let EntityRef::Inline(entity) = &main.entity else {
        panic!("primary must be inline")
    };
    let other_id = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("otherId"))
        .unwrap();
    assert_eq!(other_id.relation.as_ref().unwrap().entity, "LocalOther");

    let view = main
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "LocalMainMainView" => Some(td),
            _ => None,
        })
        .unwrap();
    let blob = serde_json::to_string(&view.state_machine.transitions).unwrap();
    assert!(blob.contains("\"LocalOther\""), "{blob}");
    assert!(!blob.contains("\"Other\""), "{blob}");
}

/// A forwarded config value (`@config.navItemsForward`) carrying an upstream
/// page path reaches the trait verbatim: no remap of config data.
#[test]
fn stage_b_forwarded_config_page_path_stays_verbatim() {
    let (schema, _diagnostics) = inline_stage_b_consumer();
    let main = stage_b_local_main(&schema);
    let view = main
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "LocalMainMainView" => Some(td),
            _ => None,
        })
        .unwrap();
    let nav = view
        .config
        .as_ref()
        .and_then(|c| c.get("navItemsViaForward"))
        .and_then(|f| f.default.clone())
        .expect("navItemsViaForward resolved");
    let blob = serde_json::to_string(&nav).unwrap();
    assert!(blob.contains("\"/main\""), "{blob}");
    assert!(!blob.contains("/local-main"), "{blob}");
}

/// The sibling `LocalSidebar` rides along un-prefixed and mounts on the
/// remapped `/local-main` page.
#[test]
fn stage_b_sibling_mounts_on_the_remapped_page_through_the_real_loader() {
    let (schema, _diagnostics) = inline_stage_b_consumer();
    let main = stage_b_local_main(&schema);
    let names: Vec<&str> = main
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"LocalSidebar"), "{names:?}");
    assert!(names.contains(&"LocalMainMainView"), "{names:?}");

    let pages = main.pages.as_ref().expect("pages carried");
    let PageRef::Inline(page) = &pages[0] else {
        panic!("page must be inline")
    };
    assert_eq!(page.path, "/local-main");
    let mounted = page.traits.as_ref().unwrap().iter().any(|tr| match tr {
        TraitReference::Reference {
            reference: Some(r), ..
        } => r == "LocalSidebar",
        _ => false,
    });
    assert!(mounted, "{:?}", page.traits);
}

// ============================================================================
// G4 (`Almadar_Compiler_Gaps.md` §82): a two-hop import (atom -> organism via
// a plain trait reference -> top-level consumer via an orbital reference)
// must re-derive an entity-shaped emit payload (`type X = Event <Entity>`,
// `EmitDefinition.payload_entity`) from the MATERIALIZED entity — after the
// top-level consumer's `fields{}` rename AND `extend{}` addition — not leave
// it frozen at the leaf atom's own compile-time shape.
// ============================================================================

/// Leaf atom: one orbital, one entity (`Item`: `id`, `hourlyRate`), one
/// trait (`ItemLog`) whose `ItemUpdated` emit is the bare `type X = Event
/// Item` alias form — `payloadEntity: "Item"`, and a `payloadSchema` frozen
/// at ITS OWN field list (exactly what `orbital-lolo`'s `bare_entity_alias_
/// name` + `resolve_payload_type` produce for a standalone atom compile).
const G4_ATOM: &str = r#"{
  "name": "test-g4-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ItemAtomOrbital",
      "id": "orb_G4ATOMORBITALID000000000",
      "entity": {
        "name": "Item",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "hourlyRate", "type": "number" }
        ]
      },
      "traits": [
        {
          "name": "ItemLog",
          "id": "trt_G4ATOMITEMLOGID00000000",
          "linkedEntity": "Item",
          "category": "interaction",
          "emits": [
            {
              "event": "ItemUpdated",
              "payloadEntity": "Item",
              "payloadSchema": [
                { "name": "id", "type": "string", "required": true },
                { "name": "hourlyRate", "type": "number" }
              ]
            }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": []
          }
        }
      ]
    }
  ]
}"#;

/// Middle organism: declares its OWN `Item` entity (same shape as the
/// atom's) and composes `ItemLog` by a PLAIN trait reference (§8) with NO
/// override at this hop — the exact `trait TimeEntryLog = TimeEntry.traits.
/// TimeEntryLog {}` shape `std-time-tracking.lolo` uses. Stays an
/// UNRESOLVED `TraitReference::Reference` in this file's own `.orb` (L1
/// never splices a reference) — the real splice happens when a consumer's
/// `inline_orbital` recursively loads and inlines this file, exactly as
/// `resolve_orbital_reference`'s own doc describes.
const G4_ORGANISM: &str = r#"{
  "name": "test-g4-organism",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ItemOrganismOrbital",
      "id": "orb_G4ORGANISMORBITALID00000",
      "uses": [ { "from": "./g4-atom.orb", "as": "Atom" } ],
      "entity": {
        "name": "Item",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "hourlyRate", "type": "number" }
        ]
      },
      "traits": [
        { "name": "ItemLog", "ref": "Atom.traits.ItemLog" }
      ]
    }
  ]
}"#;

/// Top-level consumer: `orbital ConsumerOrbital = Organism.orbitals.
/// ItemOrganismOrbital { fields{hourlyRate:billRate} extend{workspaceId} }`
/// — the exact `project-friday.lolo` `TimeEntryOrbital` shape (§8b).
const G4_CONSUMER: &str = r#"{
  "name": "test-g4-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ConsumerOrbital",
      "id": "orb_G4CONSUMERORBITALID00000",
      "uses": [ { "from": "./g4-organism.orb", "as": "Organism" } ],
      "entity": "Organism.orbitals.ItemOrganismOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Organism.orbitals.ItemOrganismOrbital",
        "fields": { "hourlyRate": "billRate" },
        "extend": [
          { "name": "workspaceId", "type": "string", "default": "" }
        ]
      }
    }
  ]
}"#;

fn inline_g4_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g4-atom.orb"), G4_ATOM).expect("write atom");
    std::fs::write(dir.path().join("g4-organism.orb"), G4_ORGANISM).expect("write organism");
    let consumer_path = dir.path().join("g4-consumer.orb");
    std::fs::write(&consumer_path, G4_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("two-hop orbital-import inline must succeed");
    schema
}

#[test]
fn two_hop_import_recomputes_entity_shaped_emit_payload_after_rename_and_extend() {
    let schema = inline_g4_consumer();
    let item_log = find_trait(&schema, 0, "ConsumerOrbitalItemLog");
    let emit = item_log
        .emits
        .as_ref()
        .expect("emits carried")
        .iter()
        .find(|e| e.event == "ItemUpdated")
        .expect("ItemUpdated emit carried");

    let names: Vec<&str> = emit
        .payload_schema
        .as_ref()
        .expect("payloadSchema carried")
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["id", "billRate", "workspaceId"],
        "the payload must mirror the MATERIALIZED entity — the renamed \
         `billRate` and the extended `workspaceId` — not the atom's frozen \
         'hourlyRate' shape: {names:?}"
    );
    assert_eq!(
        emit.payload_entity.as_deref(),
        Some("ConsumerOrbitalItem"),
        "the marker itself must follow the primary entity's own \
         unconditional prefix-rename (no explicit `entity` override in \
         this reference, so 'Item' -> 'ConsumerOrbitalItem'), or the later \
         lookup by name misses"
    );

    let entity_fields: Vec<&str> = match &schema.orbitals[0].entity {
        EntityRef::Inline(def) => def
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect(),
        other => panic!("entity must be materialized inline, got {other:?}"),
    };
    assert_eq!(
        names, entity_fields,
        "the payload's field set must equal the materialized entity's own \
         field set exactly, field for field"
    );
}

/// G1 (`docs/Almadar_Compiler_Gaps.md` §82): cross-import listen sources.
/// One upstream file, TWO orbitals of the SAME alias — `AlphaOrbital`'s
/// `AlphaCreate` trait, and `BetaOrbital`'s `BetaListener` trait, which
/// listens on `AlphaOrbital.AlphaCreate.CREATED` via the 3-part
/// `Orbital.Trait.EVENT` form (mirrors `std-realtime-chat`'s
/// `ChannelCreate` listening on `ChatMessageOrbital.ChatRoom.CREATE_CHANNEL`).
/// A consumer that imports BOTH orbitals must re-point that source at
/// whichever LOCAL name each import was given, regardless of which
/// `orbital X = …` line comes first in the consumer file.
const G1_UPSTREAM: &str = r#"{
  "name": "test-g1-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AlphaOrbital",
      "id": "orb_G1ALPHAORBITALID000000000",
      "entity": {
        "name": "AlphaThing",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "AlphaCreate",
          "id": "trt_G1ALPHACREATEID000000000",
          "linkedEntity": "AlphaThing",
          "category": "interaction",
          "emits": [ { "event": "CREATED" } ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "CREATE", "effects": [ [ "emit", "CREATED" ] ] } ]
          }
        }
      ]
    },
    {
      "name": "BetaOrbital",
      "id": "orb_G1BETAORBITALID0000000000",
      "entity": {
        "name": "BetaThing",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "BetaListener",
          "id": "trt_G1BETALISTENERID00000000",
          "linkedEntity": "BetaThing",
          "category": "interaction",
          "listens": [
            {
              "event": "SEEN",
              "triggers": "REFRESH",
              "source": { "kind": "orbital", "orbital": "AlphaOrbital", "trait": "AlphaCreate" }
            }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "REFRESH" } ]
          }
        }
      ]
    }
  ]
}"#;

/// Both imports declared, `LocalAlpha` (the listen source's TARGET) FIRST —
/// the order the old `orbital_rename`-only code happened to require nothing
/// special for, since neither import benefited from the fix either way.
const G1_CONSUMER_ALPHA_FIRST: &str = r#"{
  "name": "test-g1-consumer-alpha-first",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "LocalAlpha",
      "id": "orb_G1LOCALALPHAID0000000000",
      "uses": [ { "from": "./g1-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.AlphaOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.AlphaOrbital" }
    },
    {
      "name": "LocalBeta",
      "id": "orb_G1LOCALBETAID00000000000",
      "uses": [ { "from": "./g1-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.BetaOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.BetaOrbital" }
    }
  ]
}"#;

/// Same two imports, declaration order REVERSED — `LocalBeta` (whose trait
/// carries the listen source) resolves FIRST, before `LocalAlpha` (the
/// source's target) has resolved at all. Proves the fix is order-
/// independent: the schema-wide precompute (`precompute_reference_form_
/// listen_targets`) is built before EITHER orbital splices.
const G1_CONSUMER_BETA_FIRST: &str = r#"{
  "name": "test-g1-consumer-beta-first",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "LocalBeta",
      "id": "orb_G1LOCALBETAID00000000000",
      "uses": [ { "from": "./g1-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.BetaOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.BetaOrbital" }
    },
    {
      "name": "LocalAlpha",
      "id": "orb_G1LOCALALPHAID0000000000",
      "uses": [ { "from": "./g1-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.AlphaOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.AlphaOrbital" }
    }
  ]
}"#;

fn inline_g1_consumer(consumer_json: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g1-upstream.orb"), G1_UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("g1-consumer.orb");
    std::fs::write(&consumer_path, consumer_json).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("cross-import listen-source inline must succeed");
    schema
}

/// Asserts `LocalBetaBetaListener`'s listen source resolves to
/// `orbital: "LocalAlpha", trait: "LocalAlphaAlphaCreate"` WITH a `traitId`
/// — same assertion for both declaration orders (`schema` locates
/// `LocalBeta` at whichever orbital index it landed at).
fn assert_beta_listener_resolves_to_local_alpha(schema: &OrbitalSchema) {
    let beta_idx = schema
        .orbitals
        .iter()
        .position(|o| o.name == "LocalBeta")
        .expect("LocalBeta orbital present");
    let listener = find_trait(schema, beta_idx, "LocalBetaBetaListener");
    let listens = listener.listens.as_ref().expect("listens carried");
    let source = listens[0].source.as_ref().expect("source");
    match source {
        ListenSource::Orbital {
            orbital,
            trait_name,
            trait_id,
            ..
        } => {
            assert_eq!(
                orbital, "LocalAlpha",
                "a cross-import source must re-point at the SIBLING import's \
                 local orbital name, not the un-prefixed upstream name"
            );
            assert_eq!(
                trait_name, "LocalAlphaAlphaCreate",
                "and at that sibling's own prefixed trait name"
            );
            assert!(
                trait_id.is_some(),
                "the rewritten source must carry the target trait's derived id"
            );
        }
        other => panic!("expected ListenSource::Orbital, got {other:?}"),
    }
}

#[test]
fn g1_cross_import_listen_source_resolves_alpha_declared_first() {
    let schema = inline_g1_consumer(G1_CONSUMER_ALPHA_FIRST);
    assert_beta_listener_resolves_to_local_alpha(&schema);
}

#[test]
fn g1_cross_import_listen_source_resolves_order_independent_beta_declared_first() {
    let schema = inline_g1_consumer(G1_CONSUMER_BETA_FIRST);
    assert_beta_listener_resolves_to_local_alpha(&schema);
}

/// G2 (`docs/Almadar_Compiler_Gaps.md` §82): `entity <Name>` does not
/// rewrite `entity`-typed config knob VALUES. Mirrors `std-hr-portal.lolo`'s
/// real shape: `WidgetSync`'s `targetEntity : entity` knob's literal default
/// names the SAME orbital's OWN primary entity ("Widget", exactly like
/// `TimeOffCalendarSync.config.targetEntity` defaults to `TimeOff`,
/// `TimeOffOrbital`'s own primary). `entity Gadget` on the reference clones
/// the primary as `GadgetOrbitalWidget` — the config VALUE must follow.
const G2_UPSTREAM: &str = r#"{
  "name": "test-g2-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SyncOrbital",
      "id": "orb_G2SYNCORBITALID000000000",
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "WidgetSync",
          "id": "trt_G2WIDGETSYNCID0000000000",
          "linkedEntity": "Widget",
          "category": "integration",
          "config": {
            "targetEntity": { "type": "entity", "default": "Widget" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "SYNC", "effects": [ [ "fetch", "@config.targetEntity" ] ] }
            ]
          }
        }
      ]
    }
  ]
}"#;

const G2_CONSUMER: &str = r#"{
  "name": "test-g2-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "GadgetOrbital",
      "id": "orb_G2GADGETORBITALID0000000",
      "uses": [ { "from": "./g2-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.SyncOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.SyncOrbital", "entity": "Gadget" }
    }
  ]
}"#;

fn inline_g2_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g2-upstream.orb"), G2_UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("g2-consumer.orb");
    std::fs::write(&consumer_path, G2_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("entity-typed config value inline must succeed");
    schema
}

#[test]
fn g2_entity_rebind_rewrites_entity_typed_config_knob_value() {
    let schema = inline_g2_consumer();
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    assert_eq!(entity.name, "Gadget", "explicit `entity Gadget` override");
    let primary_id = entity
        .id
        .clone()
        .expect("materialized primary has a derived id");

    let sync = find_trait(&schema, 0, "GadgetOrbitalWidgetSync");
    let config = sync.config.as_ref().expect("config carried");
    let target_entity = config
        .get("targetEntity")
        .expect("targetEntity knob carried");
    assert_eq!(
        target_entity.default,
        Some(orbital_core_base::schema::SExpression::String(
            ("Gadget".to_string()).into()
        )),
        "an `entity`-typed knob's literal default must follow the SAME \
         entity_subs rename every other entity reference goes through, not \
         stay pinned to the pre-rename upstream name"
    );
    assert_eq!(
        target_entity.ref_id.as_deref(),
        Some(primary_id.as_str()),
        "the V4-W5 dual-carry refId must be refreshed to the RENAMED \
         entity's current id, never left pointing at the stale upstream id"
    );
}

// ============================================================================
// G3 (`docs/Almadar_Compiler_Gaps.md` §82): `fields {}` reaches every
// structurally-recognized field-reference position, not just `@entity.<f>`.
// ============================================================================

/// One orbital (`DealOrbital`), entity `Deal { title, amount }` with a
/// `read_policy` reading `(object/get @entity title)` (the access-policy
/// shape G3 names), one trait `DealBoard` whose config carries every
/// position the gap doc calls out: a bare `[string]` field list
/// (`formFields`), a bare `string` field name (`searchField`), and an
/// `@item.<f>` binding (`cardTitleBinding`) — the row-alias sigil widened
/// alongside `@entity.`.
const G3_UPSTREAM: &str = r#"{
  "name": "test-g3-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "DealOrbital",
      "id": "orb_G3DEALORBITALID000000000",
      "entity": {
        "name": "Deal",
        "id": "ent_G3DEALENTITYID0000000000",
        "persistence": "persistent",
        "read_policy": ["=", ["object/get", "@entity", "title"], "@user.id"],
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "title", "type": "string" },
          { "name": "amount", "type": "number" }
        ]
      },
      "traits": [
        {
          "name": "DealBoard",
          "id": "trt_G3DEALBOARDID00000000000",
          "linkedEntity": "Deal",
          "category": "interaction",
          "config": {
            "formFields": { "type": "[string]", "default": ["title", "amount"] },
            "searchField": { "type": "string", "default": "title" },
            "cardTitleBinding": { "type": "string", "default": "@item.title" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT" } ]
          }
        }
      ]
    }
  ]
}"#;

const G3_CONSUMER: &str = r#"{
  "name": "test-g3-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "LeadOrbital",
      "id": "orb_G3LEADORBITALID000000000",
      "uses": [ { "from": "./g3-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.DealOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.DealOrbital",
        "entity": "Lead",
        "fields": { "title": "company", "amount": "value" }
      }
    }
  ]
}"#;

fn inline_g3_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g3-upstream.orb"), G3_UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("g3-consumer.orb");
    std::fs::write(&consumer_path, G3_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("field rename inline must succeed");
    schema
}

/// The bug: before this fix, `rewrite_entity_field_paths` only rewrote
/// `@entity.<f>` strings, so `formFields` (a bare `[string]` list),
/// `searchField` (a bare `string`), `cardTitleBinding` (`@item.<f>`, not
/// `@entity.<f>`), and the `read_policy`'s `(object/get @entity title)` all
/// kept naming the pre-rename upstream field — silently, since nothing
/// validated the mismatch. All four must now read `company`, never `title`.
#[test]
fn g3_fields_rename_reaches_bare_config_defaults_item_sigil_and_object_get_policy() {
    let schema = inline_g3_consumer();
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    assert_eq!(entity.name, "Lead");
    assert!(
        entity
            .fields
            .iter()
            .any(|f| f.name.as_deref() == Some("company")),
        "the renamed field must be named 'company' on the materialized entity, got {:?}",
        entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect::<Vec<_>>()
    );
    let read_policy = entity.read_policy.as_ref().expect("read_policy carried");
    assert_eq!(
        read_policy,
        &orbital_core_base::schema::SExpression::Array(vec![
            orbital_core_base::schema::SExpression::String(("=".to_string()).into()),
            orbital_core_base::schema::SExpression::Array(vec![
                orbital_core_base::schema::SExpression::String(("object/get".to_string()).into()),
                orbital_core_base::schema::SExpression::String(("@entity".to_string()).into()),
                orbital_core_base::schema::SExpression::String(("company".to_string()).into()),
            ]),
            orbital_core_base::schema::SExpression::String(("@user.id".to_string()).into()),
        ]),
        "`(object/get @entity title)` in the entity's OWN read_policy must \
         follow the rename to `company`, not stay pinned to 'title'"
    );

    let board = find_trait(&schema, 0, "LeadOrbitalDealBoard");
    let cfg = board.config.as_ref().expect("config carried");

    let form_fields = cfg.get("formFields").expect("formFields knob carried");
    assert_eq!(
        form_fields.default,
        Some(orbital_core_base::schema::SExpression::Array(vec![
            orbital_core_base::schema::SExpression::String(("company".to_string()).into()),
            orbital_core_base::schema::SExpression::String(("value".to_string()).into()),
        ])),
        "a bare `[string]` field-name list must be rewritten element-by-element"
    );

    let search_field = cfg.get("searchField").expect("searchField knob carried");
    assert_eq!(
        search_field.default,
        Some(orbital_core_base::schema::SExpression::String(
            ("company".to_string()).into()
        )),
        "a bare `string` field-name default must be rewritten"
    );

    let card_title_binding = cfg
        .get("cardTitleBinding")
        .expect("cardTitleBinding knob carried");
    assert_eq!(
        card_title_binding.default,
        Some(orbital_core_base::schema::SExpression::String(
            ("@item.company".to_string()).into()
        )),
        "`@item.<f>` must follow the SAME field rename `@entity.<f>` already did"
    );
}

// ============================================================================
// G5 (`docs/Almadar_Compiler_Gaps.md` §82): `events {}` reaches a SOURCED
// listen's source-side `event` key, when the source trait is within this
// same import, and re-derives the id it shares with the local declaration.
// ============================================================================

/// One orbital (`DocOrbital`), two traits: `Inbox` declares `emits {
/// APPROVE }`; `Decision` declares a LOCAL `APPROVE` event (`stateMachine.
/// events`, carrying the SHARED id `evt_G5APPROVEEVT0000000001`) and a
/// `ListenSource::Trait` entry sourced from `Inbox.APPROVE`, mapped to the
/// SAME local trigger. `events { APPROVE: ASSET_REVIEW_APPROVE }` on the
/// reference must rename BOTH `Inbox`'s emit key AND `Decision`'s sourced
/// listen's `event`/`triggers` keys, and re-derive the shared id so the
/// ledger's `curName` (backfilled at `APPROVE` by `ensure_stamped`, since
/// this fixture supplies no ledger of its own) stays truthful.
const G5_UPSTREAM: &str = r#"{
  "name": "test-g5-upstream",
  "version": "1.0.0",
  "ledger": {
    "schemaVersion": 1,
    "entries": {
      "evt_G5APPROVEEVT0000000001": {
        "id": "evt_G5APPROVEEVT0000000001",
        "kind": "event",
        "bakedName": "APPROVE",
        "curName": "APPROVE",
        "renames": [],
        "owner": "workspace",
        "parent": "trt_G5DECISIONID000000000"
      },
      "evt_G5INBOXEVT0000000002": {
        "id": "evt_G5INBOXEVT0000000002",
        "kind": "event",
        "bakedName": "APPROVE",
        "curName": "APPROVE",
        "renames": [],
        "owner": "workspace",
        "parent": "trt_G5INBOXID0000000000000"
      }
    }
  },
  "orbitals": [
    {
      "name": "DocOrbital",
      "id": "orb_G5DOCORBITALID000000000",
      "entity": {
        "name": "Document",
        "id": "ent_G5DOCUMENTENTITYID00000",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "Inbox",
          "id": "trt_G5INBOXID0000000000000",
          "linkedEntity": "Document",
          "category": "interaction",
          "emits": [ { "event": "APPROVE", "eventId": "evt_G5INBOXEVT0000000002", "scope": "internal" } ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT" } ]
          }
        },
        {
          "name": "Decision",
          "id": "trt_G5DECISIONID000000000",
          "linkedEntity": "Document",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "APPROVE", "id": "evt_G5APPROVEEVT0000000001" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "APPROVE",
                "eventId": "evt_G5APPROVEEVT0000000001"
              }
            ]
          },
          "listens": [
            {
              "event": "APPROVE",
              "eventId": "evt_G5APPROVEEVT0000000001",
              "source": { "kind": "trait", "trait": "Inbox" },
              "triggers": "APPROVE",
              "triggersId": "evt_G5APPROVEEVT0000000001"
            }
          ]
        }
      ]
    }
  ]
}"#;

const G5_CONSUMER: &str = r#"{
  "name": "test-g5-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AssetLibraryOrbital",
      "id": "orb_G5ASSETLIBRARYID0000000",
      "uses": [ { "from": "./g5-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.DocOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.DocOrbital",
        "events": { "APPROVE": "ASSET_REVIEW_APPROVE" }
      }
    }
  ]
}"#;

fn inline_g5_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g5-upstream.orb"), G5_UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("g5-consumer.orb");
    std::fs::write(&consumer_path, G5_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("event rename inline must succeed");
    schema
}

/// The bug: before this fix, `events {}` renamed `Decision`'s LOCAL trigger
/// key but left `Inbox`'s emit AND the sourced listen's `event` (source-side
/// key) at `APPROVE`, and never re-derived the shared id — so the ledger's
/// `curName` (backfilled `APPROVE` by `ensure_stamped`) disagreed with the
/// renamed declaration, which is exactly the `ORB_ID_NAME_MISMATCH` shape
/// `orb validate` caught on the full `g5-asset-events.lolo` repro.
#[test]
fn g5_events_rename_reaches_sourced_listen_and_rederives_the_shared_id() {
    let schema = inline_g5_consumer();

    let inbox = find_trait(&schema, 0, "AssetLibraryOrbitalInbox");
    let emits = inbox.emits.as_ref().expect("emits carried");
    assert_eq!(
        emits[0].event, "ASSET_REVIEW_APPROVE",
        "Inbox's OWN emit key must follow the orbital-scope events rename"
    );

    let decision = find_trait(&schema, 0, "AssetLibraryOrbitalDecision");
    let listens = decision.listens.as_ref().expect("listens carried");
    let listen = &listens[0];
    assert_eq!(
        listen.event, "ASSET_REVIEW_APPROVE",
        "the SOURCED listen's source-side `event` key must follow the \
         rename too — Inbox is within THIS import, so it shares the same \
         renamed vocabulary, not an independent call site's own rename"
    );
    assert_eq!(
        listen.triggers.as_deref(),
        Some("ASSET_REVIEW_APPROVE"),
        "the local trigger key must also be renamed (already worked pre-fix)"
    );
    assert_eq!(
        listen.event_id, listen.triggers_id,
        "the source-side and local-side ids must still agree post-rename"
    );
    let final_id = listen
        .event_id
        .clone()
        .expect("event id carried after rename");
    // This id has exactly ONE owner in the whole schema (`Decision`, and
    // nothing else), so the sole-owner branch of `reconcile_orbital_ref_
    // event_id_renames` renames the ledger entry IN PLACE rather than
    // minting a fresh id (which would orphan the original row and trip
    // `ORB_ID_LEDGER_ORPHAN` instead of fixing anything) — same id, updated
    // `curName`.
    assert_eq!(
        final_id, "evt_G5APPROVEEVT0000000001",
        "sole-owner case: the id itself must NOT change (only the ledger's \
         curName does) — minting a fresh id here would orphan the original \
         row with nothing left to explain it"
    );

    let ledger = schema.ledger.as_ref().expect("schema is stamped");
    let entry = ledger
        .entries
        .get(&final_id)
        .unwrap_or_else(|| panic!("ledger must carry a row for the id now in use: {final_id}"));
    assert_eq!(
        entry.cur_name, "ASSET_REVIEW_APPROVE",
        "the ledger's curName for whichever id the schema now uses must \
         agree with the renamed key — this is what makes ORB_ID_NAME_MISMATCH \
         disappear"
    );
    assert!(
        entry
            .renames
            .iter()
            .any(|r| r.from == "APPROVE" && r.to == "ASSET_REVIEW_APPROVE"),
        "the rename must be logged in the ledger's own history, got {:?}",
        entry.renames
    );
    assert!(
        ledger
            .entries
            .values()
            .flat_map(|e| e.renames.iter())
            .all(|r| r.at.is_empty()),
        "resolve must not stamp wall-clock time on rename records (G-ORB-107)"
    );

    // The event declaration + its transition must also carry the new id.
    let ev = decision
        .state_machine
        .events
        .as_ref()
        .expect("events carried")
        .iter()
        .find(|e| e.key == "ASSET_REVIEW_APPROVE")
        .expect("renamed event declaration carried");
    assert_eq!(ev.id.as_deref(), Some(final_id.as_str()));
    let transition = decision
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "ASSET_REVIEW_APPROVE")
        .expect("renamed transition carried");
    assert_eq!(transition.event_id.as_deref(), Some(final_id.as_str()));
}

// ============================================================================
// G3 follow-up (`docs/Almadar_Compiler_Gaps.md` §82): a `fields {}` rename
// must also reach a config-forward's DESTINATION, not just its declaring
// trait — a JSX-hoisted inline render's `columns: @config.columns` forward
// resolves to its embedder's declared literal AFTER this trait's own field
// rewrite already ran, in the SAME per-trait loop iteration.
// ============================================================================

/// One orbital (`TimeEntryPanelOrbital`), two traits: `TimeEntryPanel`
/// declares `columns : [ColumnSpec] = [{ field: hourlyRate, key: hourlyRate
/// }]` and embeds `@trait.InlineTableViewRender` in a render effect;
/// `InlineTableViewRender` (the JSX-hoisted sibling) declares its OWN
/// `columns` knob with default `"@config.columns"` — a forward token that
/// only resolves to the literal once `resolve_orbital_reference`'s
/// config-forward step runs, AFTER field-rewriting has already visited
/// `InlineTableViewRender` earlier in the same loop.
const G3_HOIST_UPSTREAM: &str = r#"{
  "name": "test-g3-hoist-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TimeEntryPanelOrbital",
      "id": "orb_G3HOISTORBITALID00000000",
      "entity": {
        "name": "TimeEntry",
        "id": "ent_G3HOISTENTITYID000000000",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "hourlyRate", "type": "number" }
        ]
      },
      "traits": [
        {
          "name": "TimeEntryPanel",
          "id": "trt_G3HOISTPANELID00000000",
          "linkedEntity": "TimeEntry",
          "category": "interaction",
          "config": {
            "columns": {
              "type": "[ColumnSpec]",
              "default": [ { "field": "hourlyRate", "key": "hourlyRate", "header": "Rate" } ]
            }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "INIT",
                "effects": [ [ "render-ui", "main", "@trait.InlineTableViewRender" ] ]
              }
            ]
          }
        },
        {
          "name": "InlineTableViewRender",
          "id": "trt_G3HOISTRENDERID0000000",
          "linkedEntity": "TimeEntry",
          "category": "interaction",
          "config": {
            "columns": { "type": "unknown", "default": "@config.columns" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT" } ]
          }
        }
      ]
    }
  ]
}"#;

const G3_HOIST_CONSUMER: &str = r#"{
  "name": "test-g3-hoist-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TimeEntryOrbital",
      "id": "orb_G3HOISTCONSUMERID000000",
      "uses": [ { "from": "./g3-hoist-upstream.orb", "as": "Up" } ],
      "entity": "Up.orbitals.TimeEntryPanelOrbital.entity",
      "traits": [],
      "reference": {
        "ref": "Up.orbitals.TimeEntryPanelOrbital",
        "entity": "TimeEntry",
        "fields": { "hourlyRate": "billRate" }
      }
    }
  ]
}"#;

fn inline_g3_hoist_consumer() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("g3-hoist-upstream.orb"), G3_HOIST_UPSTREAM)
        .expect("write upstream");
    let consumer_path = dir.path().join("g3-hoist-consumer.orb");
    std::fs::write(&consumer_path, G3_HOIST_CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("hoisted-render field rename inline must succeed");
    schema
}

/// The bug: the config-forward step that resolves `InlineTableViewRender`'s
/// `columns: @config.columns` reads `TimeEntryPanel`'s declared default from
/// `parent_configs` — a snapshot taken BEFORE the per-trait loop's field
/// rewrite ran on ANY trait — so the forwarded literal still names
/// `hourlyRate` even though `InlineTableViewRender` itself was already
/// field-rewritten (to no effect, since its own default was just the
/// `"@config.columns"` string at that point). Must now read `billRate` in
/// both `field` and `key`.
#[test]
fn g3_field_rename_reaches_a_hoisted_renders_forwarded_columns() {
    let schema = inline_g3_hoist_consumer();
    let render = find_trait(&schema, 0, "TimeEntryOrbitalInlineTableViewRender");
    let columns = render
        .config
        .as_ref()
        .and_then(|cfg| cfg.get("columns"))
        .and_then(|f| f.default.as_ref())
        .expect("columns default carried (forwarded)");
    let orbital_core_base::schema::SExpression::Array(items) = columns else {
        panic!("columns default must be an array, got {columns:?}");
    };
    let orbital_core_base::schema::SExpression::Object(entry) = &items[0] else {
        panic!("columns[0] must be an object, got {:?}", items[0]);
    };
    assert_eq!(
        entry.get("field"),
        Some(&orbital_core_base::schema::SExpression::String(
            ("billRate".to_string()).into()
        )),
        "the forwarded columns[0].field must follow the rename, not stay pinned to 'hourlyRate'"
    );
    assert_eq!(
        entry.get("key"),
        Some(&orbital_core_base::schema::SExpression::String(
            ("billRate".to_string()).into()
        )),
        "the forwarded columns[0].key must follow the rename, not stay pinned to 'hourlyRate'"
    );
}

/// One upstream orbital named like its import (so the import keeps unprefixed names) whose board embeds and
/// listens to an atom-internal child named `LoadingSpinner`.
fn desk_upstream(orbital: &str, board: &str) -> String {
    format!(
        r#"{{
  "name": "test-desk-{orbital}",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "{orbital}",
      "entity": {{ "name": "{orbital}Row", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [
        {{
          "name": "{board}",
          "linkedEntity": "{orbital}Row",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }}, {{ "key": "SPUN" }} ],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "stack", "children": ["@trait.LoadingSpinner"] }}] ] }},
              {{ "from": "idle", "to": "idle", "event": "SPUN", "effects": [] }}
            ]
          }},
          "listens": [ {{ "event": "SPIN", "triggers": "SPUN", "source": {{ "kind": "trait", "trait": "LoadingSpinner" }} }} ]
        }},
        {{
          "name": "LoadingSpinner",
          "linkedEntity": "{orbital}Row",
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "INIT" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", {{ "type": "spinner" }}] ] }} ]
          }},
          "emits": [ {{ "event": "SPIN" }} ]
        }}
      ],
      "pages": [ {{ "name": "{orbital}Page", "path": "/{board}", "traits": [ {{ "ref": "{board}" }}, {{ "ref": "LoadingSpinner" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn inline_desks(orbitals: &[(&str, &str)], watcher: bool) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut consumers = Vec::new();
    for (orbital, board) in orbitals {
        std::fs::write(dir.path().join(format!("{orbital}.orb")), desk_upstream(orbital, board)).expect("write upstream");
        consumers.push(format!(
            r#"{{ "name": "{orbital}", "uses": [ {{ "from": "./{orbital}.orb", "as": "Desk{orbital}" }} ], "entity": "Desk{orbital}.orbitals.{orbital}.entity", "traits": [], "reference": {{ "ref": "Desk{orbital}.orbitals.{orbital}" }} }}"#
        ));
    }
    if watcher {
        consumers.push(r#"{ "name": "WatchOrbital", "entity": { "name": "Watch", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] }, "traits": [ { "name": "Watcher", "linkedEntity": "Watch", "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [ { "key": "INIT" }, { "key": "SEEN" } ], "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "watch" }] ] }, { "from": "idle", "to": "idle", "event": "SEEN", "effects": [] } ] }, "listens": [ { "event": "SPIN", "triggers": "SEEN", "source": { "kind": "orbital", "orbital": "PostOrbital", "trait": "LoadingSpinner" } } ] } ], "pages": [ { "name": "WatchPage", "path": "/watch", "traits": [ { "ref": "Watcher" } ] } ] }"#.to_string());
    }
    let consumer = format!(r#"{{ "name": "test-desks", "version": "1.0.0", "orbitals": [ {} ] }}"#, consumers.join(", "));
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path)).expect("desks inline");
    schema
}

#[test]
fn two_imports_carrying_one_atom_internal_child_name_get_one_name_each() {
    let schema = inline_desks(&[("PostOrbital", "PostBoard"), ("CommentOrbital", "CommentBoard")], true);
    let post = trait_names(&schema, 0);
    let comment = trait_names(&schema, 1);
    assert!(post.contains(&"PostOrbitalLoadingSpinner".to_string()), "{post:?}");
    assert!(comment.contains(&"CommentOrbitalLoadingSpinner".to_string()), "{comment:?}");
    assert!(post.contains(&"PostBoard".to_string()) && comment.contains(&"CommentBoard".to_string()), "a name only one orbital carries keeps it");
    let board = serde_json::to_string(find_trait(&schema, 0, "PostBoard")).unwrap();
    assert!(board.contains("@trait.PostOrbitalLoadingSpinner"), "the embed follows the rename: {board}");
    let own_listen = find_trait(&schema, 0, "PostBoard").listens.as_ref().unwrap();
    assert!(
        own_listen.iter().any(|l| matches!(&l.source, Some(ListenSource::Trait { trait_name, .. }) if trait_name == "PostOrbitalLoadingSpinner")),
        "the orbital's own listen follows the rename"
    );
    let watcher = find_trait(&schema, 2, "Watcher").listens.as_ref().unwrap();
    assert!(
        watcher.iter().any(|l| matches!(&l.source, Some(ListenSource::Orbital { orbital, trait_name, .. }) if orbital == "PostOrbital" && trait_name == "PostOrbitalLoadingSpinner")),
        "another orbital's qualified listen follows the rename: {watcher:?}"
    );
    let page_refs: Vec<String> = schema.orbitals[0]
        .pages
        .iter()
        .flatten()
        .filter_map(|p| match p {
            PageRef::Inline(pd) => Some(pd),
            _ => None,
        })
        .flat_map(|pd| pd.traits.iter().flatten())
        .filter_map(|t| match t {
            TraitReference::Reference { reference, .. } => reference.clone(),
            _ => None,
        })
        .collect();
    assert!(page_refs.contains(&"PostOrbitalLoadingSpinner".to_string()), "the page mounts the renamed trait: {page_refs:?}");
    let duplicates: Vec<_> = validate_comprehensive_without_services(&schema)
        .errors
        .into_iter()
        .filter(|e| e.code.as_str() == "ORB_T_DUPLICATE_NAME")
        .collect();
    assert!(duplicates.is_empty(), "{duplicates:?}");
}

#[test]
fn a_single_import_keeps_its_atom_internal_child_name() {
    let schema = inline_desks(&[("PostOrbital", "PostBoard")], false);
    let post = trait_names(&schema, 0);
    assert!(post.contains(&"LoadingSpinner".to_string()), "{post:?}");
}
