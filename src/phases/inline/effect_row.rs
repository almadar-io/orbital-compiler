//! Post-inline re-derivation of each trait's effect row.
//!
//! L1 stamps a row from the trait's own source, where a `@config.surface` slot
//! has no identity yet — one file cannot see a call site's config. Inline changes
//! that twice over: `apply_overrides_to_trait` merges the call site's config onto
//! the atom's defaults and `rewrite_config_bindings` substitutes the result into
//! the effect bodies, and every `TraitReference::Reference` becomes a full
//! `TraitReference::Inline` carrying the atom's own effects.
//!
//! So the row a consumer sees post-inline must be re-derived from the substituted
//! bodies, not inherited. Without this, two things break at once: a composed trait
//! (69% of the corpus) would report an empty row, and a config-bound slot would
//! stay forever unresolved — both of which make a resource check pass vacuously,
//! which is worse than not having the check.
//!
//! Re-derivation runs through the same `EffectRowBuilder` the lowerer uses, so the
//! two layers cannot disagree about what an effect row contains.

use orbital_core_base::effect_row::{EffectRowBuilder, EffectSite};
use std::sync::Arc;
use orbital_core_base::schema::{Orbital, TraitReference};

/// Re-derive the effect row of every inline trait on `orbital` from its
/// post-inline effects. Call once per orbital, after `inline_orbital`.
pub(crate) fn recompute_effect_rows(orbital: &mut Orbital) {
    for trait_ref in orbital.traits.iter_mut() {
        let TraitReference::Inline(def) = trait_ref else {
            // A reference that survived inlining is an error other phases
            // report; deriving a row from a stub would invent a surface.
            continue;
        };
        let def = Arc::make_mut(def);

        let mut builder = EffectRowBuilder::new();
        for transition in &def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                builder.add_effects(EffectSite::Transition, effects);
            }
        }
        if let Some(ticks) = &def.ticks {
            if !ticks.is_empty() {
                builder.declare_ticks();
            }
            for tick in ticks {
                builder.add_effects(EffectSite::Tick, &tick.effects);
            }
        }
        def.effect_row = builder.finish();
    }
}
