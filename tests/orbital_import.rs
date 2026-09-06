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
use orbital_compiler::phases::validation::validate_comprehensive;
use orbital_core::schema::{EntityRef, ListenSource, OrbitalSchema, PageRef, TraitReference};

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

fn find_trait<'a>(schema: &'a OrbitalSchema, orbital_idx: usize, name: &str) -> &'a orbital_core::schema::TraitDefinition {
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
        schema.orbitals.iter().map(|o| o.reference.is_some()).collect::<Vec<_>>()
    );
}

#[test]
fn unconditional_prefix_renames_every_trait_and_entity() {
    let schema = inline_consumer();
    let names = trait_names(&schema, 0);
    assert!(names.contains(&"FirstImportThingList".to_string()), "{names:?}");
    assert!(names.contains(&"FirstImportThingDetail".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n == "ThingList" || n == "ThingDetail"), "{names:?}");

    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    assert_eq!(entity.name, "FirstImportThing");
    let aux = schema.orbitals[0]
        .auxiliary_entities
        .as_ref()
        .expect("aux entities carried");
    assert!(aux.iter().any(|e| matches!(e, EntityRef::Inline(d) if d.name == "FirstImportThingTag")));
}

#[test]
fn entity_rebind_and_fields_override_rewrite_field_tokens() {
    let schema = inline_consumer();
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("primary entity must be inline");
    };
    // `fields { title: heading }` renamed the entity's own field key.
    assert!(entity.fields.iter().any(|f| f.name.as_deref() == Some("heading")));
    assert!(!entity.fields.iter().any(|f| f.name.as_deref() == Some("title")));
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
        ListenSource::Orbital { orbital, trait_name, .. } => {
            assert_eq!(orbital, "FirstImport", "orbital must be renamed to the LOCAL orbital");
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
        assert!(refs.iter().all(|r| r.starts_with("FirstImport")), "{refs:?}");
    }

    // Bare-string navigate literal on ThingList: "/things/:id" -> "/local-things/:id".
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let list_effects = serde_json::to_string(
        list.state_machine.transitions[0].effects.as_ref().unwrap(),
    )
    .unwrap();
    assert!(list_effects.contains("/local-things/:id"), "{list_effects}");
    assert!(!list_effects.contains("\"/things/:id\""), "{list_effects}");

    // `(str/concat "/things/" …)` navigate literal on ThingDetail: prefix
    // rewritten against the ":id"-stripped page-path map.
    let detail = find_trait(&schema, 0, "FirstImportThingDetail");
    let detail_effects = serde_json::to_string(
        detail.state_machine.transitions[0].effects.as_ref().unwrap(),
    )
    .unwrap();
    assert!(detail_effects.contains("/local-things/"), "{detail_effects}");
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
        Some(orbital_core::schema::SExpression::Number(25.0)),
        "orbital.config must fold the call-site override"
    );
    let list = find_trait(&schema, 0, "FirstImportThingList");
    assert_eq!(
        list.config
            .as_ref()
            .and_then(|c| c.get("limit"))
            .and_then(|f| f.default.clone()),
        Some(orbital_core::schema::SExpression::Number(25.0)),
        "the forwarding trait's own declared default must resolve through the orbital rung"
    );
}

#[test]
fn two_imports_of_one_upstream_are_disjoint_by_name_and_id() {
    let schema = inline_consumer();
    let first_names = trait_names(&schema, 0);
    let second_names = trait_names(&schema, 1);
    assert!(second_names.contains(&"SecondImportThingList".to_string()), "{second_names:?}");
    assert!(second_names.contains(&"SecondImportThingDetail".to_string()), "{second_names:?}");
    let overlap: Vec<&String> = first_names.iter().filter(|n| second_names.contains(n)).collect();
    assert!(overlap.is_empty(), "trait names must be disjoint: {overlap:?}");

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
    assert_ne!(first_entity.id, second_entity.id, "entity ids must also be disjoint per import");

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
        Some(orbital_core::schema::SExpression::Number(10.0)),
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
    assert!(owner.identity, "identity flag must survive the rename untouched");
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
    assert_eq!(parent_id.relation.as_ref().map(|r| r.entity.as_str()), Some("FirstImportThing"));
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

/// Gap (D): a config-default STRING that exactly equals an upstream page
/// path (`ThingList.config.navItems[0].href == "/things"`) — outside any
/// `(navigate …)` s-expression — must be remapped through the same
/// upstream->local page-path map the navigate-literal rewrite uses.
#[test]
fn config_default_page_path_is_remapped_exact_match_only() {
    let schema = inline_consumer();
    let list = find_trait(&schema, 0, "FirstImportThingList");
    let nav_items = list
        .config
        .as_ref()
        .and_then(|c| c.get("navItems"))
        .and_then(|f| f.default.clone())
        .expect("navItems default carried");
    let blob = serde_json::to_string(&nav_items).unwrap();
    assert!(blob.contains("/local-things"), "{blob}");
    assert!(!blob.contains("\"/things\""), "{blob}");
    // The sibling `label` value ("Things") merely CONTAINS no page path and
    // must be left untouched — exact-match only, no substring rewriting.
    assert!(blob.contains("Things"), "{blob}");
}

/// The real-organism proof's acceptance bar, reproduced at fixture scale:
/// none of the five materialization-gap codes fire on this schema.
/// `validate_comprehensive` is used (not just `inline_schema`) because these
/// are POST-inline structural checks (`trait_validator`, `entity_access`,
/// `entity`/`cross_orbital::relations`) — inlining alone can't observe them.
#[test]
fn materialized_schema_trips_none_of_the_five_gap_codes() {
    let schema = inline_consumer();
    let result = validate_comprehensive(&schema);
    let codes: Vec<&str> = result.errors.iter().map(|e| e.code.as_str()).collect();
    for forbidden in [
        "ORB_E_INVALID_RELATION",
        "ORB_X_UNRESOLVED_RELATION_TARGET",
        "ORB_T_ENTITY_NOT_REBINDABLE",
        "ORB_ID_LEDGER_ORPHAN",
        "ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED",
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
    assert!(names.contains(&"FirstImportThingListPage".to_string()), "{names:?}");
    assert!(names.contains(&"FirstImportThingDetailPage".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "ThingListPage" || n == "ThingDetailPage"),
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
/// `validate_comprehensive` used by the test above (which only checks page
/// PATH uniqueness, `phases/validation/page.rs`) — so this test exercises
/// the OIR path directly rather than `validate_comprehensive`.
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
    let oir_result = orbital_core::ir::OirValidator::new(&module).validate();
    let dup_page_name_errors: Vec<&orbital_core::ir::ValidationError> = oir_result
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
    assert_eq!(all.len(), first.len() + second.len(), "page ids must be unique within an import too");
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
    std::fs::write(dir.path().join("upstream.orb"), CONFIG_FORWARD_UPSTREAM).expect("write upstream");
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
        Some(orbital_core::schema::SExpression::String("VIEW".to_string())),
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
