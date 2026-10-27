//! `redis` effect validation: `(redis <origin> <command> { auth?, user?, db? }? { key?, emit? }?)`. The origin must
//! be a `redis://` or `rediss://` origin (the `egress` and option-key checks are the std lint's), the command a non-empty list of text, and the
//! trailing objects share one namespace of closed keys. The credential slot `auth` is a `secret`-typed knob, which
//! `orbital_core::secrets` checks on the unsubstituted program.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::{each_call_of, visit_effects};
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core_base::http_wire::split_tcp_url;
use orbital_core_base::schema::{ConfigField, OrbitalSchema, SExpression, SObject};
use orbital_core_base::secrets::config_knob;
use std::collections::HashMap;

const OPTION_KEYS: [&str; 5] = ["auth", "user", "db", "key", "emit"];
const EMIT_KEYS: [&str; 2] = ["success", "failure"];

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

fn invalid(at: &str, message: impl Into<String>) -> ValidationError {
    ValidationError::error(ErrorCode::RedisCallInvalid, at, message)
}

fn check_origin(
    value: &SExpression,
    config: Option<&HashMap<String, ConfigField>>,
    at: &str,
    result: &mut ValidationResult,
) {
    let SExpression::String(text) = value else {
        return;
    };
    let literal = if let Some(knob) = config_knob(text) {
        match config.and_then(|c| c.get(knob)) {
            Some(field) if field.field_type == "url" => match &field.default {
                Some(SExpression::String(default)) if !is_dynamic(default) => default.clone(),
                _ => return,
            },
            _ => return,
        }
    } else if is_dynamic(text) {
        return;
    } else {
        text.clone()
    };
    match split_tcp_url(&literal) {
        Some((origin, "")) if matches!(origin.scheme(), "redis" | "rediss") => {}
        _ => result.add_error(
            invalid(at, format!("`redis` origin '{literal}' must be a redis:// or rediss:// origin without a path"))
                .with_suggestion("Write redis://host[:port] or rediss://host[:port]"),
        ),
    }
}

fn check_command(value: &SExpression, at: &str, result: &mut ValidationResult) {
    let SExpression::Array(items) = value else {
        return;
    };
    if matches!(items.first(), Some(SExpression::String(head)) if !is_dynamic(head) && head.is_empty()) || items.is_empty() {
        result.add_error(invalid(at, "`redis` command must start with a command name"));
        return;
    }
    for (index, item) in items.iter().enumerate() {
        if matches!(item, SExpression::Number(_) | SExpression::Bool(_) | SExpression::Null | SExpression::Object(_)) {
            result.add_error(
                invalid(at, format!("`redis` command item {} must be text, not a {}", index + 1, literal_name(item)))
                    .with_suggestion("Convert a number with a string operator, e.g. (str/from-number @payload.ttl)"),
            );
        }
    }
}

fn literal_name(value: &SExpression) -> &'static str {
    match value {
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        SExpression::Null => "null",
        SExpression::Object(_) => "object",
        _ => "value",
    }
}

fn check_options(arr: &[SExpression], declared: &[super::DeclaredEmit], at: &str, result: &mut ValidationResult) {
    let mut seen: Vec<&str> = Vec::new();
    let mut has_auth = false;
    let mut has_user = false;
    for trailing in &arr[3.min(arr.len())..] {
        let SExpression::Object(options) = trailing else {
            continue;
        };
        for name in options.keys() {
            if !OPTION_KEYS.contains(&name.as_str()) {
                continue;
            }
            if seen.contains(&name.as_str()) {
                result.add_error(invalid(at, format!("`redis` option `{name}` is given twice")));
            } else {
                seen.push(name);
            }
        }
        has_auth |= options.get("auth").is_some_and(|v| !matches!(v, SExpression::Null));
        has_user |= options.get("user").is_some_and(|v| !matches!(v, SExpression::Null));
        check_db(options, at, result);
        if options.contains_key("key") || options.contains_key("emit") {
            result.merge(check_key_and_cancelled(options, at, "redis"));
        }
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &EMIT_KEYS, Some(declared), at));
        }
    }
    if has_user && !has_auth {
        result.add_error(invalid(at, "`redis` option `user` needs `auth`: an ACL user logs in with a password"));
    }
}

fn check_db(options: &SObject, at: &str, result: &mut ValidationResult) {
    if let Some(SExpression::Number(n)) = options.get("db") {
        if n.fract() != 0.0 || *n < 0.0 || *n > f64::from(u32::MAX) {
            result.add_error(invalid(at, format!("`redis` option `db` must be a whole database number, got {n}")));
        }
    }
}

fn check_call(
    arr: &[SExpression],
    config: Option<&HashMap<String, ConfigField>>,
    declared: &[super::DeclaredEmit],
    at: &str,
    result: &mut ValidationResult,
) {
    if !(3..=5).contains(&arr.len()) {
        return;
    }
    check_origin(&arr[1], config, at, result);
    check_command(&arr[2], at, result);
    check_options(arr, declared, at, result);
}

/// Shape, credential-slot and key/emit checks for every `redis` effect.
pub(crate) fn validate_redis_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_call_of(expr, "redis", &mut |arr| {
                check_call(arr, trait_def.config.as_ref(), &declared, at, &mut result)
            });
        });
    }
    result
}
