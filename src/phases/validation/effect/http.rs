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
use super::set::ExprReturnCtx;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::value::{Value, ValueMap};
use orbital_core_base::ir::{OirExpr, OirType};
use orbital_core_base::http_wire::{is_header_name, parse_grant_request, parse_http_headers, parse_origin, parse_origin_prefix, parse_delay_ms, parse_retry, parse_timeout_ms, split_url, GrantRequest, HttpRetry, OriginParts, RetryOperand};
use orbital_core_base::secrets::config_knob;
use orbital_core_base::schema::{ConfigField, OrbitalSchema, SExpression, SObject, TraitDefinition};
use std::collections::HashMap;

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const REQUEST_KEYS: [&str; 18] = [
    "query",
    "headers",
    "json",
    "form",
    "text",
    "bytes",
    "multipart",
    "auth",
    "timeoutMs",
    "delayMs",
    "responseAs",
    "paginate",
    "stream",
    "retry",
    "formEncoding",
    "queryEncoding",
    "presign",
    "webpush",
];
const BODY_KEYS: [&str; 6] = ["json", "form", "text", "bytes", "multipart", "webpush"];
const MULTIPART_KEYS: [&str; 2] = ["kind", "parts"];
const MULTIPART_PART_KEYS: [&str; 3] = ["headers", "text", "bytes"];
const SLOT_KEYS: [&str; 2] = ["credential", "scope"];
const TOKEN_BODY_KEYS: [&str; 2] = ["json", "form"];
const STORE_AS_KEYS: [&str; 2] = ["name", "scope"];
const PAGINATE_KEYS: [&str; 5] = ["style", "param", "next", "collect", "max"];
const AUTH_SCHEMES: [&str; 10] = [
    "oauth-client",
    "bearer",
    "api-key",
    "basic",
    "oauth2-client",
    "token-exchange",
    "jwt-bearer",
    "hmac-sha256",
    "aws-sigv4",
    "vapid",
];

fn auth_keys(scheme: &str) -> &'static [&'static str] {
    match scheme {
        "bearer" => &["scheme", "credential"],
        "api-key" => &["scheme", "credential", "name", "in"],
        "basic" => &["scheme", "user", "credential"],
        "oauth2-client" => &["scheme", "tokenUrl", "clientId", "clientSecret", "scope", "grantRequest", "onUnauthorized"],
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
            "grantRequest",
            "rotate",
            "onUnauthorized",
        ],
        "jwt-bearer" => &["scheme", "key", "alg", "claims", "tokenUrl", "grant", "scope", "expiresMarginMs", "storeAs", "grantRequest", "onUnauthorized"],
        "aws-sigv4" => &["scheme", "accessKeyId", "secretAccessKey", "sessionToken", "region", "service"],
        "vapid" => &["scheme", "privateKey", "publicKey", "subject"],
        "oauth-client" => &[
            "scheme", "op", "client", "clients", "code", "state", "subject", "provider", "refreshToken", "token", "accessToken",
        ],
        "hmac-sha256" => &["scheme", "credential", "name", "signs", "encoding"],
        _ => &[],
    }
}

type Config<'a> = Option<&'a HashMap<String, ConfigField>>;

fn egress_allows(egress: Option<&[String]>, origin: &OriginParts) -> bool {
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
    // D-A88: with a declared `egress` list an `origin` may come from data; the driver refuses, before sending, an
    // origin the list does not hold. A `tokenUrl` stays static.
    let origin_from_data = !whole_url && site.egress.is_some_and(|egress| !egress.is_empty());
    let SExpression::String(s) = value else {
        if !origin_from_data {
            result.add_error(not_static(site.path, what));
        }
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
                if !origin_from_data {
                    result.add_error(not_static(site.path, what));
                }
                return;
            }
        }
    } else if is_dynamic(s) {
        if !origin_from_data {
            result.add_error(not_static(site.path, what));
        }
        return;
    } else {
        s.clone()
    };
    let origin = if whole_url {
        split_url(&literal).map(|(origin, _)| origin)
    } else {
        parse_origin_prefix(&literal).map(|(origin, _)| origin)
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
            .with_suggestion("Use a declared HTTP auth scheme"),
        );
        return;
    };
    check_closed(auth, auth_keys(scheme), "auth", site, result);
    if let Some(SExpression::String(on)) = auth.get("onUnauthorized") {
        if !on.starts_with('@') && !["reply", "refresh"].contains(&on.as_str()) {
            result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, "auth.onUnauthorized must be reply or refresh"));
        }
    }
    if scheme == "aws-sigv4" {
        for key in ["accessKeyId", "secretAccessKey", "region", "service"] {
            if !present(auth, key) || matches!(auth.get(key),Some(SExpression::String(value)) if value.is_empty()) {
                result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,format!("aws-sigv4 auth.{key} is required")));
            }
        }
    }
    let header_auth = scheme == "hmac-sha256"
        || (scheme == "api-key" && matches!(auth.get("in"), Some(SExpression::String(location)) if location == "header"));
    if header_auth {
        if let Some(SExpression::String(name)) = auth.get("name") {
            if !name.starts_with('@') && !orbital_core_base::http_wire::is_header_name(name) {
                result.add_error(ValidationError::error(ErrorCode::HttpHeaderInvalid, site.path, "HTTP auth header name must be a token"));
            }
        }
    }
    if scheme == "token-exchange" || scheme == "jwt-bearer" {
        check_token_exchange(auth, site, result);
    }
    if scheme == "jwt-bearer" {
        let invalid_alg = match auth.get("alg") {
            Some(SExpression::String(alg)) => !alg.starts_with('@') && orbital_core_base::jwt_alg::JwtAlg::parse(alg).is_none(),
            Some(SExpression::Array(_)) => false,
            _ => true,
        };
        if invalid_alg {
            result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, "jwt-bearer auth.alg must be RS256 or ES256"));
        }
        let invalid_claims = match auth.get("claims") {
            Some(SExpression::Object(claims)) => claims.contains_key("iss"),
            Some(SExpression::Array(_)) => false,
            Some(SExpression::String(binding)) if binding.starts_with('@') => false,
            _ => true,
        };
        if invalid_claims {
            result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, "jwt-bearer claims must be an object without iss; issuer comes from the credential"));
        }
    }
    if matches!(scheme, "oauth2-client" | "token-exchange" | "jwt-bearer") {
        if let Some(grant_request) = auth.get("grantRequest") {
            match grant_request {
                SExpression::Object(options) => {
                    check_closed(options, GrantRequest::KEYS, "auth.grantRequest", site, result);
                    if let Some(SExpression::Object(retry)) = options.get("retry") {
                        check_closed(retry, HttpRetry::KEYS, "auth.grantRequest.retry", site, result);
                    }
                }
                _ if orbital_core_eval::lower::lower_sexpr(grant_request).is_ok_and(|expr| expr.is_literal()) => {
                    if let Err(error) = parse_grant_request(&Value::from(grant_request.clone())) {
                        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, error.message()));
                    }
                }
                _ => {}
            }
        }
        if let Some(token_url) = auth.get("tokenUrl") {
            check_url_position(token_url, true, "tokenUrl origin", site, result);
        }
    }
}

/// A credential slot as `(credential, scope)`: a bare knob string, or a `{ credential, scope? }` object.
fn slot_identity(value: &SExpression) -> Option<(&str, &str)> {
    match value {
        SExpression::String(name) => Some((name, "app")),
        SExpression::Object(slot) => {
            let name = match slot.get("credential") {
                Some(SExpression::String(name)) => name.as_str(),
                _ => return None,
            };
            match slot.get("scope") {
                None => Some((name, "app")),
                Some(SExpression::String(scope)) => Some((name, scope)),
                Some(_) => None,
            }
        }
        _ => None,
    }
}

fn rotate_invalid(site: &Site, message: &str) -> ValidationError {
    ValidationError::error(ErrorCode::HttpTokenRotateInvalid, site.path, message.to_string()).with_suggestion(
        "Use `rotate: { field: \"refresh_token\", credential: <the body.refresh_token credential> }` with `grant: refresh_token`",
    )
}

fn check_rotate(auth: &SObject, rotate: &SExpression, site: &Site, result: &mut ValidationResult) {
    let SExpression::Object(rotate) = rotate else {
        result.add_error(rotate_invalid(site, "auth.rotate must be { field, credential }"));
        return;
    };
    let mut keys: Vec<&String> = rotate.keys().collect();
    keys.sort();
    for key in keys {
        if !["field", "credential"].contains(&key.as_str()) {
            result.add_error(rotate_invalid(site, &format!("`{key}` is not a key of auth.rotate")));
        }
    }
    if !matches!(auth.get("grant"), Some(SExpression::String(grant)) if grant == "refresh_token") {
        result.add_error(rotate_invalid(site, "auth.rotate needs `grant: refresh_token`"));
    }
    if !matches!(rotate.get("field"), Some(SExpression::String(field)) if !field.is_empty()) {
        result.add_error(rotate_invalid(site, "auth.rotate.field must be a non-empty string"));
    }
    let Some(credential) = rotate.get("credential").and_then(slot_identity) else {
        result.add_error(rotate_invalid(site, "auth.rotate.credential must be a secret knob or { credential, scope? }"));
        return;
    };
    let body_slot = auth.get("body").and_then(|body| match body {
        SExpression::Object(body) => body.values().find_map(|fields| match fields {
            SExpression::Object(fields) => fields.get("refresh_token"),
            _ => None,
        }),
        _ => None,
    });
    if body_slot.and_then(slot_identity) != Some(credential) || !matches!(body_slot, Some(SExpression::Object(_))) {
        result.add_error(rotate_invalid(site, "auth.rotate.credential must be the credential of auth.body.refresh_token"));
    }
}

fn check_token_exchange(auth: &SObject, site: &Site, result: &mut ValidationResult) {
    if let Some(rotate) = auth.get("rotate") {
        check_rotate(auth, rotate, site, result);
    }
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
        let app_scope = matches!(store_as.get("scope"),None)
            || matches!(store_as.get("scope"),Some(SExpression::String(scope)) if scope == "app");
        let json = match serde_json::to_value(auth) {
            Ok(json) => json,
            Err(_) => {
                result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,"cannot inspect grant cache scope"));
                return;
            }
        };
        if app_scope && json.as_object().is_some_and(orbital_core_base::secrets::auth_uses_user_credentials) {
            result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,"a token exchange using user credentials requires a user-scoped cache"));
        }
    }
}

fn check_request(method: Option<&str>, request: &SObject, site: &Site, result: &mut ValidationResult) {
    check_closed(request, &REQUEST_KEYS, "request", site, result);
    if let Some(SExpression::String(encoding)) = request.get("queryEncoding") {
        if !encoding.starts_with('@') && !["uri", "rfc3986"].contains(&encoding.as_str()) {
            result.add_error(
                ValidationError::error(ErrorCode::HttpOptionUnknown, site.path, "request.queryEncoding must be uri or rfc3986")
                    .with_valid_values(vec!["uri".to_string(), "rfc3986".to_string()]),
            );
        }
    }

    let bodies = BODY_KEYS.iter().filter(|k| present(request, k)).count();
    if bodies > 1 || (bodies == 1 && method == Some("GET")) {
        let reason = if bodies > 1 {
            "more than one of json/form/text/bytes/multipart/webpush"
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
    if let Some(SExpression::Object(presign)) = request.get("presign") {
        check_closed(presign, &["expiresSec"], "presign", site, result);
        let valid_expiry = match presign.get("expiresSec") {
            Some(SExpression::Number(n)) => n.fract() == 0.0 && (1.0..=604800.0).contains(n),
            Some(SExpression::Array(_)) => true,
            Some(SExpression::String(value)) => value.starts_with('@'),
            _ => false,
        };
        let valid_auth = match request.get("auth") {
            Some(SExpression::Object(auth)) => matches!(auth.get("scheme"),Some(SExpression::String(scheme)) if scheme == "aws-sigv4"),
            Some(SExpression::Array(_)) => true,
            Some(SExpression::String(value)) => value.starts_with('@'),
            _ => false,
        };
        if !valid_expiry || !valid_auth || bodies > 0 || ["paginate","stream","retry"].iter().any(|key| present(request,key)) {
            result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,"presign requires aws-sigv4, expiresSec 1..604800 and no body/pagination/stream/retry"));
        }
    }
    if let Some(SExpression::Object(retry)) = request.get("retry") {
        check_closed(retry, HttpRetry::KEYS, "retry", site, result);
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

fn check_webpush(request: &SObject, call: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let Some(SExpression::Object(push)) = request.get("webpush") else { return };
    check_closed(push,&["subscription","payload","ttl","urgency"],"webpush",site,result);
    let invalid_ttl = match push.get("ttl") {
        None | Some(SExpression::Null) | Some(SExpression::Array(_)) => false,
        Some(SExpression::Number(n)) => !n.is_finite() || *n < 0.0 || n.fract() != 0.0,
        Some(SExpression::String(value)) => !value.starts_with('@'),
        _ => true,
    };
    let invalid_urgency = match push.get("urgency") {
        None | Some(SExpression::Array(_)) => false,
        Some(SExpression::String(value)) => !value.starts_with('@') && !["very-low","low","normal","high"].contains(&value.as_str()),
        _ => true,
    };
    if invalid_ttl || invalid_urgency {
        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,"webpush requires a nonnegative whole TTL and urgency very-low/low/normal/high"));
    }
    if let Some(SExpression::Object(subscription)) = push.get("subscription") {
        check_closed(subscription,&["endpoint","keys"],"webpush.subscription",site,result);
        if let Some(SExpression::Object(keys)) = subscription.get("keys") {
            check_closed(keys,&["p256dh","auth"],"webpush.subscription.keys",site,result);
        }
        if !present(request,"query") {
            if let (Some(SExpression::String(origin)),Some(SExpression::String(path)),Some(SExpression::String(endpoint))) = (call.get(2),call.get(3),subscription.get("endpoint")) {
                if ![origin,path,endpoint].iter().any(|value| value.starts_with('@')) && format!("{}{path}",origin.trim_end_matches('/')) != *endpoint {
                    result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid,site.path,"webpush subscription.endpoint must equal the declared HTTP URL"));
                }
            }
        }
    }
}

pub(super) fn http_objects(arr: &[SExpression]) -> (Option<&SObject>, Option<&SObject>) {
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
    (request, options)
}

fn operand_from_oir(expr: &OirExpr, ctx: &ExprReturnCtx<'_>) -> RetryOperand {
    match expr {
        OirExpr::Null | OirExpr::Bool { .. } | OirExpr::Number { .. } | OirExpr::String { .. } => expr
            .to_sexpression()
            .map(Value::from)
            .map(RetryOperand::Literal)
            .unwrap_or(RetryOperand::Unresolved),
        OirExpr::Array { items } => RetryOperand::Items(items.iter().map(|item| operand_from_oir(item, ctx)).collect()),
        _ => crate::type_infer::infer_oir_expr_type(expr, ctx)
            .map(RetryOperand::Typed)
            .unwrap_or(RetryOperand::Unresolved),
    }
}

fn operand(expr: &SExpression, ctx: &ExprReturnCtx<'_>) -> RetryOperand {
    crate::phases::resolve::resolve_sexpr_standalone(expr)
        .map(|expr| operand_from_oir(&expr, ctx))
        .unwrap_or(RetryOperand::Unresolved)
}

fn check_headers(expr: &SExpression, grant: bool, path: &str, ctx: &ExprReturnCtx<'_>, result: &mut ValidationResult) {
    let SExpression::Object(headers) = expr else {
        let error = match operand(expr, ctx) {
            RetryOperand::Literal(value) => parse_http_headers(Some(&value)).err().map(|error| error.message()),
            RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { fields: Vec::new() }) => Some("headers must evaluate to an object".to_string()),
            RetryOperand::Items(_) => Some("headers must be an object".to_string()),
            _ => None,
        };
        if let Some(error) = error {
            result.add_error(ValidationError::error(ErrorCode::HttpHeaderInvalid, path, error));
        }
        return;
    };
    for (name, value) in headers {
        if !is_header_name(name) {
            result.add_error(ValidationError::error(ErrorCode::HttpHeaderInvalid, path, "HTTP header name must be a token"));
            continue;
        }
        if grant && GrantRequest::is_driver_owned_header(name) {
            result.add_error(ValidationError::error(ErrorCode::HttpHeaderInvalid, path, "grant Authorization and Content-Type headers are kernel-owned"));
            continue;
        }
        let error = match operand(value, ctx) {
            RetryOperand::Literal(value) => {
                let mut header = ValueMap::new();
                header.insert(name.clone(), value);
                parse_http_headers(Some(&Value::Object(header))).err().map(|error| error.message())
            }
            RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::String) => Some(format!("HTTP header `{name}` must evaluate to a string")),
            RetryOperand::Items(_) => Some(format!("HTTP header `{name}` must evaluate to a string")),
            _ => None,
        };
        if let Some(error) = error {
            result.add_error(ValidationError::error(ErrorCode::HttpHeaderInvalid, path, error));
        }
    }
}

fn check_timeout(expr: &SExpression, path: &str, ctx: &ExprReturnCtx<'_>, result: &mut ValidationResult) {
    let error = match operand(expr, ctx) {
        RetryOperand::Literal(value) => parse_timeout_ms(Some(&value)).err().map(|error| error.message()),
        RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Number) => Some("timeoutMs must evaluate to a number".to_string()),
        RetryOperand::Items(_) => Some("timeoutMs must be a whole number".to_string()),
        _ => None,
    };
    if let Some(error) = error {
        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, path, error));
    }
}

fn check_delay(expr: &SExpression, path: &str, ctx: &ExprReturnCtx<'_>, result: &mut ValidationResult) {
    let error = match operand(expr, ctx) {
        RetryOperand::Literal(value) => parse_delay_ms(Some(&value)).err().map(|error| error.message()),
        RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Number) => Some("delayMs must evaluate to a number".to_string()),
        RetryOperand::Items(_) => Some("delayMs must be a whole number of milliseconds, zero or more".to_string()),
        _ => None,
    };
    if let Some(error) = error {
        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, path, error));
    }
}

fn check_retry(expr: &SExpression, path: &str, ctx: &ExprReturnCtx<'_>, result: &mut ValidationResult) {
    let error = match expr {
        SExpression::Object(fields) => {
            if fields.keys().any(|name| !HttpRetry::KEYS.contains(&name.as_str())) {
                return;
            }
            let fields: Vec<_> = fields.iter().map(|(name, value)| (name.clone(), operand(value, ctx))).collect();
            HttpRetry::check_partial(&fields).err().map(|error| error.message())
        }
        _ => match operand(expr, ctx) {
            RetryOperand::Literal(value) => parse_retry(&value).err().map(|error| error.message()),
            RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { fields: Vec::new() }) => Some("retry must evaluate to an object".to_string()),
            RetryOperand::Items(_) => Some("retry must be an object".to_string()),
            _ => None,
        },
    };
    if let Some(error) = error {
        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, path, error));
    }
}

fn check_request_operands(request: &SObject, grant: bool, path: &str, ctx: &ExprReturnCtx<'_>, result: &mut ValidationResult) {
    if let Some(headers) = request.get("headers") {
        check_headers(headers, grant, path, ctx, result);
    }
    if let Some(timeout) = request.get("timeoutMs") {
        check_timeout(timeout, path, ctx, result);
    }
    if !grant {
        if let Some(delay) = request.get("delayMs") {
            check_delay(delay, path, ctx, result);
        }
    }
    if let Some(retry) = request.get("retry") {
        check_retry(retry, path, ctx, result);
    }
}

pub(super) fn check_http_operand_types(arr: &[SExpression], path: &str, ctx: &ExprReturnCtx<'_>) -> ValidationResult {
    let mut result = ValidationResult::new();
    if arr.first().and_then(SExpression::as_str) != Some("http") {
        return result;
    }
    let (request, _) = http_objects(arr);
    let Some(request) = request else { return result };
    check_request_operands(request, false, path, ctx, &mut result);
    if let Some(SExpression::Object(auth)) = request.get("auth") {
        if matches!(auth.get("scheme").and_then(SExpression::as_str), Some("token-exchange" | "oauth2-client" | "jwt-bearer")) {
            if let Some(grant_request) = auth.get("grantRequest") {
                match grant_request {
                    SExpression::Object(options) => check_request_operands(options, true, path, ctx, &mut result),
                    _ => {
                        if let RetryOperand::Typed(ty) = operand(grant_request, ctx) {
                            if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { fields: Vec::new() }) {
                                result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, path, "auth.grantRequest must evaluate to an object"));
                            }
                        }
                    }
                }
            }
        }
    }
    result
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
    let oauth_endpoint = matches!(arr.get(2), Some(SExpression::String(origin)) if origin == "oauth-endpoint");
    if let Some(origin) = arr.get(2).filter(|_| !oauth_endpoint) {
        check_url_position(origin, false, "origin", site, result);
    }

    let (request, options) = http_objects(arr);
    if let Some(request) = request {
        check_request(method, request, site, result);
        check_webpush(request, arr, site, result);
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
    each_call_of(expr, "http", visit);
}

pub(super) fn each_call_of<'a>(expr: &'a SExpression, head: &str, visit: &mut dyn FnMut(&'a [SExpression])) {
    match expr {
        SExpression::Array(arr) => {
            if matches!(arr.first(), Some(SExpression::String(h)) if h == head) {
                visit(arr);
            }
            for item in arr {
                each_call_of(item, head, visit);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                each_call_of(v, head, visit);
            }
        }
        _ => {}
    }
}

const BEACON_REQUEST_KEYS: [&str; 2] = ["text", "key"];
const BEACON_OPTION_KEYS: [&str; 2] = ["key", "emit"];
const BEACON_EMIT_KEYS: [&str; 3] = ["success", "failure", "cancelled"];

fn beacon_unknown(site: &Site, owner: &str, key: &str, allowed: &[&str]) -> ValidationError {
    ValidationError::error(
        ErrorCode::BeaconOptionUnknown,
        site.path,
        format!("`{key}` is not a key of browser/beacon {owner}"),
    )
    .with_valid_values(allowed.iter().map(|s| s.to_string()).collect())
    .with_suggestion("A beacon posts `text` as text/plain: no headers, body kinds, auth, retry, paginate, stream, presign or multipart")
}

/// `browser/beacon`: an absolute http(s) URL whose origin is in `egress`, a request of `text` (and `key`), options
/// of `key` and `emit`.
fn check_beacon_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    match arr.get(1) {
        Some(SExpression::String(url)) if !is_dynamic(url) && config_knob(url).is_none() => {
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                check_url_position(&arr[1], true, "beacon URL origin", site, result);
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BeaconUrlNotAbsolute,
                        site.path,
                        format!("browser/beacon URL '{url}' is not an absolute http(s) URL"),
                    )
                    .with_suggestion("Write the full URL, e.g. https://collector.example.com/e; a relative endpoint is refused"),
                );
            }
        }
        Some(other) => check_url_position(other, true, "beacon URL origin", site, result),
        None => {}
    }
    let request = match arr.get(2) {
        Some(SExpression::Object(request)) => Some(request),
        _ => None,
    };
    match request {
        Some(request) => {
            let mut keys: Vec<&String> = request.keys().collect();
            keys.sort();
            for key in keys {
                if !BEACON_REQUEST_KEYS.contains(&key.as_str()) {
                    result.add_error(beacon_unknown(site, "request", key, &BEACON_REQUEST_KEYS));
                }
            }
            if !present(request, "text") {
                result.add_error(text_required(site));
            }
        }
        None if arr.get(2).is_none() => result.add_error(text_required(site)),
        None => {}
    }
    let options = match arr.get(3) {
        Some(SExpression::Object(options)) => Some(options),
        _ => None,
    };
    if let Some(options) = options {
        let mut keys: Vec<&String> = options.keys().collect();
        keys.sort();
        for key in keys {
            if !BEACON_OPTION_KEYS.contains(&key.as_str()) {
                result.add_error(beacon_unknown(site, "options", key, &BEACON_OPTION_KEYS));
            }
        }
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &BEACON_EMIT_KEYS, Some(site.declared), site.path));
        }
    }
    let request_key = request.and_then(|r| r.get("key"));
    let option_key = options.and_then(|o| o.get("key"));
    if request_key.is_some() && option_key.is_some() {
        result.add_error(
            ValidationError::error(ErrorCode::BeaconOptionUnknown, site.path, "browser/beacon `key` is given in both the request and the options")
                .with_suggestion("Name the call once"),
        );
    }
    let mut keyed = options.cloned().unwrap_or_default();
    if let Some(key) = request_key {
        keyed.insert("key".to_string(), key.clone());
    }
    result.merge(check_key_and_cancelled(&keyed, site.path, "browser/beacon"));
}

fn text_required(site: &Site) -> ValidationError {
    ValidationError::error(ErrorCode::BeaconTextRequired, site.path, "browser/beacon needs a request carrying `text`")
        .with_suggestion("Write `{ text: <the body> }` after the URL")
}

/// `browser/page-facts`: a non-empty, duplicate-free list of the closed words.
fn check_page_facts_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let invalid = |message: String| {
        ValidationError::error(ErrorCode::PageFactsFieldInvalid, site.path, message)
            .with_valid_values(orbital_core_base::ir::PageFactField::ALL.iter().map(|f| f.as_str().to_string()).collect())
            .with_suggestion("List the facts to read, e.g. [referrer, screenWidth]")
    };
    let Some(SExpression::Array(words)) = arr.get(1) else {
        result.add_error(invalid("browser/page-facts takes a list of page facts".to_string()));
        return;
    };
    if words.is_empty() {
        result.add_error(invalid("browser/page-facts needs at least one fact".to_string()));
    }
    let mut seen: Vec<&str> = Vec::new();
    for word in words {
        match word.as_str().and_then(orbital_core_base::ir::PageFactField::parse) {
            None => result.add_error(invalid(format!("{} is not a page fact", serde_json::to_string(word).unwrap_or_default()))),
            Some(field) if seen.contains(&field.as_str()) => {
                result.add_error(invalid(format!("page fact '{}' is listed twice", field.as_str())));
            }
            Some(field) => seen.push(field.as_str()),
        }
    }
    if let Some(SExpression::Object(options)) = arr.get(2) {
        result.merge(check_key_and_cancelled(options, site.path, "browser/page-facts"));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure"], Some(site.declared), site.path));
        }
    }
}

/// `browser/prompt` / `browser/embed`: the positional arity, a closed `{ key, emit }` options object whose emit
/// routes are `success`, `failure` and `cancelled`, a string `key`, and a literal response schema that is a JSON object.
fn check_on_device_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let head = arr.first().and_then(SExpression::as_str).unwrap_or_default();
    let positional = if head == "browser/prompt" { 2 } else { 1 };
    let suggestion = if head == "browser/prompt" {
        "Write (browser/prompt <text> <response schema text> { key: <string>, emit: { success: <EVENT>, failure: <EVENT>, cancelled: <EVENT> } })"
    } else {
        "Write (browser/embed [<text>, …] { key: <string>, emit: { success: <EVENT>, failure: <EVENT>, cancelled: <EVENT> } })"
    };
    if !(positional + 1..=positional + 2).contains(&arr.len()) {
        result.add_error(
            ValidationError::error(
                ErrorCode::HostEffectArity,
                site.path,
                format!("`{head}` takes {positional} argument(s) and an optional options object, got {}", arr.len() - 1),
            )
            .with_suggestion(suggestion),
        );
        return;
    }
    if let Some(SExpression::String(schema)) = arr.get(2).filter(|_| head == "browser/prompt").filter(|s| !matches!(s, SExpression::String(text) if is_dynamic(text))) {
        if !matches!(serde_json::from_str::<serde_json::Value>(schema), Ok(serde_json::Value::Object(_))) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OnDeviceSchemaInvalid,
                    site.path,
                    "browser/prompt response schema must be JSON Schema text: a JSON object",
                )
                .with_suggestion("Build it with (json/stringify { type: \"object\", … })"),
            );
        }
    }
    match arr.get(positional + 1) {
        None => {}
        Some(SExpression::Object(options)) => {
            if let Some(unknown) = options.keys().find(|k| !["key", "emit"].contains(&k.as_str())) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::HostEffectOptionUnknown,
                        site.path,
                        format!("`{head}` options accept only key and emit, got `{unknown}`"),
                    )
                    .with_valid_values(vec!["key".to_string(), "emit".to_string()]),
                );
            }
            result.merge(check_key_and_cancelled(options, site.path, head));
            if let Some(emit) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(emit, &["success", "failure", "cancelled"], Some(site.declared), site.path));
            }
        }
        Some(_) => result.add_error(
            ValidationError::error(ErrorCode::HostEffectOptionUnknown, site.path, format!("`{head}` options must be an object"))
                .with_suggestion(suggestion),
        ),
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
            let site = Site {
                path: at,
                egress: schema.egress.as_deref(),
                config: trait_def.config.as_ref(),
                declared: &declared,
            };
            each_http_call(expr, &mut |arr| check_http_call(arr, &site, &mut result));
            each_call_of(expr, "browser/beacon", &mut |arr| check_beacon_call(arr, &site, &mut result));
            each_call_of(expr, "browser/page-facts", &mut |arr| check_page_facts_call(arr, &site, &mut result));
            for head in super::keyed_call::ON_DEVICE_OPERATORS {
                each_call_of(expr, head, &mut |arr| check_on_device_call(arr, &site, &mut result));
            }
        });
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate().filter(|(_, tick)| tick.background) {
            for (j, effect) in tick.effects.iter().enumerate() {
                for head in super::keyed_call::ON_DEVICE_OPERATORS {
                    each_call_of(effect, head, &mut |_| {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::OnDeviceEffectOnServer,
                                format!("{path}.ticks[{i}].effects[{j}]"),
                                format!("`{head}` asks the on-device model, which only a browser host holds, but the `[background]` tick '{}' runs on the server", tick.name),
                            )
                            .with_suggestion("Drop [background] so the browser runs the tick, or emit an event a browser-side trait handles"),
                        );
                    });
                }
            }
        }
    }
    result
}
