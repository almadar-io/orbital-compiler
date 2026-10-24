//! Which endpoints list a trait (EXEC-ENDPOINT-4): the facts every interaction check reads.

use crate::phases::embed_graph::EmbedGraph;
use orbital_core_base::effect_row::EffectSite;
use orbital_core_base::schema::{Orbital, OrbitalSchema, TraitDefinition, TraitReference};
use std::collections::HashSet;

#[derive(Default)]
pub(crate) struct Mounts {
    route: HashSet<String>,
    endpoint: HashSet<String>,
}

fn keys(trait_ref: &TraitReference) -> Vec<String> {
    let mut keys = Vec::new();
    match trait_ref {
        TraitReference::Inline(def) => keys.push(def.name.clone()),
        TraitReference::Reference { reference, name, .. } => {
            keys.extend(reference.iter().chain(name.iter()).cloned());
        }
    }
    keys.extend(trait_ref.trait_ref_id().map(str::to_string));
    keys
}

impl Mounts {
    pub(crate) fn of_orbital(orbital: &Orbital) -> Self {
        let mut mounts = Self::default();
        for page in orbital.pages.iter().flatten().filter_map(|p| p.as_inline()) {
            let into = if page.is_route() { &mut mounts.route } else { &mut mounts.endpoint };
            for trait_ref in page.traits.iter().flatten() {
                into.extend(keys(trait_ref));
            }
        }
        let graph = EmbedGraph::build(&orbital.traits);
        let forward = graph.embedded_children();
        let mut queue: Vec<String> = mounts.route.iter().cloned().collect();
        while let Some(current) = queue.pop() {
            for child in forward.get(current.as_str()).into_iter().flatten() {
                if mounts.route.insert(child.to_string()) {
                    queue.push(child.to_string());
                }
            }
        }
        mounts
    }

    fn lists(set: &HashSet<String>, def: &TraitDefinition) -> bool {
        set.contains(&def.name) || def.id.as_ref().is_some_and(|id| set.contains(id))
    }

    pub(crate) fn on_route(&self, def: &TraitDefinition) -> bool {
        Self::lists(&self.route, def)
    }

    pub(crate) fn on_endpoint(&self, def: &TraitDefinition) -> bool {
        Self::lists(&self.endpoint, def)
    }
}

/// A program is an orbital with pages, or any orbital of a multi-orbital file; a lone orbital with no
/// pages is an atom, reached by events and specified with `expects page`.
pub(crate) fn is_program(schema: &OrbitalSchema, orbital: &Orbital) -> bool {
    orbital.pages.as_ref().is_some_and(|p| !p.is_empty())
        || schema.orbitals.iter().filter(|o| !o.is_spec()).count() > 1
}

pub(crate) fn renders_ui(trait_def: &TraitDefinition) -> bool {
    let mut effects = Vec::new();
    for transition in &trait_def.state_machine.transitions {
        for effect in transition.effects.iter().flatten() {
            effect.collect_effect_uses(EffectSite::Transition, &mut effects);
        }
    }
    for tick in trait_def.ticks.iter().flatten() {
        for effect in &tick.effects {
            effect.collect_effect_uses(EffectSite::Tick, &mut effects);
        }
    }
    effects.iter().any(|effect| effect.kind == "render-ui")
}

/// Whether the trait paints a page body (`orbital_core::slots::MAIN_SLOTS`). A trait that only toasts or opens an
/// overlay is a helper for the INIT rules (EXEC-ENDPOINT-4); an unresolved slot counts as a body.
pub(crate) fn renders_body(trait_def: &TraitDefinition) -> bool {
    let mut effects = Vec::new();
    for transition in &trait_def.state_machine.transitions {
        for effect in transition.effects.iter().flatten() {
            effect.collect_effect_uses(EffectSite::Transition, &mut effects);
        }
    }
    for tick in trait_def.ticks.iter().flatten() {
        for effect in &tick.effects {
            effect.collect_effect_uses(EffectSite::Tick, &mut effects);
        }
    }
    effects.iter().any(|effect| {
        effect.kind == "render-ui"
            && (!effect.resolved
                || effect.resource.as_deref().is_none_or(|slot| orbital_core_base::slots::MAIN_SLOTS.contains(&slot)))
    })
}
