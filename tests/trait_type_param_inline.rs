//! Inline-phase tests for Increment 3-II — trait-level type parameters with
//! explicit call-site arguments (`docs/Almadar_LOLO.md`).
//!
//! `orbital-lolo` lowers a trait's own `(v : Type, ...)` header into
//! `TraitDefinition.typeParams` and a call-site `:: v string` (or `:: E
//! Booking`) reinterpretation into `TraitReference.typeArgs` — see
//! `orbital-lolo/tests/trait_type_param_sentinel.rs` for the L1 lowering
//! side. This file drives the L2 inline phase end-to-end (real `uses` +
//! `ref` resolution, mirroring `config_override_round_trip.rs`'s fixture
//! style) to prove:
//!   1. an explicit PRIMITIVE arg (`:: v string`) substitutes a scalar
//!      `field_type`, no `properties`;
//!   2. an explicit ENTITY-named arg (`:: e Booking`) substitutes
//!      `field_type: "object"` + the entity's flattened `properties` —
//!      same shape an Entity-kind default produces;
//!   3. an Entity-kind param with NO arg still defaults to the trait's
//!      `linkedEntity` (the 3-I behavior, unchanged, now routed through the
//!      3-II-aware substitution path);
//!   4. a Type-kind param with NO arg leaves the sentinel for the
//!      defensive sweep (`ORB_T_UNRESOLVED_TYPE_PARAM`), which is NOT
//!      silently defaulted to `linkedEntity` even though one exists.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{type_param_sweep, ValidationContext};
use orbital_core::schema::{OrbitalSchema, TraitReference};

/// Atom fixture: one orbital with a single trait declaring `typeParams` (the
/// caller supplies the raw JSON array) and an `emits { CandidateFound {
/// candidate : $v } }`-shaped payload (`$v` sentinel hardcoded to match a
/// param named `V` — every test below declares its param as `V`).
fn atom_fixture(type_params_json: &str) -> String {
    format!(
        r#"{{
  "name": "test-lookup-atom",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "LookupAtomOrbital",
      "entity": {{
        "name": "LookupRecord",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "name": "LookupRun",
          "linkedEntity": "LookupRecord",
          "category": "interaction",
          "typeParams": {type_params},
          "emits": [
            {{
              "event": "CandidateFound",
              "payloadSchema": [ {{ "name": "candidate", "type": "$v" }} ]
            }}
          ],
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT" }} ],
            "events": [
              {{
                "key": "CandidateFound",
                "payloadSchema": [ {{ "name": "candidate", "type": "$v" }} ]
              }}
            ]
          }}
        }}
      ]
    }}
  ]
}}"#,
        type_params = type_params_json,
    )
}

/// Composer fixture referencing `AtomAlias.traits.LookupRun`, optionally
/// carrying `typeArgs` (the caller supplies the raw JSON fragment, including
/// a leading comma, or an empty string for "no explicit args").
fn composer_source(type_args_fragment: &str, atom_rel_path: &str, entity_extra_fields: &str) -> String {
    format!(
        r#"{{
  "name": "test-lookup-composer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "LookupComposerOrbital",
      "uses": [ {{ "from": "{atom}", "as": "AtomAlias" }} ],
      "entity": {{
        "name": "Booking",
        "persistence": "runtime",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }}{entity_extra_fields}
        ]
      }},
      "traits": [
        {{
          "ref": "AtomAlias.traits.LookupRun",
          "linkedEntity": "Booking"{type_args}
        }}
      ]
    }}
  ]
}}"#,
        atom = atom_rel_path,
        type_args = type_args_fragment,
        entity_extra_fields = entity_extra_fields,
    )
}

fn materialize_and_inline(
    type_params_json: &str,
    type_args_fragment: &str,
    entity_extra_fields: &str,
) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&atom_path, atom_fixture(type_params_json)).expect("write atom");
    std::fs::write(
        &composer_path,
        composer_source(type_args_fragment, "./atom.orb", entity_extra_fields),
    )
    .expect("write composer");

    let mut schema = parse_file(&composer_path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&composer_path)).expect("inline must succeed");
    schema
}

fn lookup_run_trait(schema: &OrbitalSchema) -> &orbital_core::schema::TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "LookupRun" => Some(def),
            _ => None,
        })
        .expect("expected inlined LookupRun")
}

fn candidate_field(trait_def: &orbital_core::schema::TraitDefinition) -> &orbital_core::schema::PayloadField {
    &trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .expect("emit payloadSchema must be populated")[0]
}

/// 1. `:: v string` — explicit PRIMITIVE arg substitutes a scalar field type.
#[test]
fn explicit_primitive_arg_substitutes_scalar_field_type() {
    let schema = materialize_and_inline(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "string"}"#,
        "",
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "string");
    assert!(
        field.properties.is_none(),
        "a primitive substitution must carry no properties"
    );

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 2. `:: e Booking` — explicit ENTITY-named arg substitutes `object` +
/// flattened `properties`, same shape an Entity-kind default produces.
/// (The atom's param is still literally named `V` — the sentinel/param name
/// is arbitrary; this test exercises the ENTITY-valued-argument branch, not
/// the specific letter.)
#[test]
fn explicit_entity_named_arg_substitutes_object_with_properties() {
    let schema = materialize_and_inline(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "Booking"}"#,
        r#", { "name": "when", "type": "string" }"#,
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("entity substitution must carry the entity's fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id", "when"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 3. Entity-kind param, NO call-site arg — still defaults to the trait's
/// `linkedEntity` (the 3-I behavior, preserved through the 3-II-aware path).
#[test]
fn entity_kind_param_with_no_arg_defaults_to_linked_entity() {
    let schema = materialize_and_inline(r#"[{"name": "v", "kind": "Entity"}]"#, "", "");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("Entity-kind default must carry the linkedEntity's fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id"], "must default to Booking (the linkedEntity), not some other entity");

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 4. Type-kind param, NO call-site arg — sentinel survives (no default),
/// and the sweep reports it with the Type-kind-specific message rather than
/// silently defaulting to `linkedEntity` even though one is present.
#[test]
fn type_kind_param_with_no_arg_leaves_sentinel_for_sweep() {
    let schema = materialize_and_inline(r#"[{"name": "v", "kind": "Type"}]"#, "", "");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(
        field.field_type, "$v",
        "Type-kind param must NOT default to linkedEntity — sentinel must survive"
    );
    assert!(field.properties.is_none());

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert_eq!(
        ctx.result.errors.len(),
        2,
        "one per surviving sentinel occurrence (emits + events): {:?}",
        ctx.result.errors
    );
    for err in &ctx.result.errors {
        assert_eq!(err.code.as_str(), "ORB_T_UNRESOLVED_TYPE_PARAM");
        assert!(
            err.message.contains("call-site argument"),
            "Type-kind message must point at the missing `:: v <arg>`, got: {}",
            err.message
        );
    }
}
