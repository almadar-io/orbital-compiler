//! The "who embeds whom" graph over one orbital's traits.
//!
//! Built once from an orbital's PRE-resolution `traits` list (a `TraitReference`
//! slice that may still hold a mix of `Inline` and `Reference` entries — the
//! validator only ever sees it fully resolved to `Inline`, but the inline
//! phase itself calls `build` on the raw, not-yet-spliced list) and shared by
//! the two consumers that must agree on the exact same structure: the inline
//! phase's `@config.<knob>` forward resolver (`inline/trait.rs`,
//! `forwarded_sibling_config_from`) and `validate_dangling_config_refs`'s
//! reachability check (`validation/config_type.rs`). One walk, one contract —
//! a forward the resolver folds to a literal is exactly a forward the
//! validator judges reachable, and vice versa.
//!
//! What counts as a trait's "declared" values, per entry kind:
//! - An `Inline` entry's declared values are its own `config {}` defaults.
//! - A `Reference` entry's declared values are its CALL-SITE config override
//!   map — those are the values IT publishes to whatever embeds it. A
//!   JSX-hoisted top-level `ref:` entry (`InlineTypographyRender4.config.content
//!   = "@config.title"`) is exactly this shape: the override forwards a knob
//!   the entry's own atom never declares.
//!
//! Only an entry with a stable pre-resolution name — an `Inline` trait's own
//! `name`, or a `Reference` entry's `name:` override — can be addressed by
//! `@trait.X` or continue a chain. A `Reference` entry with no `name:`
//! override has no pre-resolution identity (its final name depends on
//! resolving `reference`, which this graph never does) and contributes
//! nothing: nothing else could address it by `@trait.X` either.
//!
//! `@trait.X` tokens are collected from: declared config defaults, and — for
//! `Inline` entries only, since a `Reference` entry has no state machine of
//! its own yet — every transition's guard + effects and every tick's guard +
//! effects.

use orbital_core_base::schema::{
    collect_state_machine_trait_tokens, collect_trait_tokens, ConfigField, TraitReference,
};
use std::collections::{HashMap, HashSet};

pub(crate) struct EmbedGraph {
    /// child trait name -> the names of every trait whose declared values
    /// reference it via `@trait.<child>`, in FIRST-SEEN order (the traits
    /// slice's own order) — `embedder_chain` takes the first entry per hop.
    referrers: HashMap<String, Vec<String>>,
    /// trait name -> its own declared values (see module doc for what
    /// "declared" means per entry kind).
    declared: HashMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>>,
}

impl EmbedGraph {
    pub(crate) fn build(traits: &[TraitReference]) -> Self {
        let mut declared: HashMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>> = HashMap::new();
        let mut referrers: HashMap<String, Vec<String>> = HashMap::new();

        for trait_ref in traits {
            let (name, config): (&str, orbital_core_base::collections::IndexMap<String, ConfigField>) = match trait_ref {
                TraitReference::Inline(td) => {
                    (td.name.as_str(), td.config.clone().unwrap_or_default())
                }
                TraitReference::Reference {
                    name: Some(name),
                    config,
                    ..
                } => (name.as_str(), config.clone().unwrap_or_default()),
                // No pre-resolution identity — see module doc.
                TraitReference::Reference { name: None, .. } => continue,
            };

            let mut children: HashSet<String> = HashSet::new();
            for field in config.values() {
                if let Some(default) = field.default.as_ref() {
                    collect_trait_tokens(default, &mut children);
                }
            }
            if let TraitReference::Inline(td) = trait_ref {
                collect_state_machine_trait_tokens(td, &mut children);
            }
            for child in children {
                if child != name {
                    referrers.entry(child).or_default().push(name.to_string());
                }
            }
            declared.insert(name.to_string(), config);
        }

        Self {
            referrers,
            declared,
        }
    }

    /// The trait that embeds `trait_name` via a bare `@trait.<trait_name>`
    /// reference — the first-recorded referrer, matching `embedder_chain`'s
    /// own first-referrer-per-hop rule. `None` for a trait nothing embeds
    /// (a standalone top-level trait). Feeds `OirTrait.host_trait`
    /// (`phases/resolve.rs`) — the compiled path's bus-key qualification
    /// needs the trait NAME, not its declared-values map, so this is a
    /// distinct accessor from `embedder_chain` rather than a wrapper over it.
    pub(crate) fn direct_host(&self, trait_name: &str) -> Option<&str> {
        self.referrers
            .get(trait_name)
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }

    /// Innermost-first chain of ancestor declared-values maps for
    /// `trait_name`: its first-recorded direct referrer, then that
    /// referrer's own first referrer, and so on out to whichever trait has
    /// none. `trait_name`'s OWN declared map is never included — callers
    /// already hold it (it's the thing being resolved), exactly the shape
    /// `forwarded_sibling_config_from` expects.
    ///
    /// First-referrer-per-hop: a trait embedded by more than one parent
    /// (shared chrome) walks out through whichever parent recorded it
    /// first — matching the existing sibling-pull `parent_of` contract,
    /// which records exactly one parent per pulled name.
    ///
    /// Bounded by the total number of traits this graph knows about, with a
    /// `visited` guard — the graph should be acyclic by construction (a
    /// trait embedding itself, directly or transitively, is not a shape any
    /// authoring path produces) but the walk must terminate defensively
    /// regardless of malformed input.
    pub(crate) fn embedder_chain(&self, trait_name: &str) -> Vec<&orbital_core_base::collections::IndexMap<String, ConfigField>> {
        let mut chain = Vec::new();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut current = trait_name;
        let bound = self.declared.len().max(1);
        while chain.len() < bound && visited.insert(current) {
            let Some(parent) = self.referrers.get(current).and_then(|v| v.first()) else {
                break;
            };
            if let Some(cfg) = self.declared.get(parent) {
                chain.push(cfg);
            }
            current = parent.as_str();
        }
        chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::SExpression;
    use serde_json::json;

    fn trait_ref(json_val: serde_json::Value) -> TraitReference {
        serde_json::from_value(json_val).expect("trait reference fixture deserializes")
    }

    fn inline_trait(
        name: &str,
        config: serde_json::Value,
        effects: serde_json::Value,
    ) -> TraitReference {
        trait_ref(json!({
            "name": name,
            "linkedEntity": "Item",
            "config": config,
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "transitions": [ {
                    "from": "idle", "to": "idle", "event": "INIT",
                    "effects": effects
                } ]
            }
        }))
    }

    fn reference(name: &str, config: serde_json::Value) -> TraitReference {
        trait_ref(json!({ "ref": "Up.traits.Whatever", "name": name, "config": config }))
    }

    /// The exact `std-timeline` shape: `Feed` (Inline, declares `title`,
    /// embeds `Stack8` via a render-ui effect) -> `Stack8` (Reference,
    /// embeds `Stack5` via `children`) -> `Stack5` (Reference, embeds
    /// `Typography4` via `children`) -> `Typography4` (Reference, whose own
    /// override forwards `content: @config.title`).
    fn timeline_shaped_traits() -> Vec<TraitReference> {
        vec![
            inline_trait(
                "Feed",
                json!({ "title": { "type": "string", "default": "Activity Timeline" } }),
                json!([["render-ui", "main", { "type": "stack", "children": ["@trait.Stack8"] }]]),
            ),
            reference(
                "Stack8",
                json!({ "gap": { "type": "unknown", "default": "md" }, "children": { "type": "unknown", "default": ["@trait.Stack5"] } }),
            ),
            reference(
                "Stack5",
                json!({ "direction": { "type": "unknown", "default": "row" }, "children": { "type": "unknown", "default": ["@trait.Typography4"] } }),
            ),
            reference(
                "Typography4",
                json!({ "content": { "type": "unknown", "default": "@config.title" } }),
            ),
        ]
    }

    /// Marker keys unique per rung (`direction`/`gap`/`title`) identify which
    /// declared map landed at which chain position, independent of the
    /// `children` key every rung shares.
    #[test]
    fn chain_is_innermost_first_and_excludes_self() {
        let graph = EmbedGraph::build(&timeline_shaped_traits());
        let chain = graph.embedder_chain("Typography4");
        assert_eq!(
            chain.len(),
            3,
            "Stack5, Stack8, Feed — Typography4 itself excluded"
        );
        assert!(chain[0].contains_key("direction"), "innermost: Stack5");
        assert!(chain[1].contains_key("gap"), "middle: Stack8");
        assert!(chain[2].contains_key("title"), "outermost: Feed");
    }

    #[test]
    fn a_trait_embedded_by_two_parents_walks_the_first_recorded() {
        let traits = vec![
            inline_trait(
                "First",
                json!({ "a": { "type": "string", "default": "A" } }),
                json!([["render-ui", "main", { "children": ["@trait.Shared"] }]]),
            ),
            inline_trait(
                "Second",
                json!({ "a": { "type": "string", "default": "B" } }),
                json!([["render-ui", "aux", { "children": ["@trait.Shared"] }]]),
            ),
            reference(
                "Shared",
                json!({ "a": { "type": "unknown", "default": "@config.a" } }),
            ),
        ];
        let graph = EmbedGraph::build(&traits);
        let chain = graph.embedder_chain("Shared");
        assert_eq!(chain.len(), 1);
        assert_eq!(
            chain[0].get("a").and_then(|f| f.default.as_ref()),
            Some(&SExpression::String(("A".to_string()).into())),
            "walks the FIRST-recorded referrer (First), not Second"
        );
    }

    #[test]
    fn cycle_terminates_and_returns_the_partial_chain() {
        // Malformed/defensive input: A embeds B, B embeds A. Real authoring
        // never produces this, but the walk must not loop forever.
        let traits = vec![
            reference(
                "A",
                json!({ "x": { "type": "unknown", "default": ["@trait.B"] } }),
            ),
            reference(
                "B",
                json!({ "y": { "type": "unknown", "default": ["@trait.A"] } }),
            ),
        ];
        let graph = EmbedGraph::build(&traits);
        let chain = graph.embedder_chain("A");
        assert!(chain.len() <= 2, "must terminate: {}", chain.len());
    }

    #[test]
    fn nameless_reference_entry_contributes_nothing() {
        let traits = vec![trait_ref(json!({
            "ref": "Up.traits.Whatever",
            "config": { "x": { "type": "unknown", "default": "@config.title" } }
        }))];
        let graph = EmbedGraph::build(&traits);
        assert!(graph.embedder_chain("Whatever").is_empty());
        assert_eq!(graph.direct_host("Whatever"), None);
    }

    #[test]
    fn unknown_trait_name_yields_empty_chain() {
        let graph = EmbedGraph::build(&timeline_shaped_traits());
        assert!(graph.embedder_chain("NeverHeardOfIt").is_empty());
    }

    /// Feeds `OirTrait.host_trait` (`phases/resolve.rs`) — the compiled
    /// path's bus-key qualification needs the embedder's NAME directly,
    /// not its declared-values map.
    #[test]
    fn direct_host_returns_the_immediate_embedder_name() {
        let graph = EmbedGraph::build(&timeline_shaped_traits());
        assert_eq!(graph.direct_host("Stack8"), Some("Feed"));
        assert_eq!(graph.direct_host("Stack5"), Some("Stack8"));
        assert_eq!(graph.direct_host("Typography4"), Some("Stack5"));
    }

    #[test]
    fn direct_host_is_none_for_a_standalone_trait() {
        let graph = EmbedGraph::build(&timeline_shaped_traits());
        assert_eq!(graph.direct_host("Feed"), None);
        assert_eq!(graph.direct_host("NeverHeardOfIt"), None);
    }
}
