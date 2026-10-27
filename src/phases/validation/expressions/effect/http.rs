//! `http` effect validation (`docs/Almadar_Embedded_Kernel.md` H.1, H.5).
//!
//! [`validate_http_effects`] — auth, request-operand and keyed-call contract. Runs
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
use orbital_core_base::http_wire::{is_header_name, parse_grant_request, parse_http_headers, parse_delay_ms, parse_retry, parse_timeout_ms, GrantRequest, HttpRetry, RetryOperand};
use orbital_core_base::schema::{OrbitalSchema, SExpression, SObject, TraitDefinition};

const BODY_KEYS: [&str; 6] = ["json", "form", "text", "bytes", "multipart", "webpush"];
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

fn is_dynamic(s: &str) -> bool {
    s.starts_with('@') || s.starts_with('?') || s.starts_with('$')
}

struct Site<'a> {
    path: &'a str,
    declared: &'a [DeclaredEmit],
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
        return;
    };
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
    if scheme == "hmac-sha256" && !matches!(auth.get("prefix"), None | Some(SExpression::Null | SExpression::String(_) | SExpression::Array(_))) {
        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, "hmac-sha256 auth.prefix must be text"));
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
                SExpression::Object(_) => {}
                _ if orbital_core_eval::lower::lower_sexpr(grant_request).is_ok_and(|expr| expr.is_literal()) => {
                    if let Err(error) = parse_grant_request(&Value::from(grant_request.clone())) {
                        result.add_error(ValidationError::error(ErrorCode::HttpAuthInvalid, site.path, error.message()));
                    }
                }
                _ => {}
            }
        }
    }
}

fn check_token_exchange(auth: &SObject, site: &Site, result: &mut ValidationResult) {
    if let Some(SExpression::Object(store_as)) = auth.get("storeAs") {
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

fn check_request(request: &SObject, site: &Site, result: &mut ValidationResult) {
    let bodies = BODY_KEYS.iter().filter(|k| present(request, k)).count();
    if let Some(SExpression::Object(auth)) = request.get("auth") {
        check_auth(auth, site, result);
    }
    if let Some(SExpression::Object(presign)) = request.get("presign") {
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
}

fn check_webpush(request: &SObject, call: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let Some(SExpression::Object(push)) = request.get("webpush") else { return };
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
            RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { fields: Vec::new(), additional: None }) => Some("headers must evaluate to an object".to_string()),
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
                parse_http_headers(Some(&Value::object(header))).err().map(|error| error.message())
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
        result.add_error(ValidationError::error(ErrorCode::HttpRequestOptionInvalid, path, error));
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
        result.add_error(ValidationError::error(ErrorCode::HttpRequestOptionInvalid, path, error));
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
            RetryOperand::Typed(ty) if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { fields: Vec::new(), additional: None }) => Some("retry must evaluate to an object".to_string()),
            RetryOperand::Items(_) => Some("retry must be an object".to_string()),
            _ => None,
        },
    };
    if let Some(error) = error {
        result.add_error(ValidationError::error(ErrorCode::HttpRequestOptionInvalid, path, error));
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
                            if !orbital_core_base::type_compat::oir_type_satisfies(&ty, &OirType::Struct { additional: None, fields: Vec::new() }) {
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
    let (request, options) = http_objects(arr);
    if let Some(request) = request {
        check_request(request, site, result);
        check_webpush(request, arr, site, result);
    }

    if let Some(options) = options {
        result.merge(check_key_and_cancelled(options, site.path, "http"));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(
                emit,
                &KEYED_CALL_EMIT_KEYS,
                Some(site.declared),
                site.path,
            ));
        }
    }
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

const BEACON_EMIT_KEYS: [&str; 3] = ["success", "failure", "cancelled"];

/// `browser/beacon`: the `key` (request or options) and `emit` options.
fn check_beacon_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let request = match arr.get(2) {
        Some(SExpression::Object(request)) => Some(request),
        _ => None,
    };
    let options = match arr.get(3) {
        Some(SExpression::Object(options)) => Some(options),
        _ => None,
    };
    if let Some(emit) = options.and_then(extract_emit_object_ref) {
        result.merge(validate_emit_config(emit, &BEACON_EMIT_KEYS, Some(site.declared), site.path));
    }
    let request_key = request.and_then(|r| r.get("key"));
    let mut keyed = options.cloned().unwrap_or_default();
    if let Some(key) = request_key {
        keyed.insert("key".to_string(), key.clone());
    }
    result.merge(check_key_and_cancelled(&keyed, site.path, "browser/beacon"));
}

/// `browser/page-facts`: the `key` and `emit` options.
fn check_page_facts_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    if let Some(SExpression::Object(options)) = arr.get(2) {
        result.merge(check_key_and_cancelled(options, site.path, "browser/page-facts"));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure"], Some(site.declared), site.path));
        }
    }
}

/// `browser/prompt` / `browser/embed`: the `key` and `emit` options.
fn check_on_device_call(arr: &[SExpression], site: &Site, result: &mut ValidationResult) {
    let head = arr.first().and_then(SExpression::as_str).unwrap_or_default();
    let positional = if head == "browser/prompt" { 2 } else { 1 };
    if let Some(SExpression::Object(options)) = arr.get(positional + 1) {
        result.merge(check_key_and_cancelled(options, site.path, head));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure", "cancelled"], Some(site.declared), site.path));
        }
    }
}

/// Whether `value` carries nothing only run time knows: no binding, no operator call.
fn is_literal(value: &SExpression) -> bool {
    match value {
        SExpression::String(text) => !is_dynamic(text),
        SExpression::Array(items) => !matches!(items.first(), Some(SExpression::String(_))) && items.iter().all(is_literal),
        SExpression::Object(fields) => fields.values().all(is_literal),
        _ => true,
    }
}

/// `browser/tab-<op>`: a request object of the op's closed keys, a closed `{ key, emit }` options object
/// whose emit routes are `success`, `failure` and `cancelled`, a string `key`, and, when the request holds nothing only
/// run time knows, the whole request as the kernel's one parser reads it.
fn check_tab_call(
    arr: &[SExpression],
    site: &Site,
    scope: &orbital_core_base::tab_call::WatchScope<'_>,
    result: &mut ValidationResult,
) {
    use orbital_core_base::tab::TabOp;
    let head = arr.first().and_then(SExpression::as_str).unwrap_or_default();
    let Some(op) = TabOp::from_name(head) else { return };
    let suggestion = format!(
        "Write ({head} {{ {} }} {{ key: <string>, emit: {{ success: <EVENT>, failure: <EVENT>, cancelled: <EVENT> }} }})",
        op.request_keys().join(", ")
    );
    match arr.get(1) {
        None => {}
        Some(SExpression::Object(request)) => {
            if let Some(unknown) = request.keys().find(|k| !op.request_keys().contains(&k.as_str())) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TabRequestInvalid,
                        site.path,
                        format!("`{head}` request has no key `{unknown}`"),
                    )
                    .with_valid_values(op.request_keys().iter().map(|k| k.to_string()).collect()),
                );
            } else if is_literal(&arr[1]) {
                let value = Value::from(arr[1].clone());
                if let Err(message) = orbital_core_base::tab_call::parse_tab_call(op, &value, Some(scope), None) {
                    result.add_error(ValidationError::error(ErrorCode::TabRequestInvalid, site.path, message));
                }
            }
        }
        Some(SExpression::String(text)) if is_dynamic(text) => {}
        Some(_) => result.add_error(
            ValidationError::error(ErrorCode::TabRequestInvalid, site.path, format!("`{head}` request must be an object"))
                .with_suggestion(suggestion),
        ),
    }
    if let Some(SExpression::Object(options)) = arr.get(2) {
        result.merge(check_key_and_cancelled(options, site.path, head));
        if let Some(emit) = extract_emit_object_ref(options) {
            result.merge(validate_emit_config(emit, &["success", "failure", "cancelled"], Some(site.declared), site.path));
        }
    }
}

/// Keyed-call, emit and auth checks for every `http` effect.
pub(crate) fn validate_http_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let inputs = orbital_core_base::external_inputs::external_inputs_of(schema);
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        let orbital = schema.orbitals.iter().find(|o| {
            o.traits.iter().any(|t| matches!(t, orbital_core_base::schema::TraitReference::Inline(td) if std::ptr::eq(td, trait_def)))
        });
        let scope = orbital_core_base::tab_call::WatchScope {
            caller_orbital: orbital.map(|o| o.name.as_str()),
            caller_trait: trait_def.name.as_str(),
            inputs: &inputs,
        };
        visit_effects(trait_def, &path, &mut |expr, at| {
            let site = Site {
                path: at,
                declared: &declared,
            };
            each_http_call(expr, &mut |arr| check_http_call(arr, &site, &mut result));
            each_call_of(expr, "browser/beacon", &mut |arr| check_beacon_call(arr, &site, &mut result));
            each_call_of(expr, "browser/page-facts", &mut |arr| check_page_facts_call(arr, &site, &mut result));
            for head in super::keyed_call::ON_DEVICE_OPERATORS {
                each_call_of(expr, head, &mut |arr| check_on_device_call(arr, &site, &mut result));
            }
            for op in orbital_core_base::tab::TabOp::ALL {
                each_call_of(expr, op.name(), &mut |arr| check_tab_call(arr, &site, &scope, &mut result));
            }
        });
    }
    result
}
