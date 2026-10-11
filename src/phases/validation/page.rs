//! Page Validation
//!
//! Validates page structure, paths, and trait references.
//! Ported from `builder/packages/shared/src/orbitals/validation/page-validator.ts`.

use super::{
    is_std_trait, ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_VIEW_TYPES,
};
use orbital_core::schema::{OrbitalSchema, PageDefinition, SExpression, TraitReference};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Regex for valid page paths (must start with /)
static PATH_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/[a-zA-Z0-9\-_/:]*$").unwrap());

/// Regex for valid page names (PascalCase)
static NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Validate all pages in a schema
pub fn validate_all_pages(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        // Check if orbital has pages defined
        let has_pages = orbital
            .pages
            .as_ref()
            .map(|p| !p.is_empty())
            .unwrap_or(false);

        if !has_pages {
            ctx.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalMissingPages,
                    format!("orbitals[{}]", i),
                    format!(
                        "Orbital '{}' has no pages defined - traits won't be accessible",
                        orbital.name
                    ),
                )
                .with_suggestion(
                    "Add a pages array with at least one page to expose the orbital's traits",
                ),
            );
        }

        if let Some(pages) = &orbital.pages {
            for (j, page_ref) in pages.iter().enumerate() {
                let path = format!("orbitals[{}].pages[{}]", i, j);
                // Only validate inline pages, not references (references are validated during resolution)
                if let Some(page) = page_ref.as_inline() {
                    let check_naming = ctx.schema_language.should_check_ascii_naming();
                    let result =
                        validate_page(page, &path, &ctx.trait_names, &ctx.trait_ids, check_naming);
                    ctx.merge(result);
                }
            }
        }
    }

    ctx.merge(validate_page_path_uniqueness(schema));
    ctx.merge(validate_page_reachability(schema));
    ctx.merge(validate_page_modifiers(schema));
}

fn inline_pages_of(schema: &OrbitalSchema) -> Vec<&PageDefinition> {
    schema
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .filter_map(|p| p.as_inline())
        .collect()
}

/// No page may sit on the host's own sign-in route, and the app's `signIn` route
/// is either that route or a page of the WHOLE program (its eager pages or its
/// recorded `uses lazy` pages) that anonymous viewers can open.
/// Run on the top-level program after its lazy children are recorded — a lazy
/// child alone cannot see the route.
pub fn validate_sign_in_route(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::{is_host_sign_in_route, PageAccess, HOST_SIGN_IN_ROUTE};
    let mut result = ValidationResult::new();
    let reserved = inline_pages_of(schema)
        .into_iter()
        .map(|p| p.path.clone())
        .chain(schema.lazy_pages.iter().flatten().map(|p| p.path.clone()))
        .filter(|path| is_host_sign_in_route(path));
    for path in reserved {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageHostReservedRoute,
                path.clone(),
                format!("'{path}' is the host's own sign-in route; no page may be declared on it"),
            )
            .with_suggestion(format!(
                "move this page to another route; name `signIn: \"{HOST_SIGN_IN_ROUTE}\"` to send viewers to the host's sign-in"
            )),
        );
    }
    let Some(sign_in) = schema.site.as_ref().and_then(|s| s.sign_in.as_ref()) else {
        return result;
    };
    if is_host_sign_in_route(sign_in) {
        return result;
    }
    let target = normalize_page_path(sign_in);
    let page = inline_pages_of(schema)
        .into_iter()
        .find(|p| normalize_page_path(&p.path) == target);
    let lazy = schema
        .lazy_pages
        .iter()
        .flatten()
        .any(|p| normalize_page_path(&p.path) == target);
    let problem = match page {
        Some(p) if p.access == Some(PageAccess::Authenticated) => Some(format!(
            "the `signIn` route '{sign_in}' is itself `access: authenticated`"
        )),
        Some(_) => None,
        None if lazy => None,
        None => Some(format!(
            "the `signIn` route '{sign_in}' is no page of this app"
        )),
    };
    if let Some(message) = problem {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageSignInUnknownRoute,
                "site.signIn".to_string(),
                message,
            )
            .with_suggestion("point `signIn:` at a page anonymous viewers can open"),
        );
    }
    result
}

/// Page `access` / `indexing` declarations: an indexable page is public,
/// titled and described (the deploying app's `origin` is checked when its
/// route manifest is built — a reusable page atom has none of its own); an
/// authenticated page has a `signIn` route to send a non-admitted viewer to.
pub fn validate_page_modifiers(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::{PageAccess, PageIndexing};
    let mut result = ValidationResult::new();
    let site = schema.site.clone().unwrap_or_default();
    let pages: Vec<(String, &PageDefinition)> = schema
        .orbitals
        .iter()
        .enumerate()
        .flat_map(|(i, o)| {
            o.pages
                .iter()
                .flatten()
                .enumerate()
                .filter_map(move |(j, p)| {
                    p.as_inline()
                        .map(|p| (format!("orbitals[{i}].pages[{j}]"), p))
                })
        })
        .collect();
    for (location, page) in &pages {
        if page.indexing == Some(PageIndexing::Index) {
            let mut missing = Vec::new();
            if page.access != Some(PageAccess::Public) {
                missing.push("`access: public`");
            }
            if page.title.is_none() {
                missing.push("`title:`");
            }
            if page.description.is_none() {
                missing.push("`description:`");
            }
            if !missing.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageIndexRequiresPublic,
                        location.clone(),
                        format!(
                            "page '{}' ({}) declares `indexing: index` but lacks {}",
                            page.name,
                            page.path,
                            missing.join(", ")
                        ),
                    )
                    .with_suggestion(
                        "an indexed page is public, titled and described; otherwise declare `indexing: noindex`",
                    ),
                );
            }
            if page.path.split('/').any(|seg| seg.starts_with(':')) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageIndexParamRoute,
                        location.clone(),
                        format!(
                            "page '{}' ({}) is a parameter route; it cannot be `indexing: index` because its concrete URLs are not declared",
                            page.name, page.path
                        ),
                    )
                    .with_suggestion("declare `indexing: noindex` on parameter routes"),
                );
            }
        }
        if page.access == Some(PageAccess::Authenticated) && site.sign_in.is_none() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageAuthNoSignIn,
                    location.clone(),
                    format!(
                        "page '{}' ({}) is `access: authenticated` but the app declares no `signIn:` route",
                        page.name, page.path
                    ),
                )
                .with_suggestion("add `signIn: \"/<route>\"` to the app bracket — the page a non-admitted viewer is sent to"),
            );
        }
    }

    let locales = schema.locales.clone().unwrap_or_default();
    for (group, locale, paths) in orbital_core::messages::duplicate_translation_locales(
        pages
            .iter()
            .map(|(_, p)| (p.path.as_str(), p.translation_of.as_deref())),
        &locales,
    ) {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageTranslationDuplicateLocale,
                paths[1].clone(),
                format!(
                    "translation group '{group}' holds {} pages in locale '{locale}' ({}); each locale may hold one",
                    paths.len(),
                    paths.join(", ")
                ),
            )
            .with_suggestion("give each page its own locale prefix, or a different `translationOf:`"),
        );
    }
    result
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
            if let Some(path) = map.get("href").and_then(literal_href) {
                out.insert(path);
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
            for page in orbital.pages.iter().flatten().filter_map(|p| p.as_inline()) {
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
    let config_uses =
        |config: Option<&HashMap<String, orbital_core::schema::ConfigField>>| {
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
/// Here reachable = the schema's BOOT page (the first inline page in
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
                .position(|p| p.as_inline().is_some())
                .map(|j| (i, j))
        });

    let signals = collect_reachability_signals(schema);

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, page_ref) in orbital.pages.iter().flatten().enumerate() {
            let Some(page) = page_ref.as_inline() else {
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
    let mut navigate_targets = signals.navigate_targets.clone();
    let lazy_pages: Vec<&orbital_core::schema::LazyPage> =
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

/// Validate a single page definition
pub fn validate_page(
    page: &PageDefinition,
    path: &str,
    known_traits: &HashSet<String>,
    known_trait_ids: &HashSet<String>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Page must have a name
    if page.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageMissingName,
                format!("{}.name", path),
                "Page name is required",
            )
            .with_suggestion(
                "Add a PascalCase name for the page (e.g., 'TaskList', 'UserProfile')",
            ),
        );
    } else if check_naming && !NAME_REGEX.is_match(&page.name) {
        // GAP-V001: Skip ASCII naming check for non-Latin schemas
        result.add_error(
            ValidationError::error(
                ErrorCode::PageMissingName,
                format!("{}.name", path),
                format!("Page name '{}' should be PascalCase", page.name),
            )
            .with_suggestion("Use PascalCase for page names (e.g., 'TaskList', 'Dashboard')"),
        );
    }

    // Page must have a path
    if page.path.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageMissingPath,
                format!("{}.path", path),
                "Page path is required",
            )
            .with_suggestion("Add a path starting with / (e.g., '/tasks', '/users/:id')"),
        );
    } else {
        // Path must start with /
        if !page.path.starts_with('/') {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Page path must start with '/': '{}'", page.path),
                )
                .with_suggestion("Prefix the path with / (e.g., '/tasks' instead of 'tasks')"),
            );
        }

        // Path format validation (skip for non-Latin schemas — Arabic/etc. paths are valid)
        if check_naming && !PATH_REGEX.is_match(&page.path) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Invalid page path format: '{}'", page.path),
                )
                .with_suggestion("Use valid path characters: letters, numbers, hyphens, underscores, and colons for params"),
            );
        }

        // Reject paths starting with /api (collides with Vite dev server proxy)
        if page.path.starts_with("/api") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Page path '{}' starts with '/api' which collides with the API server proxy", page.path),
                )
                .with_suggestion("Choose a different path that doesn't start with '/api' (e.g., '/requests' instead of '/apirequests')"),
            );
        }
    }

    // Page must have traits (trait-driven UI)
    if let Some(traits) = &page.traits {
        if traits.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageEmptyTraits,
                    format!("{}.traits", path),
                    "Page traits array cannot be empty",
                )
                .with_suggestion("Add at least one trait to render UI on the page"),
            );
        }

        // Validate each trait reference
        for (i, trait_ref) in traits.iter().enumerate() {
            let trait_name = trait_ref.trait_name().unwrap_or("<anonymous>");
            let trait_path = format!("{}.traits[{} ({})]", path, i, trait_name);
            let trait_result =
                validate_page_trait_ref(trait_ref, &trait_path, known_traits, known_trait_ids);
            result.merge(trait_result);
        }
    } else {
        // Traits array is optional for pages that use primaryEntity + viewType
        // Only warn if neither traits nor primaryEntity is specified
        if page.primary_entity.is_none() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageMissingTraits,
                    format!("{}.traits", path),
                    "Page should have traits array for trait-driven UI",
                )
                .with_suggestion("Add traits: ['TraitName'] to define page behavior"),
            );
        }
    }

    // Validate view type if specified
    if let Some(view_type) = &page.view_type {
        if !VALID_VIEW_TYPES.contains(&view_type.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidViewType,
                    format!("{}.viewType", path),
                    format!("Invalid view type: '{}'", view_type),
                )
                .with_valid_values(VALID_VIEW_TYPES.iter().map(|s| s.to_string()).collect()),
            );
        }
    }

    // Check for deprecated static sections
    // This is checked via the PageDefinition type - if it has sections, warn
    // Note: The Rust schema type may not have sections field, but we check just in case

    result
}

/// Validate a page trait reference
fn validate_page_trait_ref(
    trait_ref: &TraitReference,
    path: &str,
    known_traits: &HashSet<String>,
    known_trait_ids: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match trait_ref {
        TraitReference::Inline(trait_def) => {
            // Inline traits are validated separately
            // Here we just ensure it has a name
            if trait_def.name.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageInvalidTraitRef,
                        path,
                        "Inline trait must have a name",
                    )
                    .with_suggestion("Add a name property to the inline trait"),
                );
            }
        }
        TraitReference::Reference {
            reference,
            name,
            ref_id,
            ..
        } => {
            // V4 id-first: when the ref_id resolves to a known inline trait, the
            // ref is valid regardless of its `reference` name — which may be
            // stale after a cross-orbital dedup rename (the id is stable). This
            // is what lets `rewrite_page_trait_refs` be deleted.
            let id_resolves = ref_id
                .as_ref()
                .is_some_and(|id| known_trait_ids.contains(id));
            let trait_name = reference.as_ref().or(name.as_ref());

            if id_resolves {
                // ok — id carries the edge
            } else if let Some(name) = trait_name {
                if !known_traits.contains(name) && !is_std_trait(name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::PageInvalidTraitRef,
                            path,
                            format!("Trait '{}' referenced on page does not exist", name),
                        )
                        .with_suggestion("Define traits inline within orbitals or use a std trait"),
                    );
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageInvalidTraitRef,
                        path,
                        "Page trait reference must have 'ref' or 'name' property",
                    )
                    .with_suggestion("Add ref: 'TraitName' to reference a trait"),
                );
            }
        }
    }

    result
}

/// Collapse a page path to its PARAM SHAPE: every `:name` segment becomes a
/// bare `:`, so `/x/:id` and `/x/:slug` normalize to the same key — a router
/// can't tell them apart at dispatch time regardless of the param's name.
pub fn normalize_page_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with(':') {
                ":"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Check for duplicate page paths across all orbitals, keyed on the
/// PARAM-SHAPE-normalized path (`normalize_page_path`) — an exact string
/// collision is one instance of a param-shape collision, so one check
/// covers both instead of the OIR layer separately re-flagging exact dups.
pub fn validate_page_path_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut normalized_to_pages: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(pages) = &orbital.pages {
            for (j, page_ref) in pages.iter().enumerate() {
                let location = format!("orbitals[{}].pages[{}]", i, j);
                // Only check inline pages for path uniqueness
                if let Some(page) = page_ref.as_inline() {
                    normalized_to_pages
                        .entry(normalize_page_path(&page.path))
                        .or_default()
                        .push((location, page.path.clone()));
                }
            }
        }
    }

    for (normalized, entries) in normalized_to_pages {
        if entries.len() > 1 {
            let locations: Vec<&str> = entries.iter().map(|(l, _)| l.as_str()).collect();
            let paths: Vec<&str> = entries.iter().map(|(_, p)| p.as_str()).collect();
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageDuplicatePath,
                    locations[0].to_string(),
                    format!(
                        "Duplicate page path (normalized to '{}'): {:?} found at: {:?}",
                        normalized, paths, locations
                    ),
                )
                .with_suggestion(
                    "Each page must have a unique path — two routes that only differ by a param NAME still collide at dispatch",
                ),
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_page() -> PageDefinition {
        PageDefinition {
            translation_of: None,
            source_page: None,
            access: None,
            indexing: None,
            title: None,
            description: None,
            name: "TaskList".to_string(),
            path: "/tasks".to_string(),
            traits: Some(vec![TraitReference::Reference {
                local: false,
                type_args: None,
                id: None,
                reference: Some("std/List".to_string()),
                name: None,
                linked_entity: None,
                config: None,
                events: None,
                fields: None,
                listens: None,
                emits_scope: None,
                event_ids: None,
                linked_entity_id: None,
                ref_id: None,
            }]),
            primary_entity: None,
            view_type: None,
            is_initial: None,
            sections: None,
            icon: None,
            label: None,
            id: None,
            roles: None,
        }
    }

    /// A page whose single trait ref carries `reference` (name) + optional
    /// `ref_id`. Used to prove the V4 id-first page-ref check.
    fn page_with_ref(reference: &str, ref_id: Option<&str>) -> PageDefinition {
        let mut p = create_valid_page();
        p.traits = Some(vec![TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some(reference.to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: ref_id.map(str::to_string),
        }]);
        p
    }

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

    #[test]
    fn test_valid_page() {
        let page = create_valid_page();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn page_trait_ref_valid_via_ref_id_when_name_stale() {
        // V4: after a molecule/dedup rename, a page ref keeps the OLD name but a
        // STABLE ref_id. With the name absent from `known_traits` but the id in
        // `known_trait_ids`, the ref is valid — the id carries the edge. This is
        // what replaced the deleted `rewrite_page_trait_refs` name rewrite.
        let page = page_with_ref("StaleOldName", Some("trt_current0000000000000000"));
        let known_traits = HashSet::new(); // stale name is NOT here
        let known_trait_ids = HashSet::from(["trt_current0000000000000000".to_string()]);
        let result = validate_page(&page, "test", &known_traits, &known_trait_ids, true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::PageInvalidTraitRef),
            "id-resolving page ref must not fire PageInvalidTraitRef: {:?}",
            result.errors
        );
    }

    #[test]
    fn page_trait_ref_invalid_when_neither_name_nor_id_resolves() {
        // Neither the name nor the ref_id resolves — a genuine dangling ref.
        let page = page_with_ref("Ghost", Some("trt_ghost000000000000000000"));
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::PageInvalidTraitRef),
            "unresolvable page ref must fire PageInvalidTraitRef"
        );
    }

    #[test]
    fn test_page_missing_name() {
        let mut page = create_valid_page();
        page.name = String::new();

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageMissingName));
    }

    #[test]
    fn test_page_missing_path() {
        let mut page = create_valid_page();
        page.path = String::new();

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageMissingPath));
    }

    #[test]
    fn test_page_invalid_path_no_slash() {
        let mut page = create_valid_page();
        page.path = "tasks".to_string(); // Missing leading /

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidPath));
    }

    #[test]
    fn test_page_empty_traits() {
        let mut page = create_valid_page();
        page.traits = Some(vec![]);

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageEmptyTraits));
    }

    #[test]
    fn test_page_invalid_trait_ref() {
        let mut page = create_valid_page();
        page.traits = Some(vec![TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some("NonExistentTrait".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        }]);

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidTraitRef));
    }

    #[test]
    fn test_page_std_trait_ref() {
        let page = create_valid_page(); // Uses std/List

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(result.is_valid()); // std traits are always valid
    }

    #[test]
    fn test_page_invalid_view_type() {
        let mut page = create_valid_page();
        page.view_type = Some("invalid".to_string());

        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidViewType));
    }

    // ------------------------------------------------------------------
    // B3-R: page-path convergence — param-shape normalization
    // (`normalize_page_path`), wired through `validate_all_pages` /
    // `validate_comprehensive_full`; the OIR layer's own duplicate-PATH
    // check is deleted (`ir/validation.rs`), so an exact collision now
    // fires exactly ONCE, from here.
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
                            "category": "interaction",
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

    fn duplicate_path_errors(schema: &OrbitalSchema) -> Vec<ErrorCode> {
        crate::phases::validate_comprehensive_full(
            schema,
            None,
            crate::phases::validation::SchemaLanguage::English,
            crate::embedded::ServiceRegistryData::default(),
            Vec::new(),
        )
        .errors
        .into_iter()
        .filter(|e| e.code == ErrorCode::PageDuplicatePath)
        .map(|e| e.code)
        .collect()
    }

    #[test]
    fn param_shape_collision_fires_once_through_validate_comprehensive_full() {
        let schema = schema_with_two_page_paths("/x/:id", "/x/:slug");
        assert_eq!(
            duplicate_path_errors(&schema).len(),
            1,
            "'/x/:id' and '/x/:slug' normalize to the same param shape ('/x/:') and must collide exactly once"
        );
    }

    #[test]
    fn different_param_shapes_do_not_collide() {
        let schema = schema_with_two_page_paths("/x/:id", "/y/:id");
        assert!(
            duplicate_path_errors(&schema).is_empty(),
            "'/x/:id' and '/y/:id' normalize to different shapes ('/x/:' vs '/y/:') and must not collide"
        );
    }

    #[test]
    fn oir_layer_no_longer_emits_its_own_duplicate_path_error() {
        let schema = schema_with_two_page_paths("/tasks", "/tasks");
        // The compiler-phase check still catches the exact collision.
        assert_eq!(
            duplicate_path_errors(&schema).len(),
            1,
            "exact duplicate must still fire once"
        );

        // `phases/validate.rs`'s own compile-time gate now calls this SAME
        // `validate_page_path_uniqueness` owner (converged — no more second
        // copy), so `compile_schema` correctly refuses this schema outright;
        // going through it would never reach OIR at all. `resolve_to_oir` is
        // Phase 5 alone (no validation gate) — the direct route to an
        // `OirModule` for what's actually under test here: `OirValidator`
        // must emit NOTHING about the path (that block was deleted; only
        // duplicate-NAME stays).
        let module =
            crate::phases::resolve::resolve_to_oir(&schema).expect("schema resolves to OIR");
        let oir_result = orbital_core::ir::OirValidator::new(&module).validate();
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
                .map(|p| orbital_core::schema::LazyPage {
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
                .map(|(path, links)| orbital_core::schema::LazyPage {
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
        schema.site = Some(orbital_core::schema::SiteMeta {
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
            "category": "interaction",
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
            "category": "interaction",
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
            "category": "interaction",
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
            "category": "interaction",
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

    #[test]
    fn silent_for_a_page_reached_only_by_a_navigate_effect() {
        let traits = serde_json::json!([{
            "name": "ReportsButton",
            "linkedEntity": "Widget",
            "category": "interaction",
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
            "category": "interaction",
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] }
        }]);
        let schema = schema_for_reachability(
            traits,
            serde_json::json!([{ "name": "StaffPage", "path": "/staff", "traits": [{ "ref": "Directory" }] }]),
        );
        assert_eq!(reachability_messages(&schema), Vec::<String>::new());
    }
}
