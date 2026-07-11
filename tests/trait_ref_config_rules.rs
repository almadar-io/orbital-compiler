//! E1/E2 call-site config rules for trait References.
//!
//! - E1 `ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY`: a call-site `config` key that
//!   matches no declared knob on the referenced trait.
//! - E2 `ORB_T_CONFIG_MISSING_REQUIRED_KEY`: a referenced trait's `!`
//!   -required knob is not supplied at the call site.
//!
//! Both are collected during `inline_schema_with_config_diagnostics` (the
//! only point where the raw call-site key set and the referenced trait's
//! untouched declaration are both visible) and converted to
//! `ValidationError`s by `validate_trait_ref_config_diagnostics`.

use orbital_compiler::phases::inline::inline_schema_with_config_diagnostics;
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::error::ErrorCode;
use orbital_compiler::phases::validation::trait_ref_config::validate_trait_ref_config_diagnostics;
use orbital_compiler::phases::InlineConfig;

/// Atom declaring three config knobs: `title` (required, no default),
/// `subtitle` (required, has a default — required + default coexist),
/// `icon` (not required). Also declares its own page binding to
/// `WidgetView` so the "atom's own page" exemption can be exercised by
/// validating this file standalone (no call-site config at all).
const ATOM_FIXTURE: &str = r#"{
  "name": "test-required-config-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "WidgetOrbital",
      "entity": {
        "name": "Widget",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true }
        ]
      },
      "traits": [
        {
          "name": "WidgetView",
          "linkedEntity": "Widget",
          "category": "interaction",
          "config": {
            "title": { "type": "string", "required": true },
            "subtitle": { "type": "string", "default": "Untitled", "required": true },
            "icon": { "type": "string", "default": "layout-panel-top" }
          },
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD", "name": "Load" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "LOAD", "effects": [] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "WidgetPage", "path": "/widget", "linkedEntity": "Widget", "traits": [ { "ref": "WidgetView" } ] }
      ]
    }
  ]
}"#;

fn composer_source(config_fragment: &str) -> String {
    format!(
        r#"{{
  "name": "test-required-config-composer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "WidgetComposerOrbital",
      "uses": [ {{ "from": "./atom.orb", "as": "AtomAlias" }} ],
      "entity": {{
        "name": "ComposerRecord",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "AtomAlias.traits.WidgetView",
          "linkedEntity": "ComposerRecord"{config}
        }}
      ]
    }}
  ]
}}"#,
        config = config_fragment,
    )
}

fn materialize(config_fragment: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM_FIXTURE).expect("write atom");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&composer_path, composer_source(config_fragment)).expect("write composer");
    (dir, composer_path)
}

fn collect_diagnostics(
    composer_path: &std::path::Path,
) -> Vec<orbital_compiler::phases::inline::TraitRefConfigDiagnostic> {
    let mut schema = parse_file(composer_path).expect("composer must parse");
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(composer_path),
        &mut diagnostics,
    )
    .expect("inline must succeed");
    diagnostics
}

// (a) Unknown call-site key → E1 fires.
#[test]
fn unknown_call_site_key_fires_e1() {
    let config_fragment = r#",
          "config": {
            "title": { "type": "string", "default": "Hello" },
            "subtitle": { "type": "string", "default": "World" },
            "entity": { "type": "unknown", "default": "@entity.id" }
          }"#;
    let (_dir, composer_path) = materialize(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    let result = validate_trait_ref_config_diagnostics(&diagnostics);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitConfigUnknownCallSiteKey
                && e.message.contains("entity")
                && e.message.contains("WidgetView")),
        "expected E1 for unknown key 'entity', got: {:#?}",
        result.errors
    );
}

// (b) Required knob missing at call site → E2 fires.
#[test]
fn missing_required_knob_fires_e2() {
    // Supplies neither `title` nor `subtitle` (both required).
    let config_fragment = r#",
          "config": {
            "icon": { "type": "string", "default": "star" }
          }"#;
    let (_dir, composer_path) = materialize(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    let result = validate_trait_ref_config_diagnostics(&diagnostics);
    let missing: Vec<&str> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::TraitConfigMissingRequiredKey)
        .map(|e| e.message.as_str())
        .collect();
    assert!(
        missing.iter().any(|m| m.contains("title")),
        "expected E2 for missing 'title', got: {:#?}",
        result.errors
    );
    assert!(
        missing.iter().any(|m| m.contains("subtitle")),
        "expected E2 for missing 'subtitle', got: {:#?}",
        result.errors
    );
    // Unknown-key rule must not double-fire for a call site with no bad keys.
    assert!(result
        .errors
        .iter()
        .all(|e| e.code != ErrorCode::TraitConfigUnknownCallSiteKey));
}

// (c) Required knob supplied via `@config.X` forward → clean.
#[test]
fn required_knob_via_config_forward_is_clean() {
    let config_fragment = r#",
          "config": {
            "title": { "type": "unknown", "default": "@config.title" },
            "subtitle": { "type": "unknown", "default": "@config.subtitle" }
          }"#;
    let (_dir, composer_path) = materialize(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    assert!(
        diagnostics.is_empty(),
        "wrapper-forward call site must be clean, got: {:#?}",
        diagnostics
    );
}

// (d) Atom's own page binding (no call-site config at all) → clean, even
// though WidgetView declares two required knobs. `required` binds
// consumers, not the atom's own standalone demo page.
#[test]
fn atoms_own_page_binding_is_exempt() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    std::fs::write(&atom_path, ATOM_FIXTURE).expect("write atom");

    let mut schema = parse_file(&atom_path).expect("atom must parse standalone");
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&atom_path),
        &mut diagnostics,
    )
    .expect("inline must succeed");
    assert!(
        diagnostics.is_empty(),
        "the atom's own page->trait binding carries no call-site config and must not trigger E2, got: {:#?}",
        diagnostics
    );
}

// (e) Required + default coexist, and the call site supplies both → clean.
#[test]
fn required_with_default_supplied_at_call_site_is_clean() {
    let config_fragment = r#",
          "config": {
            "title": { "type": "string", "default": "Overridden Title" },
            "subtitle": { "type": "string", "default": "Overridden Subtitle" }
          }"#;
    let (_dir, composer_path) = materialize(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    assert!(
        diagnostics.is_empty(),
        "required knob supplied at call site alongside its declared default must be clean, got: {:#?}",
        diagnostics
    );
}

// A `.lolo` alias trait (`trait Alias = Wrapped -> X { config { ... } }`)
// can declare its OWN typed knob inside the reference's config block —
// structurally a DECLARATION (real type tag / annotation), not a plain
// wiring override of one of `WidgetView`'s knobs. `alias.orb` below
// declares a new `steps` knob this way while also overriding the wrapped
// trait's required `title`/`subtitle` via plain wiring.
const ALIAS_FIXTURE: &str = r#"{
  "name": "test-required-config-alias",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "AliasOrbital",
      "uses": [ { "from": "./atom.orb", "as": "AtomAlias" } ],
      "entity": {
        "name": "AliasRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "AtomAlias.traits.WidgetView",
          "name": "AliasWidgetView",
          "linkedEntity": "AliasRecord",
          "config": {
            "title": { "type": "unknown", "default": "Alias Title" },
            "subtitle": { "type": "unknown", "default": "Alias Subtitle" },
            "steps": { "type": "array", "default": [], "label": "Steps" }
          }
        }
      ],
      "pages": [
        { "name": "AliasPage", "path": "/alias", "linkedEntity": "AliasRecord", "traits": [ { "ref": "AliasWidgetView" } ] }
      ]
    }
  ]
}"#;

fn materialize_alias_composer(
    composer_config_fragment: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM_FIXTURE).expect("write atom");
    std::fs::write(dir.path().join("alias.orb"), ALIAS_FIXTURE).expect("write alias");
    let composer_path = dir.path().join("alias_composer.orb");
    let composer_source = format!(
        r#"{{
  "name": "test-required-config-alias-composer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "AliasComposerOrbital",
      "uses": [ {{ "from": "./alias.orb", "as": "AliasAlias" }} ],
      "entity": {{
        "name": "ComposerRecord",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "AliasAlias.traits.AliasWidgetView",
          "linkedEntity": "ComposerRecord"{config}
        }}
      ]
    }}
  ]
}}"#,
        config = composer_config_fragment,
    );
    std::fs::write(&composer_path, composer_source).expect("write alias composer");
    let alias_path = dir.path().join("alias.orb");
    (dir, alias_path, composer_path)
}

// (f) The alias's own declaration-form knob (`steps`, declared inside its
// own reference to WidgetView) must not be flagged unknown against
// WidgetView's declared config — union in the alias's own declarations.
#[test]
fn alias_declared_knob_at_alias_site_is_clean() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM_FIXTURE).expect("write atom");
    let alias_path = dir.path().join("alias.orb");
    std::fs::write(&alias_path, ALIAS_FIXTURE).expect("write alias");

    let mut schema = parse_file(&alias_path).expect("alias must parse");
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&alias_path),
        &mut diagnostics,
    )
    .expect("inline must succeed");
    assert!(
        diagnostics.is_empty(),
        "alias's own declaration-form knob 'steps' must not be flagged unknown, got: {:#?}",
        diagnostics
    );
}

// (g) A call site one level up, referencing the ALIAS (not the wrapped
// trait), passes the alias-declared knob (`steps=`) — must resolve clean.
// This exercises the case-2 fix: the alias's resolved TraitDefinition must
// carry `steps` as its own declared config so a downstream reference to
// the ALIAS sees it as known, not just the wrapped trait's original knobs.
#[test]
fn call_site_passing_alias_declared_knob_is_clean() {
    let config_fragment = r#",
          "config": {
            "title": { "type": "unknown", "default": "Composer Title" },
            "subtitle": { "type": "unknown", "default": "Composer Subtitle" },
            "steps": { "type": "unknown", "default": [ 1, 2, 3 ] }
          }"#;
    let (_dir, _alias_path, composer_path) = materialize_alias_composer(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    assert!(
        diagnostics.is_empty(),
        "call site passing alias-declared knob 'steps' must be clean, got: {:#?}",
        diagnostics
    );
}

// (h) A genuinely unknown key on the alias (matches neither WidgetView's
// declared config nor the alias's own declarations) must still fire E1.
#[test]
fn genuinely_unknown_key_on_alias_still_fires_e1() {
    let config_fragment = r#",
          "config": {
            "title": { "type": "unknown", "default": "Composer Title" },
            "subtitle": { "type": "unknown", "default": "Composer Subtitle" },
            "steps": { "type": "unknown", "default": [ 1, 2, 3 ] },
            "bogusKnob": { "type": "unknown", "default": "nope" }
          }"#;
    let (_dir, _alias_path, composer_path) = materialize_alias_composer(config_fragment);
    let diagnostics = collect_diagnostics(&composer_path);
    let result = validate_trait_ref_config_diagnostics(&diagnostics);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitConfigUnknownCallSiteKey
                && e.message.contains("bogusKnob")),
        "expected E1 for genuinely unknown key 'bogusKnob', got: {:#?}",
        result.errors
    );
    assert!(
        result.errors.iter().all(|e| {
            e.code != ErrorCode::TraitConfigUnknownCallSiteKey || e.message.contains("bogusKnob")
        }),
        "no OTHER key should be flagged unknown, got: {:#?}",
        result.errors
    );
}
