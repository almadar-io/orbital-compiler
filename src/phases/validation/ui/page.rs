//! Page Validation
//!
//! Validates page structure, paths, and trait references.
//! Ported from `builder/packages/shared/src/orbitals/validation/page-validator.ts`.

use super::effect::{expr_return_incompatible_in_ctx, BindingTypes, ExprReturnCtx};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::protocols;
use orbital_core_base::schema::{OrbitalSchema, PageDefinition, SExpression, TraitReference};
use std::collections::{HashMap, HashSet};

/// Validate all pages in a schema
pub fn validate_all_pages(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(validate_protocol_pages(schema));
}

/// Structural reachability signals collected once across the WHOLE schema —
/// a link declared by orbital A's shell can target a page orbital B
/// declares, so reachability is a schema-wide question, never per-orbital.
struct ReachabilitySignals {
    /// Literal `href` string values found anywhere in a trait's config tree
    /// or a rendered `render-ui` payload, scalar or nested inside an
    /// object-array entry — one recursive walk covers both shapes.
    ///
    /// `href` is the only key used here, deliberately: the patterns
    /// registry (`embedded.rs::EmbeddedPatternProp`) carries no dedicated
    /// `kind` for a link prop, and the sibling names `to`/`path` are reused
    /// in this SAME registry for state-machine transition targets, SVG path
    /// data, gradient stops, and canvas coordinates (`state-graph.to`,
    /// `learning-canvas.shapes[].path`, `draw-shape.gradient.to`,
    /// `math-canvas.angles[].to`) — matching on those would be a heuristic,
    /// not a structural fact, so they are excluded.
    hrefs: HashSet<String>,
    /// Literal (or static-prefix, from a `str/concat` dynamic target)
    /// targets of every `navigate` effect in the schema.
    navigate_targets: HashSet<String>,
}

/// An href's path when it is known without evaluation: a literal string, or a
/// `str/concat` of literal strings (an imported page's `"/ar"` + `"/blog/x"`).
fn literal_href(expr: &SExpression) -> Option<String> {
    let path = match expr {
        SExpression::String(s) if !s.starts_with('@') => s.clone(),
        SExpression::Array(items) => (match items.split_first() {
            Some((SExpression::String(op), parts)) if op == "str/concat" => parts
                .iter()
                .map(|p| match p {
                    SExpression::String(s) if !s.starts_with('@') => Some(s.as_str()),
                    _ => None,
                })
                .collect::<Option<String>>()?,
            _ => return None,
        }).into(),
        _ => return None,
    };
    (!path.is_empty()).then_some(path.into_inner())
}

fn collect_href_strings(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::Object(map) => {
            // `navigatesTo` is an action entry's link (`userMenuItems`, page actions): the UI emits it as `UI:NAVIGATE`.
            for key in ["href", "navigatesTo"] {
                if let Some(path) = map.get(key).and_then(literal_href) {
                    out.insert(path);
                }
            }
            for v in map.values() {
                collect_href_strings(v, out);
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_href_strings(item, out);
            }
        }
        _ => {}
    }
}

fn collect_navigate_targets(expr: &SExpression, out: &mut HashSet<String>) {
    if let SExpression::Array(items) = expr {
        if let (Some(SExpression::String(op)), Some(target)) = (items.first(), items.get(1)) {
            if op == "navigate" {
                match target {
                    SExpression::String(s) => {
                        out.insert(s.clone().into_inner());
                    }
                    // `["str/concat", "/inspection/", "@entity.id"]` — the
                    // literal first segment is a checkable static prefix,
                    // mirroring `effect/navigate.rs::extract_static_path_prefix`.
                    SExpression::Array(inner) => {
                        if let (
                            Some(SExpression::String(concat_op)),
                            Some(SExpression::String(prefix)),
                        ) = (inner.first(), inner.get(1))
                        {
                            if concat_op == "str/concat" {
                                out.insert(prefix.clone().into_inner());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for item in items {
            collect_navigate_targets(item, out);
        }
    } else if let SExpression::Object(map) = expr {
        for v in map.values() {
            collect_navigate_targets(v, out);
        }
    }
}

/// Every link and navigate target in `schema`, sorted; recorded on a lazy page so links between lazy pages count.
pub(crate) fn reachability_targets(schema: &OrbitalSchema) -> Vec<String> {
    let signals = collect_reachability_signals(schema);
    let mut targets: Vec<String> = signals
        .hrefs
        .into_iter()
        .chain(signals.navigate_targets)
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

fn collect_reachability_signals(schema: &OrbitalSchema) -> ReachabilitySignals {
    let mut hrefs = HashSet::new();
    let mut navigate_targets = HashSet::new();
    // The page access host navigates a non-admitted viewer to the app's
    // declared `signIn` route — a declared navigate target like any effect's.
    if let Some(sign_in) = schema.site.as_ref().and_then(|s| s.sign_in.as_ref()) {
        navigate_targets.insert(sign_in.clone());
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            if let Some(config) = &td.config {
                for field in config.values() {
                    if let Some(default) = &field.default {
                        collect_href_strings(default, &mut hrefs);
                    }
                }
            }
            let effect_sources = td
                .state_machine
                .transitions
                .iter()
                .filter_map(|t| t.effects.as_ref())
                .chain(td.ticks.iter().flatten().map(|t| &t.effects));
            for effects in effect_sources {
                for effect in effects {
                    if let SExpression::Array(arr) = effect {
                        if matches!(arr.first(), Some(SExpression::String(op)) if op == "render-ui")
                        {
                            if let Some(payload) = arr.get(2) {
                                collect_href_strings(payload, &mut hrefs);
                            }
                        }
                    }
                    collect_navigate_targets(effect, &mut navigate_targets);
                }
            }
        }
    }

    // `@pages` lists every root page that declares `@label`, so a program using the
    // sigil links each of them.
    if schema_uses_pages_sigil(schema) {
        for orbital in &schema.orbitals {
            for page in orbital
                .pages
                .iter()
                .flatten()
                .filter_map(|p| p.as_inline())
                .filter(|p| p.is_route())
            {
                if page.label.is_some() {
                    hrefs.insert(page.path.clone());
                }
            }
        }
    }

    ReachabilitySignals {
        hrefs,
        navigate_targets,
    }
}

fn mentions_pages_sigil(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) => s == "@pages",
        SExpression::Array(items) => items.iter().any(mentions_pages_sigil),
        SExpression::Object(map) => map.values().any(mentions_pages_sigil),
        _ => false,
    }
}

fn schema_uses_pages_sigil(schema: &OrbitalSchema) -> bool {
    let config_uses = |config: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>| {
        config
            .into_iter()
            .flat_map(|c| c.values())
            .filter_map(|f| f.default.as_ref())
            .any(mentions_pages_sigil)
    };
    config_uses(schema.config.as_ref())
        || schema.orbitals.iter().any(|orbital| {
            config_uses(orbital.config.as_ref())
                || orbital.traits.iter().any(|tr| {
                    let TraitReference::Inline(td) = tr else {
                        return false;
                    };
                    config_uses(td.config.as_ref())
                        || td
                            .state_machine
                            .transitions
                            .iter()
                            .filter_map(|t| t.effects.as_ref())
                            .flatten()
                            .any(mentions_pages_sigil)
                })
        })
}

/// The literal a `@config.<knob>` address value stands for at this orbital: the knob's declared default,
/// read orbital-first then app, as trait config resolves.
fn literal_of<'a>(
    value: &'a SExpression,
    schema: &'a OrbitalSchema,
    orbital: &'a orbital_core_base::schema::Orbital,
) -> Option<&'a SExpression> {
    match value {
        SExpression::String(text) => match text.strip_prefix("@config.") {
            Some(knob) if !knob.contains('.') => orbital
                .config
                .as_ref()
                .and_then(|c| c.get(knob))
                .or_else(|| schema.config.as_ref().and_then(|c| c.get(knob)))
                .and_then(|field| field.default.as_ref())
                .filter(|default| !matches!(default, SExpression::String(t) if t.starts_with('@'))),
            Some(_) => None,
            None if text.starts_with('@') => None,
            None => Some(value),
        },
        SExpression::Number(_) => Some(value),
        _ => None,
    }
}

fn config_types(
    schema: &OrbitalSchema,
    orbital: &orbital_core_base::schema::Orbital,
) -> BindingTypes {
    let knobs = schema
        .config
        .iter()
        .flatten()
        .chain(orbital.config.iter().flatten())
        .map(|(name, field)| {
            (
                name.clone(),
                crate::phases::resolve::config_field_oir_type(field, schema).unwrap_or_else(|_| orbital_core_base::ir::OirType::erased()),
            )
        })
        .collect();
    BindingTypes {
        payload: HashMap::new(),
        user: orbital_core_base::ir::OirType::Struct {
            additional: None,
            fields: Default::default(),
        },
        config: knobs,
        prev_payload: None,
        optional_payload: HashSet::new(),
        locals: HashMap::new(),
        type_params: HashSet::new(),
    }
}

fn address_problems(
    spec: &protocols::ProtocolSpec,
    page: &PageDefinition,
    schema: &OrbitalSchema,
    orbital: &orbital_core_base::schema::Orbital,
) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(fields) = spec.address.as_deref() else {
        if page.address.is_some() {
            problems.push(format!("protocol '{}' takes no address", spec.name));
        }
        return problems;
    };
    let Some(address) = page.address.as_ref() else {
        problems.push(format!(
            "protocol '{}' needs an address record {{ {} }}",
            spec.name,
            fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return problems;
    };
    let SExpression::Object(record) = address else {
        problems.push(format!("the address of '{}' must be a record", spec.name));
        return problems;
    };
    for key in record.keys() {
        if !fields.iter().any(|f| f.name == key.as_str()) {
            problems.push(format!(
                "'{key}' is no field of the '{}' address",
                spec.name
            ));
        }
    }
    let types = config_types(schema, orbital);
    let entity = HashMap::new();
    let payload = HashMap::new();
    let ctx = ExprReturnCtx {
        entity: &entity,
        payload: &payload,
        types: Some(&types),
        schema: Some(schema),
    };
    for field in fields {
        let Some(value) = record.get(field.name.as_str()) else {
            if field.required {
                problems.push(format!(
                    "the '{}' address needs '{}'",
                    spec.name, field.name
                ));
            }
            continue;
        };
        let declared = orbital_core_base::type_compat::compat_kind_oir(&field.ty);
        if let Some(found) = expr_return_incompatible_in_ctx(value, &declared, &ctx) {
            problems.push(format!(
                "'{}' of the '{}' address is {} where {} is declared",
                field.name,
                spec.name,
                orbital_core_base::type_compat::oir_type_kind(&found),
                field.ty
            ));
        }
    }
    problems
}

fn grant_problem(
    spec: &protocols::ProtocolSpec,
    page: &PageDefinition,
    schema: &OrbitalSchema,
    orbital: &orbital_core_base::schema::Orbital,
) -> Option<(ErrorCode, String)> {
    use orbital_core_base::os_grants::{NetKind, StdioKind};
    let grants = schema.orbital_os_grants(orbital);
    match spec.grant? {
        protocols::GrantKind::Stdio => (!(grants.stdio.contains(&StdioKind::In)
            && grants.stdio.contains(&StdioKind::Out)))
        .then(|| {
            (
                ErrorCode::PageEndpointGrantMissing,
                format!(
                    "'{}' endpoints read and write: they need `stdio: [in, out]`",
                    spec.name
                ),
            )
        }),
        protocols::GrantKind::NetListen => {
            let record = match page.address.as_ref() {
                Some(SExpression::Object(record)) => Some(&**record),
                _ => None,
            };
            let host = record
                .and_then(|r| r.get("host"))
                .and_then(|v| literal_of(v, schema, orbital));
            let port = record
                .and_then(|r| r.get("port"))
                .and_then(|v| literal_of(v, schema, orbital));
            match (host, port) {
                (Some(SExpression::String(host)), Some(SExpression::Number(port)))
                    if port.fract() == 0.0 && (0.0..=65535.0).contains(port) =>
                {
                    (!grants.net_allows(host, *port as u16, NetKind::Listen)).then(|| {
                        (
                            ErrorCode::PageEndpointGrantMissing,
                            format!("'{}' endpoint {host}:{} has no covering `net` listen grant", spec.name, *port as u16),
                        )
                    })
                }
                _ => Some((
                    ErrorCode::PageAddressInvalid,
                    format!(
                        "'{}' endpoint host and port must be literals or `@config` knobs with a declared default, so the `net` listen grant can be checked",
                        spec.name
                    ),
                )),
            }
        }
    }
}

/// Protocol pages (EXEC-ENDPOINT-1/2): the protocol is a row of the table, the address is a record typed
/// against the row's address, and the row's grant covers it.
pub fn validate_protocol_pages(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.is_spec() {
            continue;
        }
        for (j, page_ref) in orbital.pages.iter().flatten().enumerate() {
            let Some(page) = page_ref.as_inline().filter(|p| !p.is_route()) else {
                continue;
            };
            let location = format!("orbitals[{i}].pages[{j}]");
            let table = protocols::table_after_preflight();
            let Some(spec) = table.iter().find(|p| p.name == page.protocol_name()).filter(|p| !p.viewer) else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageUnknownProtocol,
                        format!("{location}.protocol"),
                        format!(
                            "page '{}' names unknown protocol '{}'",
                            page.name,
                            page.protocol_name()
                        ),
                    )
                    .with_valid_values(
                        table
                            .iter()
                            .map(|p| p.name.clone())
                            .collect(),
                    ),
                );
                continue;
            };
            for problem in address_problems(spec, page, schema, orbital) {
                result.add_error(ValidationError::error(
                    ErrorCode::PageAddressInvalid,
                    format!("{location}.address"),
                    format!("page '{}': {problem}", page.name),
                ));
            }
            if let Some((code, problem)) = grant_problem(spec, page, schema, orbital) {
                result.add_error(
                    ValidationError::error(
                        code,
                        location,
                        format!("page '{}': {problem}", page.name),
                    )
                    .with_suggestion("Declare the grant in the app header, e.g. `net: { \"host:port\": [listen] }`"),
                );
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn href_obj(href: SExpression) -> SExpression {
        let mut m = std::collections::HashMap::new();
        m.insert("href".to_string(), href);
        SExpression::object(m.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    fn concat(parts: &[SExpression]) -> SExpression {
        let mut items = vec![SExpression::String(("str/concat".to_string()).into())];
        items.extend(parts.iter().cloned());
        SExpression::Array(items.into())
    }

    #[test]
    fn href_built_from_literal_strings_counts_as_that_path() {
        let mut out = HashSet::new();
        let expr = href_obj(concat(&[
            SExpression::string("/ar"),
            SExpression::string("/blog/x"),
        ]));
        collect_href_strings(&expr, &mut out);
        assert!(out.contains("/ar/blog/x"), "{out:?}");
    }

    #[test]
    fn control_a_plain_string_href_still_counts() {
        let mut out = HashSet::new();
        collect_href_strings(&href_obj(SExpression::string("/blog")), &mut out);
        assert!(out.contains("/blog"));
    }

    #[test]
    fn edge_a_concat_with_a_binding_is_not_a_literal_path() {
        let mut out = HashSet::new();
        let expr = href_obj(concat(&[
            SExpression::string("@config.pathPrefix"),
            SExpression::string("/blog"),
        ]));
        collect_href_strings(&expr, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn edge_a_concat_with_a_nested_expression_is_not_a_literal_path() {
        let mut out = HashSet::new();
        let nested = SExpression::Array(vec![
            SExpression::string("str/upper"),
            SExpression::string("/x"),
        ].into());
        collect_href_strings(
            &href_obj(concat(&[SExpression::string("/ar"), nested])),
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn edge_an_empty_concat_counts_as_nothing() {
        let mut out = HashSet::new();
        collect_href_strings(
            &href_obj(concat(&[SExpression::String(String::new().into())])),
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    // ------------------------------------------------------------------
    // B3-R: the OIR layer's own duplicate-PATH check is deleted
    // (`ir/validation.rs`); the std lint `page-path-uniqueness` owns the
    // param-shape collision.
    // ------------------------------------------------------------------

    fn schema_with_two_page_paths(path_a: &str, path_b: &str) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "test-page-path-convergence",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "WidgetOrbital",
                    "entity": {
                        "name": "Widget",
                        "persistence": "persistent",
                        "fields": [ { "name": "id", "type": "string", "required": true } ]
                    },
                    "traits": [
                        {
                            "name": "WidgetView",
                            "linkedEntity": "Widget",
                            "stateMachine": {
                                "states": [ { "name": "idle", "isInitial": true } ],
                                "events": [ { "key": "OPEN" } ],
                                "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN", "effects": [] } ]
                            }
                        }
                    ],
                    "pages": [
                        { "name": "ByIdPage", "path": path_a, "traits": [ { "ref": "WidgetView" } ] },
                        { "name": "BySlugPage", "path": path_b, "traits": [ { "ref": "WidgetView" } ] }
                    ]
                }
            ]
        }))
        .expect("page-path fixture must deserialize")
    }

    #[test]
    fn oir_layer_no_longer_emits_its_own_duplicate_path_error() {
        let schema = schema_with_two_page_paths("/tasks", "/tasks");
        // Page-path uniqueness is the std lint `std-orb-lint-page-path-uniqueness`
        // (the `orb validate` program), not a compiler pass. `resolve_to_oir` is
        // Phase 5 alone (no validation gate) — the direct route to an
        // `OirModule` for what's actually under test here: `OirValidator`
        // must emit NOTHING about the path (that block was deleted; only
        // duplicate-NAME stays).
        let module =
            crate::phases::resolve::resolve_to_oir(&schema).expect("schema resolves to OIR");
        let oir_result = orbital_core_base::ir::OirValidator::new(&module).validate();
        assert!(
            !oir_result
                .errors
                .iter()
                .any(|e| e.message.to_lowercase().contains("path")),
            "OIR layer must not report a path-duplicate finding anymore: {:#?}",
            oir_result.errors
        );
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use serde_json::{json, Value};

    fn trait_json(name: &str, event: &str, renders: bool) -> Value {
        let effects = if renders {
            json!([["render-ui", "main", {"type": "card"}]])
        } else {
            json!([])
        };
        json!({"name": name, "stateMachine": {
            "states": [{"name": "ready", "isInitial": true}],
            "transitions": [{"from": "ready", "to": "ready", "event": event, "effects": effects}]
        }})
    }

    fn orbital(name: &str, traits: Vec<Value>, pages: Value) -> Value {
        let mut orbital = json!({
            "name": name,
            "entity": {"name": format!("{name}Row"), "persistence": "runtime", "fields": [{"name": "id", "type": "string"}]},
            "traits": traits
        });
        if !pages.is_null() {
            orbital["pages"] = pages;
        }
        orbital
    }

    fn route(path: &str, traits: &[&str]) -> Value {
        let refs: Vec<Value> = traits.iter().map(|t| json!({"ref": t})).collect();
        json!({"name": format!("P{}", path.replace('/', "")), "path": path, "traits": refs})
    }

    fn endpoint(protocol: &str, address: Value, traits: &[&str]) -> Value {
        let refs: Vec<Value> = traits.iter().map(|t| json!({"ref": t})).collect();
        let mut page = json!({"name": format!("E{protocol}"), "path": "", "protocol": protocol, "traits": refs});
        if !address.is_null() {
            page["address"] = address;
        }
        page
    }

    fn grants() -> Value {
        json!({"net": [{"target": "0.0.0.0:8080", "kinds": ["listen"]}]})
    }

    fn run(orbitals: Vec<Value>, grants: Value) -> Vec<ErrorCode> {
        let mut schema = json!({"name": "p", "orbitals": orbitals});
        if !grants.is_null() {
            schema["osGrants"] = grants;
        }
        let schema: OrbitalSchema = serde_json::from_value(schema).unwrap();
        let mut ctx = ValidationContext::new();
        validate_all_pages(&schema, &mut ctx);
        ctx.result.errors.into_iter().map(|e| e.code).collect()
    }

    fn one(traits: Vec<Value>, pages: Value, grants: Value) -> Vec<ErrorCode> {
        run(vec![orbital("Main", traits, pages)], grants)
    }

    #[test]
    fn a_route_page_may_carry_a_trait_that_draws_nothing() {
        let page = json!([route("/t", &["V", "H"])]);
        let codes = one(
            vec![trait_json("V", "INIT", true), trait_json("H", "GO", false)],
            page,
            Value::Null,
        );
        assert!(
            !codes.contains(&ErrorCode::TraitMissingRenderUI),
            "{codes:?}"
        );
    }

    #[test]
    fn a_trait_embedded_by_a_route_trait_is_still_a_route_trait() {
        let host = json!({"name": "Host", "stateMachine": {
            "states": [{"name": "ready", "isInitial": true}],
            "transitions": [{"from": "ready", "to": "ready", "event": "INIT",
                "effects": [["render-ui", "main", {"type": "stack", "children": ["@trait.Child"]}]]}]
        }});
        let codes = one(
            vec![host.clone(), trait_json("Child", "GO", true)],
            json!([route("/t", &["Host"])]),
            Value::Null,
        );
        assert!(!codes.contains(&ErrorCode::TraitUnmounted), "{codes:?}");
        let codes = one(
            vec![host, trait_json("Child", "GO", false)],
            json!([route("/t", &["Host"])]),
            Value::Null,
        );
        assert!(!codes.contains(&ErrorCode::TraitUnmounted), "{codes:?}");
    }

    #[test]
    fn an_atom_with_no_pages_mounts_nothing_and_is_not_checked_for_interaction() {
        let codes = one(vec![trait_json("T", "GO", false)], Value::Null, Value::Null);
        for code in [ErrorCode::TraitUnmounted, ErrorCode::TraitMissingRenderUI] {
            assert!(!codes.contains(&code), "{code:?} in {codes:?}");
        }
    }

    fn server_trait() -> Vec<Value> {
        vec![trait_json("V", "INIT", true), trait_json("S", "GO", false)]
    }

    fn with_http(address: Value, grants: Value, config: Value) -> Vec<ErrorCode> {
        let mut main = orbital(
            "Main",
            server_trait(),
            json!([route("/t", &["V"]), endpoint("http", address, &["S"])]),
        );
        if !config.is_null() {
            main["config"] = config;
        }
        run(vec![main], grants)
    }

    fn literal_address() -> Value {
        json!({"host": "0.0.0.0", "port": 8080, "path": "/"})
    }

    #[test]
    fn an_http_endpoint_needs_a_covering_net_listen_grant() {
        let codes = with_http(literal_address(), Value::Null, Value::Null);
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        let codes = with_http(literal_address(), grants(), Value::Null);
        assert!(
            !codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        let other = json!({"net": [{"target": "0.0.0.0:9090", "kinds": ["listen"]}]});
        let codes = with_http(literal_address(), other, Value::Null);
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        let tcp = json!({"net": [{"target": "0.0.0.0:8080", "kinds": ["tcp"]}]});
        let codes = with_http(literal_address(), tcp, Value::Null);
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
    }

    #[test]
    fn an_http_address_reading_config_is_covered_by_the_grant_of_its_default() {
        let config = json!({"port": {"type": "number", "default": 8080}, "host": {"type": "string", "default": "0.0.0.0"}});
        let address = json!({"host": "@config.host", "port": "@config.port"});
        let codes = with_http(address.clone(), grants(), config.clone());
        assert!(
            !codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        assert!(!codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
        let codes = with_http(address, Value::Null, config);
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
    }

    #[test]
    fn an_address_knob_without_a_default_is_refused_so_the_grant_is_checked_exactly() {
        let config =
            json!({"port": {"type": "number"}, "host": {"type": "string", "default": "0.0.0.0"}});
        let codes = with_http(
            json!({"host": "@config.host", "port": "@config.port"}),
            grants(),
            config,
        );
        assert!(codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
        let codes = with_http(
            json!({"host": "0.0.0.0", "port": ["+", 8000, 80]}),
            grants(),
            Value::Null,
        );
        assert!(
            codes.contains(&ErrorCode::PageAddressInvalid),
            "a computed port cannot be checked against a grant: {codes:?}"
        );
        let codes = with_http(
            json!({"host": "0.0.0.0", "port": 9999}),
            grants(),
            Value::Null,
        );
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "control: an exact address outside the grant: {codes:?}"
        );
    }

    #[test]
    fn an_address_is_typed_against_the_protocol_row() {
        for bad in [
            json!({"host": "0.0.0.0", "port": "8080"}),
            json!({"port": 8080}),
            json!({"host": "0.0.0.0", "port": 8080, "extra": 1}),
        ] {
            let codes = with_http(bad.clone(), grants(), Value::Null);
            assert!(
                codes.contains(&ErrorCode::PageAddressInvalid),
                "{bad} {codes:?}"
            );
        }
        let config = json!({"port": {"type": "string", "default": "x"}});
        let codes = with_http(
            json!({"host": "0.0.0.0", "port": "@config.port"}),
            grants(),
            config,
        );
        assert!(codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
        let codes = with_http(
            json!({"host": "0.0.0.0", "port": 8080}),
            grants(),
            Value::Null,
        );
        assert!(!codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
    }

    #[test]
    fn a_protocol_without_an_address_takes_none() {
        let page = json!([
            route("/t", &["V"]),
            endpoint("boot", json!({"x": 1}), &["S"])
        ]);
        let codes = one(server_trait(), page, Value::Null);
        assert!(codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
        let page = json!([route("/t", &["V"]), endpoint("boot", Value::Null, &["S"])]);
        let codes = one(server_trait(), page, Value::Null);
        assert!(!codes.contains(&ErrorCode::PageAddressInvalid), "{codes:?}");
    }

    #[test]
    fn a_stdio_endpoint_needs_a_stdio_grant() {
        let page = json!([route("/t", &["V"]), endpoint("stdio", Value::Null, &["S"])]);
        let codes = one(server_trait(), page.clone(), Value::Null);
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        let codes = one(server_trait(), page.clone(), json!({"stdio": ["in"]}));
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "reading alone is not enough: {codes:?}"
        );
        let codes = one(
            server_trait(),
            page.clone(),
            json!({"stdio": ["out", "err"]}),
        );
        assert!(
            codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
        let codes = one(server_trait(), page, json!({"stdio": ["in", "out"]}));
        assert!(
            !codes.contains(&ErrorCode::PageEndpointGrantMissing),
            "{codes:?}"
        );
    }

    #[test]
    fn an_unknown_protocol_is_refused() {
        let page = json!([route("/t", &["V"]), endpoint("grpc", Value::Null, &["S"])]);
        let codes = one(server_trait(), page, Value::Null);
        assert!(codes.contains(&ErrorCode::PageUnknownProtocol), "{codes:?}");
        let page = json!([route("/t", &["V"]), endpoint("route", Value::Null, &["S"])]);
        let codes = one(server_trait(), page, Value::Null);
        assert!(codes.contains(&ErrorCode::PageUnknownProtocol), "{codes:?}");
        let page = json!([route("/t", &["V"]), endpoint("boot", Value::Null, &["S"])]);
        let codes = one(server_trait(), page, Value::Null);
        assert!(
            !codes.contains(&ErrorCode::PageUnknownProtocol),
            "{codes:?}"
        );
    }

}
