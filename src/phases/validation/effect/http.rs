//! `http` effect validation (`docs/Almadar_Embedded_Kernel.md` H.1, H.5).
//!
//! [`validate_http_effects`] — surface shape, egress, keyed-call contract. Runs
//! in the comprehensive validation (post-inline: a substituted `url` knob is a
//! plain literal origin by then, so the same rules cover both forms). The
//! secret-flow rules need the unsubstituted `@config.<knob>` reads and live in
//! `orbital_core::secrets`.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::keyed_call::{check_key_and_cancelled, trailing_options, KEYED_CALL_EMIT_KEYS};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::secrets::config_knob;
use orbital_core::schema::{ConfigField, OrbitalSchema, SExpression, SObject, TraitDefinition};
use std::collections::HashMap;

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const REQUEST_KEYS: [&str; 14] = [
    "query",
    "headers",
    "json",
    "form",
    "text",
    "bytes",
    "multipart",
    "auth",
    "timeoutMs",
    "responseAs",
    "paginate",
    "stream",
    "retry",
    "formEncoding",
];
const BODY_KEYS: [&str; 5] = ["json", "form", "text", "bytes", "multipart"];
const RETRY_KEYS: [&str; 3] = ["max", "on", "backoffMs"];
const MULTIPART_KEYS: [&str; 2] = ["kind", "parts"];
const MULTIPART_PART_KEYS: [&str; 3] = ["headers", "text", "bytes"];
const SLOT_KEYS: [&str; 2] = ["credential", "scope"];
const TOKEN_BODY_KEYS: [&str; 2] = ["json", "form"];
const STORE_AS_KEYS: [&str; 2] = ["name", "scope"];
const PAGINATE_KEYS: [&str; 5] = ["style", "param", "next", "collect", "max"];
const AUTH_SCHEMES: [&str; 6] = [
    "bearer",
    "api-key",
    "basic",
    "oauth2-client",
    "token-exchange",
    "hmac-sha256",
];

fn auth_keys(scheme: &str) -> &'static [&'static str] {
    match scheme {
        "bearer" => &["scheme", "credential"],
        "api-key" => &["scheme", "credential", "name", "in"],
        "basic" => &["scheme", "user", "credential"],
        "oauth2-client" => &["scheme", "tokenUrl", "clientId", "clientSecret", "scope"],
        "token-exchange" => &[
            "scheme",
            "tokenUrl",
            "grant",
            "body",
            "clientAuth",
            "clientId",
            "clientSecret",
            "accessField",
            "expiresField",
            "expiresMarginMs",
            "storeAs",
        ],
        "hmac-sha256" => &["scheme", "credential", "name", "signs", "encoding"],
        _ => &[],
    }
}

type Config<'a> = Option<&'a HashMap<String, ConfigField>>;

#[derive(Debug, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: Option<u16>,
}

fn parse_origin(s: &str) -> Option<Origin> {
    let (scheme, rest) = s.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return None;
    }
    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let (inside, after) = bracketed.split_once(']')?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (format!("[{}]", inside.to_ascii_lowercase()), port)
    } else {
        match rest.split_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
            None => (rest.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() || host == "[]" {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().ok()?),
    };
    let default = if scheme == "https" { 443 } else { 80 };
    Some(Origin {
        scheme,
        host,
        port: port.filter(|p| *p != default),
    })
}

/// The origin part of a full URL (`scheme://host[:port]/…`).
fn origin_of_url(s: &str) -> Option<Origin> {
    let (scheme, rest) = s.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    parse_origin(&format!("{scheme}://{}", &rest[..end]))
}

/// An `http` origin: `scheme://host[:port]` plus an optional path prefix (never a query or fragment).
fn origin_with_prefix(s: &str) -> Option<Origin> {
    let (_, rest) = s.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[authority_end..].contains(['?', '#', '\\']) {
        return None;
    }
    origin_of_url(s)
}

fn egress_allows(egress: Option<&[String]>, origin: &Origin) -> bool {
    egress
        .into_iter()
        .flatten()
        .filter_map(|e| parse_origin(e))
        .any(|e| &e == origin)
}

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

struct Site<'a> {
    path: &'a str,
    egress: Option<&'a [String]>,
    config: Config<'a>,
    declared: &'a [DeclaredEmit],
}

fn undeclared(site: &Site, what: &str, value: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::HttpEgressUndeclared,
        site.path,
        format!("{what} '{value}' is not declared in the program's `egress`"),
    )
    .with_suggestion("Add the exact scheme://host[:port] origin to the app's `egress: [...]`")
}

/// A URL-valued position (`origin`, `tokenUrl`) checked against `egress`.
/// `whole_url` = the value is a full URL whose origin part counts (tokenUrl).
fn check_url_position(
    value: &SExpression,
    whole_url: bool,
    what: &str,
    site: &Site,
    result: &mut ValidationResult,
) {
    let SExpression::String(s) = value else {
        result.add_error(not_static(site.path, what));
        return;
    };
    let literal = if let Some(knob) = config_knob(s) {
        let field = site.config.and_then(|c| c.get(knob));
        match field {
            Some(f) if f.field_type == "url" => match &f.default {
                Some(SExpression::String(d)) if !is_dynamic(d) => d.clone(),
                _ => return,
            },
            _ => {
                result.add_error(not_static(site.path, what));
                return;
            }
        }
    } else if is_dynamic(s) {
        result.add_error(not_static(site.path, what));
        return;
    } else {
        s.clone()
    };
    let origin = if whole_url {
        origin_of_url(&literal)
    } else {
        origin_with_prefix(&literal)
    };
    match origin {
        Some(o) if egress_allows(site.egress, &o) => {}
        _ => result.add_error(undeclared(site, what, &literal)),
    }
}

fn not_static(path: &str, what: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::HttpOriginNotStatic,
        path,
        format!("`http` {what} must be a string literal or a `url`-typed @config knob (egress cannot be checked otherwise)"),
    )
    .with_suggestion("Declare `origin: url = \"https://host\"` in config and pass @config.origin")
}

fn unknown_key(site: &Site, owner: &str, key: &str, allowed: &[&str]) -> ValidationError {
    ValidationError::error(
        ErrorCode::HttpOptionUnknown,
        site.path,
        format!("`{key}` is not a key of http {owner}"),
    )
    .with_valid_values(allowed.iter().map(|s| s.to_string()).collect())
    .with_suggestion("Use only the keys the `http` effect declares")
}

fn check_closed(
    obj: &SObject,
    allowed: &[&str],
    owner: &str,
    site: &Site,
    result: &mut ValidationResult,
) {
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        if !allowed.contains(&key.as_str()) {
            result.add_error(unknown_key(site, owner, key, allowed));
        }
    }
}

fn present(obj: &SObject, key: &str) -> bool {
    !matches!(obj.get(key), None | Some(SExpression::Null))
}

fn check_auth(auth: &SObject, site: &Site, result: &mut ValidationResult) {
    let scheme = match auth.get("scheme") {
        Some(SExpression::String(s)) => Some(s.as_str()),
        _ => None,
    };
    let Some(scheme) = scheme.filter(|s| AUTH_SCHEMES.contains(s)) else {
        result.add_error(
            ValidationError::error(
                ErrorCode::HttpAuthSchemeUnknown,
                site.path,
                "`http` auth.scheme must be one of the closed scheme set",
            )
            .with_valid_values(AUTH_SCHEMES.iter().map(|s| s.to_string()).collect())
            .with_suggestion("Use bearer, api-key, basic, oauth2-client, token-exchange or hmac-sha256"),
        );
        return;
    };
    check_closed(auth, auth_keys(scheme), "auth", site, result);
    if scheme == "token-exchange" {
        check_token_exchange(auth, site, result);
    }
    if scheme == "oauth2-client" || scheme == "token-exchange" {
        if let Some(token_url) = auth.get("tokenUrl") {
            check_url_position(token_url, true, "tokenUrl origin", site, result);
        }
    }
}

fn check_token_exchange(auth: &SObject, site: &Site, result: &mut ValidationResult) {
    if let Some(SExpression::Object(body)) = auth.get("body") {
        check_closed(body, &TOKEN_BODY_KEYS, "auth.body", site, result);
        for fields in body.values() {
            let SExpression::Object(fields) = fields else { continue };
            for field in fields.values() {
                if let SExpression::Object(slot) = field {
                    if slot.contains_key("credential") {
                        check_closed(slot, &SLOT_KEYS, "auth.body credential slot", site, result);
                    }
                }
            }
        }
    }
    if let Some(SExpression::Object(store_as)) = auth.get("storeAs") {
        check_closed(store_as, &STORE_AS_KEYS, "auth.storeAs", site, result);
    }
}

fn check_request(method: Option<&str>, request: &SObject, site: &Site, result: &mut ValidationResult) {
    check_closed(request, &REQUEST_KEYS, "request", site, result);

    let bodies = BODY_KEYS.iter().filter(|k| present(request, k)).count();
    if bodies > 1 || (bodies == 1 && method == Some("GET")) {
        let reason = if bodies > 1 {
            "more than one of json/form/text/bytes/multipart"
        } else {
            "a body on GET"
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::HttpBodyConflict,
                site.path,
                format!("`http` request has {reason}"),
            )
            .with_suggestion("Send at most one body kind, and none on GET"),
        );
    }

    if let Some(SExpression::Object(auth)) = request.get("auth") {
        check_auth(auth, site, result);
    }
    if let Some(SExpression::Object(retry)) = request.get("retry") {
        check_closed(retry, &RETRY_KEYS, "retry", site, result);
    }
    if let Some(SExpression::Object(multipart)) = request.get("multipart") {
        check_closed(multipart, &MULTIPART_KEYS, "multipart", site, result);
        if let Some(SExpression::Array(parts)) = multipart.get("parts") {
            for part in parts {
                if let SExpression::Object(part) = part {
                    check_closed(part, &MULTIPART_PART_KEYS, "multipart part", site, result);
                }
            }
        }
    }
    if let Some(SExpression::Object(paginate)) = request.get("paginate") {
        check_closed(paginate, &PAGINATE_KEYS, "paginate", site, result);
        if !present(paginate, "max") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::HttpPaginateMaxMissing,
                    site.path,
                    "`http` paginate requires `max`",
                )
                .with_suggestion("Bound the walk with `max: <number>`"),
            );
        }
    }
}

fn check_http_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let method = match arr.get(1) {
        Some(SExpression::String(m)) if METHODS.contains(&m.as_str()) => Some(m.as_str()),
        _ => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::HttpMethodUnknown,
                    site.path,
                    "`http` method must be one of GET POST PUT PATCH DELETE",
                )
                .with_valid_values(METHODS.iter().map(|s| s.to_string()).collect()),
            );
            None
        }
    };
    if let Some(origin) = arr.get(2) {
        check_url_position(origin, false, "origin", site, result);
    }

    let options = trailing_options(arr).or_else(|| match (arr.len(), arr.last()) {
        (6, Some(SExpression::Object(o))) => Some(o),
        _ => None,
    });
    let request = match (arr.len(), options.is_some()) {
        (6, _) | (5, false) => match arr.get(4) {
            Some(SExpression::Object(r)) => Some(r),
            _ => None,
        },
        _ => None,
    };
    if let Some(request) = request {
        check_request(method, request, site, result);
    }

    if let Some(options) = options {
        result.merge(check_key_and_cancelled(options, site.path, "http"));
        let emit = extract_emit_object_ref(options);
        if let Some(emit) = emit {
            result.merge(validate_emit_config(
                emit,
                &KEYED_CALL_EMIT_KEYS,
                Some(site.declared),
                site.path,
            ));
        }
        let streams = request.is_some_and(|r| present(r, "stream"));
        let has_on_message = emit.is_some_and(|e| present(e, "onMessage") || present(e, "on_message"));
        if streams && !has_on_message {
            result.add_error(stream_without_on_message(site.path));
        }
    } else if request.is_some_and(|r| present(r, "stream")) {
        result.add_error(stream_without_on_message(site.path));
    }
}

fn stream_without_on_message(path: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::HttpStreamWithoutOnMessage,
        path,
        "`http` stream requires `emit.onMessage`",
    )
    .with_suggestion("Add `emit: { onMessage: <EVENT> }` to the call options")
}

pub(super) fn visit_effects(trait_def: &TraitDefinition, path: &str, visit: &mut dyn FnMut(&SExpression, &str)) {
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(guard) = &transition.guard {
            visit(guard, &format!("{path}.stateMachine.transitions[{i}].guard"));
        }
        for (j, effect) in transition.effects.iter().flatten().enumerate() {
            visit(
                effect,
                &format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
            );
        }
    }
    for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
        if let Some(guard) = &tick.guard {
            visit(guard, &format!("{path}.ticks[{i}].guard"));
        }
        for (j, effect) in tick.effects.iter().enumerate() {
            visit(effect, &format!("{path}.ticks[{i}].effects[{j}]"));
        }
    }
}

fn each_http_call<'a>(expr: &'a SExpression, visit: &mut dyn FnMut(&'a [SExpression])) {
    match expr {
        SExpression::Array(arr) => {
            if matches!(arr.first(), Some(SExpression::String(h)) if h == "http") {
                visit(arr);
            }
            for item in arr {
                each_http_call(item, visit);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                each_http_call(v, visit);
            }
        }
        _ => {}
    }
}

/// Shape, egress and keyed-call checks for every `http` effect, plus the
/// program's own `egress` entries.
pub(crate) fn validate_http_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (i, entry) in schema.egress.iter().flatten().enumerate() {
        if parse_origin(entry).is_none() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::HttpEgressEntryInvalid,
                    format!("egress[{i}]"),
                    format!("egress entry '{entry}' is not scheme://host[:port]"),
                )
                .with_suggestion("Write an origin only: no path, query, credentials or trailing slash"),
            );
        }
    }
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_http_call(expr, &mut |arr| {
                let site = Site {
                    path: at,
                    egress: schema.egress.as_deref(),
                    config: trait_def.config.as_ref(),
                    declared: &declared,
                };
                check_http_call(arr, &site, &mut result);
            });
        });
    }
    result
}
