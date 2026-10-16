//! Phase 3.A round-trip tests — config override substitution in the inline
//! phase.
//!
//! These tests exercise the hybrid composition contract:
//!   - Atoms embed `@config.<param>` binding strings inside their effects
//!     (typically inside render-ui payloads, but anywhere SExpression values
//!     live).
//!   - Composers (molecules/organisms) supply concrete values at the call
//!     site via `TraitReference.config: Option<orbital_core::schema::SObject>`.
//!   - The inline phase substitutes BEFORE the validator runs, so by the
//!     time binding.rs + pattern-prop validation sees the effect, any
//!     successfully bound `@config.X` has been replaced with a concrete
//!     literal / array / object / number / bool / null.
//!
//! The tests drive `inline_schema` end-to-end via JSON fixtures built in
//! memory. This mirrors the atomic-propagation suite's "construct JSON,
//! parse, compile, assert on lowered artefacts" approach but stays at the
//! inline-phase boundary because that's where Phase 3.A's substitution
//! lands.
//!
//! Coverage:
//!   1. `config_override_substitutes_string_param` — scalar string
//!      substitution for the simplest `@config.slot` → `"content"` case.
//!   2. `config_override_leaves_binding_when_param_missing` — a missing
//!      config key is NOT treated as an error; the binding is left as-is so
//!      the downstream validator can flag it.
//!   3. `config_override_substitutes_array_param` — array-valued config
//!      value (the common `filterFields: ["status", "category"]` pattern)
//!      is substituted as a whole SExpression::Array node.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};

/// Shared atom fixture. Declares a single orbital with a trait whose OPEN
/// transition embeds three `@config.*` binding forms inside a render-ui
/// payload:
///   - `"slot": "@config.slot"` (string-valued param)
///   - `"filterFields": "@config.filterFields"` (array-valued param)
///   - `"label": "@config.label"` (missing-param form used by test #2)
///
/// The atom file is exposed via a relative import in the composer fixture
/// below, so the loader's filesystem branch resolves it instead of the
/// embedded std-behaviors shortcut.
const ATOM_FIXTURE: &str = r#"{
  "name": "test-config-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TestConfigAtomOrbital",
      "entity": {
        "name": "TestRecord",
        "persistence": "runtime",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "name": "TestConfigAtomTrait",
          "linkedEntity": "TestRecord",
          "category": "interaction",
          "stateMachine": {
            "states": [
              { "name": "idle", "isInitial": true },
              { "name": "open" }
            ],
            "events": [
              { "key": "OPEN", "name": "Open" }
            ],
            "transitions": [
              {
                "from": "idle",
                "to": "open",
                "event": "OPEN",
                "effects": [
                  ["render-ui", "main", {
                    "type": "section",
                    "slot": "@config.slot",
                    "filterFields": "@config.filterFields",
                    "count": ["array/len", "@config.filterFields"],
                    "label": "@config.label"
                  }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}"#;

/// Build a composer fixture string parameterized by the caller-supplied
/// `config` JSON fragment. Written as a function so each test can vary the
/// config argument without duplicating the surrounding shell.
fn composer_source(config_fragment: &str, atom_rel_path: &str) -> String {
    format!(
        r#"{{
  "name": "test-config-composer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "TestConfigComposerOrbital",
      "uses": [
        {{ "from": "{atom}", "as": "AtomAlias" }}
      ],
      "entity": {{
        "name": "ComposerRecord",
        "persistence": "runtime",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }}
        ]
      }},
      "traits": [
        {{
          "ref": "AtomAlias.traits.TestConfigAtomTrait",
          "linkedEntity": "ComposerRecord"{config}
        }}
      ]
    }}
  ]
}}"#,
        atom = atom_rel_path,
        config = config_fragment,
    )
}

/// Write the atom + composer fixtures into a fresh temp dir, return the
/// composer path.
fn materialize_fixtures(config_fragment: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&atom_path, ATOM_FIXTURE).expect("write atom");
    let composer = composer_source(config_fragment, "./atom.orb");
    std::fs::write(&composer_path, composer).expect("write composer");
    (dir, composer_path)
}

/// Pull the single inlined `TestConfigAtomTrait` out of the composer schema
/// post-inline and return its OPEN transition's first effect (the render-ui
/// array). Panics with a clear message if the shape has drifted.
fn extract_render_ui_payload(path: &std::path::Path) -> SExpression {
    let mut schema = parse_file(path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(path)).expect("inline must succeed");

    let orbital = &schema.orbitals[0];
    let inlined = orbital
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "TestConfigAtomTrait" => Some(def),
            _ => None,
        })
        .expect("expected inlined TestConfigAtomTrait");

    let transition = inlined
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "OPEN")
        .expect("OPEN transition must exist");

    let effects = transition.effects.as_ref().expect("effects present");
    effects[0].clone()
}

/// Read the render-ui effect's object payload (the third array element) as
/// a `orbital_core::schema::SObject`. Panics on shape mismatch.
fn extract_render_ui_object(
    payload: &SExpression,
) -> orbital_core_base::schema::SObject {
    match payload {
        SExpression::Array(items) => {
            assert_eq!(items.len(), 3, "render-ui effect must be 3 elements");
            match &items[2] {
                SExpression::Object(map) => map.clone(),
                other => panic!("expected render-ui payload to be Object, got: {:?}", other),
            }
        }
        other => panic!("expected render-ui effect to be Array, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test 1 — String-valued config substitution
// ---------------------------------------------------------------------------

#[test]
fn config_override_substitutes_string_param() {
    // Composer supplies `config: { "slot": "content" }`. The inlined trait's
    // render-ui payload must carry the literal string "content" in place of
    // `@config.slot`. The other two bindings are left as-is (because
    // `filterFields` and `label` are not in the config map for THIS test).
    let config_fragment = r#",
          "config": {
            "slot": { "type": "unknown", "default": "content" }
          }"#;
    let (_dir, composer_path) = materialize_fixtures(config_fragment);
    let payload = extract_render_ui_payload(&composer_path);
    let obj = extract_render_ui_object(&payload);

    assert_eq!(
        obj.get("slot"),
        Some(&SExpression::String("content".to_string())),
        "@config.slot must be substituted with the literal 'content'"
    );
    // The other two bindings must remain — they weren't in the config map.
    assert_eq!(
        obj.get("filterFields"),
        Some(&SExpression::String("@config.filterFields".to_string())),
        "unmatched @config bindings must be left as-is"
    );
    assert_eq!(
        obj.get("label"),
        Some(&SExpression::String("@config.label".to_string())),
        "unmatched @config bindings must be left as-is"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Missing-config-param leaves binding untouched
// ---------------------------------------------------------------------------

#[test]
fn config_override_leaves_binding_when_param_missing() {
    // Composer supplies NO `config` key at all. All three @config.* bindings
    // must survive the inline phase unmodified so the downstream binding
    // validator can flag them as unresolved. The inliner MUST NOT drop them
    // silently, turn them into empty strings, or raise its own error — it
    // simply leaves them alone.
    let (_dir, composer_path) = materialize_fixtures("");
    let payload = extract_render_ui_payload(&composer_path);
    let obj = extract_render_ui_object(&payload);

    assert_eq!(
        obj.get("slot"),
        Some(&SExpression::String("@config.slot".to_string())),
        "missing config must leave @config.slot binding as-is"
    );
    assert_eq!(
        obj.get("filterFields"),
        Some(&SExpression::String("@config.filterFields".to_string())),
        "missing config must leave @config.filterFields binding as-is"
    );
    assert_eq!(
        obj.get("label"),
        Some(&SExpression::String("@config.label".to_string())),
        "missing config must leave @config.label binding as-is"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Array-valued config substitution
// ---------------------------------------------------------------------------

#[test]
fn config_override_substitutes_array_param() {
    // Composer supplies `config: { "filterFields": ["status", "category"] }`.
    // The inlined trait's render-ui payload must carry the literal array in
    // place of `@config.filterFields`. This exercises the "whole-node
    // replacement" path — config substitution is NOT limited to strings;
    // any SExpression value type can land in place of the binding.
    let config_fragment = r#",
          "config": {
            "filterFields": { "type": "unknown", "default": ["status", "category"] }
          }"#;
    let (_dir, composer_path) = materialize_fixtures(config_fragment);
    let payload = extract_render_ui_payload(&composer_path);
    let obj = extract_render_ui_object(&payload);

    let expected = SExpression::Array(vec![
        SExpression::String("status".to_string()),
        SExpression::String("category".to_string()),
    ]);
    assert_eq!(
        obj.get("filterFields"),
        Some(&expected),
        "@config.filterFields must be substituted with the literal array"
    );
    // `slot` and `label` are still bindings — not in config map.
    assert_eq!(
        obj.get("slot"),
        Some(&SExpression::String("@config.slot".to_string()))
    );
    assert_eq!(
        obj.get("label"),
        Some(&SExpression::String("@config.label".to_string()))
    );
}

// ---------------------------------------------------------------------------
// Test 3b — an empty list an operator reads keeps its declared knob
// ---------------------------------------------------------------------------

#[test]
fn config_override_keeps_an_empty_list_an_operator_reads_a_reference_to_its_knob() {
    // `[]` carries no element type of its own: substituted into an operator's argument it would be a list nothing
    // types. The knob declares the list's type, so there the binding stays and the checker reads the knob. As a
    // render prop the empty list is the value.
    let config_fragment = r#",
          "config": {
            "filterFields": { "type": "unknown", "default": [] }
          }"#;
    let (_dir, composer_path) = materialize_fixtures(config_fragment);
    let obj = extract_render_ui_object(&extract_render_ui_payload(&composer_path));

    assert_eq!(
        obj.get("count"),
        Some(&SExpression::Array(vec![
            SExpression::String("array/len".to_string()),
            SExpression::String("@config.filterFields".to_string()),
        ])),
        "an operator's argument stays a reference to the knob that types it"
    );
    assert_eq!(
        obj.get("filterFields"),
        Some(&SExpression::Array(vec![])),
        "control: the same empty list as a render prop is the value"
    );
}

#[test]
fn config_override_substitutes_a_list_with_elements_and_a_scalar_an_operator_reads() {
    for (default, expected) in [
        (r#"["status"]"#, SExpression::Array(vec![SExpression::String("status".to_string())])),
        (r#""status""#, SExpression::String("status".to_string())),
    ] {
        let config_fragment = format!(
            r#",
          "config": {{
            "filterFields": {{ "type": "unknown", "default": {default} }}
          }}"#
        );
        let (_dir, composer_path) = materialize_fixtures(&config_fragment);
        let obj = extract_render_ui_object(&extract_render_ui_payload(&composer_path));
        assert_eq!(
            obj.get("count"),
            Some(&SExpression::Array(vec![SExpression::String("array/len".to_string()), expected])),
            "control: {default} types itself, so it is substituted"
        );
    }
}

// ---------------------------------------------------------------------------
// Two-level chain (consumer → variation → base) config-override propagation.
//
// Regression coverage for the import-load deferral fix in `inline_orbital` /
// `apply_overrides_to_trait`. Before the fix, a config override on a trait that
// is itself a reference (a "variation" that derives from a base atom via
// `= Base.traits.X`) was clobbered: when the variation was loaded as an import,
// its `@config.X` bindings were eagerly substituted with the variation's own
// value, leaving nothing for the outer consumer's override to replace. The fix
// defers `@config.X` substitution at import-load time (folding values into the
// resolved trait's `config[k].default` so they carry forward) and bakes only at
// the top-level target.
//
// BASE declares `titleBinding` (default `@item.name`) and renders
// `title: @config.titleBinding`. VARIATION derives from BASE and sets
// `titleBinding: @item.variationVal`. CONSUMER derives from VARIATION.
//   - Test 4: consumer overrides `titleBinding: @item.dishName` → consumer wins.
//   - Test 5: consumer overrides nothing → variation's value carries through.
// ---------------------------------------------------------------------------

const TWO_LVL_BASE: &str = r#"{
  "name": "test-2lvl-base",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TwoLvlBaseOrbital",
      "entity": {
        "name": "BaseRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "TwoLvlBaseTrait",
          "linkedEntity": "BaseRecord",
          "category": "interaction",
          "config": {
            "titleBinding": { "type": "string", "default": "@item.name" }
          },
          "stateMachine": {
            "states": [
              { "name": "idle", "isInitial": true },
              { "name": "open" }
            ],
            "events": [ { "key": "OPEN", "name": "Open" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "open",
                "event": "OPEN",
                "effects": [
                  ["render-ui", "main", { "type": "section", "title": "@config.titleBinding" }]
                ]
              }
            ]
          }
        }
      ]
    }
  ]
}"#;

const TWO_LVL_VARIATION: &str = r#"{
  "name": "test-2lvl-variation",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TwoLvlVariationOrbital",
      "uses": [ { "from": "./base.orb", "as": "Base" } ],
      "entity": {
        "name": "VariationRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "Base.traits.TwoLvlBaseTrait",
          "name": "TwoLvlVariationTrait",
          "config": { "titleBinding": { "type": "unknown", "default": "@item.variationVal" } }
        }
      ]
    }
  ]
}"#;

/// Build a consumer fixture that imports the variation and optionally overrides
/// `titleBinding`. `config_fragment` is either empty or a leading-comma config
/// block (matching the existing `composer_source` convention).
fn two_lvl_consumer_source(config_fragment: &str) -> String {
    format!(
        r#"{{
  "name": "test-2lvl-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "TwoLvlConsumerOrbital",
      "uses": [ {{ "from": "./variation.orb", "as": "Var" }} ],
      "entity": {{
        "name": "ConsumerRecord",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "Var.traits.TwoLvlVariationTrait",
          "name": "TwoLvlConsumerTrait"{config}
        }}
      ]
    }}
  ]
}}"#,
        config = config_fragment,
    )
}

/// Materialize base + variation + consumer fixtures into one temp dir (so the
/// `./base.orb` / `./variation.orb` relative imports resolve) and return the
/// consumer path.
fn materialize_two_lvl(consumer_config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("base.orb"), TWO_LVL_BASE).expect("write base");
    std::fs::write(dir.path().join("variation.orb"), TWO_LVL_VARIATION).expect("write variation");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, two_lvl_consumer_source(consumer_config))
        .expect("write consumer");
    (dir, consumer_path)
}

/// Resolve the consumer and return the `title` value from the inlined
/// `TwoLvlConsumerTrait`'s OPEN render-ui payload.
fn two_lvl_resolved_title(path: &std::path::Path) -> SExpression {
    let mut schema = parse_file(path).expect("consumer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(path)).expect("inline must succeed");

    let inlined = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "TwoLvlConsumerTrait" => Some(def),
            _ => None,
        })
        .expect("expected inlined TwoLvlConsumerTrait");
    let transition = inlined
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "OPEN")
        .expect("OPEN transition must exist");
    let effects = transition.effects.as_ref().expect("effects present");
    match &effects[0] {
        SExpression::Array(items) => match &items[2] {
            SExpression::Object(map) => map
                .get("title")
                .cloned()
                .expect("title key present in render-ui payload"),
            other => panic!("expected render-ui payload Object, got {:?}", other),
        },
        other => panic!("expected render-ui effect Array, got {:?}", other),
    }
}

#[test]
fn config_override_propagates_through_variation_chain() {
    // Consumer overrides titleBinding through the variation; the consumer's
    // value must win over BOTH the variation's value and the base default.
    let config_fragment = r#",
          "config": { "titleBinding": { "type": "unknown", "default": "@item.dishName" } }"#;
    let (_dir, consumer_path) = materialize_two_lvl(config_fragment);
    let title = two_lvl_resolved_title(&consumer_path);
    assert_eq!(
        title,
        SExpression::String("@item.dishName".to_string()),
        "2-level config override (consumer → variation → base) must reach the \
         render-ui binding; got {title:?}. If this is `@item.variationVal` the \
         consumer override was lost; if `@config.titleBinding` it was never \
         substituted at the top level."
    );
}

#[test]
fn config_default_carries_through_variation_chain() {
    // Consumer overrides nothing: the variation's own value must carry through
    // the chain (not collapse to the base default, not stay an unresolved
    // binding).
    let (_dir, consumer_path) = materialize_two_lvl("");
    let title = two_lvl_resolved_title(&consumer_path);
    assert_eq!(
        title,
        SExpression::String("@item.variationVal".to_string()),
        "with no consumer override, the variation's config value must carry \
         through the chain and bake at the top level; got {title:?}"
    );
}

// ---------------------------------------------------------------------------
// Hoisted sibling-forward chain (same-alias auto-pull, `inline/trait.rs`'s
// `forwarded_sibling_config`/`forwarded_sibling_config_from`).
//
// JSX-generated chrome nests several trait layers deep: an atom's main trait
// (`Feed`, declaring `title`) embeds `@trait.Stack8`, which embeds
// `@trait.Stack5`, which embeds `@trait.Typography4` — and only
// `Typography4.content` (`default "@config.title"`) actually reads the
// forwarded knob. `Typography4`'s IMMEDIATE embedder (`Stack5`) declares no
// `title`, nor does `Stack8` — the value sits three rungs up, on `Feed`.
// Regression: `std-timeline`'s `TimelineFeed` hit exactly this shape
// (`docs/tender-booping-moler.md` B4-R ground truth (n)).
//
// Test 6 proves the chain resolves all the way to `Feed`'s declared value.
// Test 7 proves a rung that itself only RELAYS the forward under a
// DIFFERENT key (`Stack8.title = @config.heading`) switches the knob being
// chased instead of treating it as a dead end.
// ---------------------------------------------------------------------------

const HOISTED_CHAIN_ATOM: &str = r#"{
  "name": "test-hoisted-chain-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "HoistedChainOrbital",
      "entity": {
        "name": "Item",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "Feed",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "title": { "type": "string", "default": "FeedTitle" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Stack8"] }] ] } ]
          }
        },
        {
          "name": "Stack8",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "gap": { "type": "string", "default": "8" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Stack5"] }] ] } ]
          }
        },
        {
          "name": "Stack5",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "direction": { "type": "string", "default": "column" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Typography4"] }] ] } ]
          }
        },
        {
          "name": "Typography4",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "content": { "type": "string", "default": "@config.title" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "text", "content": "@config.content" }] ] } ]
          }
        }
      ]
    }
  ]
}"#;

const HOISTED_CHAIN_CONSUMER: &str = r#"{
  "name": "test-hoisted-chain-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "HoistedChainConsumerOrbital",
      "uses": [ { "from": "./atom.orb", "as": "AtomAlias" } ],
      "entity": {
        "name": "ConsumerItem",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "AtomAlias.traits.Feed", "linkedEntity": "ConsumerItem", "name": "ConsumerFeed" }
      ]
    }
  ]
}"#;

/// Inline `atom`+`consumer` (writing `atom` as `./atom.orb`) and return the
/// pulled trait named `trait_name`'s OPEN transition's render-ui `content`
/// prop (panics with the whole schema's trait names on a shape mismatch, so
/// a failed pull is easy to diagnose).
fn hoisted_chain_resolved_content(atom: &str, consumer: &str, trait_name: &str) -> SExpression {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer).expect("write consumer");

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
    let pulled = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == trait_name => Some(td),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected pulled trait {trait_name}; got {names:?}"));
    let transition = pulled
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "OPEN")
        .expect("OPEN transition must exist");
    let effects = transition.effects.as_ref().expect("effects present");
    match &effects[0] {
        SExpression::Array(items) => match &items[2] {
            SExpression::Object(map) => map
                .get("content")
                .cloned()
                .expect("content key present in render-ui payload"),
            other => panic!("expected render-ui payload Object, got {:?}", other),
        },
        other => panic!("expected render-ui effect Array, got {:?}", other),
    }
}

#[test]
fn sibling_forward_resolves_through_three_deep_hoisted_chain() {
    // `Typography4` is pulled three hops removed from `Feed` (its immediate
    // embedder `Stack5`, then `Stack8`, then `Feed`) — only `Feed` declares
    // `title`. The forward must reach it.
    let content =
        hoisted_chain_resolved_content(HOISTED_CHAIN_ATOM, HOISTED_CHAIN_CONSUMER, "Typography4");
    assert_eq!(
        content,
        SExpression::String("FeedTitle".to_string()),
        "content must resolve through Stack5 -> Stack8 -> Feed to FeedTitle's \
         declared title, not stay an unresolved @config.content/@config.title \
         binding: {content:?}"
    );
}

const KEY_SWITCH_ATOM: &str = r#"{
  "name": "test-key-switch-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "KeySwitchOrbital",
      "entity": {
        "name": "Item",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "Feed",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "heading": { "type": "string", "default": "X" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Stack8"] }] ] } ]
          }
        },
        {
          "name": "Stack8",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "title": { "type": "string", "default": "@config.heading" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Typography4"] }] ] } ]
          }
        },
        {
          "name": "Typography4",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "content": { "type": "string", "default": "@config.title" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "OPEN" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
              "effects": [ ["render-ui", "main", { "type": "text", "content": "@config.content" }] ] } ]
          }
        }
      ]
    }
  ]
}"#;

const KEY_SWITCH_CONSUMER: &str = r#"{
  "name": "test-key-switch-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "KeySwitchConsumerOrbital",
      "uses": [ { "from": "./atom.orb", "as": "AtomAlias" } ],
      "entity": {
        "name": "ConsumerItem",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "AtomAlias.traits.Feed", "linkedEntity": "ConsumerItem", "name": "ConsumerFeed" }
      ]
    }
  ]
}"#;

#[test]
fn sibling_forward_switches_key_through_relaying_rung() {
    // `Typography4.content` forwards `title`. Its immediate embedder
    // `Stack8` doesn't declare a CONCRETE `title` — it only relays the
    // forward under a different name (`title: "@config.heading"`). The walk
    // must switch to chasing `heading` and resolve it on `Feed`.
    let content =
        hoisted_chain_resolved_content(KEY_SWITCH_ATOM, KEY_SWITCH_CONSUMER, "Typography4");
    assert_eq!(
        content,
        SExpression::String("X".to_string()),
        "content must resolve through the key switch (title -> heading) to \
         Feed's declared heading value 'X': {content:?}"
    );
}

// ---------------------------------------------------------------------------
// §B4-R2 — call-site config forward on a `TraitReference::Reference` ENTRY
// itself (`phases::embed_graph::EmbedGraph`).
//
// Tests 6/7 above cover a forward hoisted through traits PULLED as siblings
// of an imported atom trait (`@trait.X` tokens discovered inside an atom
// loaded via `uses`). This is a THIRD, distinct composition shape found in
// the real corpus (`std-timeline.orb`'s `TimelineOrbital`): the forward
// lives in the CALL-SITE `config` override of an explicit, TOP-LEVEL `ref:`
// entry in the consumer's OWN `traits` list (JSX-hoisted at authoring time
// by `orbital-lolo`) — not in the referenced atom's own declared config.
//
// `Feed` (Inline, declares `title`) embeds `@trait.Stack8` in a render-ui
// effect; `Stack8`, `Stack5`, `Typography4` are each explicit `ref:` entries
// in the SAME orbital's `traits` list, each with a call-site `config`
// override — `Stack8.config.children = ["@trait.Stack5"]`,
// `Stack5.config.children = ["@trait.Typography4"]`,
// `Typography4.config.content = "@config.title"`. Only `Typography4`'s
// override is itself a forward; the resolver must walk OUT through Stack5 ->
// Stack8 -> Feed (their call-site overrides carry no `title` key) to reach
// `Feed`'s declared value.
// ---------------------------------------------------------------------------

const REF_FORWARD_WIDGETS: &str = r#"{
  "name": "test-ref-forward-widgets",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "WidgetsOrbital",
      "entity": {
        "name": "Widget",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "StackRender",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": { "children": { "type": "unknown", "default": [] } },
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        },
        {
          "name": "TypographyRender",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": { "content": { "type": "string", "default": "" } },
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }
      ]
    }
  ]
}"#;

/// `Typography16` is a lone, unconnected `ref:` entry — nothing embeds it and
/// it embeds nothing — whose call-site override forwards `@config.subtitle`,
/// a knob NO trait in the orbital declares. Proves the true-dangle case
/// alongside the resolving one, in the same fixture the validator and
/// resolver both read.
fn ref_forward_consumer_source() -> String {
    r#"{
  "name": "test-ref-forward-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TimelineLikeOrbital",
      "uses": [ { "from": "./widgets.orb", "as": "Widgets" } ],
      "entity": {
        "name": "Item",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "Feed",
          "linkedEntity": "Item",
          "category": "interaction",
          "config": { "title": { "type": "string", "default": "Activity Timeline" } },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT",
              "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Stack8"] }] ] } ]
          }
        },
        {
          "ref": "Widgets.traits.StackRender", "name": "Stack8", "linkedEntity": "Item",
          "config": { "children": { "type": "unknown", "default": ["@trait.Stack5"] } }
        },
        {
          "ref": "Widgets.traits.StackRender", "name": "Stack5", "linkedEntity": "Item",
          "config": { "children": { "type": "unknown", "default": ["@trait.Typography4"] } }
        },
        {
          "ref": "Widgets.traits.TypographyRender", "name": "Typography4", "linkedEntity": "Item",
          "config": { "content": { "type": "unknown", "default": "@config.title" } }
        },
        {
          "ref": "Widgets.traits.TypographyRender", "name": "Typography16", "linkedEntity": "Item",
          "config": { "content": { "type": "unknown", "default": "@config.subtitle" } }
        }
      ]
    }
  ]
}"#
    .to_string()
}

/// Write the widgets + consumer fixtures into a fresh temp dir, inline the
/// consumer, and return the fully-inlined `OrbitalSchema` — the SAME object
/// both the resolver (already applied, by the time this returns) and the
/// validator (called by the tests below on this object directly) read.
fn materialize_ref_forward_schema() -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("widgets.orb"), REF_FORWARD_WIDGETS).expect("write widgets");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, ref_forward_consumer_source()).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn ref_forward_trait_content(schema: &OrbitalSchema, trait_name: &str) -> SExpression {
    let td = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == trait_name => Some(td),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected inlined trait {trait_name}"));
    td.config
        .as_ref()
        .and_then(|c| c.get("content"))
        .and_then(|f| f.default.clone())
        .unwrap_or_else(|| panic!("{trait_name}.config.content must be present"))
}

#[test]
fn ref_entry_call_site_config_forward_resolves_through_embedder_chain() {
    let schema = materialize_ref_forward_schema();
    let content = ref_forward_trait_content(&schema, "Typography4");
    assert_eq!(
        content,
        SExpression::String("Activity Timeline".to_string()),
        "Typography4's call-site `content: @config.title` must resolve through \
         Stack5 -> Stack8 -> Feed to Feed's declared title, not stay an \
         unresolved @config.title binding: {content:?}"
    );
}

#[test]
fn ref_entry_call_site_config_forward_dangling_key_stays_unresolved_and_is_flagged() {
    let schema = materialize_ref_forward_schema();

    // Resolver side: nobody in the orbital declares `subtitle` anywhere in
    // its referrer chain (Typography16 embeds nothing and is embedded by
    // nothing), so the forward must survive inline unresolved.
    let content = ref_forward_trait_content(&schema, "Typography16");
    assert_eq!(
        content,
        SExpression::String("@config.subtitle".to_string()),
        "an undeclared forward must stay a literal @config.subtitle binding, \
         not resolve to garbage or vanish: {content:?}"
    );

    // Validator side: the SAME post-inline schema must flag it. Proves the
    // resolver and `validate_dangling_config_refs` read the same
    // `EmbedGraph` — a forward the resolver leaves unresolved is exactly a
    // forward the validator judges unreachable.
    let result = validate_comprehensive_without_services(&schema);
    let flagged = result
        .errors
        .iter()
        .any(|e| e.code == ErrorCode::ConfigDanglingRef && e.path.contains("Typography16"));
    assert!(
        flagged,
        "expected ORB_CONFIG_DANGLING_REF on Typography16; errors: {:#?}",
        result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
            .collect::<Vec<_>>()
    );
}
