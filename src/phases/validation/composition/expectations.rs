//! `expects` declarations — consumer-side requirement contracts.
//!
//! Standalone (single-orbital slice) expectations are TRUSTED: they satisfy
//! the identity rules (handled in `user_identity`/`entity_access`, which read
//! the helpers here) and license relation/persist/fetch/`linkedEntity`
//! targets (via `collect_names`). Every `ORB_S_EXPECTATION_*` and
//! `ORB_S_EXPECTED_FIELD_*` finding but the entity/payload field-type mismatch
//! is reported by the std `.lolo` lint rules; this module keeps that mismatch,
//! the spec propositions and the `--final` check.
//!
//! An expectation whose provider is **absent** is DEFERRED, never an error:
//! that is the declaration doing its job — a promise about the neighborhood,
//! checked at the link step once a consumer supplies it. `entity_access`
//! already trusts any `expects entity [identity]` to satisfy the roster requirement
//! (`ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY`), so erroring here contradicted
//! it; the two now agree. This is what lets a multi-orbital LIBRARY organism
//! drop its own `[identity]`, declare what it expects, and stay valid
//! standalone — the composed program is where the provider is checked.

use super::user_identity::vocabulary;
use super::{field_type_str, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use crate::phases::resolve::declared_struct_type;
use orbital_core_base::ir::types::{field_shape, payload_field_shape, OirType};
use orbital_core_base::schema::{
    ConfigField, EntityDefinition, EntityRef, ExpectDeclaration, FieldDefinition, Orbital,
    OrbitalSchema, PageDefinition, PayloadField, TraitDefinition, TraitReference,
};
use orbital_core_base::type_compat::{checked_sub_with_named, enum_value_is_member};

/// An orbital's `expects entity [identity]` declaration, if it carries one.
pub(super) struct IdentityExpectation<'a> {
    pub(super) name: Option<&'a str>,
    pub(super) shape: Option<&'a [FieldDefinition]>,
}

/// The orbital's first `expects entity [identity]` declaration (duplicates are an L1
/// parse error, so first-is-only).
pub(super) fn identity_expectation(orbital: &Orbital) -> Option<IdentityExpectation<'_>> {
    orbital.expects.as_ref()?.iter().find_map(|e| match e {
        ExpectDeclaration::Identity { name, shape, .. } => Some(IdentityExpectation {
            name: name.as_deref(),
            shape: shape.as_deref(),
        }),
        _ => None,
    })
}

/// Every entity name the orbital's expects declarations license. A NAMED
/// `expects entity [identity]` counts: it asserts the entity exists, and the dual form
/// (`expects entity X` alongside `expects entity X [identity]`) is
/// ELOLO_DUPLICATE_EXPECTATION — `deriveExpectations` folds the entity
/// declaration into the identity one, so identity is the only spelling a
/// derived slice can carry for its identity-entity relation targets.
pub(crate) fn expected_entity_names(orbital: &Orbital) -> Vec<&str> {
    orbital
        .expects
        .as_ref()
        .map(|expects| {
            expects
                .iter()
                .filter_map(|e| match e {
                    ExpectDeclaration::Entity { name, .. } => Some(name.as_str()),
                    ExpectDeclaration::Identity {
                        name: Some(name), ..
                    } => Some(name.as_str()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// View an `expects entity [identity]` declared shape as the identity entity it stands
/// in for, so the existing `@user` field/vocabulary machinery checks against
/// it unchanged. `identity: true` because it models the viewer's roster.
pub(super) fn shape_as_identity_entity(name: &str, shape: &[FieldDefinition]) -> EntityDefinition {
    EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        store: None,
        shared: false,
        identity: true,
        browser: false,
        locale_instances: None,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: name.to_string(),
        collection: None,
        persistence: None,
        fields: shape.to_vec(),
        instances: None,
        id: None,
        invariants: None,
        indexes: None,
    }
}

pub fn validate_expectations(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    validate_expectation_links(schema, &mut result);
    validate_propositions(schema, &mut result);
    ctx.merge(result);
}

// =============================================================================
// Spec propositions: `@invariant` / `@holds`, proved by `orbital_core_eval::proof`
// =============================================================================

/// Every spec proposition the program cannot be proved to keep is an error: a confirmed counterexample is
/// `ORB_S_INVARIANT_VIOLATED`, anything the prover cannot decide is `ORB_S_INVARIANT_UNPROVED`.
fn validate_propositions(schema: &OrbitalSchema, result: &mut ValidationResult) {
    use orbital_core_base::schema::format_sexpr;
    use orbital_core_eval::proof::{obligations::Proposition, prove, Verdict};
    for finding in prove(schema) {
        let (code, detail) = match &finding.verdict {
            Verdict::Proved => continue,
            Verdict::Violated { counterexample } => (
                ErrorCode::SchemaInvariantViolated,
                format!("is broken by {} — counterexample, confirmed by the evaluator: {counterexample}", finding.site),
            ),
            Verdict::Unproved { reason, counterexample } => (
                ErrorCode::SchemaInvariantUnproved,
                match counterexample {
                    Some(c) => format!("cannot be proved at {}: {reason} (possible counterexample: {c})", finding.site),
                    None => format!("cannot be proved at {}: {reason}", finding.site),
                },
            ),
        };
        let (what, path) = match &finding.proposition {
            Proposition::Row { entity, expr } => (
                format!("`@invariant {}` of entity `{entity}`", format_sexpr(expr)),
                proposition_path(
                    schema,
                    |e| matches!(e, ExpectDeclaration::Entity { name, invariants: Some(list), .. } if name == entity && list.contains(expr)),
                    |o| {
                        std::iter::once(&o.entity)
                            .chain(o.auxiliary_entities.iter().flatten())
                            .enumerate()
                            .find_map(|(k, e)| match e {
                                orbital_core_base::schema::EntityRef::Inline(def)
                                    if def.name == *entity
                                        && def.invariants.iter().flatten().any(|i| i == expr) =>
                                {
                                    Some(if k == 0 {
                                        "entity".to_string()
                                    } else {
                                        format!("auxiliaryEntities[{}]", k - 1)
                                    })
                                }
                                _ => None,
                            })
                    },
                ),
            ),
            Proposition::State {
                trait_name,
                state,
                expr,
            } => (
                format!(
                    "`state {state} @holds {}` of trait `{trait_name}`",
                    format_sexpr(expr)
                ),
                proposition_path(
                    schema,
                    |e| matches!(e, ExpectDeclaration::State { trait_name: t, state: s, holds: Some(_), .. } if t == trait_name && s == state),
                    |o| {
                        body_trait(o, trait_name).and_then(|(j, td)| {
                            td.state_machine
                                .states
                                .iter()
                                .position(|s| s.name == *state && s.holds.as_ref() == Some(expr))
                                .map(|k| format!("traits[{j}].stateMachine.states[{k}]"))
                        })
                    },
                ),
            ),
            Proposition::Transition {
                trait_name,
                from,
                event,
                to,
                expr,
            } => (
                format!(
                    "`{from} --{event}--> {to} @ensures {}` of trait `{trait_name}`",
                    format_sexpr(expr)
                ),
                proposition_path(
                    schema,
                    |e| {
                        matches!(e, ExpectDeclaration::Transition { trait_name: t, from: f, event: v, to: o, ensures: Some(list), .. }
                        if t == trait_name && f == from && v == event && o == to && list.contains(expr))
                    },
                    |o| {
                        body_trait(o, trait_name).and_then(|(j, td)| {
                            td.state_machine
                                .transitions
                                .iter()
                                .position(|t| {
                                    t.event == *event
                                        && t.to == *to
                                        && t.from.states().contains(&from.as_str())
                                        && t.ensures.iter().flatten().any(|e| e == expr)
                                })
                                .map(|k| format!("traits[{j}].stateMachine.transitions[{k}]"))
                        })
                    },
                ),
            ),
        };
        let suggestion = match code {
            ErrorCode::SchemaInvariantViolated => "Guard the transition so the write keeps the proposition, or change what it writes.",
            _ => "State the proposition and the guards it needs with comparisons, and/or/not and arithmetic by constants over number, string, boolean and enum fields; write values the prover can follow (a literal record, not a whole-record binding).",
        };
        result.add_error(
            ValidationError::error(code, &path, format!("{what} {detail}."))
                .with_suggestion(suggestion.to_string()),
        );
    }
}

/// The path of the first `expects` entry that carries the proposition, else of the real declaration that states it.
fn proposition_path(
    schema: &OrbitalSchema,
    matches: impl Fn(&ExpectDeclaration) -> bool,
    in_body: impl Fn(&orbital_core_base::schema::Orbital) -> Option<String>,
) -> String {
    let orbitals = || schema.orbitals.iter().enumerate();
    orbitals()
        .find_map(|(i, o)| {
            o.expects
                .iter()
                .flatten()
                .position(&matches)
                .map(|j| format!("orbitals[{i}].expects[{j}]"))
        })
        .or_else(|| orbitals().find_map(|(i, o)| in_body(o).map(|p| format!("orbitals[{i}].{p}"))))
        .unwrap_or_else(|| "orbitals".to_string())
}

fn body_trait<'a>(
    o: &'a orbital_core_base::schema::Orbital,
    name: &str,
) -> Option<(usize, &'a orbital_core_base::schema::TraitDefinition)> {
    o.traits.iter().enumerate().find_map(|(j, t)| match t {
        orbital_core_base::schema::TraitReference::Inline(td) if td.name == name => Some((j, td)),
        _ => None,
    })
}

// =============================================================================
// Compose link-checks (multi-orbital schemas only)
// =============================================================================

/// Every inline entity in the schema, paired with its owning orbital's name.
fn providers(schema: &OrbitalSchema) -> Vec<(&EntityDefinition, &str)> {
    let mut out = Vec::new();
    for orbital in schema.orbitals.iter().filter(|o| !o.is_spec()) {
        for er in
            std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            if let EntityRef::Inline(def) = er {
                out.push((def, orbital.name.as_str()));
            }
        }
    }
    out
}

/// Every trait-declared config knob named `name`, with the trait that declares it.
fn declared_knobs<'a>(schema: &'a OrbitalSchema, name: &str) -> Vec<(&'a str, &'a ConfigField)> {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter_map(|t| match t {
            TraitReference::Inline(td) => td
                .config
                .as_ref()
                .and_then(|c| c.get(name))
                .map(|k| (td.name.as_str(), k)),
            _ => None,
        })
        .collect()
}

fn find_trait<'a>(schema: &'a OrbitalSchema, name: &str) -> Option<&'a TraitDefinition> {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == name => Some(td),
            _ => None,
        })
}

/// What nothing in the program provides for `expectation`, phrased for the message, or `None`
/// when a provider exists. The one place that decides "absent", shared by `--final` and the strict
/// (spec) check so the two cannot drift.
fn unprovided(
    expectation: &ExpectDeclaration,
    schema: &OrbitalSchema,
    providers: &[(&EntityDefinition, &str)],
) -> Option<String> {
    match expectation {
        ExpectDeclaration::Entity { name, .. } => {
            (!providers.iter().any(|(def, _)| def.name == *name))
                .then(|| format!("entity `{name}`"))
        }
        ExpectDeclaration::Identity {
            name: Some(name), ..
        } => (!providers.iter().any(|(def, _)| def.name == *name))
            .then(|| format!("identity `{name}`")),
        ExpectDeclaration::Identity { name: None, .. } => {
            (!providers.iter().any(|(def, _)| def.identity))
                .then(|| "an [identity] entity".to_string())
        }
        ExpectDeclaration::Event {
            trait_name, event, ..
        } => {
            let declared = schema.orbitals.iter().any(|o| {
                o.traits
                    .iter()
                    .any(|t| matches!(t, TraitReference::Inline(td) if td.name == *trait_name))
            });
            (!declared).then(|| format!("trait `{trait_name}` (to emit `{event}`)"))
        }
        ExpectDeclaration::State { trait_name, .. }
        | ExpectDeclaration::Transition { trait_name, .. } => find_trait(schema, trait_name)
            .is_none()
            .then(|| format!("trait `{trait_name}`")),
        ExpectDeclaration::Page { path, protocol, .. } => {
            let declared = expected_pages(schema, path, protocol.as_deref())
                .next()
                .is_some();
            (!declared).then(|| match protocol {
                Some(word) => format!("a `{word}` page"),
                None => format!("page `{path}`"),
            })
        }
        ExpectDeclaration::Config { name, .. } => declared_knobs(schema, name)
            .is_empty()
            .then(|| format!("config knob `{name}` on a trait")),
    }
}

fn expected_pages<'a>(
    schema: &'a OrbitalSchema,
    route: &'a str,
    protocol: Option<&'a str>,
) -> impl Iterator<Item = &'a PageDefinition> + 'a {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .filter_map(|p| p.as_inline())
        .filter(move |page| match protocol {
            Some(word) => page.protocol.as_deref() == Some(word),
            None => page.is_route() && page.path == route,
        })
}

fn unsatisfied_error(orbital: &str, what: &str, path: &str, cause: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::SchemaExpectationUnsatisfied,
        path,
        format!("`{orbital}` expects {what}, but nothing in this program provides it — {cause}."),
    )
    .with_suggestion(
        "Provide it in an orbital of this program (give the providing import that name, or declare it here)."
            .to_string(),
    )
}

/// `orb validate --final` (G-ORB-184): where a generation concludes, the program is the whole app,
/// so an expectation nothing in it provides is no longer deferred to an importer — it is an error.
pub fn validate_final_expectations(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let providers = providers(schema);
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, expectation) in orbital.expects.iter().flatten().enumerate() {
            let path = format!("orbitals[{} ({})].expects[{}]", i, orbital.name, j);
            if let Some(what) = unprovided(expectation, schema, &providers) {
                result.add_error(unsatisfied_error(
                    &orbital.name,
                    &what,
                    &path,
                    "its reads, writes and navigation would reach nothing",
                ));
            }
        }
    }
    result
}

fn validate_expectation_links(schema: &OrbitalSchema, result: &mut ValidationResult) {
    let providers = providers(schema);
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let Some(expects) = &orbital.expects else {
            continue;
        };
        if orbital.is_spec() {
            continue;
        }
        for (j, expectation) in expects.iter().enumerate() {
            let path = format!("orbitals[{} ({})].expects[{}]", i, orbital.name, j);
            match expectation {
                ExpectDeclaration::Identity {
                    name,
                    shape: Some(shape),
                    ..
                } => {
                    let provider = match name {
                        Some(name) => providers
                            .iter()
                            .find(|(def, _)| def.name == *name)
                            .filter(|(def, _)| def.identity),
                        None => providers.iter().find(|(def, _)| def.identity),
                    };
                    if let Some((def, _)) = provider {
                        check_shape(&orbital.name, def, shape, &path, schema, result);
                    }
                }
                ExpectDeclaration::Entity {
                    name,
                    shape: Some(shape),
                    ..
                } => {
                    if let Some((def, _)) = providers.iter().find(|(def, _)| def.name == *name) {
                        check_shape(&orbital.name, def, shape, &path, schema, result);
                    }
                }
                ExpectDeclaration::Event {
                    trait_name,
                    event,
                    payload: Some(expected),
                    ..
                } => {
                    let emitted = find_trait(schema, trait_name)
                        .and_then(|td| td.emits.iter().flatten().find(|e| e.event == *event));
                    if let Some(emit) = emitted {
                        check_event_payload(
                            &orbital.name,
                            trait_name,
                            event,
                            expected,
                            emit.payload_schema.as_deref().unwrap_or(&[]),
                            &path,
                            schema,
                            result,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// The payload an `expects trait T { emits { E { fields } } }` names, against what the provider's
/// `E` carries: every expected field present, of a compatible type, and required whenever the
/// expectation requires it; a closed value set must be a subset of the provider's.
fn check_event_payload(
    consumer: &str,
    trait_name: &str,
    event: &str,
    expected: &[PayloadField],
    provided: &[PayloadField],
    path: &str,
    schema: &OrbitalSchema,
    result: &mut ValidationResult,
) {
    for want in expected {
        let field_path = format!("{}.{}", path, want.name);
        let Some(have) = provided.iter().find(|p| p.name == want.name) else {
            continue;
        };
        let required_ok = !want.required.unwrap_or(false) || have.required.unwrap_or(false);
        let types_ok = expectation_shapes_compatible(
            payload_field_shape(want),
            payload_field_shape(have),
            want.values.as_ref().map(|_| false),
            have.values.as_ref().map(|_| false),
            schema,
        );
        let values_ok = match (&want.values, &have.values) {
            (Some(w), Some(h)) => w.iter().all(|m| enum_value_is_member(m, h)),
            _ => true,
        };
        if required_ok && types_ok && values_ok {
            continue;
        }
        let detail = if !required_ok {
            "the event declares it optional, but the expectation requires it".to_string()
        } else if !values_ok {
            "the expected values are not all members of the event's".to_string()
        } else {
            format!(
                "the event declares `{}`, the expectation `{}`",
                have.field_type, want.field_type
            )
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaExpectedFieldTypeMismatch,
                &field_path,
                format!(
                    "`{consumer}` expects `{trait_name}.{event}` payload `{}` : {}, but {detail}.",
                    want.name, want.field_type
                ),
            )
            .with_suggestion(format!(
                "Align `{}` in `{event}`'s payload and the expectation.",
                want.name
            )),
        );
    }
}

/// A field's type as a message names it: a relation names the entity it points at.
fn field_type_label(field: &FieldDefinition) -> String {
    match &field.relation {
        Some(relation) => format!("relation to `{}`", relation.entity),
        None => field_type_str(field),
    }
}

/// The §5.1 variance rules, per expected field: the provider must declare the
/// name (width: extra provider fields are fine), the types must be compatible
/// per orbital-core's `type_compat`, closed literal unions compare by subset,
/// and optionality is one-directional — a required expectation demands a
/// required provider field, while an optional expectation accepts either.
fn check_shape(
    consumer: &str,
    provider: &EntityDefinition,
    shape: &[FieldDefinition],
    path: &str,
    schema: &OrbitalSchema,
    result: &mut ValidationResult,
) {
    for expected in shape {
        let Some(field_name) = expected.name.as_deref() else {
            continue;
        };
        let field_path = format!("{}.{}", path, field_name);
        let Some(actual) = provider
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(field_name))
        else {
            continue;
        };

        let expected_type = field_type_label(expected);
        let actual_type = field_type_label(actual);
        // Optionality is one-directional: the failure is a consumer DEMANDING
        // a guarantee the provider does not give (expected required, provider
        // optional). A provider that always supplies the field satisfies an
        // optional expectation — required is the stronger guarantee.
        let required_match =
            !expected.required.unwrap_or(false) || actual.required.unwrap_or(false);
        let vocab_ok = match vocabulary(expected) {
            None => true,
            // An open provider (no declared vocabulary) knows every member.
            Some(members) => match vocabulary(actual) {
                None => true,
                Some(provider_members) => members
                    .iter()
                    .all(|m| enum_value_is_member(m, provider_members)),
            },
        };
        if required_match && vocab_ok && types_compatible(expected, actual, schema) {
            continue;
        }

        let detail = if !required_match {
            format!(
                "`{}` declares it optional, but the expectation requires it",
                provider.name
            )
        } else if !vocab_ok {
            let missing: Vec<String> = vocabulary(expected)
                .map(|members| {
                    let provider_members = vocabulary(actual);
                    members
                        .iter()
                        .filter(|m| {
                            !provider_members
                                .map(|p| enum_value_is_member(m, p))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "values {} are not members of the provider's vocabulary",
                missing.join(", ")
            )
        } else {
            format!(
                "type `{}` is not compatible with `{}`",
                expected_type, actual_type
            )
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaExpectedFieldTypeMismatch,
                &field_path,
                format!(
                    "`{}` expects `{}.{} : {}`, but `{}` declares `{}` — {}.",
                    consumer,
                    provider.name,
                    field_name,
                    expected_type,
                    provider.name,
                    actual_type,
                    detail
                ),
            )
            .with_suggestion(format!(
                "Align the two declarations: retype `{}`'s expectation or `{}`'s field so they agree.",
                consumer, provider.name
            )),
        );
    }
}

/// Provider values must satisfy the consumer's shape without a conversion.
/// Vocabulary membership is checked separately by the expectation's subset rule.
fn types_compatible(
    expected: &FieldDefinition,
    actual: &FieldDefinition,
    schema: &OrbitalSchema,
) -> bool {
    expectation_shapes_compatible(
        field_shape(expected),
        field_shape(actual),
        vocabulary(expected).map(|_| expected.values.as_ref().is_none_or(Vec::is_empty)),
        vocabulary(actual).map(|_| actual.values.as_ref().is_none_or(Vec::is_empty)),
        schema,
    )
}

fn expectation_shapes_compatible(
    mut expected: OirType,
    mut actual: OirType,
    expected_vocabulary: Option<bool>,
    actual_vocabulary: Option<bool>,
    schema: &OrbitalSchema,
) -> bool {
    for (mut shape, vocabulary) in [
        (&mut expected, expected_vocabulary),
        (&mut actual, actual_vocabulary),
    ] {
        let Some(array_item) = vocabulary else {
            continue;
        };
        while let OirType::Optional { inner } = shape {
            shape = inner;
        }
        if array_item {
            let OirType::Array { inner } = shape else {
                continue;
            };
            shape = inner;
            while let OirType::Optional { inner } = shape {
                shape = inner;
            }
        }
        if matches!(shape, OirType::Enum { .. }) {
            *shape = OirType::String;
        }
    }
    checked_sub_with_named(&actual, &expected, &|name| {
        declared_struct_type(schema, name)
    })
    .is_some()
}

#[cfg(test)]
mod final_expectation_tests {
    use super::*;

    fn program(provider_entity: &str, identity: bool) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [
                { "name": "Apps", "entity": { "name": provider_entity, "persistence": "persistent", "identity": identity, "fields": [{ "name": "id", "type": "string" }] }, "traits": [], "pages": [] },
                { "name": "Builder", "entity": { "name": "BuilderView", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                  "expects": [{ "kind": "entity", "name": "StudioApp" }], "traits": [], "pages": [] }
            ]
        }))
        .expect("program json")
    }

    #[test]
    fn an_expectation_no_entity_provides_is_an_error_in_a_finished_program() {
        let r = validate_final_expectations(&program("AppsENStudioApp", false));
        assert!(
            r.errors
                .iter()
                .any(|e| e.code == ErrorCode::SchemaExpectationUnsatisfied
                    && e.message.contains("StudioApp")),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn control_a_provided_name_passes() {
        assert!(validate_final_expectations(&program("StudioApp", false))
            .errors
            .is_empty());
    }

    fn alias_program() -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "Llm",
                "entity": { "name": "Call", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "types": {
                    "Arg": [{ "name": "value", "type": "string", "required": true }],
                    "Other": [{ "name": "value", "type": "number", "required": true }]
                },
                "traits": [],
                "pages": []
            }]
        }))
        .expect("program json")
    }

    fn list_of(name: &str) -> OirType {
        OirType::array(OirType::TypeRef {
            name: name.to_string(),
        })
    }

    #[test]
    fn a_list_of_an_alias_satisfies_an_expectation_of_the_same_alias() {
        let schema = alias_program();
        assert!(expectation_shapes_compatible(
            list_of("Arg"),
            list_of("Arg"),
            None,
            None,
            &schema
        ));
    }

    #[test]
    fn control_a_list_of_another_alias_does_not_satisfy_the_expectation() {
        let schema = alias_program();
        assert!(!expectation_shapes_compatible(
            list_of("Arg"),
            list_of("Other"),
            None,
            None,
            &schema
        ));
        assert!(!expectation_shapes_compatible(
            list_of("Arg"),
            list_of("Undeclared"),
            None,
            None,
            &schema
        ));
    }
}
