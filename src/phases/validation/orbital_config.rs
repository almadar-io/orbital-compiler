//! `ORB_O_CONFIG_DEAD_KNOB` — a declared `Orbital.config` / `OrbitalSchema.config`
//! knob (`Almadar_Orbital_Import.md` §4.5) that no trait in the schema forwards.
//! A knob is "forwarded" by a trait's own declared config field whose
//! `default` OR `forwarded_from` is the exact literal `"@config.<knob>"` (no
//! dotted tail):
//!   - `default` survives as the literal when the forward was never resolved
//!     (a hand-authored `Inline` trait declaring the forward itself, or a
//!     deferred import load where `ctx.defer_config_subst` keeps the token
//!     live for the downstream consumer).
//!   - `forwarded_from` carries the SAME token when the inline phase's
//!     forward resolver (`forwarded_sibling_config_from`, `inline/trait.rs`)
//!     COLLAPSED it into its resolved value — the literal itself is gone
//!     from `default` by the time this runs, so the provenance lives in the
//!     IR field instead of a side channel (`ConfigField::forwarded_from`'s
//!     doc comment). One owner, in `.orb` itself, so both the Rust compiler
//!     and the JS twin's `lintWiring` `orbital-config-knob-unforwarded` read
//!     the same resolved-schema evidence.
//! Runs post-inline so the whole (already-flattened) trait set is visible in
//! one schema. A declared-unforwarded knob is dead: nothing publishes it, so
//! no importer's call-site override can ever reach it.

use std::collections::HashSet;

use orbital_core::schema::{ConfigField, OrbitalSchema, SExpression, TraitReference};

use super::config_type::declared_config_keys;
use super::error::{ErrorCode, ValidationError};
use super::ValidationResult;

/// True when `field.default` or `field.forwarded_from` is the exact
/// forwarding literal `"@config.<knob>"`.
fn forwards_knob(field: &ConfigField, knob: &str) -> bool {
    let target_matches = |s: &str| s.strip_prefix("@config.") == Some(knob);
    matches!(&field.default, Some(SExpression::String(s)) if target_matches(s))
        || field.forwarded_from.as_deref().is_some_and(target_matches)
}

/// True when SOME trait anywhere in the schema forwards `knob`.
fn is_forwarded_anywhere(schema: &OrbitalSchema, knob: &str) -> bool {
    schema.orbitals.iter().any(|orbital| {
        orbital.traits.iter().any(|tr| {
            let TraitReference::Inline(td) = tr else {
                return false;
            };
            td.config
                .as_ref()
                .is_some_and(|cfg| cfg.values().any(|field| forwards_knob(field, knob)))
        })
    })
}

fn check_knob(
    schema: &OrbitalSchema,
    knob: &str,
    path: String,
    scope_desc: &str,
    result: &mut ValidationResult,
) {
    if !is_forwarded_anywhere(schema, knob) {
        result.add_error(
            ValidationError::error(
                ErrorCode::OrbitalConfigDeadKnob,
                path,
                format!(
                    "Declared {scope_desc} config knob '{knob}' is not forwarded by any trait's config default (`@config.{knob}`) — nothing publishes it, so no call site can ever reach it."
                ),
            )
            .with_suggestion(format!(
                "Forward it from a trait (`config {{ {knob}: @config.{knob} }}`) or delete the declaration."
            )),
        );
    }
}

/// Every declared `schema.config` / `orbital.config` knob must be forwarded
/// by some trait's `config[*].default`/`forwarded_from` == `"@config.<knob>"`,
/// checked across the WHOLE schema (a trait in any orbital may be the
/// forwarder). Declared keys come from the shared `declared_config_keys`
/// owner (`config_type.rs`) — app-level knobs checked once, orbital-level
/// knobs per orbital.
pub fn validate_orbital_config_forwarding(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let app_keys: HashSet<&str> = schema
        .config
        .iter()
        .flat_map(|c| c.keys())
        .map(|k| k.as_str())
        .collect();
    for knob in &app_keys {
        check_knob(
            schema,
            knob,
            format!("config({knob})"),
            "app-level",
            &mut result,
        );
    }

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for knob in declared_config_keys(schema, orbital)
            .into_iter()
            .filter(|k| !app_keys.contains(k))
        {
            check_knob(
                schema,
                knob,
                format!("orbitals[{} ({})].config({})", i, orbital.name, knob),
                "orbital",
                &mut result,
            );
        }
    }

    result
}
