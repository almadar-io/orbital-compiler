//! A literal passed through a `uses` config override (`uses Chrome { config { product: "rawi" } }`)
//! must reach the referenced trait's `@config.product` forward even when the atom that declares
//! the override is itself imported as an orbital (`orbital SiteEN = Landing.orbitals.LandingOrbital`).
//! Before the fix it resolved when the atom was validated alone but dangled
//! (`ORB_CONFIG_DANGLING_REF`) once an organism imported the atom.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};

const CHROME: &str = r#"{
  "name": "test-chrome",
  "version": "1.0.0",
  "config": { "product": { "type": "string", "default": "kflow" } },
  "orbitals": [
    {
      "name": "ChromeOrbital",
      "entity": { "name": "ChromeView", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        {
          "name": "Header",
          "linkedEntity": "ChromeView",
          "category": "interaction",
          "config": {
            "label": { "type": "string", "default": "@config.product" },
            "brand": { "type": "string", "default": ["if", ["==", "@config.product", "rawi"], "Rawi", "KFlow"] }
          },
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }
      ]
    }
  ]
}"#;

fn landing(product: &str) -> String {
    format!(
        r#"{{
  "name": "test-landing",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "LandingOrbital",
      "uses": [ {{ "from": "./chrome.orb", "as": "Chrome", "config": {{ "product": {{ "type": "unknown", "default": "{product}" }} }} }} ],
      "entity": {{ "name": "LandingView", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
      "traits": [ {{ "ref": "Chrome.traits.Header", "name": "LandingHeader", "linkedEntity": "LandingView" }} ]
    }}
  ]
}}"#
    )
}

const ORGANISM: &str = r#"{
  "name": "test-site",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SiteEN",
      "uses": [ { "from": "./landing.orb", "as": "Landing" } ],
      "entity": "Landing.orbitals.LandingOrbital.entity",
      "reference": { "ref": "Landing.orbitals.LandingOrbital" },
      "traits": []
    }
  ]
}"#;

fn materialize(entry: &str, product: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("chrome.orb"), CHROME).expect("chrome");
    std::fs::write(dir.path().join("landing.orb"), landing(product)).expect("landing");
    std::fs::write(dir.path().join("site.orb"), ORGANISM).expect("site");
    let path = dir.path().join(entry);
    let mut schema = parse_file(&path).expect("parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).expect("inline succeeds");
    schema
}

fn header_label(schema: &OrbitalSchema) -> Option<SExpression> {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name.ends_with("LandingHeader") => {
                td.config.as_ref()?.get("label")?.default.clone()
            }
            _ => None,
        })
}

fn header_brand(schema: &OrbitalSchema) -> Option<SExpression> {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name.ends_with("LandingHeader") => {
                td.config.as_ref()?.get("brand")?.default.clone()
            }
            _ => None,
        })
}

fn dangling(schema: &OrbitalSchema) -> usize {
    validate_comprehensive_without_services(schema)
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
        .count()
}

#[test]
fn the_override_literal_reaches_the_forward_when_the_atom_is_validated_alone() {
    let schema = materialize("landing.orb", "rawi");
    assert_eq!(
        header_label(&schema),
        Some(SExpression::String("rawi".into()))
    );
    assert_eq!(dangling(&schema), 0);
}

#[test]
fn the_override_literal_still_reaches_the_forward_through_an_orbital_import() {
    let schema = materialize("site.orb", "rawi");
    assert_eq!(
        header_label(&schema),
        Some(SExpression::String("rawi".into()))
    );
    assert_eq!(
        dangling(&schema),
        0,
        "ORB_CONFIG_DANGLING_REF through the orbital import"
    );
}

#[test]
fn control_a_different_literal_is_carried_unchanged() {
    let schema = materialize("site.orb", "nata");
    assert_eq!(
        header_label(&schema),
        Some(SExpression::String("nata".into()))
    );
}

#[test]
fn a_forward_inside_an_expression_resolves_through_an_orbital_import() {
    let schema = materialize("site.orb", "rawi");
    let brand = header_brand(&schema).expect("brand knob");
    assert!(
        !format!("{brand:?}").contains("@config.product"),
        "the @config.product read inside the expression must be resolved: {brand:?}"
    );
    assert_eq!(dangling(&schema), 0);
}
