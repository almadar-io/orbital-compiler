//! Reflection answered at build time: a `behavior/describe` whose argument is a literal trait value
//! names a trait the build already has, so its answer is computed here and the effect becomes the
//! emit its success route would have delivered: `(emit S { result: (quote <description>) })`.
//! Every runtime already runs that, so a program with no host (a static site) still gets the
//! answer. Runs on the program a runtime receives, after validation has seen the program as written.

use orbital_core_base::json_value::{from_json_value};
use super::{CompileResult, CompilerError};
use std::sync::Arc;
use orbital_core_base::schema::quote::quote_expr;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};
use std::path::Path;
use std::sync::OnceLock;

/// `orb behaviors describe` for a trait value: the same answer the host gives at runtime. A
/// relative literal resolves from the program that names it (`from`), as its `uses` do.
pub type TraitDescriber =
    fn(&serde_json::Value, Option<&Path>) -> Result<serde_json::Value, String>;

static DESCRIBER: OnceLock<TraitDescriber> = OnceLock::new();

/// The describer lives in the binary (it reads installed packages and their sidecars), so the
/// binary registers it, as it registers the `.lolo` lowerer.
pub fn register_trait_describer(describer: TraitDescriber) {
    let _ = DESCRIBER.set(describer);
}

const DESCRIBE: &str = "behavior/describe";

/// The answers one fold has computed. A fold owns its cache and drops it on return, so nothing a
/// build described outlives the build.
#[derive(Default)]
struct Described(std::collections::HashMap<(String, Option<std::path::PathBuf>), serde_json::Value>);

/// Fold a program a runtime receives: the program and every lazily-used behavior it carries,
/// sharing one set of answers.
pub fn fold_program(
    schema: &mut OrbitalSchema,
    lazy: &mut [super::lazy::ResolvedLazy],
    from: Option<&Path>,
) -> CompileResult<()> {
    fold_program_with(schema, lazy, from, &mut Described::default())
}

fn fold_program_with(
    schema: &mut OrbitalSchema,
    lazy: &mut [super::lazy::ResolvedLazy],
    from: Option<&Path>,
    described: &mut Described,
) -> CompileResult<()> {
    fold_schema(schema, from, described)?;
    for child in lazy {
        fold_program_with(&mut child.schema, &mut child.lazy, from, described)?;
    }
    Ok(())
}

/// Fold every literal `behavior/describe` in `schema`'s traits.
pub fn fold_literal_reflection(
    schema: &mut OrbitalSchema,
    from: Option<&Path>,
) -> CompileResult<()> {
    fold_schema(schema, from, &mut Described::default())
}

fn fold_schema(
    schema: &mut OrbitalSchema,
    from: Option<&Path>,
    described: &mut Described,
) -> CompileResult<()> {
    for orbital in &mut schema.orbitals {
        for t in &mut orbital.traits {
            let TraitReference::Inline(td) = t else {
                continue;
            };
            let td = Arc::make_mut(td);
            let knobs = literal_knobs(td.config.as_ref());
            let mut answered = Vec::new();
            let mut folded: CompileResult<()> = Ok(());
            td.state_machine.edit(|machine| {
                folded = (|| {
                    for transition in &mut machine.transitions {
                        for effect in transition.effects.iter_mut().flatten() {
                            fold(effect, &knobs, from, described, &mut answered)?;
                        }
                    }
                    Ok(())
                })();
            });
            folded?;
            for tick in td.ticks.iter_mut().flatten() {
                for effect in &mut tick.effects {
                    fold(effect, &knobs, from, described, &mut answered)?;
                }
            }
            drop_unreachable_failures(td, &answered);
        }
    }
    Ok(())
}

/// A describe answered at build time cannot fail, so its failure route is dead: an `internal` failure event nothing
/// left in the trait fires loses its declaration, its local listen and its arms, keeping the folded program as valid
/// as the program written. An `external` one stays for the traits that hear it.
fn drop_unreachable_failures(td: &mut orbital_core_base::schema::TraitDefinition, answered: &[String]) {
    use orbital_core_base::schema::EventScope;
    let fires = |event: &str, td: &orbital_core_base::schema::TraitDefinition| {
        let named = |e: &SExpression| mentions(e, event);
        td.state_machine.transitions.iter().flat_map(|t| t.effects.iter().flatten()).any(named)
            || td.ticks.iter().flatten().flat_map(|tick| tick.effects.iter()).any(named)
            || td.listens.iter().flat_map(|l| l.iter()).any(|l| l.source.is_some() && (l.triggers.as_deref() == Some(event) || (l.triggers.is_none() && l.event == event)))
    };
    let dead: Vec<&String> = answered
        .iter()
        .filter(|event| {
            let internal = td.emits.iter().flat_map(|e| e.iter()).any(|e| &e.event == *event && matches!(e.scope, Some(EventScope::Internal)));
            internal && !fires(event, td)
        })
        .collect();
    if dead.is_empty() {
        return;
    }
    let is_dead = |event: &str| dead.iter().any(|d| d.as_str() == event);
    if let Some(emits) = td.emits.as_mut() {
        emits.edit(|emits| emits.retain(|e| !is_dead(&e.event)));
    }
    if let Some(listens) = td.listens.as_mut() {
        listens.edit(|listens| listens.retain(|l| l.source.is_some() || !is_dead(&l.event)));
    }
    td.state_machine.edit(|machine| machine.transitions.retain(|t| !is_dead(&t.event)));
}

/// Whether `expr` names `event` anywhere: an `(emit …)`, an emit route, or any other place a step could fire it from.
fn mentions(expr: &SExpression, event: &str) -> bool {
    match expr {
        SExpression::String(s) => s.as_str() == event,
        SExpression::Array(items) => items.iter().any(|e| mentions(e, event)),
        SExpression::Object(map) => map.values().any(|e| mentions(e, event)),
        _ => false,
    }
}

/// A trait's config knobs whose default is a literal trait value: `@config.<knob>` names that value at build time,
/// since a trait's config never changes while it runs.
type LiteralKnobs = std::collections::HashMap<String, SExpression>;

fn literal_knobs(
    config: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
) -> LiteralKnobs {
    config
        .into_iter()
        .flatten()
        .filter_map(|(name, field)| {
            let value = field.default.as_ref()?;
            is_literal_trait_value(value).then(|| (format!("@config.{name}"), value.clone()))
        })
        .collect()
}

/// Fold `expr`, recording in `failures` the failure route of every describe it answered.
fn fold(
    expr: &mut SExpression,
    knobs: &LiteralKnobs,
    from: Option<&Path>,
    described: &mut Described,
    failures: &mut Vec<String>,
) -> CompileResult<()> {
    let failure = failure_route(expr);
    if let Some(answered) = answer(expr, knobs, from, described)? {
        *expr = answered;
        failures.extend(failure);
        return Ok(());
    }
    match expr {
        SExpression::Array(items) => items.iter_mut().try_for_each(|e| fold(e, knobs, from, described, failures)),
        SExpression::Object(map) => map.values_mut().try_for_each(|e| fold(e, knobs, from, described, failures)),
        _ => Ok(()),
    }
}

/// The failure route a `behavior/describe` declares, if `expr` is one.
fn failure_route(expr: &SExpression) -> Option<String> {
    let SExpression::Array(items) = expr else {
        return None;
    };
    if items.first().and_then(SExpression::as_str) != Some(DESCRIBE) {
        return None;
    }
    items
        .get(2)
        .and_then(SExpression::as_object)
        .and_then(|o| o.get("emit"))
        .and_then(SExpression::as_object)
        .and_then(|e| e.get("failure"))
        .and_then(SExpression::as_str)
        .map(str::to_string)
}

/// The emit a literal describe folds to; `None` for anything else.
fn answer(
    expr: &SExpression,
    knobs: &LiteralKnobs,
    from: Option<&Path>,
    described: &mut Described,
) -> CompileResult<Option<SExpression>> {
    let SExpression::Array(items) = expr else {
        return Ok(None);
    };
    if items.first().and_then(SExpression::as_str) != Some(DESCRIBE) {
        return Ok(None);
    }
    let value = match items.get(1) {
        Some(SExpression::String(name)) => knobs.get(name.as_str()),
        other => other.filter(|v| is_literal_trait_value(v)),
    };
    let Some(value) = value else {
        return Ok(None);
    };
    let success = items
        .get(2)
        .and_then(SExpression::as_object)
        .and_then(|o| o.get("emit"))
        .and_then(SExpression::as_object)
        .and_then(|e| e.get("success"))
        .and_then(SExpression::as_str);
    let describer = DESCRIBER.get().ok_or_else(|| {
        reflection_error("no trait describer is registered in this process".to_string())
    })?;
    let value_json = serde_json::to_value(value)
        .map_err(|e| reflection_error(format!("cannot read the trait value: {e}")))?;
    let description = describe_once(described, *describer, &value_json, from)?;
    let Some(success) = success else {
        // Nothing observes a describe without a success route.
        return Ok(Some(SExpression::Array(vec![SExpression::String(
            "do".into(),
        )].into())));
    };
    let description: SExpression = from_json_value(&description)
        .map_err(|e| reflection_error(format!("the description is not program data: {e}")))?;
    let mut payload = orbital_core_base::schema::SObject::new();
    payload.insert("result".into(), quote_expr(&description));
    Ok(Some(SExpression::Array(vec![
        SExpression::string("emit"),
        SExpression::string(success.to_string()),
        SExpression::object(payload),
    ].into())))
}

/// One answer per trait value and program location within a fold: a site names the same trait
/// from many pages, and each answer resolves a whole behavior.
fn describe_once(
    described: &mut Described,
    describer: TraitDescriber,
    value: &serde_json::Value,
    from: Option<&Path>,
) -> CompileResult<serde_json::Value> {
    let key = (
        value.to_string(),
        from.and_then(Path::parent).map(Path::to_path_buf),
    );
    if let Some(hit) = described.0.get(&key) {
        return Ok(hit.clone());
    }
    let description = describer(value, from)
        .map_err(|e| reflection_error(format!("cannot describe {value} at build time: {e}")))?;
    described.0.insert(key, description.clone());
    Ok(description)
}

/// `{ behavior, trait, … }` with no binding or call anywhere inside: known at build time.
fn is_literal_trait_value(value: &SExpression) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    let named = |k: &str| map.get(k).and_then(SExpression::as_str).is_some();
    named("behavior") && named("trait") && map.values().all(is_literal)
}

fn is_literal(value: &SExpression) -> bool {
    match value {
        SExpression::String(s) => !s.starts_with('@'),
        SExpression::Array(items) => {
            items.first().and_then(SExpression::as_str).is_none() && items.iter().all(is_literal)
        }
        SExpression::Object(map) => map.values().all(is_literal),
        _ => true,
    }
}

fn reflection_error(message: String) -> CompilerError {
    CompilerError::Reflection(message)
}

#[cfg(test)]
mod tests {
    use orbital_core_base::json_value::{from_json_str, from_json_value};
    use super::*;
    use orbital_core_base::schema::quote::{decode_quote_body, quote_body_of};

    static CALLS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    fn stub(value: &serde_json::Value, _from: Option<&Path>) -> Result<serde_json::Value, String> {
        CALLS
            .lock()
            .unwrap()
            .push(value["trait"].as_str().unwrap_or_default().to_string());
        match value["trait"].as_str() {
            Some("Missing") => Err("no such trait".into()),
            _ => Ok(serde_json::json!({
                "value": value,
                "machine": { "stateMachine": { "transitions": [{ "from": "a", "to": "b", "event": "GO", "guard": ["not", "@entity.done"] }] } }
            })),
        }
    }

    fn program(effects: serde_json::Value) -> OrbitalSchema {
        register_trait_describer(stub);
        from_json_value(&serde_json::json!({
            "name": "p",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "fields": [] },
                "traits": [{
                    "name": "Diagram",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
                    }
                }]
            }]
        }))
        .expect("program")
    }

    fn effects(schema: &OrbitalSchema) -> serde_json::Value {
        let TraitReference::Inline(td) = &schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        serde_json::to_value(&td.state_machine.transitions[0].effects).expect("effects")
    }

    const ROUTE: &str = r#"{ "emit": { "success": "DESCRIBED", "failure": "DESCRIBE_FAILED" } }"#;

    fn describe(arg: serde_json::Value) -> serde_json::Value {
        serde_json::json!([
            "behavior/describe",
            arg,
            from_json_str::<serde_json::Value>(ROUTE).unwrap()
        ])
    }

    #[test]
    fn a_literal_describe_becomes_the_emit_its_success_route_delivers() {
        let lit = serde_json::json!({ "behavior": "almadar-behaviors/std-checkout", "trait": "CheckoutRules" });
        let mut schema = program(serde_json::json!([describe(lit.clone())]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let out = effects(&schema);
        assert_eq!(out[0][0], "emit");
        assert_eq!(out[0][1], "DESCRIBED");
        let quoted: SExpression = from_json_value(&out[0][2]["result"]).unwrap();
        let body = quote_body_of(&quoted).expect("a quote");
        assert!(!body.contains('@'), "bindings inside the answer stay data");
        let answer = serde_json::to_value(decode_quote_body(body).unwrap()).unwrap();
        assert_eq!(answer["value"], lit);
        assert_eq!(
            answer["machine"]["stateMachine"]["transitions"][0]["guard"],
            serde_json::json!(["not", "@entity.done"])
        );
    }

    /// A trait that describes on INIT and declares both routes, like std-rasm-source; `failed` is an extra arm's effects.
    fn routed(failure_scope: &str, extra: serde_json::Value) -> OrbitalSchema {
        register_trait_describer(stub);
        let lit = serde_json::json!({ "behavior": "std/std-door", "trait": "DoorLife" });
        from_json_value(&serde_json::json!({
            "name": "p",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "fields": [] },
                "traits": [{
                    "name": "Source",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [
                            { "from": "idle", "to": "idle", "event": "INIT", "effects": [describe(lit)] },
                            { "from": "idle", "to": "idle", "event": "DESCRIBED", "effects": [] },
                            { "from": "idle", "to": "idle", "event": "DESCRIBE_FAILED", "effects": [] },
                            { "from": "idle", "to": "idle", "event": "RETRY", "effects": extra }
                        ]
                    },
                    "emits": [
                        { "event": "DESCRIBED", "scope": "internal" },
                        { "event": "DESCRIBE_FAILED", "scope": failure_scope }
                    ],
                    "listens": [{ "event": "DESCRIBE_FAILED" }]
                }]
            }]
        }))
        .expect("program")
    }

    fn declared(schema: &OrbitalSchema) -> (Vec<String>, Vec<String>, Vec<String>) {
        let TraitReference::Inline(td) = &schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        (
            td.emits.iter().flat_map(|e| e.iter()).map(|e| e.event.clone()).collect(),
            td.listens.iter().flat_map(|l| l.iter()).map(|l| l.event.clone()).collect(),
            td.state_machine.transitions.iter().map(|t| t.event.clone()).collect(),
        )
    }

    #[test]
    fn a_folded_describe_drops_the_internal_failure_route_nothing_else_fires() {
        let mut schema = routed("internal", serde_json::json!([]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let (emits, listens, transitions) = declared(&schema);
        assert_eq!(emits, vec!["DESCRIBED"], "the failure can no longer happen");
        assert!(listens.is_empty(), "{listens:?}");
        assert_eq!(transitions, vec!["INIT", "DESCRIBED", "RETRY"]);
    }

    #[test]
    fn control_a_failure_route_another_step_still_fires_stays() {
        let mut schema = routed("internal", serde_json::json!([["emit", "DESCRIBE_FAILED", { "error": "again" }]]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let (emits, _, transitions) = declared(&schema);
        assert_eq!(emits, vec!["DESCRIBED", "DESCRIBE_FAILED"]);
        assert!(transitions.contains(&"DESCRIBE_FAILED".to_string()));
    }

    #[test]
    fn edge_an_external_failure_route_stays_for_the_traits_that_hear_it() {
        let mut schema = routed("external", serde_json::json!([]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let (emits, _, _) = declared(&schema);
        assert_eq!(emits, vec!["DESCRIBED", "DESCRIBE_FAILED"]);
    }

    #[test]
    fn a_describe_nested_in_a_control_form_is_folded_too() {
        let lit = serde_json::json!({ "behavior": "std/std-door", "trait": "DoorLife" });
        let mut schema = program(serde_json::json!([[
            "when",
            "@entity.ready",
            ["do", describe(lit)]
        ]]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let out = effects(&schema);
        assert_eq!(out[0][2][1][0], "emit");
    }

    #[test]
    fn control_a_describe_of_a_value_known_only_at_runtime_is_left_for_the_host() {
        let mut schema = program(serde_json::json!([
            describe(serde_json::json!("@entity.picked")),
            describe(serde_json::json!({ "behavior": "@entity.behavior", "trait": "T" })),
            ["behavior/source", { "behavior": "std/std-door", "trait": "DoorLife" }, { "emit": { "success": "SOURCED" } }]
        ]));
        let before = effects(&schema);
        fold_literal_reflection(&mut schema, None).expect("fold");
        assert_eq!(effects(&schema), before);
    }

    fn program_with_subject(default: Option<serde_json::Value>) -> OrbitalSchema {
        let mut schema = program(serde_json::json!([describe(serde_json::json!("@config.subject"))]));
        let mut field = serde_json::json!({ "type": "TraitRef" });
        if let Some(d) = default {
            field["default"] = d;
        }
        let TraitReference::Inline(td) = &mut schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        let mut config = orbital_core_base::collections::IndexMap::default();
        config.insert("subject".to_string(), from_json_value(&field).expect("config field"));
        Arc::make_mut(td).config = Some(config);
        schema
    }

    #[test]
    fn a_describe_of_a_knob_whose_default_is_a_literal_trait_folds_like_the_literal() {
        let lit = serde_json::json!({ "behavior": "almadar-behaviors/std-checkout", "trait": "CheckoutRules" });
        let mut schema = program_with_subject(Some(lit.clone()));
        fold_literal_reflection(&mut schema, None).expect("fold");
        let out = effects(&schema);
        assert_eq!(out[0][0], "emit");
        let quoted: SExpression = from_json_value(&out[0][2]["result"]).unwrap();
        let answer = serde_json::to_value(decode_quote_body(quote_body_of(&quoted).unwrap()).unwrap()).unwrap();
        assert_eq!(answer["value"], lit);
    }

    #[test]
    fn control_a_knob_with_no_literal_default_is_left_for_the_host() {
        for default in [None, Some(serde_json::json!({ "behavior": "@entity.behavior", "trait": "T" }))] {
            let mut schema = program_with_subject(default);
            let before = effects(&schema);
            fold_literal_reflection(&mut schema, None).expect("fold");
            assert_eq!(effects(&schema), before);
        }
    }

    #[test]
    fn a_describe_nothing_observes_folds_to_nothing() {
        let mut schema = program(serde_json::json!([
            ["behavior/describe", { "behavior": "std/std-door", "trait": "DoorLife" }, { "emit": { "failure": "F" } }]
        ]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        assert_eq!(effects(&schema), serde_json::json!([["do"]]));
    }

    #[test]
    fn a_literal_that_cannot_be_described_fails_the_build() {
        let mut schema = program(serde_json::json!([describe(
            serde_json::json!({ "behavior": "std/std-door", "trait": "Missing" })
        )]));
        let err = fold_literal_reflection(&mut schema, None).expect_err("must fail");
        assert!(err.to_string().contains("no such trait"), "{err}");
    }

    fn calls_for(name: &str) -> usize {
        CALLS.lock().unwrap().iter().filter(|t| *t == name).count()
    }

    #[test]
    fn the_same_literal_is_described_once_within_one_fold() {
        let lit = serde_json::json!({ "behavior": "std/std-door", "trait": "DescribedOnce" });
        let mut schema = program(serde_json::json!([
            describe(lit.clone()),
            ["when", true, describe(lit)]
        ]));
        fold_literal_reflection(&mut schema, None).expect("fold");
        assert_eq!(calls_for("DescribedOnce"), 1);
        let out = effects(&schema);
        assert_eq!(out[0], out[1][2], "every occurrence gets the same answer");
    }

    #[test]
    fn control_a_second_fold_describes_again_because_nothing_outlives_the_first() {
        let lit = serde_json::json!({ "behavior": "std/std-door", "trait": "DescribedPerFold" });
        let mut a = program(serde_json::json!([describe(lit.clone())]));
        let mut b = program(serde_json::json!([describe(lit)]));
        fold_literal_reflection(&mut a, None).expect("fold");
        fold_literal_reflection(&mut b, None).expect("fold");
        assert_eq!(calls_for("DescribedPerFold"), 2);
        assert_eq!(effects(&a)[0], effects(&b)[0]);
    }

    #[test]
    fn a_program_and_its_lazy_behaviors_share_one_fold() {
        let lit = serde_json::json!({ "behavior": "std/std-door", "trait": "DescribedAcrossLazy" });
        let mut root = program(serde_json::json!([describe(lit.clone())]));
        let mut lazy = vec![super::super::lazy::ResolvedLazy {
            orbital: "Child".to_string(),
            orb_ref: "child".to_string(),
            schema: program(serde_json::json!([describe(lit)])),
            lazy: Vec::new(),
            cache_key: None,
        }];
        fold_program(&mut root, &mut lazy, None).expect("fold");
        assert_eq!(calls_for("DescribedAcrossLazy"), 1);
    }
}
