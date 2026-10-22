//! `oauth-client` auth, the `oauth-endpoint` target and the `oauth-authorize` / `oauth-connection-*` effects
//! (`docs/Almadar_Embedded_Kernel.md`, "OAuth"): the closed client object, op and provider vocabulary, the op's
//! inputs and target word, and the request keys an OAuth call may carry. A secret slot being a `secret` knob is
//! `orbital_core::secrets`; a discovered endpoint never widens egress and is refused at runtime.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use super::emit_declaration::collect_declared_emits;
use super::http::{each_call_of, http_objects, visit_effects};
use super::keyed_call::check_key_and_cancelled;
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::oauth_ask::OAuthProvider;
use orbital_core_base::oauth_client::{OAuthClientDecl, OAuthEndpointKind, OAuthOp};
use orbital_core_base::http_wire::{parse_origin, split_url};
use orbital_core_base::schema::{OrbitalSchema, SExpression, SObject};

const CLIENT_MODES: [&str; 2] = ["real", "mock"];
const CLIENT_AUTHS: [&str; 3] = ["basic", "post", "none"];
const ENDPOINT_KEYS: [&str; 4] = ["authorization", "token", "revocation", "userinfo"];
const AUTHORIZE_KEYS: [&str; 4] = ["redirectUri", "scope", "subject", "params"];
const EMIT_KEYS: [&str; 2] = ["success", "failure"];
const INPUT_KEYS: [&str; 7] = ["code", "state", "subject", "provider", "refreshToken", "token", "accessToken"];

fn is_dynamic(value: &str) -> bool {
    value.starts_with('@') || value.starts_with('?') || value.starts_with('$')
}

fn err(code: ErrorCode, at: &str, message: impl Into<String>) -> ValidationError {
    ValidationError::error(code, at, message.into())
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// A closed word at `key` of `obj`: the literal is checked; a binding or expression is left to runtime.
fn check_word(obj: &SObject, key: &str, allowed: &[&str], code: ErrorCode, at: &str, what: &str, result: &mut ValidationResult) {
    match obj.get(key) {
        Some(SExpression::String(word)) if !is_dynamic(word) && !allowed.contains(&word.as_str()) => {
            result.add_error(
                err(code, at, format!("{what} `{key}` must be one of {}, not `{word}`", allowed.join(", "))).with_valid_values(words(allowed)),
            );
        }
        Some(SExpression::String(_)) | Some(SExpression::Array(_)) => {}
        Some(_) => result.add_error(err(code, at, format!("{what} `{key}` must be a word"))),
        None => result.add_error(err(code, at, format!("{what} needs `{key}`"))),
    }
}

fn egress_declared(egress: Option<&[String]>, url: &str) -> bool {
    let Some((origin, _)) = split_url(url) else { return false };
    egress.into_iter().flatten().filter_map(|entry| parse_origin(entry)).any(|allowed| allowed == origin)
}

fn check_url_egress(value: Option<&SExpression>, egress: Option<&[String]>, what: &str, at: &str, result: &mut ValidationResult) {
    if let Some(SExpression::String(url)) = value {
        if !is_dynamic(url) && !egress_declared(egress, url) {
            result.add_error(
                err(ErrorCode::HttpEgressUndeclared, at, format!("{what} '{url}' is not declared in the program's `egress`"))
                    .with_suggestion("Add the exact scheme://host[:port] origin to the app's `egress: [...]`"),
            );
        }
    }
}

fn check_client(client: &SExpression, egress: Option<&[String]>, at: &str, result: &mut ValidationResult) {
    let obj = match client {
        SExpression::Object(obj) => obj,
        SExpression::String(binding) if is_dynamic(binding) => return,
        SExpression::Array(_) => return,
        _ => {
            result.add_error(err(ErrorCode::OAuthClientInvalid, at, "an OAuth client is an object { tag, mode, issuer | endpoints, clientId, clientSecret?, clientAuth }"));
            return;
        }
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        if !OAuthClientDecl::KEYS.contains(&key.as_str()) {
            result.add_error(
                err(ErrorCode::OAuthClientInvalid, at, format!("`{key}` is not a key of an OAuth client"))
                    .with_valid_values(words(OAuthClientDecl::KEYS)),
            );
        }
    }
    check_word(obj, "tag", OAuthProvider::WORDS, ErrorCode::OAuthClientInvalid, at, "OAuth client", result);
    check_word(obj, "mode", &CLIENT_MODES, ErrorCode::OAuthClientInvalid, at, "OAuth client", result);
    check_word(obj, "clientAuth", &CLIENT_AUTHS, ErrorCode::OAuthClientInvalid, at, "OAuth client", result);
    if !obj.contains_key("clientId") {
        result.add_error(err(ErrorCode::OAuthClientInvalid, at, "an OAuth client needs `clientId`"));
    }
    let present = |key: &str| !matches!(obj.get(key), None | Some(SExpression::Null));
    match (present("issuer"), present("endpoints")) {
        (true, false) => check_url_egress(obj.get("issuer"), egress, "OAuth issuer", at, result),
        (false, true) => match obj.get("endpoints") {
            Some(SExpression::Object(endpoints)) => {
                let mut keys: Vec<&String> = endpoints.keys().collect();
                keys.sort();
                for key in keys {
                    if !ENDPOINT_KEYS.contains(&key.as_str()) {
                        result.add_error(
                            err(ErrorCode::OAuthClientInvalid, at, format!("`{key}` is not an OAuth endpoint"))
                                .with_valid_values(words(&ENDPOINT_KEYS)),
                        );
                    } else {
                        check_url_egress(endpoints.get(key), egress, "OAuth endpoint", at, result);
                    }
                }
                for required in ["authorization", "token"] {
                    if !endpoints.contains_key(required) {
                        result.add_error(err(ErrorCode::OAuthClientInvalid, at, format!("OAuth endpoints need `{required}`")));
                    }
                }
            }
            Some(SExpression::String(_)) | Some(SExpression::Array(_)) => {}
            _ => result.add_error(err(ErrorCode::OAuthClientInvalid, at, "OAuth `endpoints` must be an object")),
        },
        _ => result.add_error(
            err(ErrorCode::OAuthClientInvalid, at, "an OAuth client has exactly one of `issuer` or `endpoints`")
                .with_suggestion("Name a discovered issuer, or the endpoints themselves"),
        ),
    }
}

fn check_provider_word(value: Option<&SExpression>, at: &str, result: &mut ValidationResult) {
    match value {
        Some(SExpression::String(word)) if is_dynamic(word) || OAuthProvider::parse(word).is_some() => {}
        Some(SExpression::Array(_)) => {}
        other => {
            let shown = other.map(|v| serde_json::to_string(v).unwrap_or_default()).unwrap_or_default();
            result.add_error(
                err(ErrorCode::OAuthProviderUnknown, at, format!("provider {shown} is not one of {}", OAuthProvider::WORDS.join(", ")))
                    .with_valid_values(words(OAuthProvider::WORDS)),
            );
        }
    }
}

fn check_http_call(arr: &[SExpression], egress: Option<&[String]>, at: &str, result: &mut ValidationResult) {
    let endpoint_target = matches!(arr.get(2), Some(SExpression::String(origin)) if origin == OAuthEndpointKind::TARGET);
    let (request, _) = http_objects(arr);
    let auth = request.and_then(|r| match r.get("auth") {
        Some(SExpression::Object(auth)) => Some(auth),
        _ => None,
    });
    let oauth_auth = auth.filter(|a| matches!(a.get("scheme"), Some(SExpression::String(s)) if s == "oauth-client"));
    let Some(auth) = oauth_auth else {
        if endpoint_target {
            result.add_error(
                err(ErrorCode::OAuthEndpointMismatch, at, "`oauth-endpoint` needs an `oauth-client` auth")
                    .with_suggestion("Write auth: { scheme: oauth-client, op: <op>, client: <client>, … }"),
            );
        }
        return;
    };
    if let Some(request) = request {
        if let Some(extra) = request.keys().find(|key| key.as_str() != "auth") {
            result.add_error(
                err(ErrorCode::OAuthRequestConflict, at, format!("an `oauth-client` request carries only `auth`, not `{extra}`"))
                    .with_suggestion("The driver builds the whole request; remove body, query, headers, retry, paginate, stream and presign"),
            );
        }
    }
    match (auth.get("client"), auth.get("clients")) {
        (Some(client), None) => check_client(client, egress, at, result),
        (None, Some(SExpression::Object(clients))) => {
            if clients.is_empty() {
                result.add_error(err(ErrorCode::OAuthClientInvalid, at, "`clients` declares no client"));
            }
            let mut tags: Vec<&String> = clients.keys().collect();
            tags.sort();
            for tag in tags {
                if OAuthProvider::parse(tag).is_none() {
                    result.add_error(
                        err(ErrorCode::OAuthProviderUnknown, at, format!("`clients` is keyed by provider tag, not `{tag}`"))
                            .with_valid_values(words(OAuthProvider::WORDS)),
                    );
                    continue;
                }
                let client = &clients[tag];
                check_client(client, egress, at, result);
                if let SExpression::Object(declared) = client {
                    if !matches!(declared.get("tag"), Some(SExpression::String(own)) if own == tag) {
                        result.add_error(err(ErrorCode::OAuthClientInvalid, at, format!("`clients.{tag}` declares a different `tag`")));
                    }
                }
            }
        }
        (None, Some(SExpression::String(binding))) if is_dynamic(binding) => {}
        (None, Some(_)) => result.add_error(err(ErrorCode::OAuthClientInvalid, at, "`clients` is an object of clients keyed by provider tag")),
        (Some(_), Some(_)) => result.add_error(err(ErrorCode::OAuthClientInvalid, at, "an `oauth-client` auth has `client` or `clients`, not both")),
        (None, None) => result.add_error(err(ErrorCode::OAuthClientInvalid, at, "an `oauth-client` auth needs `client`")),
    }
    match auth.get("provider") {
        Some(provider) => check_provider_word(Some(provider), at, result),
        None => {}
    }
    let op = match auth.get("op") {
        Some(SExpression::String(word)) if is_dynamic(word) => return,
        Some(SExpression::String(word)) => match OAuthOp::parse(word) {
            Some(op) => op,
            None => {
                result.add_error(
                    err(ErrorCode::OAuthOpUnknown, at, format!("`{word}` is not an oauth-client op"))
                        .with_valid_values(words(OAuthOp::WORDS)),
                );
                return;
            }
        },
        _ => {
            result.add_error(
                err(ErrorCode::OAuthOpUnknown, at, "an `oauth-client` auth needs an `op`").with_valid_values(words(OAuthOp::WORDS)),
            );
            return;
        }
    };
    let (required, optional) = op.inputs();
    for key in required {
        if !matches!(auth.get(*key), Some(value) if !matches!(value, SExpression::Null)) {
            result.add_error(err(ErrorCode::OAuthOpUnknown, at, format!("oauth-client op `{}` needs `{key}`", op.name())));
        }
    }
    if auth.contains_key("clients") && op != OAuthOp::Code {
        result.add_error(
            err(ErrorCode::OAuthOpUnknown, at, format!("oauth-client op `{}` takes a single `client`; only `code` routes by `clients`", op.name()))
                .with_suggestion("Declare `client` for this op"),
        );
    }
    for key in INPUT_KEYS {
        if auth.contains_key(key) && !required.contains(&key) && !optional.contains(&key) {
            result.add_error(err(ErrorCode::OAuthOpUnknown, at, format!("oauth-client op `{}` does not take `{key}`", op.name())));
        }
    }
    let method = match arr.get(1) {
        Some(SExpression::String(method)) => Some(method.as_str()),
        _ => None,
    };
    match (op.endpoint(), endpoint_target) {
        (Some(kind), true) => {
            match arr.get(3) {
                Some(SExpression::String(word)) if word == kind.name() => {}
                other => {
                    let shown = other.map(|v| serde_json::to_string(v).unwrap_or_default()).unwrap_or_default();
                    result.add_error(err(
                        ErrorCode::OAuthEndpointMismatch,
                        at,
                        format!("oauth-client op `{}` is sent to `oauth-endpoint {}`, not {shown}", op.name(), kind.name()),
                    ));
                }
            }
            let expected = if kind == OAuthEndpointKind::Userinfo { "GET" } else { "POST" };
            if method.is_some_and(|m| m != expected) {
                result.add_error(err(ErrorCode::OAuthEndpointMismatch, at, format!("`oauth-endpoint {}` is called with {expected}", kind.name())));
            }
        }
        (Some(kind), false) => result.add_error(err(
            ErrorCode::OAuthEndpointMismatch,
            at,
            format!("oauth-client op `{}` is sent to `oauth-endpoint {}`", op.name(), kind.name()),
        )),
        (None, true) => result.add_error(err(
            ErrorCode::OAuthEndpointMismatch,
            at,
            "oauth-client op `discovery` is a GET of the issuer's document, not an `oauth-endpoint`",
        )),
        (None, false) => {
            if method.is_some_and(|m| m != "GET") {
                result.add_error(err(ErrorCode::OAuthEndpointMismatch, at, "oauth-client op `discovery` is called with GET"));
            }
        }
    }
}

fn check_options(options: Option<&SExpression>, head: &str, declared: &[DeclaredEmit], at: &str, result: &mut ValidationResult) {
    let keyed = head == "oauth-authorize";
    let allowed: &[&str] = if keyed { &["key", "emit"] } else { &["emit"] };
    match options {
        None => {}
        Some(SExpression::Object(options)) => {
            if let Some(key) = options.keys().find(|k| !allowed.contains(&k.as_str())) {
                result.add_error(
                    err(ErrorCode::HostEffectOptionUnknown, at, format!("`{head}` options accept only {}, got `{key}`", allowed.join(" and ")))
                        .with_valid_values(words(allowed)),
                );
            }
            if keyed {
                result.merge(check_key_and_cancelled(options, at, head));
            }
            if let Some(emit) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(emit, &EMIT_KEYS, Some(declared), at));
            }
        }
        Some(_) => result.add_error(err(ErrorCode::HostEffectOptionUnknown, at, format!("`{head}` options must be an object"))),
    }
}

fn check_authorize(arr: &[SExpression], egress: Option<&[String]>, declared: &[DeclaredEmit], at: &str, result: &mut ValidationResult) {
    if !(3..=4).contains(&arr.len()) {
        result.add_error(
            err(ErrorCode::HostEffectArity, at, format!("`oauth-authorize` takes a client, its options and optional emit options, got {}", arr.len() - 1))
                .with_suggestion("Write (oauth-authorize <client> { redirectUri: <url> } { emit: { success: <EVENT> } })"),
        );
        return;
    }
    check_client(&arr[1], egress, at, result);
    match &arr[2] {
        SExpression::Object(options) => {
            let mut keys: Vec<&String> = options.keys().collect();
            keys.sort();
            for key in keys {
                if !AUTHORIZE_KEYS.contains(&key.as_str()) {
                    result.add_error(
                        err(ErrorCode::OAuthOptionUnknown, at, format!("`{key}` is not an oauth-authorize option"))
                            .with_valid_values(words(&AUTHORIZE_KEYS)),
                    );
                }
            }
            if !options.contains_key("redirectUri") {
                result.add_error(err(ErrorCode::OAuthOptionUnknown, at, "oauth-authorize needs a `redirectUri`"));
            }
        }
        SExpression::Array(_) => {}
        SExpression::String(binding) if is_dynamic(binding) => {}
        _ => result.add_error(err(ErrorCode::OAuthOptionUnknown, at, "oauth-authorize options must be an object")),
    }
    check_options(arr.get(3), "oauth-authorize", declared, at, result);
}

fn check_connection(head: &str, arr: &[SExpression], declared: &[DeclaredEmit], at: &str, result: &mut ValidationResult) {
    if !(2..=3).contains(&arr.len()) {
        result.add_error(
            err(ErrorCode::HostEffectArity, at, format!("`{head}` takes a provider word and optional emit options, got {}", arr.len() - 1))
                .with_suggestion(format!("Write ({head} google {{ emit: {{ success: <EVENT> }} }})")),
        );
        return;
    }
    check_provider_word(arr.get(1), at, result);
    check_options(arr.get(2), head, declared, at, result);
}

pub(crate) fn validate_oauth_effects(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let egress = schema.egress.as_deref();
    for (trait_def, path) in inline_traits(schema) {
        let declared = collect_declared_emits(trait_def);
        visit_effects(trait_def, &path, &mut |expr, at| {
            each_call_of(expr, "http", &mut |arr| check_http_call(arr, egress, at, &mut result));
            each_call_of(expr, "oauth-authorize", &mut |arr| check_authorize(arr, egress, &declared, at, &mut result));
            for head in ["oauth-connection-status", "oauth-connection-forget"] {
                each_call_of(expr, head, &mut |arr| check_connection(head, arr, &declared, at, &mut result));
            }
        });
    }
    result
}
