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

/// Knob name a `@config.<knob>` token names — `None` for anything else,
/// including a dotted `@config.<knob>.<path>` (a sibling's own forward, not
/// a declaration of `<knob>`).
fn forwarded_knob(token: &str) -> Option<&str> {
    token
        .strip_prefix("@config.")
        .filter(|knob| !knob.is_empty() && !knob.contains('.'))
}

/// Every `@config.<knob>` token a field carries: its whole `default`, any
/// token nested in it, and the provenance of forwards the inline phase
/// already folded (`forwarded_from`, `nested_forwarded_from`).
fn field_forward_tokens(field: &ConfigField) -> impl Iterator<Item = &str> {
    let mut tokens: Vec<&str> = Vec::new();
    if let Some(default) = field.default.as_ref() {
        collect_config_tokens(default, &mut tokens);
    }
    tokens.extend(field.forwarded_from.as_deref());
    tokens.extend(field.nested_forwarded_from.iter().map(String::as_str));
    tokens.into_iter()
}

fn collect_config_tokens<'a>(expr: &'a SExpression, out: &mut Vec<&'a str>) {
    match expr {
        SExpression::String(s) if s.starts_with("@config.") => out.push(s.as_str()),
        SExpression::Array(items) => items.iter().for_each(|i| collect_config_tokens(i, out)),
        SExpression::Object(map) => map.values().for_each(|v| collect_config_tokens(v, out)),
        _ => {}
    }
}

/// Every knob some trait publishes: the knobs traits forward directly, closed
/// over orbital-level knobs whose `forwarded_from` chains further out — an
/// imported orbital's knob that the consumer folded from its own app knob
/// (`forward_consumer_app_knobs`, `inline/orbital.rs`) publishes that app
/// knob through whichever trait forwards the orbital knob.
fn published_knobs(schema: &OrbitalSchema) -> HashSet<String> {
    let mut published: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            for field in td.config.iter().flat_map(|c| c.values()) {
                for knob in field_forward_tokens(field).filter_map(forwarded_knob) {
                    published.insert(knob.to_string());
                }
            }
        }
    }
    loop {
        let mut grew = false;
        for orbital in &schema.orbitals {
            for (name, field) in orbital.config.iter().flat_map(|c| c.iter()) {
                if !published.contains(name) {
                    continue;
                }
                if let Some(root) = field.forwarded_from.as_deref().and_then(forwarded_knob) {
                    if !published.contains(root) {
                        published.insert(root.to_string());
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            return published;
        }
    }
}

fn check_knob(
    published: &HashSet<String>,
    knob: &str,
    path: String,
    scope_desc: &str,
    result: &mut ValidationResult,
) {
    if !published.contains(knob) {
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
    let published = published_knobs(schema);

    let app_keys: HashSet<&str> = schema
        .config
        .iter()
        .flat_map(|c| c.keys())
        .map(|k| k.as_str())
        .collect();
    for knob in &app_keys {
        check_knob(
            &published,
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
                &published,
                knob,
                format!("orbitals[{} ({})].config({})", i, orbital.name, knob),
                "orbital",
                &mut result,
            );
        }
    }

    result
}
