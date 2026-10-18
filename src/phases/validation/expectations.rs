//! `expects` declarations — consumer-side requirement contracts.
//!
//! Standalone (single-orbital slice) expectations are TRUSTED: they satisfy
//! the identity rules (handled in `user_identity`/`entity_access`, which read
//! the helpers here) and license relation/persist/fetch/`linkedEntity`
//! targets (via `collect_names`). This module owns the two checks that need
//! no sibling context or the whole program:
//!
//! - **Self-consistency (every schema):** a `persist` payload field outside
//!   the consumer's own declared `expects entity X { shape }` fails with
//!   `ORB_S_EXPECTATION_FIELD_UNDECLARED` — the declaration is the consumer's
//!   source of truth. Existence-only expectations (no shape) opt out.
//! - **Compose link-checks (every schema):** an expectation whose provider IS
//!   present in this program must be compatible with it — shape per the
//!   proposal's variance rules (width: provider may know more; field types via
//!   orbital-core's `type_compat` only; closed literal unions: expectation
//!   members ⊆ provider members), and an `expects entity [identity]` provider must
//!   carry the `[identity]` tag.
//!
//! An expectation whose provider is **absent** is DEFERRED, never an error:
//! that is the declaration doing its job — a promise about the neighborhood,
//! checked at the link step once a consumer supplies it. `entity_access`
//! already trusts any `expects entity [identity]` to satisfy the roster requirement
//! (`ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY`), so erroring here contradicted
//! it; the two now agree. This is what lets a multi-orbital LIBRARY organism
//! drop its own `[identity]`, declare what it expects, and stay valid
//! standalone — the composed program is where the provider is checked.

use super::fired_events::{collect_schema_fired_events, trait_has_dynamic_event_prop_binding, FiredEvents};
use super::entity_mutation::trait_lands_a_write;
use super::user_identity::vocabulary;
use super::{field_type_str, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, ExpectDeclaration, FieldDefinition, Orbital, OrbitalSchema,
    PayloadField, SExpression, StringOrArray, TraitDefinition, TraitReference,
};
use orbital_core_base::type_compat::{enum_value_is_member, set_value_type_compatible};

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

/// The declared shape of the orbital's `expects entity <name>`, if declared.
pub(super) fn entity_expectation<'a>(
    orbital: &'a Orbital,
    name: &str,
) -> Option<Option<&'a [FieldDefinition]>> {
    orbital.expects.as_ref()?.iter().find_map(|e| match e {
        ExpectDeclaration::Entity { name: n, shape, .. } if n == name => Some(shape.as_deref()),
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
        shared: false,
        identity: true,
        local: false,
        seed_mock: false,
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
    }
}

pub fn validate_expectations(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    validate_persist_payload_self_consistency(schema, &mut result);
    // Runs for EVERY schema. Orbital count used to gate this as a proxy for
    // "is this a complete program", which mis-classified a multi-orbital
    // library as complete; the real distinction is now made per expectation —
    // absent provider = deferred, present-but-incompatible = error.
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
                proposition_path(schema, |e| matches!(e, ExpectDeclaration::Entity { name, invariants: Some(list), .. } if name == entity && list.contains(expr))),
            ),
            Proposition::State { trait_name, state, expr } => (
                format!("`state {state} @holds {}` of trait `{trait_name}`", format_sexpr(expr)),
                proposition_path(schema, |e| matches!(e, ExpectDeclaration::State { trait_name: t, state: s, holds: Some(_), .. } if t == trait_name && s == state)),
            ),
        };
        let suggestion = match code {
            ErrorCode::SchemaInvariantViolated => "Guard the transition so the write keeps the proposition, or change what it writes.",
            _ => "State the proposition and the guards it needs with comparisons, and/or/not and arithmetic by constants over number, string, boolean and enum fields; write values the prover can follow (a literal record, not a whole-record binding).",
        };
        result.add_error(ValidationError::error(code, &path, format!("{what} {detail}.")).with_suggestion(suggestion.to_string()));
    }
}

/// The path of the first `expects` entry that carries the proposition.
fn proposition_path(schema: &OrbitalSchema, matches: impl Fn(&ExpectDeclaration) -> bool) -> String {
    schema
        .orbitals
        .iter()
        .enumerate()
        .find_map(|(i, o)| o.expects.iter().flatten().position(&matches).map(|j| format!("orbitals[{i}].expects[{j}]")))
        .unwrap_or_else(|| "orbitals".to_string())
}

// =============================================================================
// Self-consistency: persist payloads vs the consumer's own declared shape
// =============================================================================

fn validate_persist_payload_self_consistency(
    schema: &OrbitalSchema,
    result: &mut ValidationResult,
) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.expects.is_none() {
            continue;
        }
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let orbital_core_base::schema::TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            // Same authored-surface rule as user_identity: persist effects
            // inside `uses[]`-inlined bodies (`source_behavior`) are the
            // imported behavior's own contract, not this orbital's declared
            // surface.
            if trait_def.source_behavior.is_some() {
                continue;
            }
            let path = super::trait_path(i, &orbital.name, j, trait_ref);
            for (t, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                if let Some(effects) = &transition.effects {
                    for (e, effect) in effects.iter().enumerate() {
                        let effect_path =
                            format!("{}.stateMachine.transitions[{}].effects[{}]", path, t, e);
                        check_persist_payload(effect, &effect_path, orbital, result);
                    }
                }
            }
            if let Some(ticks) = &trait_def.ticks {
                for (t, tick) in ticks.iter().enumerate() {
                    for (e, effect) in tick.effects.iter().enumerate() {
                        let effect_path = format!("{}.ticks[{}].effects[{}]", path, t, e);
                        check_persist_payload(effect, &effect_path, orbital, result);
                    }
                }
            }
        }
    }
}

/// `["persist", "create", "X", { …payload }]` writing to an `expects entity X
/// { shape }` must stay inside the declared shape — the declaration is the
/// opting-in; payload fields are checked nowhere else.
fn check_persist_payload(
    effect: &SExpression,
    path: &str,
    orbital: &Orbital,
    result: &mut ValidationResult,
) {
    let SExpression::Array(arr) = effect else {
        return;
    };
    if !matches!(arr.first(), Some(SExpression::String(op)) if op == "persist") || arr.len() < 3 {
        return;
    }
    if matches!(&arr[1], SExpression::String(op) if op == "batch") {
        for w in orbital_core_base::persist_batch::batch_writes(&arr[2]) {
            if let Some(SExpression::Object(payload)) = w.data {
                check_persist_payload_keys(w.entity, payload, path, orbital, result);
            }
        }
        return;
    }
    // Mirror effect/persist.rs's format detection.
    let entity_index = if matches!(&arr[1], SExpression::String(op) if op == "create" || op == "upsert" || op == "update" || op == "delete")
    {
        2
    } else {
        1
    };
    let Some(SExpression::String(entity_name)) = arr.get(entity_index) else {
        return;
    };
    let Some(SExpression::Object(payload)) = arr.get(entity_index + 1) else {
        return; // binding/computed payload: nothing static to check
    };
    // A trailing object carrying `emit:` is the resolver's options bag, not
    // the data payload (same convention as effect/persist.rs).
    if entity_index + 1 == arr.len() - 1 && payload.contains_key("emit") {
        return;
    }
    check_persist_payload_keys(entity_name, payload, path, orbital, result);
}

fn check_persist_payload_keys(
    entity_name: &str,
    payload: &orbital_core_base::schema::SObject,
    path: &str,
    orbital: &Orbital,
    result: &mut ValidationResult,
) {
    let Some(Some(shape)) = entity_expectation(orbital, entity_name) else {
        return; // no expectation, or existence-only: as unchecked as today
    };
    for key in payload.keys() {
        if shape
            .iter()
            .any(|f| f.name.as_deref() == Some(key.as_str()))
        {
            continue;
        }
        let mut declared: Vec<&str> = shape.iter().filter_map(|f| f.name.as_deref()).collect();
        declared.sort_unstable();
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaExpectationFieldUndeclared,
                path,
                format!(
                    "`{}` persists `{}` to `{}`, but its `expects entity {}` shape declares only: \
                     {}. The declaration is this orbital's own source of truth for the fields it \
                     touches.",
                    orbital.name,
                    key,
                    entity_name,
                    entity_name,
                    declared.join(", ")
                ),
            )
            .with_suggestion(format!(
                "Add `{}` to the `expects entity {}` shape, or remove it from the persist payload.",
                key, entity_name
            )),
        );
    }
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

fn find_trait<'a>(schema: &'a OrbitalSchema, name: &str) -> Option<&'a TraitDefinition> {
    schema.orbitals.iter().flat_map(|o| o.traits.iter()).find_map(|t| match t {
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
            (!providers.iter().any(|(def, _)| def.name == *name)).then(|| format!("entity `{name}`"))
        }
        ExpectDeclaration::Identity {
            name: Some(name), ..
        } => (!providers.iter().any(|(def, _)| def.name == *name))
            .then(|| format!("identity `{name}`")),
        ExpectDeclaration::Identity { name: None, .. } => (!providers.iter().any(|(def, _)| def.identity))
            .then(|| "an [identity] entity".to_string()),
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
        ExpectDeclaration::State { trait_name, .. } | ExpectDeclaration::Transition { trait_name, .. } => {
            find_trait(schema, trait_name).is_none().then(|| format!("trait `{trait_name}`"))
        }
        ExpectDeclaration::Page { path, .. } => {
            let declared = schema.orbitals.iter().any(|o| {
                o.pages
                    .iter()
                    .flatten()
                    .any(|p| p.as_inline().is_some_and(|page| page.path == *path))
            });
            (!declared).then(|| format!("page `{path}`"))
        }
    }
}

fn unsatisfied_error(
    orbital: &str,
    what: &str,
    path: &str,
    cause: &str,
) -> ValidationError {
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
    let mut fired: Option<FiredEvents> = None;
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let Some(expects) = &orbital.expects else {
            continue;
        };
        if orbital.is_spec() {
            continue;
        }
        for (j, expectation) in expects.iter().enumerate() {
            let path = format!("orbitals[{} ({})].expects[{}]", i, orbital.name, j);
            if expectation.is_strict() {
                if let Some(what) = unprovided(expectation, schema, &providers) {
                    result.add_error(unsatisfied_error(
                        &orbital.name,
                        &what,
                        &path,
                        "a spec obligation is never deferred",
                    ));
                }
            }
            match expectation {
                ExpectDeclaration::Identity { name, shape, .. } => {
                    check_identity_link(&orbital.name, name, shape, &providers, &path, result);
                }
                ExpectDeclaration::Entity { name, shape, .. } => {
                    let provider = providers.iter().find(|(def, _)| def.name == *name);
                    match provider {
                        // No provider here at all — DEFERRED to whoever imports
                        // this program (see the module doc). Not an error.
                        None => {}
                        Some((def, _)) => {
                            if let Some(shape) = shape {
                                check_shape(&orbital.name, def, shape, &path, result);
                            }
                            check_entity_facts(&orbital.name, def, expectation, &path, result);
                            if expectation.is_strict() && persistent_never_written(expectation, schema) {
                                result.add_error(unsatisfied_error(
                                    &orbital.name,
                                    &format!("entity `{name}` to be persisted"),
                                    &path,
                                    &format!("no trait in the program writes `{name}` with a `persist create|update|upsert`"),
                                ));
                            }
                        }
                    }
                }
                ExpectDeclaration::State { trait_name, state, .. } => {
                    if let Some(td) = find_trait(schema, trait_name) {
                        if !td.state_machine.states.iter().any(|s| s.name == *state) {
                            result.add_error(unsatisfied_error(
                                &orbital.name,
                                &format!("trait `{trait_name}` to declare state `{state}`"),
                                &path,
                                &format!("`{trait_name}` declares no such state"),
                            ));
                        }
                    }
                }
                ExpectDeclaration::Transition {
                    trait_name,
                    from,
                    event,
                    to,
                    ..
                } => {
                    if let Some(td) = find_trait(schema, trait_name) {
                        let has_edge = td.state_machine.transitions.iter().any(|t| {
                            t.event == *event
                                && t.to == *to
                                && match &t.from {
                                    StringOrArray::Single(f) => f == from,
                                    StringOrArray::Multiple(fs) => fs.contains(from),
                                }
                        });
                        if !has_edge {
                            result.add_error(unsatisfied_error(
                                &orbital.name,
                                &format!("trait `{trait_name}` to move `{from}` to `{to}` on `{event}`"),
                                &path,
                                &format!("`{trait_name}` has no such transition"),
                            ));
                        }
                    }
                }
                // A present trait of that name must emit the event; no such trait here is
                // DEFERRED to whoever imports this program, like every other expectation.
                ExpectDeclaration::Event {
                    trait_name,
                    event,
                    payload,
                    ..
                } => {
                    let provider = schema.orbitals.iter().find_map(|o| {
                        o.traits.iter().find_map(|t| match t {
                            TraitReference::Inline(td) if td.name == *trait_name => {
                                Some((td, o.name.as_str()))
                            }
                            _ => None,
                        })
                    });
                    if let Some((td, provider_orbital)) = provider {
                        let emitted = td.emits.iter().flatten().find(|e| e.event == *event);
                        if let (Some(emit), Some(expected)) = (emitted, payload) {
                            check_event_payload(
                                &orbital.name,
                                trait_name,
                                event,
                                expected,
                                emit.payload_schema.as_deref().unwrap_or(&[]),
                                &path,
                                result,
                            );
                        }
                        let emits = emitted.is_some();
                        if emits
                            && expectation.is_strict()
                            && !fired.get_or_insert_with(|| collect_schema_fired_events(schema)).contains_for_trait(trait_name, event)
                            && !trait_has_dynamic_event_prop_binding(td)
                        {
                            result.add_error(unsatisfied_error(
                                &orbital.name,
                                &format!("trait `{trait_name}` to fire `{event}`"),
                                &path,
                                &format!("`{trait_name}` declares `{event}` in `emits` but nothing in it fires it"),
                            ));
                        }
                        if !emits {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::SchemaExpectationUnsatisfied,
                                    &path,
                                    format!(
                                        "`{}` expects `{}.{}`, but trait `{}` in `{}` does not emit `{}`.",
                                        orbital.name, trait_name, event, trait_name, provider_orbital, event
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Declare `{event}` in `{trait_name}`'s `emits`, or listen to the trait that emits it."
                                )),
                            );
                        }
                    }
                }
                // A route expectation carries no shape, so there is nothing to
                // link-check against a present provider, and an absent one is
                // DEFERRED like every other expectation. Reachability is not
                // weakened: the declaration is only ever derived for a target
                // that already matched a page of the whole organism, so a
                // genuine typo derives nothing and still fails `validate_navigate`.
                ExpectDeclaration::Page {
                    path: route,
                    traits: Some(expected),
                    ..
                } => {
                    let page = schema
                        .orbitals
                        .iter()
                        .flat_map(|o| o.pages.iter().flatten())
                        .find_map(|p| p.as_inline().filter(|page| page.path == *route));
                    if let Some(page) = page {
                        let mounted: Vec<&str> = page
                            .traits
                            .iter()
                            .flatten()
                            .filter_map(TraitReference::trait_name)
                            .collect();
                        let missing: Vec<&str> = expected
                            .iter()
                            .map(String::as_str)
                            .filter(|t| !mounted.contains(t))
                            .collect();
                        if !missing.is_empty() {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::SchemaExpectationProviderMismatch,
                                    &path,
                                    format!(
                                        "`{}` expects page `{route}` to mount {}, but the page mounts: {}.",
                                        orbital.name,
                                        missing.join(", "),
                                        if mounted.is_empty() { "nothing".to_string() } else { mounted.join(", ") }
                                    ),
                                )
                                .with_suggestion(format!("Mount {} on the page, or drop it from the expectation.", missing.join(", "))),
                            );
                        }
                    }
                }
                ExpectDeclaration::Page { traits: None, .. } => {}
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
    result: &mut ValidationResult,
) {
    for want in expected {
        let field_path = format!("{}.{}", path, want.name);
        let Some(have) = provided.iter().find(|p| p.name == want.name) else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::SchemaExpectedFieldNotProvided,
                    &field_path,
                    format!(
                        "`{consumer}` expects `{trait_name}.{event}` to carry `{}`; the event declares no such payload field.",
                        want.name
                    ),
                )
                .with_suggestion(format!(
                    "Add `{}` to `{event}`'s payload in `{trait_name}`, or drop it from the expectation.",
                    want.name
                )),
            );
            continue;
        };
        let required_ok = !want.required.unwrap_or(false) || have.required.unwrap_or(false);
        let types_ok = set_value_type_compatible(&want.field_type, &have.field_type)
            || set_value_type_compatible(&have.field_type, &want.field_type);
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
            format!("the event declares `{}`, the expectation `{}`", have.field_type, want.field_type)
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

/// What an `expects entity X [persistent, shared] { @read … }` states about the provider beyond its fields: the
/// storage mode, `shared`/`local`, each stated access policy (structurally equal) and each declared waiver.
fn check_entity_facts(
    consumer: &str,
    provider: &EntityDefinition,
    expectation: &ExpectDeclaration,
    path: &str,
    result: &mut ValidationResult,
) {
    let ExpectDeclaration::Entity {
        persistence,
        shared,
        local,
        read_policy,
        create_policy,
        update_policy,
        delete_policy,
        access_waivers,
        ..
    } = expectation
    else {
        return;
    };
    let mut mismatch = |what: String, suggestion: String| {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaExpectationProviderMismatch,
                path,
                format!("`{consumer}` expects `{}` {what}.", provider.name),
            )
            .with_suggestion(suggestion),
        );
    };
    if let Some(mode) = persistence {
        let actual = provider.persistence.clone().unwrap_or_default();
        if actual != *mode {
            mismatch(
                format!("to be {mode:?}, but it is {actual:?}").to_lowercase(),
                format!("Change `{}`'s storage mode, or the expectation's.", provider.name),
            );
        }
    }
    if *shared && !provider.shared {
        mismatch("to be `shared`, but it is not".to_string(), format!("Tag `{}` `[shared]`, or drop the tag from the expectation.", provider.name));
    }
    if *local && !provider.local {
        mismatch("to be `local`, but it is not".to_string(), format!("Tag `{}` `[local]`, or drop the tag from the expectation.", provider.name));
    }
    for (op, expected, actual) in [
        ("read", read_policy, &provider.read_policy),
        ("create", create_policy, &provider.create_policy),
        ("update", update_policy, &provider.update_policy),
        ("delete", delete_policy, &provider.delete_policy),
    ] {
        if let Some(expected) = expected {
            if actual.as_ref() != Some(expected) {
                mismatch(
                    format!("to carry the `@{op}` policy it states, but its `@{op}` is {}", if actual.is_some() { "a different expression" } else { "absent" }),
                    format!("Make `{}`'s `@{op}` the same expression, or change the expectation.", provider.name),
                );
            }
        }
    }
    if let Some(wanted) = access_waivers {
        for (op, expected, actual) in [
            ("read", &wanted.read, provider.access_waivers.as_ref().and_then(|w| w.read.as_ref())),
            ("create", &wanted.create, provider.access_waivers.as_ref().and_then(|w| w.create.as_ref())),
            ("update", &wanted.update, provider.access_waivers.as_ref().and_then(|w| w.update.as_ref())),
            ("delete", &wanted.delete, provider.access_waivers.as_ref().and_then(|w| w.delete.as_ref())),
        ] {
            if expected.is_some() && actual.is_none() {
                mismatch(
                    format!("to declare `@{op} none` (its omission, with a reason), but it does not"),
                    format!("Declare `@{op} none \"<reason>\"` on `{}`, or drop it from the expectation.", provider.name),
                );
            }
        }
    }
}

/// An `expects entity X [persistent]` obliges the program to write `X`: true when no trait of the program
/// lands a `persist create|update|upsert` of it.
fn persistent_never_written(expectation: &ExpectDeclaration, schema: &OrbitalSchema) -> bool {
    let ExpectDeclaration::Entity {
        name,
        persistence: Some(orbital_core_base::schema::Persistence::Persistent),
        ..
    } = expectation
    else {
        return false;
    };
    !schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .any(|t| matches!(t, TraitReference::Inline(td) if trait_lands_a_write(td, name)))
}

fn check_identity_link(
    consumer: &str,
    name: &Option<String>,
    shape: &Option<Vec<FieldDefinition>>,
    providers: &[(&EntityDefinition, &str)],
    path: &str,
    result: &mut ValidationResult,
) {
    match name {
        Some(name) => {
            let named = providers.iter().find(|(def, _)| def.name == *name);
            match named {
                // No provider here at all — DEFERRED to the importer.
                None => {}
                Some((def, provider_orbital)) if !def.identity => result.add_error(
                    ValidationError::error(
                        ErrorCode::SchemaExpectationProviderMismatch,
                        path,
                        format!(
                            "`{}` expects `{}` to be the [identity] entity, but `{}` provides \
                             `{}` without the [identity] tag — `@user` will never resolve to it.",
                            consumer, name, provider_orbital, name
                        ),
                    )
                    .with_suggestion(format!(
                        "Tag `{}` [identity] in `{}`, or change the expectation to \
                         `expects entity {}`.",
                        name, provider_orbital, name
                    )),
                ),
                Some((def, _)) => {
                    if let Some(shape) = shape {
                        check_shape(consumer, def, shape, path, result);
                    }
                }
            }
        }
        None => {
            let identity = providers.iter().find(|(def, _)| def.identity);
            match identity {
                // No identity here at all — DEFERRED to the importer.
                None => {}
                Some((def, _)) => {
                    if let Some(shape) = shape {
                        check_shape(consumer, def, shape, path, result);
                    }
                }
            }
        }
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
            result.add_error(
                ValidationError::error(
                    ErrorCode::SchemaExpectedFieldNotProvided,
                    &field_path,
                    format!(
                        "`{}` expects `{}.{}`; `{}` declares no such field.",
                        consumer, provider.name, field_name, provider.name
                    ),
                )
                .with_suggestion(format!(
                    "Add `{}` to `{}`, or drop it from `{}`'s expectation.",
                    field_name, provider.name, consumer
                )),
            );
            continue;
        };

        for (mark, wanted, has) in [
            ("@key", expected.primary_key, actual.primary_key),
            ("@unique", expected.unique, actual.unique),
        ] {
            if wanted == Some(true) && has != Some(true) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SchemaExpectationProviderMismatch,
                        &field_path,
                        format!(
                            "`{}` expects `{}.{}` to be marked `{mark}`; `{}` declares it without.",
                            consumer, provider.name, field_name, provider.name
                        ),
                    )
                    .with_suggestion(format!("Mark `{field_name}` `{mark}` in `{}`, or drop the mark from the expectation.", provider.name)),
                );
            }
        }

        let expected_type = field_type_str(expected);
        let actual_type = field_type_str(actual);
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
        if required_match && vocab_ok && types_compatible(expected, actual) {
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

/// Type compatibility via orbital-core's canonical predicate ONLY. The
/// consumer's declared type is what it assumes the provider's values to be, so
/// either assignment direction discharges the check; two closed literal unions
/// (a `string`+`values` expectation against an `enum`+`values` provider) are
/// string-kind by construction and compare by the subset rule instead.
fn types_compatible(expected: &FieldDefinition, actual: &FieldDefinition) -> bool {
    let e = field_type_str(expected);
    let a = field_type_str(actual);
    if set_value_type_compatible(&e, &a) || set_value_type_compatible(&a, &e) {
        return true;
    }
    vocabulary(expected).is_some() && vocabulary(actual).is_some()
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
}
