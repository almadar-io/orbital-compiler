//! Phase 3.A round-trip tests — config override substitution in the inline
//! phase.
//!
//! These tests exercise the hybrid composition contract:
//!   - Atoms embed `@config.<param>` binding strings inside their effects
//!     (typically inside render-ui payloads, but anywhere SExpression values
//!     live).
//!   - Composers (molecules/organisms) supply concrete values at the call
//!     site via `TraitReference.config: Option<HashMap<String, SExpression>>`.
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
use orbital_core::schema::{SExpression, TraitReference};

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
/// a `HashMap<String, SExpression>`. Panics on shape mismatch.
fn extract_render_ui_object(payload: &SExpression) -> std::collections::HashMap<String, SExpression> {
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
            "slot": "content"
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
            "filterFields": ["status", "category"]
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
          "config": { "titleBinding": "@item.variationVal" }
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
    std::fs::write(&consumer_path, two_lvl_consumer_source(consumer_config)).expect("write consumer");
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
          "config": { "titleBinding": "@item.dishName" }"#;
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
