//! `postgres` effect validation: `(postgres <origin> <sql> <params> { auth?, user, database?, tls?, timeoutMs? }? { key?, emit? }?)`.
//! The origin must be a `postgres://` or `postgresql://` origin the program's `egress` lists, a literal statement must
//! pass the read-only guard (the kernel checks every statement again at run time), `params` is a list, and the trailing
//! objects share one namespace of closed keys. The credential slot `auth` is a `secret`-typed knob, which
//! `orbital_core::secrets` checks on the unsubstituted program.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::{each_call_of, visit_effects};
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config};
use orbital_core_base::http_wire::{parse_egress_entry, split_tcp_url, OriginParts};
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

fn egress_lists(egress: Option<&[String]>, origin: &OriginParts) -> bool {
    egress.into_iter().flatten().filter_map(|entry| parse_egress_entry(entry)).any(|entry| &entry == origin)
}

fn not_static(at: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::HttpOriginNotStatic,
        at,
        "`postgres` origin must be a string literal or a `url`-typed @config knob (egress cannot be checked otherwise)",
    )
    .with_suggestion("Declare `origin: url = \"postgres://host:5432\"` in config and pass @config.origin")
}

fn check_origin(
    value: &SExpression,
    egress: Option<&[String]>,
    config: Option<&HashMap<String, ConfigField>>,
    at: &str,
    result: &mut ValidationResult,
) {
    let from_data = egress.is_some_and(|egress| !egress.is_empty());
    let SExpression::String(text) = value else {
        if !from_data {
            result.add_error(not_static(at));
        }
        return;
    };
    let literal = if let Some(knob) = config_knob(text) {
        match config.and_then(|c| c.get(knob)) {
            Some(field) if field.field_type == "url" => match &field.default {
                Some(SExpression::String(default)) if !is_dynamic(default) => default.clone(),
                _ => return,
            },
            _ => {
                if !from_data {
                    result.add_error(not_static(at));
                }
                return;
            }
        }
    } else if is_dynamic(text) {
        if !from_data {
            result.add_error(not_static(at));
        }
        return;
    } else {
        text.clone()
    };
    match split_tcp_url(&literal) {
        Some((origin, "")) if matches!(origin.scheme(), "postgres" | "postgresql") => {
            if !egress_lists(egress, &origin) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::HttpEgressUndeclared,
                        at,
                        format!("`postgres` origin '{literal}' is not declared in the program's `egress`"),
                    )
                    .with_suggestion("Add the exact postgres://host[:port] origin to the app's `egress: [...]`"),
                );
            }
        }
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
            result.add_error(
                ValidationError::error(ErrorCode::HostEffectOptionUnknown, at, "`postgres` options must be objects")
                    .with_suggestion("Write { auth: @config.password, db: 0 } and { key: <string>, emit: { success: <EVENT>, failure: <EVENT> } }"),
            );
            continue;
        };
        for name in options.keys() {
            if !OPTION_KEYS.contains(&name.as_str()) {
                result.add_error(
                    ValidationError::error(ErrorCode::HostEffectOptionUnknown, at, format!("`postgres` options accept only {}, got `{name}`", OPTION_KEYS.join(", ")))
                        .with_valid_values(OPTION_KEYS.iter().map(|k| k.to_string()).collect()),
                );
            } else if seen.contains(&name.as_str()) {
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
    schema: &OrbitalSchema,
    config: Option<&HashMap<String, ConfigField>>,
    declared: &[super::DeclaredEmit],
    at: &str,
    result: &mut ValidationResult,
) {
    if !(4..=6).contains(&arr.len()) {
        result.add_error(
            ValidationError::error(
                ErrorCode::HostEffectArity,
                at,
                format!("`postgres` takes an origin, a statement, its params and up to two option objects, got {}", arr.len() - 1),
            )
            .with_suggestion("Write (postgres \"postgres://host:5432\" \"SELECT * FROM t WHERE id = $1\" [@payload.id] { auth: @config.password, user: \"app\" } { emit: { success: <EVENT> } })"),
        );
        return;
    }
    check_origin(&arr[1], schema.egress.as_deref(), config, at, result);
    check_statement(&arr[2], &arr[3], at, result);
    check_options(arr, declared, at, result);
}

/// Shape, egress, credential-slot and key/emit checks for every `postgres` effect.
pub(crate) fn validate_postgres_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_call_of(expr, "postgres", &mut |arr| {
                check_call(arr, schema, trait_def.config.as_ref(), &declared, at, &mut result)
            });
        });
    }
    result
}
