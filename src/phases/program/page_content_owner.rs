//! The slot-outlet contract: **which single trait owns a page's content
//! region.** Compiled-path port of `@almadar/core`'s `page-content-owner.ts`
//! (see that file's module doc for the full history — three calibrated
//! `viewer-stranded` drafts, 832 -> 672 -> 135 findings, withdrawn because
//! the defect they hunt ("the trait that OWNS this page's content replaced
//! that content with something unrelated") cannot be asked without an
//! owner).
//!
//! Lives in `orbital-compiler`, not `orbital-core`: the writer test below
//! reads the pattern registry (`sync::content::is_content_body_pattern_type`),
//! which is compiler-tier data — L1 (`orbital-lolo`) never needs this fact.
//!
//! Ownership is derived from what the page already declares, never named or
//! inferred from a trait's name:
//!   - **channel** — the page's shell designates its body through a config
//!     slot (`contentTrait` / `idleContent` / a bound body tree). An explicit
//!     authoring act, so it wins outright.
//!   - **sole-writer** — no channel, and exactly one page-declared trait
//!     paints a content-grade body into `main`.
//!   - **ambiguous** — more than one candidate. A RESULT, not a failure to
//!     paper over: a caller must report it rather than pick.
//!   - **none** — the page paints no content body at all.

use crate::phases::embed_graph::EmbedAdjacency;
use crate::phases::validation::sync::content::{is_content_body_pattern, is_main_slot_render_ui};
use orbital_core_base::schema::{PageDefinition, SExpression, TraitDefinition, TraitReference};
use std::collections::{HashMap, HashSet, VecDeque};

/// Which trait owns a page's content region, and how that was established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageContentOwner {
    Channel(String),
    SoleWriter(String),
    Ambiguous(Vec<String>),
    None,
}

/// True when this effect list paints a content-grade body into a main slot,
/// including through `if` branches — a generic recursive walk over the
/// effect tree's own array structure (an `if` node's branches are nested
/// effect arrays, so no special-cased handling is needed to reach them).
///
/// `unbound_trait_knobs` names every `trait`-typed config field on this
/// trait whose resolved value is `none` (`Null` — owner ruling 2026-09-13):
/// a `(if @config.<knob> then else?)` node for one of these knobs provably
/// never takes `then` in THIS composed instance (`@config.<knob>` is falsy),
/// so `then` is skipped — only `else`, if present, is scanned. Without this
/// a headless composition (e.g. project-friday's `AssetDriveLink { config {
/// uiTrait: none } }`) is wrongly credited with owning the page's content
/// merely because its generated atom's SOURCE still contains a render-ui
/// call that can never fire here.
///
/// By the time THIS check runs (post `inline_orbital`), a trait-typed
/// knob's `(if @config.<knob> ...)` condition has already been rewritten to
/// a literal `Bool` by `coerce_trait_knob_conditions`
/// (`phases/inline/{trait,rewrite}.rs`) — the `@config.<knob>` STRING form
/// below never actually survives to here for that case; it is kept as a
/// defensive fallback for any tree this function is ever handed BEFORE that
/// coercion runs. The `Bool(false)` arm is the one that actually fires for
/// AssetDriveLink et al., and generalizes correctly beyond trait knobs: a
/// literal-false condition means `then` is unreachable dead code, full stop.
fn writes_content_main(
    effects: Option<&[SExpression]>,
    unbound_trait_knobs: &HashSet<&str>,
) -> bool {
    fn scan(node: &SExpression, unbound_trait_knobs: &HashSet<&str>) -> bool {
        if let SExpression::Array(arr) = node {
            if let [SExpression::String(op), cond, _then_branch, rest @ ..] = arr.as_slice() {
                if op == "if" {
                    let then_unreachable = match cond {
                        SExpression::Bool(false) => true,
                        SExpression::String(s) => s
                            .strip_prefix("@config.")
                            .is_some_and(|knob| unbound_trait_knobs.contains(knob)),
                        _ => false,
                    };
                    if then_unreachable {
                        return rest.first().is_some_and(|e| scan(e, unbound_trait_knobs));
                    }
                }
            }
        }
        if is_main_slot_render_ui(node) {
            let SExpression::Array(arr) = node else {
                return false;
            };
            return arr.get(2).is_some_and(is_content_body_pattern);
        }
        let SExpression::Array(items) = node else {
            return false;
        };
        items.iter().any(|item| scan(item, unbound_trait_knobs))
    }
    effects
        .into_iter()
        .flatten()
        .any(|node| scan(node, unbound_trait_knobs))
}

/// True when the trait paints a content-grade body into `main` anywhere in
/// its transition or tick effects. The content-grade classification itself
/// stays in `sync::content` — single owner, no local lists.
///
/// The JS twin also scans `trait.initialEffects` — a factory-runtime-only
/// field with NO representation in `TraitDefinition` (confirmed: zero
/// occurrences of `"initialEffects"` across the `@almadar/std` +
/// `@almadar-io/behaviors` registry corpus). Nothing to port; the compiled
/// path never carries this field.
pub(crate) fn is_content_main_writer(trait_def: &TraitDefinition) -> bool {
    let unbound_trait_knobs: HashSet<&str> = trait_def
        .config
        .as_ref()
        .map(|config| {
            config
                .iter()
                .filter(|(_, cf)| {
                    cf.field_type == "trait" && matches!(cf.default, Some(SExpression::Null))
                })
                .map(|(name, _)| name.as_str())
                .collect()
        })
        .unwrap_or_default();
    trait_def
        .state_machine
        .transitions
        .iter()
        .any(|t| writes_content_main(t.effects.as_deref(), &unbound_trait_knobs))
        || trait_def
            .ticks
            .iter()
            .flatten()
            .any(|tick| writes_content_main(Some(&tick.effects), &unbound_trait_knobs))
}

/// Every node transitively reachable FROM `start` in `adjacency` — never
/// includes `start` itself unless some cycle points back to it. Mirrors the
/// JS `reach` closure exactly (BFS seeded with `[start]`, `out` collects only
/// the neighbors of a popped node, never the popped node itself).
fn reach<'a>(start: &str, adjacency: &EmbedAdjacency<'a>) -> HashSet<&'a str> {
    let mut out: HashSet<&'a str> = HashSet::new();
    let mut seen: HashSet<&'a str> = HashSet::new();
    let mut walk: VecDeque<&'a str> = VecDeque::new();
    // Normalize `start` (borrowed with the caller's own lifetime) to the
    // adjacency map's own `&'a str` key, if it has outgoing edges at all.
    if let Some((&key, _)) = adjacency.get_key_value(start) {
        walk.push_back(key);
    }
    while let Some(from) = walk.pop_front() {
        if seen.contains(from) {
            continue;
        }
        seen.insert(from);
        if let Some(targets) = adjacency.get(from) {
            for &t in targets {
                out.insert(t);
                walk.push_back(t);
            }
        }
    }
    out
}

/// Reduce a candidate set to the trait(s) that actually own the region,
/// using the two DIFFERENT relations a trait graph carries:
///   - **designation** (`channel_adjacency` — config slots like
///     `contentTrait`/`idleContent`): "my body is X". A designator is never
///     the body, so the DEEPER link wins.
///   - **containment** (`embed_adjacency` — a `@trait.X` in render effects):
///     "X is part of what I paint". A contained trait is never a rival, so
///     the OUTER one wins.
///
/// Exported so a caller that already HAS an owner (`ORB_PAGE_SECOND_CONTENT_BODY`)
/// can ask "is THIS other candidate actually a rival, or just this owner's
/// ancestor/descendant along the same designation-or-containment graph" —
/// `reduce_to_owners(&[owner, candidate], ...).len() == 1` means related, not
/// a second writer. One algorithm, not a second copy of it.
///
/// `candidates` is treated as an ORDERED, DEDUPED collection (mirrors a JS
/// `Set`'s insertion order) — callers build it that way; a duplicate entry
/// would just be redundant work, never a correctness issue, since membership
/// tests below are set-shaped.
pub(crate) fn reduce_to_owners(
    candidates: &[String],
    channel_adjacency: &EmbedAdjacency<'_>,
    embed_adjacency: &EmbedAdjacency<'_>,
) -> Vec<String> {
    let mut dropped: HashSet<&str> = HashSet::new();
    for a in candidates {
        let designates = reach(a, channel_adjacency);
        let contains = reach(a, embed_adjacency);
        for b in candidates {
            if a == b {
                continue;
            }
            if designates.contains(b.as_str()) {
                dropped.insert(a.as_str()); // a names b as its body
            } else if contains.contains(b.as_str()) {
                dropped.insert(b.as_str()); // b is a part of a
            }
        }
    }
    candidates
        .iter()
        .filter(|name| !dropped.contains(name.as_str()))
        .cloned()
        .collect()
}

/// Every page-declared trait ref resolved to a name, in declaration order —
/// mirrors `validation::slot::resolve_page_traits`'s two branches (an
/// `Inline` mount's own name; a `Reference` mount's `name:` override or its
/// `ref:`/`reference` target).
fn declared_page_trait_names(page: &PageDefinition) -> Vec<String> {
    page.traits
        .as_ref()
        .map(|mounts| {
            mounts
                .iter()
                .filter_map(|mount| match mount {
                    TraitReference::Inline(def) => Some(def.name.clone()),
                    TraitReference::Reference {
                        reference, name, ..
                    } => name.clone().or_else(|| reference.clone()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the content owner of one page.
///
/// - `traits` — every INLINE trait in the orbital, by name (a page-declared
///   `Reference` mount with no matching `Inline` entry contributes only its
///   name to `declared_page_trait_names`; it can never itself be judged a
///   content-main-writer, matching the JS's `Map<string, Trait>` — a
///   `Reference` entry is never inserted there either).
/// - `channel_adjacency` — `EmbedGraph::config_embedded_children()`, the
///   content channel: `@trait.X` references in trait CONFIG only.
/// - `embed_adjacency` — `EmbedGraph::embedded_children()`, the full
///   config+state-machine embed graph: "containment".
pub(crate) fn resolve_page_content_owner(
    page: &PageDefinition,
    traits: &HashMap<&str, &TraitDefinition>,
    channel_adjacency: &EmbedAdjacency<'_>,
    embed_adjacency: &EmbedAdjacency<'_>,
) -> PageContentOwner {
    let declared = declared_page_trait_names(page);
    if declared.is_empty() {
        return PageContentOwner::None;
    }

    // 1. The content channel — a page-declared trait designating a body
    //    through its config. Transitive: a shell may designate a wrapper
    //    that itself designates the body.
    let mut channel: Vec<String> = Vec::new();
    let mut channel_seen: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = declared.iter().cloned().collect();
    while let Some(from) = queue.pop_front() {
        if seen.contains(&from) {
            continue;
        }
        seen.insert(from.clone());
        if let Some(targets) = channel_adjacency.get(from.as_str()) {
            for target in targets {
                if let Some(trait_def) = traits.get(*target) {
                    if is_content_main_writer(trait_def) && channel_seen.insert(target.to_string())
                    {
                        channel.push(target.to_string());
                    }
                }
                queue.push_back(target.to_string());
            }
        }
    }
    let terminal = reduce_to_owners(&channel, channel_adjacency, embed_adjacency);
    if terminal.len() == 1 {
        return PageContentOwner::Channel(terminal.into_iter().next().unwrap());
    }
    if terminal.len() > 1 {
        let mut sorted = terminal;
        sorted.sort();
        return PageContentOwner::Ambiguous(sorted);
    }

    // 2. No channel: the page's own content-grade `main` writers. A trait
    //    some other page trait designates through config is already
    //    accounted for above, so anything left is a direct page-level writer.
    let writers: Vec<String> = declared
        .iter()
        .filter(|name| {
            traits
                .get(name.as_str())
                .is_some_and(|t| is_content_main_writer(t))
        })
        .cloned()
        .collect();
    let distinct = reduce_to_owners(&writers, channel_adjacency, embed_adjacency);
    if distinct.is_empty() {
        return PageContentOwner::None;
    }
    if distinct.len() == 1 {
        return PageContentOwner::SoleWriter(distinct.into_iter().next().unwrap());
    }
    let mut sorted = distinct;
    sorted.sort();
    PageContentOwner::Ambiguous(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{Orbital, TraitReference};
    use serde_json::json;

    fn trait_def(json_val: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(json_val).expect("trait definition fixture deserializes")
    }

    /// A content-grade writer: an INIT arm rendering a `data-grid` (an
    /// entity-kind-prop pattern) into `main`.
    fn list_trait(name: &str) -> TraitDefinition {
        trait_def(json!({
            "name": name,
            "linkedEntity": "Item",
            "scope": "instance",
            "stateMachine": {
                "states": [{ "name": "browsing", "isInitial": true }],
                "transitions": [
                    { "from": "browsing", "event": "INIT", "to": "browsing",
                      "effects": [["render-ui", "main", { "type": "data-grid", "entity": "?data" }]] }
                ]
            }
        }))
    }

    /// Chrome: writes only a modal slot, never main.
    fn chrome_trait(name: &str, config: Option<serde_json::Value>) -> TraitDefinition {
        let mut obj = json!({
            "name": name,
            "scope": "instance",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "event": "INIT", "to": "idle",
                      "effects": [["render-ui", "modal", null]] }
                ]
            }
        });
        if let Some(cfg) = config {
            obj.as_object_mut()
                .unwrap()
                .insert("config".to_string(), cfg);
        }
        trait_def(obj)
    }

    /// A `@config.<knob>` naming another trait — a channel forward.
    fn trait_ref_config(knob: &str, trait_name: &str) -> serde_json::Value {
        json!({ knob: { "type": "trait", "default": format!("@trait.{trait_name}") } })
    }

    fn page_of(refs: &[&str]) -> PageDefinition {
        let traits: Vec<serde_json::Value> = refs.iter().map(|r| json!({ "ref": r })).collect();
        serde_json::from_value(json!({ "name": "P", "path": "/p", "traits": traits }))
            .expect("page fixture deserializes")
    }

    /// Build both adjacency views from the SAME set of trait definitions, as
    /// `orbital.traits` inline entries — matches how the real caller (an
    /// `EmbedGraph` built over `orbital.traits`) supplies them.
    fn adjacencies_of(
        traits: &[TraitDefinition],
    ) -> (EmbedAdjacency<'static>, EmbedAdjacency<'static>) {
        let refs: Vec<TraitReference> =
            traits.iter().cloned().map(|td| TraitReference::Inline(std::sync::Arc::new(td))).collect();
        let orbital = Box::leak(Box::new(Orbital {
            spec: false,
            types: None,
            name: "O".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: orbital_core_base::schema::EntityRef::Reference("Item".to_string()),
            auxiliary_entities: None,
            traits: refs,
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        }));
        let graph = Box::leak(Box::new(crate::phases::embed_graph::EmbedGraph::build(
            &orbital.traits,
        )));
        (graph.config_embedded_children(), graph.embedded_children())
    }

    fn resolve(traits: Vec<TraitDefinition>, page: &PageDefinition) -> PageContentOwner {
        let (channel, embed) = adjacencies_of(&traits);
        let by_name: HashMap<&str, &TraitDefinition> = Box::leak(Box::new(traits))
            .iter()
            .map(|t| (t.name.as_str(), t))
            .collect();
        resolve_page_content_owner(page, &by_name, &channel, &embed)
    }

    #[test]
    fn is_content_main_writer_true_for_list_false_for_chrome() {
        assert!(is_content_main_writer(&list_trait("Browse")));
        assert!(!is_content_main_writer(&chrome_trait("Overlay", None)));
    }

    /// A generated service atom's render is `(if @config.uiTrait
    /// (render-ui main {content-grade body}) (render-ui main null))` — when
    /// the composed instance's `uiTrait` resolves to `none` (`Null`), the
    /// then-branch provably never fires, so this headless composition must
    /// NOT be credited with owning the page's content (project-friday's
    /// `AssetDriveLink { config { uiTrait: none } }`, owner ruling
    /// 2026-09-13, `ORB_PAGE_SECOND_CONTENT_BODY`).
    #[test]
    fn unbound_trait_knob_guarded_content_body_is_not_a_writer() {
        let headless = trait_def(json!({
            "name": "AssetDriveLink",
            "scope": "instance",
            "config": { "uiTrait": { "type": "trait", "default": serde_json::Value::Null } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "event": "INIT", "to": "idle",
                      "effects": [[
                          "if", "@config.uiTrait",
                          ["render-ui", "main", { "type": "data-grid", "entity": "?data" }],
                          ["render-ui", "main", null]
                      ]] }
                ]
            }
        }));
        assert!(!is_content_main_writer(&headless));
    }

    /// By the time `is_content_main_writer` actually runs on a validated
    /// schema (post `inline_orbital`), `coerce_trait_knob_conditions` has
    /// already rewritten the `@config.<knob>` condition to a literal
    /// `Bool(false)` — the STRING form the sibling test above uses never
    /// survives that far in practice. This is the shape that regressed:
    /// project-friday's real `AssetDriveLink` kept tripping
    /// `ORB_PAGE_SECOND_CONTENT_BODY` because the original fix only matched
    /// the (already-superseded) string condition.
    #[test]
    fn already_coerced_bool_false_condition_is_not_a_writer() {
        let headless = trait_def(json!({
            "name": "AssetDriveLink",
            "scope": "instance",
            "config": { "uiTrait": { "type": "trait", "default": serde_json::Value::Null } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "event": "INIT", "to": "idle",
                      "effects": [[
                          "if", false,
                          ["render-ui", "main", { "type": "data-grid", "entity": "?data" }],
                          ["render-ui", "main", null]
                      ]] }
                ]
            }
        }));
        assert!(!is_content_main_writer(&headless));
    }

    /// The same coerced shape with `Bool(true)` — the then-branch IS
    /// reachable, so it IS a content writer. Guards against a fix that
    /// treats every literal-bool condition as dead rather than checking the
    /// value.
    #[test]
    fn already_coerced_bool_true_condition_is_a_writer() {
        let bound = trait_def(json!({
            "name": "AssetDriveLink",
            "scope": "instance",
            "config": { "uiTrait": { "type": "trait", "default": "@trait.DefaultForm" } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "event": "INIT", "to": "idle",
                      "effects": [[
                          "if", true,
                          ["render-ui", "main", { "type": "data-grid", "entity": "?data" }],
                          ["render-ui", "main", null]
                      ]] }
                ]
            }
        }));
        assert!(is_content_main_writer(&bound));
    }

    /// The same shape, but `uiTrait` resolves to a BOUND `@trait.<Name>`
    /// string — the then-branch is reachable, so it IS a content writer.
    #[test]
    fn bound_trait_knob_guarded_content_body_is_a_writer() {
        let bound = trait_def(json!({
            "name": "AssetDriveLink",
            "scope": "instance",
            "config": { "uiTrait": { "type": "trait", "default": "@trait.DefaultForm" } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "event": "INIT", "to": "idle",
                      "effects": [[
                          "if", "@config.uiTrait",
                          ["render-ui", "main", { "type": "data-grid", "entity": "?data" }],
                          ["render-ui", "main", null]
                      ]] }
                ]
            }
        }));
        assert!(is_content_main_writer(&bound));
    }

    #[test]
    fn prefers_the_channel_the_shell_designating_a_body_through_config() {
        let shell = chrome_trait(
            "AppLayout",
            Some(trait_ref_config("contentTrait", "Catalog")),
        );
        let owner = resolve(
            vec![shell, list_trait("Catalog")],
            &page_of(&["AppLayout", "Catalog"]),
        );
        assert_eq!(owner, PageContentOwner::Channel("Catalog".to_string()));
    }

    #[test]
    fn falls_back_to_sole_writer_when_no_channel_designates_a_body() {
        let owner = resolve(
            vec![chrome_trait("Overlay", None), list_trait("Catalog")],
            &page_of(&["Overlay", "Catalog"]),
        );
        assert_eq!(owner, PageContentOwner::SoleWriter("Catalog".to_string()));
    }

    #[test]
    fn reports_ambiguous_rather_than_picking_when_two_bodies_share_a_page() {
        let owner = resolve(
            vec![list_trait("Catalog"), list_trait("Detail")],
            &page_of(&["Catalog", "Detail"]),
        );
        assert_eq!(
            owner,
            PageContentOwner::Ambiguous(vec!["Catalog".to_string(), "Detail".to_string()])
        );
    }

    #[test]
    fn reports_none_for_a_page_that_paints_no_content_body() {
        let owner = resolve(vec![chrome_trait("Overlay", None)], &page_of(&["Overlay"]));
        assert_eq!(owner, PageContentOwner::None);
    }

    #[test]
    fn reports_none_for_a_page_with_no_declared_traits() {
        let page: PageDefinition =
            serde_json::from_value(json!({ "name": "P", "path": "/p", "traits": [] })).unwrap();
        let owner = resolve(vec![list_trait("Catalog")], &page);
        assert_eq!(owner, PageContentOwner::None);
    }

    #[test]
    fn follows_the_channel_transitively_through_a_wrapper() {
        let shell = chrome_trait(
            "AppLayout",
            Some(trait_ref_config("contentTrait", "Wrapper")),
        );
        let wrapper = chrome_trait("Wrapper", Some(trait_ref_config("body", "Catalog")));
        let owner = resolve(
            vec![shell, wrapper, list_trait("Catalog")],
            &page_of(&["AppLayout"]),
        );
        assert_eq!(owner, PageContentOwner::Channel("Catalog".to_string()));
    }
}
