//! Regression for the cross-run nondeterminism found while proving Stage B's
//! `orbital X = Alias.orbitals.Y { … }` import splice on Project Friday's
//! `project-friday.lolo`: `orb resolve` of a schema with orbital imports
//! ALONGSIDE a plain (non-import) orbital that auto-pulls several `@trait.X`
//! render siblings produced a DIFFERENT trait order — and a different
//! `_metadata.configProvenance` record order — on each run of the same
//! binary against the same input. Root cause: `sibling_trait_refs_of`
//! (`src/phases/inline/trait.rs`) collected `@trait.X` names into a
//! `HashSet<String>` and callers iterated that set directly to build the
//! sibling-pull worklist, and `record_config_provenance`
//! (`src/phases/inline/rewrite.rs`) walks `HashMap`-backed render-ui prop
//! objects to build `_metadata.configProvenance` — both HashMap/HashSet
//! iteration orders vary per process (Rust's default hasher is randomly
//! seeded per `HashMap`/`HashSet` instance), so two resolves of the SAME
//! `.orb` produced DIFFERENT trait/provenance array order even though every
//! individual construct validated. Found PRE-EXISTING: neither function is
//! part of the orbital-import program's own diff — the auto-pull mechanism
//! and the provenance recorder predate Stage A/B; the import splice merely
//! put a reproducing shape (an import-bearing file with a non-import orbital
//! carrying enough auto-pulled siblings) in front of it.
//!
//! Fix: `sibling_trait_refs_of` now sorts its result by name before
//! returning (`trait.rs`), and `inline_schema_with_orbital_diagnostics`
//! sorts each orbital's `configProvenance` records by
//! `(trait, patternPath, prop, knob)` right before they are finalized into
//! `_metadata` (`mod.rs`) — canonicalizing at the ONE owner of each array
//! rather than sorting at scattered call sites. Neither fix recovers true
//! "first referenced in source order" (impossible without changing
//! `SExpression::Object`'s `HashMap` representation in `orbital-core`, which
//! is out of this crate's scope for this defect) — sorting by name is the
//! deterministic fallback the plan's own rule names for exactly this case.
//!
//! This test asserts CANONICAL (key-sorted) JSON equality across two
//! independent `inline_schema` runs, matching the crate's own established
//! convention for HashMap-backed structs
//! (`src/phases/inline/orbital.rs`'s `canonical_json` / `assert_or_write_fixture`
//! doc comment: "`orbital-compiler`'s `serde_json` … a raw `to_value` of a
//! `HashMap`-backed struct is NOT deterministic across runs"). Raw
//! (non-canonical) byte-identity is a SEPARATE, broader, pre-existing gap —
//! every `HashMap<String, _>`-typed schema field (dominated by
//! `SExpression::Object`) serializes in per-process hash order via
//! `orbital-cli`'s plain `serde_json::to_string_pretty` — outside this
//! crate and outside this defect's scope; reported, not fixed, here.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::{OrbitalSchema, TraitReference};

/// One upstream orbital importable by reference — kept minimal (no roles,
/// no policies) since this fixture exists only to exercise "≥2 orbital
/// imports coexisting with a sibling-pull elsewhere in the file", not the
/// import override surface (already covered by `tests/orbital_import.rs`).
const THING_UPSTREAM: &str = r#"{
  "name": "test-determinism-thing-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ThingOrbital",
      "entity": {
        "name": "Thing",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "ThingMain",
          "linkedEntity": "Thing",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "ThingPage", "path": "/things", "linkedEntity": "Thing", "traits": [ { "ref": "ThingMain" } ] }
      ]
    }
  ]
}"#;

/// One upstream atom orbital whose main trait embeds THREE render-look
/// siblings, each referenced from a DIFFERENT `config` field default (the
/// `std-browse` "look variant" shape: `denseBodyContent`/`feedBodyContent`/
/// …). Names are deliberately NOT in alphabetical declaration order
/// (`ZetaView`, `AlphaGrid`, `MidTable`) so a test that merely reproduced
/// declaration order by coincidence would not pass — only an explicit
/// canonicalization (sort-by-name) does.
const WIDGET_UPSTREAM: &str = r#"{
  "name": "test-determinism-widget-upstream",
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
          "config": {
            "zetaLook": { "type": "string", "default": "@trait.ZetaView" },
            "alphaLook": { "type": "string", "default": "@trait.AlphaGrid" },
            "midLook": { "type": "string", "default": "@trait.MidTable" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] }
            ]
          }
        },
        {
          "name": "ZetaView",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "VIEW" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "VIEW", "effects": [ ["render-ui", "main", { "type": "data-list", "entity": "Widget" } ] ] }
            ]
          }
        },
        {
          "name": "AlphaGrid",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "VIEW" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "VIEW", "effects": [ ["render-ui", "main", { "type": "data-grid", "entity": "Widget" } ] ] }
            ]
          }
        },
        {
          "name": "MidTable",
          "linkedEntity": "Widget",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "VIEW" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "VIEW", "effects": [ ["render-ui", "main", { "type": "dense-table", "entity": "Widget" } ] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "WidgetPage", "path": "/widget", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetMain" }, { "ref": "ZetaView" }, { "ref": "AlphaGrid" }, { "ref": "MidTable" } ] }
      ]
    }
  ]
}"#;

/// Three orbitals in ONE consumer file: two `orbital X = Alias.orbitals.Y {}`
/// imports (`FirstThingImport`/`SecondThingImport`) — the ">=2 orbital
/// imports" half — and `PanelOrbital`, a PLAIN inline orbital (not an
/// import) that explicitly composes only `Widget.traits.WidgetMain`, whose
/// three siblings must be auto-pulled. This mirrors the reported shape
/// exactly: the orbital that exhibited nondeterministic trait order
/// (`PersonOrbital` in `project-friday.lolo`) was the plain orbital, not
/// either import — the two imports merely had to be present in the same
/// resolve for the bug to matter (codegen file/route order, the JS runtime's
/// trait registration order, and `_metadata.configProvenance` all derive
/// from ONE resolve of the WHOLE file).
const CONSUMER: &str = r#"{
  "name": "test-determinism-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "FirstThingImport",
      "uses": [ { "from": "./thing.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ThingOrbital" }
    },
    {
      "name": "SecondThingImport",
      "uses": [ { "from": "./thing.orb", "as": "Up" } ],
      "entity": "Up.orbitals.ThingOrbital.entity",
      "traits": [],
      "reference": { "ref": "Up.orbitals.ThingOrbital" }
    },
    {
      "name": "PanelOrbital",
      "uses": [ { "from": "./widget.orb", "as": "Widget" } ],
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

/// Fresh parse + inline of the fixture — called twice per test so each call
/// gets brand-new `HashMap`/`HashSet` instances (Rust's default hasher seeds
/// per-instance, not per-process, so two calls in ONE test process already
/// exercise different hash-iteration orders — no need to spawn a second
/// process to reproduce the bug).
fn inline_fresh() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("thing.orb"), THING_UPSTREAM).expect("write thing upstream");
    std::fs::write(dir.path().join("widget.orb"), WIDGET_UPSTREAM).expect("write widget upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

/// This fixture pins no ids (unlike `tests/orbital_import.rs`'s `FirstImport`
/// ledger entry), so every entity/orbital/page/trait id is freshly minted by
/// the id-stamping phase on EACH `inline_fresh()` call — a real, EXPECTED,
/// and entirely different source of run-to-run difference from the ordering
/// bug this test guards against (explicitly excluded by the reproduction
/// this test is modeled on: "this is NOT the `.lolo` without ids mints fresh
/// ULIDs behavior"). Mask it out before comparing so the test is sensitive
/// to ORDER, not to id generation. `ledger.entries` is dropped outright (its
/// own KEYS are fresh ids); every ULID-shaped VALUE elsewhere
/// (`<prefix>_<26 alnum chars>`, prefixes minted by this crate's id
/// stamping: `orb`/`ent`/`pag`/`trt`/`evt`) is replaced with a fixed
/// placeholder wherever it appears (ids, `entityRefIds` maps, ref pointers).
fn looks_like_minted_id(s: &str) -> bool {
    const PREFIXES: [&str; 5] = ["orb", "ent", "pag", "trt", "evt"];
    let Some((prefix, rest)) = s.split_once('_') else {
        return false;
    };
    PREFIXES.contains(&prefix)
        && rest.len() == 26
        && rest.chars().all(|c| c.is_ascii_alphanumeric())
}

fn mask_minted_ids(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == "ledger" {
                    continue;
                }
                out.insert(k.clone(), mask_minted_ids(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(mask_minted_ids).collect())
        }
        serde_json::Value::String(s) if looks_like_minted_id(s) => {
            serde_json::Value::String("<ID>".to_string())
        }
        other => other.clone(),
    }
}

/// `orbital_core::util::canonical_json` — the one production owner, shared
/// with `src/phases/inline/orbital.rs`'s Stage-B fixture byte-equality
/// assertions and the CLI's schema serialization sites. This masks ONLY
/// object-key order (which carries no meaning in JSON); array order is
/// preserved and compared as-is, which is what this test is actually proving
/// deterministic.
use orbital_core_eval::util::canonical_json;

#[test]
fn panel_orbital_auto_pulls_all_three_siblings() {
    // Sanity check the fixture actually exercises the auto-pull path before
    // asserting anything about order.
    let schema = inline_fresh();
    let panel = &schema.orbitals[2];
    assert_eq!(panel.name, "PanelOrbital");
    let names: Vec<String> = panel
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect();
    for sibling in ["ZetaView", "AlphaGrid", "MidTable"] {
        assert!(
            names.iter().any(|n| n == sibling),
            "expected auto-pulled sibling {sibling} in PanelOrbital, got {names:?}"
        );
    }
}

#[test]
fn resolve_is_deterministic_across_independent_runs() {
    let a = inline_fresh();
    let b = inline_fresh();

    let a_json = canonical_json(&mask_minted_ids(
        &serde_json::to_value(&a).expect("schema a serializes"),
    ));
    let b_json = canonical_json(&mask_minted_ids(
        &serde_json::to_value(&b).expect("schema b serializes"),
    ));

    let a_pretty = serde_json::to_string_pretty(&a_json).expect("pretty a");
    let b_pretty = serde_json::to_string_pretty(&b_json).expect("pretty b");

    assert_eq!(
        a_pretty, b_pretty,
        "orb resolve must be deterministic: two independent inline_schema runs over the \
         SAME input produced different canonical JSON — this includes `_metadata` \
         (configProvenance), so a diff here means either the sibling-pull order rule in \
         `sibling_trait_refs_of` (trait.rs) or the configProvenance sort in \
         `inline_schema_with_orbital_diagnostics` (mod.rs) regressed"
    );

    // The specific class of bug this guards against: PanelOrbital's traits
    // (index 2 — after the two imports) must land in the SAME order both
    // times, not just contain the same set.
    let panel_names = |s: &OrbitalSchema| -> Vec<String> {
        s.orbitals[2]
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some(td.name.clone()),
                other => other.trait_name().map(|s| s.to_string()),
            })
            .collect()
    };
    assert_eq!(
        panel_names(&a),
        panel_names(&b),
        "PanelOrbital's trait order (main + 3 auto-pulled siblings) must be \
         reproducible across independent resolves"
    );
}
