//! `ORB_HTTP_AUTH_ON_CLIENT`: an `http` call with an `auth` key is server-only
//! (`orbital_core::effects::http_auth`), so it must not sit where its trait runs
//! on the client — a `local` trait, or a tick the browser runs. A keyless `http`
//! runs there (`docs/Almadar_Embedded_Kernel.md` H.7, P12). The generic
//! `ORB_T_LOCAL_TRAIT_TOUCHES_SERVER` / `ORB_T_CLIENT_TICK_SERVER_EFFECT` also fire
//! for the same call; this code names the actual cause.
//!
//! A keyless `http` is client work (its leg runs in the browser store), so a transition or tick that
//! also carries server work has no single executor: `ORB_X_MIXED_RESIDENCE`, like a transition that
//! reads a browser-stored and a server-stored entity.

use super::super::{inline_traits, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::effects::{http_auth, sexpr_runs_server_effect, HttpAuth, Residence};
use orbital_core::runtime::dispatch_mode::DispatchMode;
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition};

fn keyed_http_calls<'a>(expr: &'a SExpression, out: &mut Vec<&'a [SExpression]>) {
    match expr {
        SExpression::Array(arr) => {
            if matches!(arr.first(), Some(SExpression::String(h)) if h == "http")
                && http_auth(&arr[1..]) == HttpAuth::Keyed
            {
                out.push(arr);
            }
            arr.iter().for_each(|item| keyed_http_calls(item, out));
        }
        SExpression::Object(obj) => obj.values().for_each(|v| keyed_http_calls(v, out)),
        _ => {}
    }
}

fn has_keyless_http(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) => {
            (matches!(arr.first(), Some(SExpression::String(h)) if h == "http")
                && http_auth(&arr[1..]) == HttpAuth::Keyless)
                || arr.iter().any(has_keyless_http)
        }
        SExpression::Object(obj) => obj.values().any(has_keyless_http),
        _ => false,
    }
}

fn report_mixed(
    result: &mut ValidationResult,
    trait_def: &TraitDefinition,
    at: String,
    effects: &[&SExpression],
    residence: &Residence,
    what: &str,
) {
    if effects.iter().any(|e| has_keyless_http(e))
        && effects.iter().any(|e| sexpr_runs_server_effect(e, residence))
    {
        result.add_error(
            ValidationError::error(
                ErrorCode::CrossMixedResidence,
                at,
                format!(
                    "trait {:?} {what} runs a keyless `http` (client work) and a server effect in one place — no single executor holds both",
                    trait_def.name
                ),
            )
            .with_suggestion("Split it: send the `http` here and emit an event a trait on the server listens to, or make the call authenticated so it runs on the server."),
        );
    }
}

fn report(
    result: &mut ValidationResult,
    trait_def: &TraitDefinition,
    at: String,
    expr: &SExpression,
    placement: &str,
) {
    let mut calls = Vec::new();
    keyed_http_calls(expr, &mut calls);
    for _ in calls {
        result.add_error(
            ValidationError::error(
                ErrorCode::HttpAuthOnClient,
                at.clone(),
                format!(
                    "trait {:?} {placement}, but this `http` carries `auth` — a credential never reaches the client, so only a keyless `http` may run there",
                    trait_def.name
                ),
            )
            .with_suggestion("Drop `auth`, or emit an event from here and make the authenticated call in a trait that runs on the server."),
        );
    }
}

pub(crate) fn validate_http_auth_placement(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let residence = Residence::of_entities(ctx.entity_definitions.values());
    for (trait_def, path) in inline_traits(schema) {
        let entity = trait_def
            .linked_entity
            .as_ref()
            .and_then(|name| ctx.entity_definitions.get(name));
        let ticks_run_on_client = trait_def.local
            || trait_def.dispatch_mode(entity, &residence) != DispatchMode::PersistedAwaited;
        let mut result = ValidationResult::new();
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            let effects: Vec<&SExpression> = transition.effects.iter().flatten().collect();
            report_mixed(
                &mut result,
                trait_def,
                format!("{path}.stateMachine.transitions[{i}].effects"),
                &effects,
                &residence,
                &format!("transition '{}'", transition.event),
            );
        }
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
            let effects: Vec<&SExpression> = tick.effects.iter().collect();
            report_mixed(
                &mut result,
                trait_def,
                format!("{path}.ticks[{i}].effects"),
                &effects,
                &residence,
                &format!("tick '{}'", tick.name),
            );
        }
        if trait_def.local {
            for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                for (j, effect) in transition.effects.iter().flatten().enumerate() {
                    report(
                        &mut result,
                        trait_def,
                        format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
                        effect,
                        "is [local], so its transitions run on the client",
                    );
                }
            }
        }
        if ticks_run_on_client {
            for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
                if tick.background {
                    continue;
                }
                for (j, effect) in tick.effects.iter().enumerate() {
                    report(
                        &mut result,
                        trait_def,
                        format!("{path}.ticks[{i}].effects[{j}]"),
                        effect,
                        &format!("runs tick '{}' in the browser", tick.name),
                    );
                }
            }
        }
        ctx.merge(result);
    }
}
