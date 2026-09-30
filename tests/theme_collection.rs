//! The resolve phase collects every theme a program declares into
//! `OirModule.themes` (inline definitions as-is, preset keys as their baked
//! preset definition) and records the app's default `data-theme` key, so the
//! compiled shells load the themes the program actually names.

use orbital_compiler::phases::{compile_schema, CompilerOptions};
use orbital_core::ir::OirModule;
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/theme-collection-app.orb")).expect("fixture parses")
}

fn compile(schema: Value) -> OirModule {
    let schema: OrbitalSchema = serde_json::from_value(schema).expect("schema deserializes");
    compile_schema(schema, &CompilerOptions::default()).expect("compiles")
}

fn names(m: &OirModule) -> Vec<&str> {
    m.themes.iter().map(|t| t.name.as_str()).collect()
}

#[test]
fn a_preset_key_on_an_orbital_carries_the_baked_preset_and_sets_the_default() {
    let mut s = fixture();
    s["orbitals"][0]["theme"] = json!("art-deco-dark");
    let m = compile(s);
    assert_eq!(names(&m), vec!["art-deco"]);
    assert_eq!(m.default_theme.as_deref(), Some("art-deco-dark"));
    let ts = m.themes[0]
        .tokens
        .type_scale
        .as_ref()
        .expect("preset carries its type axis");
    assert!(
        ts.font_import.is_some(),
        "the baked preset keeps its fontImport"
    );
}

#[test]
fn the_app_theme_wins_the_default_and_comes_first() {
    let mut s = fixture();
    s["theme"] = json!("terminal-dark");
    s["orbitals"][0]["theme"] = json!("clay-light");
    let m = compile(s);
    assert_eq!(names(&m), vec!["terminal", "clay"]);
    assert_eq!(m.default_theme.as_deref(), Some("terminal-dark"));
}

#[test]
fn an_inline_theme_defaults_to_its_light_key() {
    let mut s = fixture();
    s["orbitals"][0]["theme"] =
        json!({ "name": "deco-inline", "tokens": { "colors": { "primary": "#0f5132" } } });
    let m = compile(s);
    assert_eq!(names(&m), vec!["deco-inline"]);
    assert_eq!(m.default_theme.as_deref(), Some("deco-inline-light"));
}

#[test]
fn control_no_theme_declared_collects_nothing() {
    let m = compile(fixture());
    assert!(m.themes.is_empty());
    assert_eq!(m.default_theme, None);
}

#[test]
fn an_unknown_preset_key_sets_the_default_but_collects_no_definition() {
    let mut s = fixture();
    s["orbitals"][0]["theme"] = json!("no-such-theme-light");
    let m = compile(s);
    assert!(m.themes.is_empty());
    assert_eq!(m.default_theme.as_deref(), Some("no-such-theme-light"));
}

#[test]
fn the_same_preset_twice_is_collected_once() {
    let mut s = fixture();
    s["theme"] = json!("gazette-light");
    s["orbitals"][0]["theme"] = json!("gazette-dark");
    let m = compile(s);
    assert_eq!(names(&m), vec!["gazette"]);
    assert_eq!(m.default_theme.as_deref(), Some("gazette-light"));
}
