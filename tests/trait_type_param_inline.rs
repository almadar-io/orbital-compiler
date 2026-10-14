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

/// Array-sentinel twin of [`atom_fixture`]: the `candidate` field's payload
/// type is `[$v]` instead of `$v`, so a resolved substitution must land as
/// `[object]` + the element shape rather than the bare form.
fn atom_fixture_array(type_params_json: &str) -> String {
    format!(
        r#"{{
  "name": "test-lookup-atom-array",
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
              "payloadSchema": [ {{ "name": "candidate", "type": "[$v]" }} ]
            }}
          ],
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "INIT" }} ],
            "events": [
              {{
                "key": "CandidateFound",
                "payloadSchema": [ {{ "name": "candidate", "type": "[$v]" }} ]
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

/// [`atom_fixture`] + an orbital-level `"types"` map (Gap A struct aliases —
/// raw JSON fragment including a leading comma, or "" for none) declared in
/// the ATOM's OWN file — the lexical-scope tests need a struct alias that
/// exists ONLY on the source side, never copied onto the composer.
fn atom_fixture_with_types(type_params_json: &str, atom_types_fragment: &str) -> String {
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
      ]{types_fragment}
    }}
  ]
}}"#,
        type_params = type_params_json,
        types_fragment = atom_types_fragment,
    )
}

/// Composer fixture referencing `AtomAlias.traits.LookupRun`, optionally
/// carrying `typeArgs` (the caller supplies the raw JSON fragment, including
/// a leading comma, or an empty string for "no explicit args") and an
/// optional orbital-level `"types"` map (Gap A struct aliases — raw JSON
/// fragment including a leading comma, or "" for none).
fn composer_source(
    type_args_fragment: &str,
    atom_rel_path: &str,
    entity_extra_fields: &str,
    types_fragment: &str,
) -> String {
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
      ]{types_fragment}
    }}
  ]
}}"#,
        atom = atom_rel_path,
        type_args = type_args_fragment,
        entity_extra_fields = entity_extra_fields,
        types_fragment = types_fragment,
    )
}

fn materialize_and_inline(
    type_params_json: &str,
    type_args_fragment: &str,
    entity_extra_fields: &str,
) -> OrbitalSchema {
    materialize_and_inline_with_types(
        type_params_json,
        type_args_fragment,
        entity_extra_fields,
        "",
    )
}

fn materialize_and_inline_with_types(
    type_params_json: &str,
    type_args_fragment: &str,
    entity_extra_fields: &str,
    composer_types_fragment: &str,
) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&atom_path, atom_fixture(type_params_json)).expect("write atom");
    std::fs::write(
        &composer_path,
        composer_source(
            type_args_fragment,
            "./atom.orb",
            entity_extra_fields,
            composer_types_fragment,
        ),
    )
    .expect("write composer");

    let mut schema = parse_file(&composer_path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&composer_path)).expect("inline must succeed");
    schema
}

/// [`atom_fixture_array`] + [`composer_source`] wired together, for the
/// `[$v]` array-sentinel substitution tests.
fn materialize_and_inline_array_with_types(
    type_params_json: &str,
    type_args_fragment: &str,
    composer_types_fragment: &str,
) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&atom_path, atom_fixture_array(type_params_json)).expect("write atom");
    std::fs::write(
        &composer_path,
        composer_source(
            type_args_fragment,
            "./atom.orb",
            "",
            composer_types_fragment,
        ),
    )
    .expect("write composer");

    let mut schema = parse_file(&composer_path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&composer_path)).expect("inline must succeed");
    schema
}

/// Lexical-scope tests (the std-inventory shape): the struct alias a
/// composer's `::` arg or the atom's own declared default names lives ONLY
/// in the ATOM's file (`atom_types_fragment`); the composer optionally
/// carries its own conflicting/matching declaration
/// (`composer_types_fragment`). Returns the raw `Result` so the conflict
/// case can assert on the error.
fn materialize_import_scope_result(
    type_params_json: &str,
    atom_types_fragment: &str,
    type_args_fragment: &str,
    composer_types_fragment: &str,
) -> Result<OrbitalSchema, orbital_compiler::phases::inline::InlineError> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let atom_path = dir.path().join("atom.orb");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(
        &atom_path,
        atom_fixture_with_types(type_params_json, atom_types_fragment),
    )
    .expect("write atom");
    std::fs::write(
        &composer_path,
        composer_source(
            type_args_fragment,
            "./atom.orb",
            "",
            composer_types_fragment,
        ),
    )
    .expect("write composer");

    let mut schema = parse_file(&composer_path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&composer_path)).map(|()| schema)
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

fn candidate_field(
    trait_def: &orbital_core::schema::TraitDefinition,
) -> &orbital_core::schema::PayloadField {
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
    assert_eq!(
        prop_names,
        vec!["id"],
        "must default to Booking (the linkedEntity), not some other entity"
    );

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

// ---------------------------------------------------------------------------
// Gap A — struct-alias `::` args, and the `scalar` primitive.
// ---------------------------------------------------------------------------

/// 5. `:: v PoApprovalPayload` — a struct-alias arg (Gap A) resolves against
/// the COMPOSER orbital's own `types` map (the scope the `::` annotation was
/// authored in — the atom's own file is a different scope entirely and is
/// never consulted here) to `object` + the alias's flattened `properties`.
#[test]
fn struct_alias_arg_resolves_to_object_with_properties() {
    let schema = materialize_and_inline_with_types(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "PoApprovalPayload"}"#,
        "",
        r#",
      "types": { "PoApprovalPayload": [ { "name": "id", "type": "string", "required": true } ] }"#,
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("struct-alias substitution must carry the alias's flattened fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 5b. Array form: `[$v]` substitutes `[object]` + the same flattened shape.
#[test]
fn struct_alias_arg_resolves_array_form() {
    let schema = materialize_and_inline_array_with_types(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "PoApprovalPayload"}"#,
        r#",
      "types": { "PoApprovalPayload": [ { "name": "id", "type": "string", "required": true } ] }"#,
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "[object]");
    let props = field
        .properties
        .as_ref()
        .expect("array struct-alias substitution must carry the alias's flattened fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id"]);
}

/// 6. `:: v date` — every primitive in `PRIMITIVE_TYPE_ARG_NAMES` resolves as
/// a primitive (no `properties`).
#[test]
fn date_arg_resolves_as_primitive() {
    let schema = materialize_and_inline(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "date"}"#,
        "",
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "date");
    assert!(field.properties.is_none());

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 7. An arg naming neither a primitive, a known entity, nor a known struct
/// alias is unresolvable — the sentinel survives for the sweep exactly as
/// the no-arg case does (Gap A adds a lookup tier; it must not turn an
/// unresolvable name into a silent pass-through).
#[test]
fn unknown_arg_name_still_leaves_sentinel_for_sweep() {
    let schema = materialize_and_inline(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#", "typeArgs": {"v": "TotallyUnknownName"}"#,
        "",
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(
        field.field_type, "$v",
        "an unresolvable arg name must leave the sentinel, not silently pass through"
    );

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert_eq!(ctx.result.errors.len(), 2, "{:?}", ctx.result.errors);
    for err in &ctx.result.errors {
        assert_eq!(err.code.as_str(), "ORB_T_UNRESOLVED_TYPE_PARAM");
    }
}

// ---------------------------------------------------------------------------
// Gap C — declaration defaults (`(v : Type = <name>)`).
// ---------------------------------------------------------------------------

/// Standalone fixture: one orbital, ONE inline trait (no `uses`/`ref` — this
/// is the "atom's own self-test page" shape Gap C targets), optionally
/// carrying an orbital-level `types` map. `default` rides inside each
/// `typeParams` entry the caller supplies.
fn standalone_fixture(type_params_json: &str, types_fragment: &str) -> String {
    format!(
        r#"{{
  "name": "test-lookup-standalone",
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
      ]{types_fragment}
    }}
  ]
}}"#,
        type_params = type_params_json,
        types_fragment = types_fragment,
    )
}

fn standalone_and_inline(type_params_json: &str, types_fragment: &str) -> OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("standalone.orb");
    std::fs::write(&path, standalone_fixture(type_params_json, types_fragment)).expect("write");
    let mut schema = parse_file(&path).expect("must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&path)).expect("inline must succeed");
    schema
}

/// 8. No call-site arg + `(v : Type = ApprovalDemoPayload)`, alias declared
/// in the SAME (standalone) orbital — resolves, letting the declaring
/// atom's own parameter-less self-test page validate.
#[test]
fn declared_default_resolves_with_no_call_site_arg() {
    let schema = standalone_and_inline(
        r#"[{"name": "v", "kind": "Type", "default": "ApprovalDemoPayload"}]"#,
        r#",
      "types": { "ApprovalDemoPayload": [ { "name": "note", "type": "string" } ] }"#,
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("declared default must carry the alias's flattened fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["note"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "a resolved default must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 9. A call-site arg WINS over a declared default — `date` default
/// present, but the composer's explicit `:: v Booking` must still produce
/// the Entity substitution, not the default's date.
#[test]
fn call_site_arg_wins_over_declared_default() {
    let schema = materialize_and_inline(
        r#"[{"name": "v", "kind": "Type", "default": "date"}]"#,
        r#", "typeArgs": {"v": "Booking"}"#,
        r#", { "name": "when", "type": "string" }"#,
    );
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(
        field.field_type, "object",
        "the explicit call-site arg (Booking, an entity) must win over the declared default (date)"
    );
    let props = field
        .properties
        .as_ref()
        .expect("entity substitution must carry fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id", "when"]);
}

/// 10. No default + no call-site arg still blocks — same fixture shape as
/// the declared-default test, minus the `default` key, proving Gap C only
/// ever ADDS a fallback tier and never turns an undeclared-default param
/// into a silent pass-through. (Mirrors `type_kind_param_with_no_arg_leaves_
/// sentinel_for_sweep` above, which covers the non-standalone/`ref` shape.)
#[test]
fn no_default_and_no_arg_still_blocks_standalone() {
    let schema = standalone_and_inline(r#"[{"name": "v", "kind": "Type"}]"#, "");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "$v");

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert_eq!(ctx.result.errors.len(), 2, "{:?}", ctx.result.errors);
    for err in &ctx.result.errors {
        assert_eq!(err.code.as_str(), "ORB_T_UNRESOLVED_TYPE_PARAM");
    }
}

// ---------------------------------------------------------------------------
// Lexical scope for an IMPORTED trait's struct-alias arg/default (the
// std-inventory hole: `uses`-importing a trait whose `::` clause or declared
// default names a struct alias that exists only in the SOURCE behavior's
// file, never the consumer's).
// ---------------------------------------------------------------------------

/// 11. (a) A composer imports `LookupRun` and supplies `:: v SourceAlias`,
/// where `SourceAlias` is declared ONLY in the atom's own file — the
/// composer's own `types` map is empty. Must resolve via the lexical merge,
/// not leave the sentinel (the std-inventory shape, minimal).
#[test]
fn imported_trait_struct_alias_arg_resolves_lexically_from_source_file() {
    let schema = materialize_import_scope_result(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#",
      "types": { "SourceAlias": [ { "name": "id", "type": "string", "required": true } ] }"#,
        r#", "typeArgs": {"v": "SourceAlias"}"#,
        "",
    )
    .expect("lexical struct-alias arg must resolve");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("source-file alias substitution must carry its flattened fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 11. (b) The composer ALSO declares its own local alias of the SAME NAME
/// (`SourceAlias`) but with a DIFFERENT shape — a blocking
/// `ConflictingTypeAlias` error, never silently resolved either way.
#[test]
fn imported_trait_struct_alias_name_collision_with_different_shape_is_an_error() {
    let err = materialize_import_scope_result(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#",
      "types": { "SourceAlias": [ { "name": "id", "type": "string", "required": true } ] }"#,
        r#", "typeArgs": {"v": "SourceAlias"}"#,
        r#",
      "types": { "SourceAlias": [ { "name": "label", "type": "string", "required": true } ] }"#,
    )
    .expect_err("a same-name different-shape collision must be a blocking error");
    assert!(
        matches!(
            &err,
            orbital_compiler::phases::inline::InlineError::ConflictingTypeAlias { name, .. }
                if name == "SourceAlias"
        ),
        "expected ConflictingTypeAlias(\"SourceAlias\"), got {err:?}"
    );
    assert_eq!(err.structured_code(), "ORB_T_CONFLICTING_TYPE_ALIAS");
}

/// Regression: an IDENTICAL shape under the same name merges fine (harmless
/// re-declaration) — the conflict check is on SHAPE, not mere co-presence.
#[test]
fn imported_trait_struct_alias_name_collision_with_identical_shape_merges_fine() {
    let schema = materialize_import_scope_result(
        r#"[{"name": "v", "kind": "Type"}]"#,
        r#",
      "types": { "SourceAlias": [ { "name": "id", "type": "string", "required": true } ] }"#,
        r#", "typeArgs": {"v": "SourceAlias"}"#,
        r#",
      "types": { "SourceAlias": [ { "name": "id", "type": "string", "required": true } ] }"#,
    )
    .expect("an identical shape under the same name must merge without conflict");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
}

/// 11. (c) No call-site arg at all — the SOURCE atom's own declared default
/// (`(v : Type = SourceAlias)`) resolves for a consumer that merely imports
/// the atom, via the same lexical merge (the default path of the same hole).
#[test]
fn imported_trait_declared_default_resolves_lexically_from_source_file() {
    let schema = materialize_import_scope_result(
        r#"[{"name": "v", "kind": "Type", "default": "SourceAlias"}]"#,
        r#",
      "types": { "SourceAlias": [ { "name": "id", "type": "string", "required": true } ] }"#,
        "",
        "",
    )
    .expect("lexical declared-default must resolve for a plain import");
    let trait_def = lookup_run_trait(&schema);
    let field = candidate_field(trait_def);
    assert_eq!(field.field_type, "object");
    let props = field
        .properties
        .as_ref()
        .expect("source-file default alias substitution must carry its flattened fields");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["id"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}

/// 12. TRANSITIVE two-level import — the actual std-inventory shape: Top
/// imports Middle's trait, which is itself Middle's `::`-bound reference
/// onto Grandchild's atom (`Top -> Middle.traits.MiddleTrait ->
/// Grandchild.traits.GrandchildRun`). Middle supplies `:: v MiddleAlias`
/// (declared in Middle's own file); Top's own reference to `MiddleTrait`
/// supplies NO `::` at all. Middle's binding must survive Top's re-splice
/// (which would otherwise clobber `type_args` to `None`, since Top's
/// reference itself carries none) and resolve using MIDDLE's lexical scope —
/// mirrors `std-inventory` importing `std-purchase-order`'s already-bound
/// `POApproval` (itself bound to `std-purchase-order`'s own
/// `PoApprovalPayload`, declared neither in `std-approval-gate` nor in
/// `std-inventory`).
#[test]
fn transitive_two_level_import_inherits_middles_binding_past_tops_argless_reference() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let grandchild_path = dir.path().join("grandchild.orb");
    let middle_path = dir.path().join("middle.orb");
    let top_path = dir.path().join("top.orb");

    std::fs::write(
        &grandchild_path,
        r#"{
  "name": "test-grandchild",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "GrandchildOrbital",
      "entity": {
        "name": "GrandchildRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "name": "GrandchildRun",
          "linkedEntity": "GrandchildRecord",
          "category": "interaction",
          "typeParams": [ { "name": "v", "kind": "Type" } ],
          "emits": [
            { "event": "CandidateFound", "payloadSchema": [ { "name": "candidate", "type": "$v" } ] }
          ],
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT" } ],
            "events": [
              { "key": "CandidateFound", "payloadSchema": [ { "name": "candidate", "type": "$v" } ] }
            ]
          }
        }
      ]
    }
  ]
}"#,
    )
    .expect("write grandchild");

    std::fs::write(
        &middle_path,
        r#"{
  "name": "test-middle",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "MiddleOrbital",
      "uses": [ { "from": "./grandchild.orb", "as": "GrandchildAlias" } ],
      "entity": {
        "name": "MiddleRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "GrandchildAlias.traits.GrandchildRun",
          "linkedEntity": "MiddleRecord",
          "typeArgs": { "v": "MiddleAlias" }
        }
      ],
      "types": {
        "MiddleAlias": [ { "name": "note", "type": "string", "required": true } ]
      }
    }
  ]
}"#,
    )
    .expect("write middle");

    std::fs::write(
        &top_path,
        r#"{
  "name": "test-top",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TopOrbital",
      "uses": [ { "from": "./middle.orb", "as": "MiddlePkg" } ],
      "entity": {
        "name": "TopRecord",
        "persistence": "runtime",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        {
          "ref": "MiddlePkg.traits.GrandchildRun",
          "linkedEntity": "TopRecord"
        }
      ]
    }
  ]
}"#,
    )
    .expect("write top");

    let mut schema = parse_file(&top_path).expect("top must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&top_path)).expect("transitive inline must succeed");

    // No `name:` override anywhere in the chain, so the spliced trait keeps
    // its original atom name (`GrandchildRun`) — `lookup_run_trait` above is
    // hardcoded to "LookupRun" for the other fixtures' shape.
    let trait_def = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "GrandchildRun" => Some(def),
            _ => None,
        })
        .expect("expected inlined GrandchildRun");
    let field = candidate_field(trait_def);
    assert_eq!(
        field.field_type, "object",
        "Middle's :: v MiddleAlias binding must survive Top's argless re-splice"
    );
    let props = field
        .properties
        .as_ref()
        .expect("must carry MiddleAlias's flattened fields, resolved from Middle's own file");
    let prop_names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(prop_names, vec!["note"]);

    let mut ctx = ValidationContext::new();
    type_param_sweep::validate_type_param_sentinels(&schema, &mut ctx);
    assert!(
        ctx.result.errors.is_empty(),
        "resolved sentinel must not trip the sweep: {:?}",
        ctx.result.errors
    );
}
