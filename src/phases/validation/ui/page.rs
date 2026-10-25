//! Page Validation
//!
//! Validates page structure, paths, and trait references.
//! Ported from `builder/packages/shared/src/orbitals/validation/page-validator.ts`.

use super::effect::{expr_return_incompatible_in_ctx, BindingTypes, ExprReturnCtx};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::protocols;
use orbital_core_base::schema::{OrbitalSchema, PageDefinition, SExpression, TraitReference};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use crate::phases::validation::closed_circuit::validate_all_circuits;
    use crate::phases::validation::trait_validator::validate_all_traits;
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

    fn http(path: &str, traits: &[&str]) -> Value {
        endpoint(
            "http",
            json!({"host": "0.0.0.0", "port": 8080, "path": path}),
            traits,
        )
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
        validate_all_traits(&schema, &mut ctx);
        validate_all_circuits(&schema, &mut ctx);
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
    fn a_trait_mounted_only_on_an_endpoint_renders_nothing() {
        let page = json!([route("/t", &["V"]), http("/", &["S"])]);
        let codes = one(
            vec![trait_json("V", "INIT", true), trait_json("S", "INIT", true)],
            page,
            grants(),
        );
        assert!(
            codes.contains(&ErrorCode::TraitEndpointRenders),
            "{codes:?}"
        );
        let page = json!([route("/t", &["V"]), http("/", &["S"])]);
        let codes = one(
            vec![trait_json("V", "INIT", true), trait_json("S", "GO", false)],
            page,
            grants(),
        );
        for code in [
            ErrorCode::TraitEndpointRenders,
            ErrorCode::TraitMissingRenderUI,
            ErrorCode::TraitUnmounted,
        ] {
            assert!(!codes.contains(&code), "{code:?} in {codes:?}");
        }
    }

    #[test]
    fn a_boot_trait_that_renders_errors() {
        let page = json!([route("/t", &["V"]), endpoint("boot", Value::Null, &["B"])]);
        let codes = one(
            vec![trait_json("V", "INIT", true), trait_json("B", "INIT", true)],
            page,
            Value::Null,
        );
        assert!(
            codes.contains(&ErrorCode::TraitEndpointRenders),
            "{codes:?}"
        );
    }

    #[test]
    fn one_trait_on_a_route_and_an_endpoint_errors() {
        let page = json!([route("/t", &["T"]), http("/", &["T"])]);
        let codes = one(vec![trait_json("T", "INIT", true)], page, grants());
        assert!(
            codes.contains(&ErrorCode::TraitOnRouteAndEndpoint),
            "{codes:?}"
        );
        assert!(
            !codes.contains(&ErrorCode::TraitEndpointRenders),
            "{codes:?}"
        );
        let page = json!([route("/t", &["T"]), route("/u", &["T"])]);
        let codes = one(vec![trait_json("T", "INIT", true)], page, Value::Null);
        assert!(
            !codes.contains(&ErrorCode::TraitOnRouteAndEndpoint),
            "{codes:?}"
        );
    }

    #[test]
    fn an_atom_with_no_pages_mounts_nothing_and_is_not_checked_for_interaction() {
        let codes = one(vec![trait_json("T", "GO", false)], Value::Null, Value::Null);
        for code in [ErrorCode::TraitUnmounted, ErrorCode::TraitMissingRenderUI] {
            assert!(!codes.contains(&code), "{code:?} in {codes:?}");
        }
    }

    #[test]
    fn a_program_trait_on_no_page_is_unmounted() {
        let two = run(
            vec![
                orbital("A", vec![trait_json("T", "INIT", false)], Value::Null),
                orbital(
                    "B",
                    vec![trait_json("U", "INIT", true)],
                    json!([route("/u", &["U"])]),
                ),
            ],
            Value::Null,
        );
        assert!(two.contains(&ErrorCode::TraitUnmounted), "{two:?}");
        let with_pages = one(
            vec![trait_json("T", "INIT", true), trait_json("U", "INIT", true)],
            json!([route("/t", &["T"])]),
            Value::Null,
        );
        assert!(
            with_pages.contains(&ErrorCode::TraitUnmounted),
            "{with_pages:?}"
        );
        let mounted = one(
            vec![trait_json("T", "INIT", true), trait_json("U", "INIT", true)],
            json!([route("/t", &["T", "U"])]),
            Value::Null,
        );
        assert!(!mounted.contains(&ErrorCode::TraitUnmounted), "{mounted:?}");
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

    #[test]
    fn only_routes_can_be_unreachable_and_the_boot_page_is_the_first_route() {
        let page = json!([http("/", &["S"]), route("/t", &["V"])]);
        let codes = one(server_trait(), page, grants());
        assert!(!codes.contains(&ErrorCode::PageUnreachable), "{codes:?}");
        let page = json!([route("/t", &["V"]), route("/u", &["V"])]);
        let codes = one(vec![trait_json("V", "INIT", true)], page, Value::Null);
        assert!(codes.contains(&ErrorCode::PageUnreachable), "{codes:?}");
    }
}

/// Validate all pages in a schema
pub fn validate_all_pages(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(validate_protocol_pages(schema));
    ctx.merge(validate_page_reachability(schema));
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
        SExpression::Array(items) => match items.split_first() {
            Some((SExpression::String(op), parts)) if op == "str/concat" => parts
                .iter()
                .map(|p| match p {
                    SExpression::String(s) if !s.starts_with('@') => Some(s.as_str()),
                    _ => None,
                })
                .collect::<Option<String>>()?,
            _ => return None,
        },
        _ => return None,
    };
    (!path.is_empty()).then_some(path)
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
                        out.insert(s.clone());
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
                                out.insert(prefix.clone());
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
    let config_uses = |config: Option<&HashMap<String, orbital_core_base::schema::ConfigField>>| {
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

/// `ORB_PAGE_UNREACHABLE`: a declared inline page nothing in the schema can
/// ever route a viewer to. Generalises the JS wiring lint's
/// `page-absent-from-nav`, which only read `href` inside object-array
/// entries and only ran when some navItems-shaped array existed anywhere.
/// Routes only (a protocol endpoint is reached by its address, not a link).
/// Here reachable = the schema's BOOT page (the first inline route page in
/// declaration order — what both shipped routers mount at the index route
/// when nothing else is requested; see `boot_position` below, never a bare
/// `path == "/"` special case), a parameterized page (any `:name` segment),
/// the target of any `navigate` effect anywhere in the schema, or a page
/// path found as a literal `href` value anywhere in trait config or a
/// rendered payload (scalar or array-entry — one structural walk covers
/// both). No opt-in gate: a schema with zero links anywhere correctly
/// reports every non-boot, non-param page as unreachable.
pub(crate) fn validate_page_reachability(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // The BOOT page — reachable by construction, whatever string its own
    // `path` carries — is the first inline page in schema-declaration order
    // (first orbital that declares one, its first page), never a bare
    // `path == "/"` special case (an author's first-declared page is
    // routinely NOT literally "/"). This is the actual rule both shipped
    // routers use for "no page requested, boot into the default":
    // - compiled path: `orbital-shell-typescript/src/backend/pages.rs:408`
    //   (`let first_page = &module.pages[0];` mounted as `<Route index>`,
    //   fed by `resolve.rs`'s `module.pages.push(...)` walk over
    //   `schema.orbitals[].pages` in declared order — same order this
    //   function's own double loop below walks).
    // - runtime path: `packages/almadar-runtime/src/resolver/schema-to-ir.ts:581`
    //   (`getPage` with no name falls back to `pages[0]`/the first entry of
    //   the `ir.pages` map, built in the same declaration order).
    let boot_position: Option<(usize, usize)> =
        schema.orbitals.iter().enumerate().find_map(|(i, orbital)| {
            orbital
                .pages
                .iter()
                .flatten()
                .position(|p| p.as_inline().is_some_and(|p| p.is_route()))
                .map(|j| (i, j))
        });

    let signals = collect_reachability_signals(schema);

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, page_ref) in orbital.pages.iter().flatten().enumerate() {
            let Some(page) = page_ref.as_inline().filter(|p| p.is_route()) else {
                continue;
            };
            if boot_position == Some((i, j)) {
                continue;
            }
            if page.path.split('/').any(|segment| segment.starts_with(':')) {
                continue;
            }
            if signals.hrefs.contains(&page.path) || signals.navigate_targets.contains(&page.path) {
                continue;
            }
            let reached_by_dynamic_prefix = signals
                .navigate_targets
                .iter()
                .any(|target| target.len() > 1 && page.path.starts_with(target.as_str()));
            if reached_by_dynamic_prefix {
                continue;
            }

            result.add_error(
                ValidationError::error(
                    ErrorCode::PageUnreachable,
                    format!("orbitals[{}].pages[{}]", i, j),
                    format!(
                        "page '{}' is declared but appears in no link, navItems entry or navigate effect",
                        page.path
                    ),
                )
                .with_suggestion(format!(
                    "add an href to a nav/footer/link component, or a navigate effect, targeting '{}', or remove the page if nothing should reach it",
                    page.path
                )),
            );
        }
    }

    // A lazy page is reached by the program's own links, or by the links inside a lazy page that is
    // itself reached; walk that to a fixpoint so an unreached page's links reach nothing.
    let mut hrefs = signals.hrefs.clone();
    let navigate_targets = signals.navigate_targets.clone();
    let lazy_pages: Vec<&orbital_core_base::schema::LazyPage> =
        schema.lazy_pages.iter().flatten().collect();
    let is_reached = |path: &str, hrefs: &HashSet<String>, targets: &HashSet<String>| {
        hrefs.contains(path)
            || targets.contains(path)
            || targets
                .iter()
                .any(|target| target.len() > 1 && path.starts_with(target.as_str()))
    };
    let mut expanded = vec![false; lazy_pages.len()];
    loop {
        let mut grew = false;
        for (k, lazy) in lazy_pages.iter().enumerate() {
            if expanded[k] || !is_reached(&lazy.path, &hrefs, &navigate_targets) {
                continue;
            }
            expanded[k] = true;
            for link in &lazy.links {
                grew |= hrefs.insert(link.clone());
            }
        }
        if !grew {
            break;
        }
    }

    for (k, lazy) in lazy_pages.iter().enumerate() {
        if lazy.path.split('/').any(|segment| segment.starts_with(':')) {
            continue;
        }
        let reached = is_reached(&lazy.path, &hrefs, &navigate_targets);
        if !reached {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageUnreachable,
                    format!("lazyPages[{k}]"),
                    format!(
                        "page '{}' is declared but appears in no link, navItems entry or navigate effect",
                        lazy.path
                    ),
                )
                .with_suggestion(format!(
                    "add an href to a nav/footer/link component, or a navigate effect, targeting '{}', or remove the `uses lazy` page if nothing should reach it",
                    lazy.path
                )),
            );
        }
    }

    result
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
                crate::phases::resolve::config_field_oir_type(field, schema),
            )
        })
        .collect();
    BindingTypes {
        payload: HashMap::new(),
        user: orbital_core_base::ir::OirType::Struct {
            additional: None,
            fields: Vec::new(),
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
    let Some(fields) = spec.address else {
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
        let Some(value) = record.get(field.name) else {
            if field.required {
                problems.push(format!(
                    "the '{}' address needs '{}'",
                    spec.name, field.name
                ));
            }
            continue;
        };
        let declared = orbital_core_base::type_compat::compat_kind_oir(field.ty);
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
                Some(SExpression::Object(record)) => Some(record),
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
            let Some(spec) = protocols::protocol(page.protocol_name()).filter(|p| !p.viewer) else {
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
                        protocols::PROTOCOLS
                            .iter()
                            .map(|p| p.name.to_string())
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
        SExpression::Object(m.into_iter().collect())
    }

    fn concat(parts: &[SExpression]) -> SExpression {
        let mut items = vec![SExpression::String("str/concat".to_string())];
        items.extend(parts.iter().cloned());
        SExpression::Array(items)
    }

    #[test]
    fn href_built_from_literal_strings_counts_as_that_path() {
        let mut out = HashSet::new();
        let expr = href_obj(concat(&[
            SExpression::String("/ar".into()),
            SExpression::String("/blog/x".into()),
        ]));
        collect_href_strings(&expr, &mut out);
        assert!(out.contains("/ar/blog/x"), "{out:?}");
    }

    #[test]
    fn control_a_plain_string_href_still_counts() {
        let mut out = HashSet::new();
        collect_href_strings(&href_obj(SExpression::String("/blog".into())), &mut out);
        assert!(out.contains("/blog"));
    }

    #[test]
    fn edge_a_concat_with_a_binding_is_not_a_literal_path() {
        let mut out = HashSet::new();
        let expr = href_obj(concat(&[
            SExpression::String("@config.pathPrefix".into()),
            SExpression::String("/blog".into()),
        ]));
        collect_href_strings(&expr, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn edge_a_concat_with_a_nested_expression_is_not_a_literal_path() {
        let mut out = HashSet::new();
        let nested = SExpression::Array(vec![
            SExpression::String("str/upper".into()),
            SExpression::String("/x".into()),
        ]);
        collect_href_strings(
            &href_obj(concat(&[SExpression::String("/ar".into()), nested])),
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn edge_an_empty_concat_counts_as_nothing() {
        let mut out = HashSet::new();
        collect_href_strings(
            &href_obj(concat(&[SExpression::String(String::new())])),
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

    // ------------------------------------------------------------------
    // Shard C: `ORB_PAGE_UNREACHABLE` — generalises the JS wiring lint's
    // `page-absent-from-nav`. Cases 1-5 mirror the JS test suite 1:1
    // (case 5's expectation is INVERTED — the JS opt-in gate is gone by
    // design); the last two are the new structural coverage (scalar href,
    // navigate-effect target) the generalisation adds.
    // ------------------------------------------------------------------

    fn schema_for_reachability(
        traits: serde_json::Value,
        pages: serde_json::Value,
    ) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "test-page-reachability",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "AppOrbital",
                    "entity": {
                        "name": "Widget",
                        "persistence": "persistent",
                        "fields": [ { "name": "id", "type": "string", "required": true } ]
                    },
                    "traits": traits,
                    "pages": pages
                }
            ]
        }))
        .expect("reachability fixture must deserialize")
    }

    fn with_lazy_pages(mut schema: OrbitalSchema, paths: &[&str]) -> OrbitalSchema {
        schema.lazy_pages = Some(
            paths
                .iter()
                .map(|p| orbital_core_base::schema::LazyPage {
                    path: p.to_string(),
                    orbital: "BlogPost".into(),
                    orb_ref: "lazy/BlogPost.orb".into(),
                    links: Vec::new(),
                })
                .collect(),
        );
        schema
    }

    /// Lazy pages from separate behaviors, each with the links its own resolved behavior carries.
    fn with_linked_lazy_pages(
        mut schema: OrbitalSchema,
        pages: &[(&str, &[&str])],
    ) -> OrbitalSchema {
        schema.lazy_pages = Some(
            pages
                .iter()
                .map(|(path, links)| orbital_core_base::schema::LazyPage {
                    path: path.to_string(),
                    orbital: format!("Doc{}", path.replace('/', "_")),
                    orb_ref: format!("lazy/Doc{}.orb", path.replace('/', "_")),
                    links: links.iter().map(|l| l.to_string()).collect(),
                })
                .collect(),
        );
        schema
    }

    #[test]
    fn a_lazy_page_linked_only_from_a_reached_lazy_page_is_reachable() {
        let schema = with_linked_lazy_pages(
            schema_for_reachability(
                layout_trait_with_nav_href_array(&["/", "/docs"]),
                serde_json::json!([
                    { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] }
                ]),
            ),
            &[
                ("/docs", &["/docs/intro"]),
                ("/docs/intro", &["/docs/traits"]),
                ("/docs/traits", &[]),
            ],
        );
        assert!(reachability_messages(&schema).is_empty());
    }

    #[test]
    fn control_links_inside_an_unreached_lazy_page_reach_nothing() {
        let schema = with_linked_lazy_pages(
            schema_for_reachability(
                layout_trait_with_nav_href_array(&["/"]),
                serde_json::json!([
                    { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] }
                ]),
            ),
            &[("/docs", &["/docs/intro"]), ("/docs/intro", &[])],
        );
        assert_eq!(
            reachability_messages(&schema),
            vec![
                "page '/docs' is declared but appears in no link, navItems entry or navigate effect".to_string(),
                "page '/docs/intro' is declared but appears in no link, navItems entry or navigate effect".to_string(),
            ]
        );
    }

    #[test]
    fn edge_a_lazy_page_linking_to_itself_does_not_reach_itself() {
        let schema = with_linked_lazy_pages(
            schema_for_reachability(
                layout_trait_with_nav_href_array(&["/"]),
                serde_json::json!([
                    { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] }
                ]),
            ),
            &[("/docs", &["/docs"])],
        );
        assert_eq!(reachability_messages(&schema).len(), 1);
    }

    #[test]
    fn a_lazy_page_nothing_links_to_is_unreachable() {
        let schema = with_lazy_pages(
            schema_for_reachability(
                layout_trait_with_nav_href_array(&["/"]),
                serde_json::json!([
                    { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] }
                ]),
            ),
            &["/blog/why"],
        );
        assert_eq!(
            reachability_messages(&schema),
            vec!["page '/blog/why' is declared but appears in no link, navItems entry or navigate effect".to_string()]
        );
    }

    #[test]
    fn control_a_linked_lazy_page_is_reachable() {
        let schema = with_lazy_pages(
            schema_for_reachability(
                layout_trait_with_nav_href_array(&["/", "/blog/why"]),
                serde_json::json!([
                    { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] }
                ]),
            ),
            &["/blog/why"],
        );
        assert!(reachability_messages(&schema).is_empty());
    }

    #[test]
    fn the_declared_sign_in_route_is_reachable_through_the_access_host() {
        let pages = serde_json::json!([
            { "name": "HomePage", "path": "/", "traits": [ { "ref": "AppLayout" } ] },
            { "name": "SignInPage", "path": "/welcome", "traits": [ { "ref": "AppLayout" } ] }
        ]);
        let mut schema =
            schema_for_reachability(layout_trait_with_nav_href_array(&["/"]), pages.clone());
        assert_eq!(
            reachability_messages(&schema).len(),
            1,
            "control: undeclared it is unreachable"
        );
        schema.site = Some(orbital_core_base::schema::SiteMeta {
            sign_in: Some("/welcome".into()),
            ..Default::default()
        });
        assert!(reachability_messages(&schema).is_empty());
    }

    fn reachability_messages(schema: &OrbitalSchema) -> Vec<String> {
        validate_page_reachability(schema)
            .errors
            .into_iter()
            .filter(|e| e.code == ErrorCode::PageUnreachable)
            .map(|e| e.message)
            .collect()
    }

    fn layout_trait_with_nav_href_array(hrefs: &[&str]) -> serde_json::Value {
        let items: Vec<serde_json::Value> = hrefs
            .iter()
            .map(|h| serde_json::json!({ "href": h, "label": "Nav" }))
            .collect();
        serde_json::json!([{
            "name": "AppLayout",
            "linkedEntity": "Widget",
            "config": { "navItems": { "type": "array", "default": items } },
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }])
    }

    fn page(name: &str, path: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "path": path, "traits": [{ "ref": "AppLayout" }] })
    }

    fn layout_trait_with_pages_sigil() -> serde_json::Value {
        serde_json::json!([{
            "name": "AppLayout",
            "linkedEntity": "Widget",
            "config": { "navItems": { "type": "array", "default": "@pages" } },
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }])
    }

    fn labelled_page(name: &str, path: &str, label: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "path": path, "label": label, "traits": [{ "ref": "AppLayout" }] })
    }

    #[test]
    fn a_labelled_page_is_reached_through_the_pages_sigil() {
        let schema = schema_for_reachability(
            layout_trait_with_pages_sigil(),
            serde_json::json!([
                labelled_page("DashboardPage", "/dashboard", "Dashboard"),
                labelled_page("StaffPage", "/staff", "Staff")
            ]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn control_a_page_without_a_label_is_not_reached_through_the_pages_sigil() {
        let schema = schema_for_reachability(
            layout_trait_with_pages_sigil(),
            serde_json::json!([
                labelled_page("DashboardPage", "/dashboard", "Dashboard"),
                page("StaffPage", "/staff")
            ]),
        );
        let messages = reachability_messages(&schema);
        assert_eq!(messages.len(), 1, "{:?}", messages);
        assert!(messages[0].contains("/staff"), "{:?}", messages);
    }

    #[test]
    fn control_a_label_without_any_pages_sigil_reaches_nothing() {
        let schema = schema_for_reachability(
            layout_trait_with_nav_href_array(&["/dashboard"]),
            serde_json::json!([
                labelled_page("DashboardPage", "/dashboard", "Dashboard"),
                labelled_page("StaffPage", "/staff", "Staff")
            ]),
        );
        let messages = reachability_messages(&schema);
        assert_eq!(messages.len(), 1, "{:?}", messages);
        assert!(messages[0].contains("/staff"), "{:?}", messages);
    }

    #[test]
    fn fires_for_a_page_reachable_by_no_link_navitem_or_navigate_target() {
        let schema = schema_for_reachability(
            layout_trait_with_nav_href_array(&["/dashboard"]),
            serde_json::json!([
                page("DashboardPage", "/dashboard"),
                page("StaffPage", "/staff")
            ]),
        );
        let messages = reachability_messages(&schema);
        assert_eq!(messages.len(), 1, "{:?}", messages);
        assert!(messages[0].contains("/staff"), "{:?}", messages);
    }

    #[test]
    fn silent_once_the_page_has_an_entry_in_a_navitems_array() {
        let schema = schema_for_reachability(
            layout_trait_with_nav_href_array(&["/dashboard", "/staff"]),
            serde_json::json!([
                page("DashboardPage", "/dashboard"),
                page("StaffPage", "/staff")
            ]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn does_not_flag_a_parameterized_detail_page() {
        let schema = schema_for_reachability(
            layout_trait_with_nav_href_array(&["/staff"]),
            serde_json::json!([
                page("StaffPage", "/staff"),
                page("StaffDetailPage", "/staff/:id")
            ]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn does_not_flag_the_app_root_page() {
        let schema = schema_for_reachability(
            layout_trait_with_nav_href_array(&[]),
            serde_json::json!([page("HomePage", "/")]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn fires_even_when_no_navitems_array_exists_anywhere_no_opt_in_gate() {
        // JS required at least one navItems-shaped array before it would
        // check anything ("has not opted into the convention"); the Rust
        // generalisation has no such gate — a MULTI-page app with zero links
        // anywhere still owes every non-root, non-param page a real route.
        // (A single-page schema is exempt — see
        // `silent_for_a_schema_with_only_one_page_no_second_page_to_link_from`
        // below — so this fixture needs a second page to isolate the claim.)
        let traits = serde_json::json!([{
            "name": "Directory",
            "linkedEntity": "Widget",
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }]);
        let schema = schema_for_reachability(
            traits,
            serde_json::json!([
                { "name": "HomePage", "path": "/", "traits": [{ "ref": "Directory" }] },
                { "name": "StaffPage", "path": "/staff", "traits": [{ "ref": "Directory" }] }
            ]),
        );
        let messages = reachability_messages(&schema);
        assert_eq!(messages.len(), 1, "{:?}", messages);
        assert!(messages[0].contains("/staff"), "{:?}", messages);
    }

    #[test]
    fn silent_for_a_scalar_href_not_nested_in_an_array() {
        // `{ type: "case-study-card", href: "/about", ... }` — a scalar
        // `href` prop directly on a rendered pattern object, not inside an
        // array-of-descriptors. The OLD JS `descriptorEvents(node, 'href')`
        // walk only extracted `href` from ARRAY entries, so this shape
        // (`patterns-registry.json`'s `case-study-card`/`feature-card`/
        // `showcase-card` prop) was invisible to it.
        let traits = serde_json::json!([{
            "name": "FeatureCard",
            "linkedEntity": "Widget",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [
                        ["render-ui", "main", { "type": "case-study-card", "href": "/about", "title": "About" }]
                    ] }
                ]
            }
        }]);
        let schema = schema_for_reachability(
            traits,
            // A second (boot) page so `/about` isn't trivially exempt as
            // the schema's first-declared page — its silence must come
            // from the scalar href, the thing under test.
            serde_json::json!([
                { "name": "HomePage", "path": "/home", "traits": [{ "ref": "FeatureCard" }] },
                { "name": "AboutPage", "path": "/about", "traits": [{ "ref": "FeatureCard" }] }
            ]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    fn layout_trait_with_user_menu(items: serde_json::Value) -> serde_json::Value {
        serde_json::json!([{
            "name": "AppLayout",
            "linkedEntity": "Widget",
            "config": { "userMenuItems": { "type": "array", "default": items } },
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }])
    }

    #[test]
    fn silent_for_a_page_reached_only_by_an_action_navigates_to() {
        // `userMenuItems: [{ label, navigatesTo: "/account" }]` — an action
        // entry the UI turns into `UI:NAVIGATE`, so it links its target.
        let schema = schema_for_reachability(
            layout_trait_with_user_menu(
                serde_json::json!([{ "label": "Account", "navigatesTo": "/account" }]),
            ),
            serde_json::json!([page("HomePage", "/"), page("AccountPage", "/account")]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn control_an_action_without_navigates_to_reaches_nothing() {
        let schema = schema_for_reachability(
            layout_trait_with_user_menu(
                serde_json::json!([{ "label": "Account", "event": "OPEN_ACCOUNT" }]),
            ),
            serde_json::json!([page("HomePage", "/"), page("AccountPage", "/account")]),
        );
        assert_eq!(reachability_messages(&schema).len(), 1);
    }

    #[test]
    fn silent_for_a_page_reached_only_by_a_navigate_effect() {
        let traits = serde_json::json!([{
            "name": "ReportsButton",
            "linkedEntity": "Widget",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "OPEN_REPORTS" }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "OPEN_REPORTS", "effects": [["navigate", "/reports"]] }
                ]
            }
        }]);
        let schema = schema_for_reachability(
            traits,
            // A second (boot) page so `/reports` isn't trivially exempt as
            // the schema's first-declared page.
            serde_json::json!([
                { "name": "HomePage", "path": "/home", "traits": [{ "ref": "ReportsButton" }] },
                { "name": "ReportsPage", "path": "/reports", "traits": [{ "ref": "ReportsButton" }] }
            ]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }

    #[test]
    fn silent_for_a_schema_with_only_one_page_it_is_the_boot_page() {
        // Every declared page IS the schema's first (and only) page, so it
        // is the boot page the shipped router mounts at its index route
        // regardless of its own literal path — the overwhelming majority
        // of the pattern registry is exactly this shape: one orbital, one
        // page, no shell, no nav, never a multi-page app a viewer routes
        // through (measured against the release binary: without this rule,
        // 1094/1226 registry files — almost entirely single-page atoms —
        // spuriously failed `orb validate`, swamping the ~20 files the
        // check exists to catch).
        let traits = serde_json::json!([{
            "name": "Directory",
            "linkedEntity": "Widget",
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }]);
        let schema = schema_for_reachability(
            traits,
            serde_json::json!([{ "name": "StaffPage", "path": "/staff", "traits": [{ "ref": "Directory" }] }]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }
}
