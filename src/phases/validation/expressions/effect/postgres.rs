//! `postgres` effect validation: `(postgres <origin> <sql> <params> { auth?, user, database?, tls?, timeoutMs? }? { key?, emit? }?)`.
//! The origin must be a `postgres://` or `postgresql://` origin (the `egress` and option-key checks are the std lint's), a literal statement must
//! pass the read-only guard (the kernel checks every statement again at run time), `params` is a list, and the trailing
//! objects share one namespace of closed keys. The credential slot `auth` is a `secret`-typed knob, which
//! `orbital_core::secrets` checks on the unsubstituted program.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::{each_call_of, visit_effects};
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core_base::http_wire::split_tcp_url;
use orbital_core_base::sql_guard::assert_read_only_select;
use orbital_core_base::schema::{ConfigField, OrbitalSchema, SExpression, SObject};
use orbital_core_base::secrets::config_knob;
use std::collections::HashMap;

const OPTION_KEYS: [&str; 7] = ["auth", "user", "database", "tls", "timeoutMs", "key", "emit"];
const EMIT_KEYS: [&str; 2] = ["success", "failure"];

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

fn invalid(at: &str, message: impl Into<String>) -> ValidationError {
    ValidationError::error(ErrorCode::PostgresCallInvalid, at, message)
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
        Some((origin, "")) if matches!(origin.scheme(), "postgres" | "postgresql") => {}
        _ => result.add_error(
            invalid(at, format!("`postgres` origin '{literal}' must be a postgres:// or postgresql:// origin without a path"))
                .with_suggestion("Write postgres://host[:port]"),
        ),
    }
}

fn check_statement(sql: &SExpression, params: &SExpression, at: &str, result: &mut ValidationResult) {
    if let SExpression::String(text) = sql {
        if !text.starts_with('@') && !text.starts_with('?') {
            let guard = assert_read_only_select(text);
            if !guard.ok {
                result.add_error(
                    invalid(at, format!("`postgres` statement is not read-only: {}", guard.reason.as_deref().unwrap_or("not a SELECT statement")))
                        .with_suggestion("The postgres effect runs one SELECT (or WITH ... SELECT) and nothing that writes"),
                );
            }
        }
    }
    if matches!(params, SExpression::String(_) | SExpression::Number(_) | SExpression::Bool(_) | SExpression::Object(_)) && !matches!(params, SExpression::String(s) if s.starts_with('@') || s.starts_with('?')) {
        result.add_error(invalid(at, "`postgres` params must be a list of bind values").with_suggestion("Write [] when the statement has no $1 placeholders"));
    }
}

fn check_options(arr: &[SExpression], declared: &[super::DeclaredEmit], at: &str, result: &mut ValidationResult) {
    let mut seen: Vec<&str> = Vec::new();
    let mut has_user = false;
    for trailing in &arr[4.min(arr.len())..] {
        let SExpression::Object(options) = trailing else {
            continue;
        };
        for name in options.keys() {
            if !OPTION_KEYS.contains(&name.as_str()) {
                continue;
            }
            if seen.contains(&name.as_str()) {
                result.add_error(invalid(at, format!("`postgres` option `{name}` is given twice")));
            } else {
                seen.push(name);
            }
        }
        has_user |= options.get("user").is_some_and(|v| !matches!(v, SExpression::Null));
        check_scalars(options, at, result);
        if options.contains_key("key") || options.contains_key("emit") {
            result.merge(check_key_and_cancelled(options, at, "postgres"));
        }
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &EMIT_KEYS, Some(declared), at));
        }
    }
    if !has_user {
        result.add_error(invalid(at, "`postgres` needs the `user` option: the role to log in as"));
    }
}

fn check_scalars(options: &SObject, at: &str, result: &mut ValidationResult) {
    if matches!(options.get("tls"), Some(SExpression::Number(_) | SExpression::Object(_))) || matches!(options.get("tls"), Some(SExpression::String(s)) if !s.starts_with('@') && !s.starts_with('?')) {
        result.add_error(invalid(at, "`postgres` option `tls` must be true or false"));
    }
    if let Some(SExpression::Number(n)) = options.get("timeoutMs") {
        if n.fract() != 0.0 || *n <= 0.0 {
            result.add_error(invalid(at, format!("`postgres` option `timeoutMs` must be a whole number of milliseconds above zero, got {n}")));
        }
    }
    for name in ["user", "database"] {
        if matches!(options.get(name), Some(SExpression::String(s)) if s.is_empty()) || matches!(options.get(name), Some(SExpression::Number(_) | SExpression::Bool(_) | SExpression::Object(_))) {
            result.add_error(invalid(at, format!("`postgres` option `{name}` must be a non-empty string")));
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
    if !(4..=6).contains(&arr.len()) {
        return;
    }
    check_origin(&arr[1], config, at, result);
    check_statement(&arr[2], &arr[3], at, result);
    check_options(arr, declared, at, result);
}

/// Shape, credential-slot and key/emit checks for every `postgres` effect.
pub(crate) fn validate_postgres_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_call_of(expr, "postgres", &mut |arr| {
                check_call(arr, trait_def.config.as_ref(), &declared, at, &mut result)
            });
        });
    }
    result
}
