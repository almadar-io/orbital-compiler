//! `ORB_O_CONFIG_DEAD_KNOB` — a declared `Orbital.config` / `OrbitalSchema.config`
//! knob (`Almadar_Orbital_Import.md` §4.5) that no trait in the schema forwards.
//! A knob is "forwarded" by a trait's own declared config field whose default
//! is the exact literal `"@config.<knob>"` (no dotted tail) — the same rule
//! `forwarded_sibling_config` (`inline/trait.rs`) uses to resolve the
//! orbital/organism config rungs. A declared-unforwarded knob is dead:
//! nothing publishes it, so no importer's call-site override can ever reach
//! it. Runs post-inline so the whole (already-flattened) trait set is
//! visible in one schema.

use std::collections::HashMap;

use orbital_core::schema::{ConfigField, OrbitalSchema, SExpression, TraitReference};

use super::error::{ErrorCode, ValidationError};
use super::ValidationResult;

/// True when `field.default` is the exact forwarding literal `"@config.<knob>"`.
fn forwards_knob(field: &ConfigField, knob: &str) -> bool {
    matches!(&field.default, Some(SExpression::String(s)) if s.strip_prefix("@config.") == Some(knob))
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

fn check_knobs(
    schema: &OrbitalSchema,
    knobs: &HashMap<String, ConfigField>,
    path_of: impl Fn(&str) -> String,
    scope_desc: &str,
    result: &mut ValidationResult,
) {
    for knob in knobs.keys() {
        if !is_forwarded_anywhere(schema, knob) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalConfigDeadKnob,
                    path_of(knob),
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
}

/// Every declared `schema.config` / `orbital.config` knob must be forwarded
/// by some trait's `config[*].default == "@config.<knob>"`, checked across
/// the WHOLE schema (a trait in any orbital may be the forwarder).
pub fn validate_orbital_config_forwarding(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let Some(app_config) = schema.config.as_ref() {
        check_knobs(
            schema,
            app_config,
            |knob| format!("config({knob})"),
            "app-level",
            &mut result,
        );
    }

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let Some(orbital_config) = orbital.config.as_ref() else {
            continue;
        };
        check_knobs(
            schema,
            orbital_config,
            |knob| format!("orbitals[{} ({})].config({})", i, orbital.name, knob),
            "orbital",
            &mut result,
        );
    }

    result
}
