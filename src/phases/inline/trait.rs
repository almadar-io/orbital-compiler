//! Trait reference inlining: call-site overrides, collision detection,
//! sibling auto-pull, and source-scoped listener pruning.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::rewrite::{
    apply_inline_config_defaults, mark_literal_list_in_place, record_config_provenance,
    rewrite_config_bindings, rewrite_entity_field_paths, rewrite_identifiers,
    rewrite_payload_entity_refs, rewrite_trait_event_fields,
};
use super::InlineError;
use orbital_core::schema::{
    ConfigField, ConfigProvenanceRecord, EntityDefinition, SExpression, SourceBehaviorMetadata,
    TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    linked_entity: Option<&String>,
    // Consumer-scope entity name → dual-carry id map. When a `linked_entity`
    // rebind rewrites the trait's `linkedEntity` NAME, the id sibling
    // (`linked_entity_id`) MUST be rewritten in the same operation to the id of
    // the rebind target in the consumer arena — otherwise the cloned trait keeps
    // the SOURCE file's entity id, which resolves to no node in the composed
    // schema (`ORB_ID_UNKNOWN_REF`). Empty map = no id remap (test / legacy
    // id-free callers); an unresolved target clears the id (justified: the
    // rebind target is not a node in the composed graph — a dangling foreign id
    // is never the right carry).
    entity_ids: &HashMap<String, String>,
    config: Option<&HashMap<String, ConfigField>>,
    name: Option<&str>,
    events: Option<&HashMap<String, String>>,
    // Per-key `@entity.<old>` → `@entity.<new>` field rename map. Mirrors
    // `events`: rewrites every entity-field read inside the resolved trait so a
    // DATA field the trait reads is satisfied by a differently-named field on
    // the rebind target. Also rewrites the trait's `entity_contract.requires`.
    fields: Option<&HashMap<String, String>>,
    // When true, fold call-site config overrides into the trait's declared
    // `config[k].default` (so they carry forward) but DO NOT substitute the
    // `@config.X` strings in the state machine. Used when resolving an import,
    // so a downstream consumer can still override those bindings. When false
    // (top-level target), the `@config.X` strings are baked to their effective
    // values as before.
    defer_config_subst: bool,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    // 1. Direct name rename — applied first so all subsequent operations
    //    refer to the final name in any error messages.
    if let Some(new_name) = name {
        if new_name != trait_def.name {
            trait_def.name = new_name.to_string();
        }
    }

    // 2. Build the entity substitution map. The `linked_entity` arg means
    //    "the atom thinks its canonical entity is X; rebind it to Y". The
    //    pre-existing `trait_def.linked_entity` IS the from-key.
    let mut entity_subs: HashMap<String, String> = HashMap::new();
    if let Some(new_entity) = linked_entity {
        if let Some(orig) = trait_def.linked_entity.clone() {
            if &orig != new_entity {
                entity_subs.insert(orig, new_entity.clone());
            }
        }
        trait_def.linked_entity = Some(new_entity.clone());
        // Keep the dual-carry id sibling in lockstep with the rebound name.
        trait_def.linked_entity_id = entity_ids.get(new_entity).cloned();
    }

    // 3. Build the event substitution map directly from the caller args.
    //    Empty entries (or self-mapped no-ops) are pruned to keep the walker
    //    from doing redundant work.
    let mut event_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = events {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                event_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 3b. Build the entity-field rename map. Each `from → to` rewrites the
    //     trait's `@entity.<from>` reads (and `(set @entity.<from> ...)`) to
    //     `@entity.<to>` so a renamed DATA field on the rebind target satisfies
    //     the read. Self-mapped no-ops and empty entries are pruned.
    let mut field_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = fields {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                field_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 3b'. Call-site payload capture. A `@payload.<field>` in this reference's
    //      call-site config is, by position, a read of the COMPOSING effect's
    //      event payload — the child atom's own payload is not in scope at the
    //      call site. Tag it `@callsitePayload.<field>` so it is resolved at the
    //      call site, never in the child's INIT scope. Freshly lowered lolo has
    //      already tagged this (idempotent here), but a baked / hand-authored
    //      `.orb` reference carries the untagged form, so tag here too. The
    //      capture then flows into `effective_config` + the folded declared
    //      default below, and `contains_callsite_payload` (step 6) keeps the
    //      bound knob out of the child body.
    let tagged_config: Option<HashMap<String, ConfigField>> = config.map(|c| {
        c.iter()
            .map(|(k, cf)| {
                let mut cf = cf.clone();
                if let Some(default) = cf.default.as_mut() {
                    orbital_core::schema::capture_callsite_payload(default);
                }
                (k.clone(), cf)
            })
            .collect()
    });
    let config = tagged_config.as_ref();

    // 3c. Build the effective config map (key → concrete SExpression value)
    //     EARLY — same precedence `effective_config` always uses (declared
    //     defaults, then call-site overrides) — and resolve any
    //     `@config.<knob>` emit-name reference to its concrete literal
    //     BEFORE the event-key rename pass below (step 5). Pinned contract:
    //     `events: {...}` renames target the RESOLVED emit name (e.g.
    //     ButtonRender's `emits { @config.action -> ... }` resolves to its
    //     declared default "ACTION", and a call-site `events: {ACTION:
    //     UPGRADE_WIDGET}` renames THAT). Resolving first means the later
    //     `rewrite_trait_event_fields` walk (which matches `event_subs`
    //     keys against the trait's declared event names) sees "ACTION", not
    //     the still-raw "@config.action" token. Gated the same as the body
    //     substitution below: a deferred import load leaves the reference
    //     live for the downstream consumer's own resolution pass.
    let mut effective_config: HashMap<String, SExpression> = HashMap::new();
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, field) in declared.iter() {
            if let Some(default) = field.default.as_ref() {
                effective_config.insert(k.clone(), default.clone());
            }
        }
    }
    if let Some(cfg) = config {
        for (k, cf) in cfg.iter() {
            if let Some(v) = cf.default.as_ref() {
                effective_config.insert(k.clone(), v.clone());
            }
        }
    }
    if !defer_config_subst {
        let resolved_knobs = resolve_config_ref_emit_names(trait_def, &effective_config);
        // Propagate a later `events:` rename onto the same knob's
        // `effective_config` entry so the body substitution below (step 6,
        // `rewrite_config_bindings`) emits the FINAL renamed event name
        // everywhere `@config.<knob>` is read — not just in the structural
        // `emits`/`state_machine.events` fields resolved above.
        for knob in resolved_knobs {
            let current = match effective_config.get(&knob) {
                Some(SExpression::String(s)) => Some(s.clone()),
                _ => None,
            };
            if let Some(current) = current {
                if let Some(renamed) = event_subs.get(&current) {
                    effective_config.insert(knob, SExpression::String(renamed.clone()));
                }
            }
        }
    }

    // 4. Walk SExpressions in transitions and ticks ONCE with both maps.
    if !entity_subs.is_empty() || !event_subs.is_empty() || !field_subs.is_empty() {
        for transition in trait_def.state_machine.transitions.iter_mut() {
            if let Some(guard) = transition.guard.as_mut() {
                rewrite_identifiers(guard, &entity_subs, &event_subs);
                rewrite_entity_field_paths(guard, &field_subs);
            }
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
        if let Some(ticks) = trait_def.ticks.as_mut() {
            for tick in ticks.iter_mut() {
                if let Some(guard) = tick.guard.as_mut() {
                    rewrite_identifiers(guard, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(guard, &field_subs);
                }
                for effect in tick.effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
    }

    // 4b. Rewrite the trait's inferred `entity_contract.requires`/`provides` so
    //     the post-inline rebind validator checks the RENAMED field names. A
    //     `fields` remap turns a `requires: ["title"]` into `requires:
    //     ["dishName"]`, matching the target entity.
    if !field_subs.is_empty() {
        if let Some(contract) = trait_def.entity_contract.as_mut() {
            for f in contract.requires.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
            for f in contract.provides.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
        }
    }

    // 5. Walk the structured event-key fields. These live outside SExpression
    //    payloads, so the SExpression walk above does not reach them.
    rewrite_trait_event_fields(trait_def, &event_subs);

    // 5b. Walk the typed payload schemas (emits[*].payloadSchema,
    //     state_machine.events[*].payloadSchema). Their `type` strings can
    //     reference the atom's original entity name (`[Matter]`); after a
    //     linkedEntity rebind those references would otherwise survive as
    //     dangling types in downstream handler.ts codegen.
    rewrite_payload_entity_refs(trait_def, &entity_subs);

    // 6. Phase 3.A: Substitute `@config.<param>` bindings with concrete
    //    SExpression values supplied by the caller. Unlike entity/event
    //    substitution (which only rewrites strings), config substitution can
    //    replace a whole node with an arbitrary SExpression (array, object,
    //    number, bool, null). The walk runs AFTER the identifier pass so
    //    any `@config.X` that references a renamed entity is still
    //    referenced by the entity rewriter's output (though in practice
    //    `@config.X` is a flat key so entity renames don't interact).
    //
    //    Missing params are left as-is — the downstream binding validator
    //    will flag unresolved `@config.X` so composers see a clean error.
    // Merge call-site config overrides with the atom's declared config-field
    // defaults so that `@config.X` references for params the molecule didn't
    // override still resolve to a concrete literal at compile time. Without
    // this, server-side codegen falls back to `user?.X` (no UserContext.X
    // exists) and trips TS2339; client-side falls back to `mergedConfig?.X`
    // which works at runtime but loses constant-fold opportunities. Merging
    // keeps both paths consistent and prunes dead branches via the eq-fold.
    // `effective_config` (key → concrete SExpression value, declared
    // defaults overridden by call-site values) was already built in step 3c
    // above — reused here for the body substitution below.
    //
    // Wrap string-headed array values bound for literal-array slots in the
    // `list` constructor so they are never mis-read as S-expression calls
    // (see `mark_literal_list_in_place`). Type-directed off each slot's
    // declared `field_type`.
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, value) in effective_config.iter_mut() {
            if let Some(field) = declared.get(k) {
                mark_literal_list_in_place(value, &field.field_type);
            }
        }
    }
    // GAP-AG-VALUE-DRIFT: fold call-site overrides AND annotations into the
    // atom's declared config schema so the inlined trait carries the
    // organism's intent end-to-end. For each call-site `ConfigField`:
    //   - `default` → replaces the atom's declared default (existing behaviour).
    //   - `label`/`description`/`synonyms`/`tier` when `Some` → overwrite
    //     the atom's declared value (organism wins). This is the NEW behaviour
    //     for annotated call-site entries; plain wiring entries (no annotations)
    //     are unchanged.
    if let Some(cfg) = config {
        if let Some(declared) = trait_def.config.as_mut() {
            for (k, cf) in cfg.iter() {
                if let Some(field) = declared.get_mut(k) {
                    if let Some(v) = cf.default.as_ref() {
                        field.default = Some(v.clone());
                    }
                    if cf.label.is_some() {
                        field.label = cf.label.clone();
                    }
                    if cf.description.is_some() {
                        field.description = cf.description.clone();
                    }
                    if cf.synonyms.is_some() {
                        field.synonyms = cf.synonyms.clone();
                    }
                    if cf.tier.is_some() {
                        field.tier = cf.tier.clone();
                    }
                }
            }
        }
        // A reference's config block can DECLARE a brand-new knob rather
        // than override one of the wrapped trait's (e.g. an alias trait
        // adding its own `steps : [StepSpec] = [...]`). Those entries have
        // no match in `declared` above (the wrapped trait never declared
        // them) and are structurally a declaration, not a plain wiring
        // value — insert them so the resolved trait carries them as its
        // OWN declared config. This is what makes the alias's knob surface
        // visible to any downstream call site that references the ALIAS
        // (not the wrapped trait): the E1 unknown-call-site-key check reads
        // `trait_def.config` off whatever `ctx.get_trait` resolves.
        for (k, cf) in cfg.iter() {
            let already_declared = trait_def.config.as_ref().is_some_and(|m| m.contains_key(k));
            if !already_declared && cf.is_declaration_form() {
                trait_def
                    .config
                    .get_or_insert_with(HashMap::new)
                    .insert(k.clone(), cf.clone());
            }
        }
    }
    if !defer_config_subst && !effective_config.is_empty() {
        record_config_provenance(trait_def, &effective_config, recorder);
        // Call-site payload captures stay UNSUBSTITUTED in the child body:
        // a knob whose effective value is a `@callsitePayload.<field>` capture
        // must remain `@config.<knob>` in the child's render so each engine
        // resolves the capture against the COMPOSING effect's event payload at
        // the call site and passes the literal into the child's config (the
        // capture value is retained on the child's declared config-field default
        // above, where the call site reads it). Baking it into the child body
        // would (mis)evaluate it in the child's own INIT scope.
        let subst_config: HashMap<String, SExpression> = effective_config
            .iter()
            .filter(|(_, v)| !orbital_core::schema::contains_callsite_payload(v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let cfg = &subst_config;
        {
            for transition in trait_def.state_machine.transitions.iter_mut() {
                if let Some(guard) = transition.guard.as_mut() {
                    rewrite_config_bindings(guard, cfg);
                }
                if let Some(effects) = transition.effects.as_mut() {
                    for effect in effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
            if let Some(ticks) = trait_def.ticks.as_mut() {
                for tick in ticks.iter_mut() {
                    if let Some(guard) = tick.guard.as_mut() {
                        rewrite_config_bindings(guard, cfg);
                    }
                    for effect in tick.effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
        }
    }
}

/// Rewrite every `emits[*].event` that is a `@config.<knob>` reference to its
/// resolved literal, and mirror the rewrite onto the matching
/// `stateMachine.events[*].key` entries (L1 lowering unions emit keys into the
/// events registry, so the raw token appears there too). `listens` does not
/// share this surface yet — the parser only accepts the reference sigil where
/// an emit event name is expected in `emits {}`; `listens {}` handler dispatch
/// still requires a leading `Identifier` token (source or bare event), so a
/// config-ref there would need parser work of its own (not part of this change).
///
/// Returns the knob names that were resolved, so the caller can propagate a
/// later `events:` rename onto the SAME `effective_config` entry — a
/// render-ui body literal bound to `@config.<knob>` (e.g. ButtonRender's
/// `action: @config.action`) is substituted from `effective_config` too, and
/// must end up holding the FINAL (resolved-then-renamed) event name, not
/// just the resolved default, or `ORB_X_RENDER_UI_EVENT_LITERAL_STALE` fires.
pub(crate) fn resolve_config_ref_emit_names(
    trait_def: &mut TraitDefinition,
    effective_config: &HashMap<String, SExpression>,
) -> Vec<String> {
    let mut resolved_triples: Vec<(String, String, String)> = Vec::new();
    {
        let Some(emits) = trait_def.emits.as_ref() else {
            return Vec::new();
        };
        for emit in emits.iter() {
            let Some(knob) = orbital_core::schema::config_ref_event_knob(&emit.event) else {
                continue;
            };
            match orbital_core::schema::resolve_config_ref_event_name(
                &emit.event,
                effective_config,
                trait_def.config.as_ref(),
            ) {
                Ok(resolved) => {
                    resolved_triples.push((knob.to_string(), emit.event.clone(), resolved))
                }
                // Left unresolved on error; the validation phase's
                // EmitConfigRefInvalid check reports the specific contract
                // clause that failed (unknown knob / non-string / no default).
                Err(_) => {}
            }
        }
    }
    if resolved_triples.is_empty() {
        return Vec::new();
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some((_, _, to)) = resolved_triples
                .iter()
                .find(|(_, from, _)| from == &emit.event)
            {
                emit.event = to.clone();
            }
        }
    }
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some((_, _, to)) = resolved_triples.iter().find(|(_, from, _)| from == &ev.key) {
                ev.key = to.clone();
            }
        }
    }
    resolved_triples
        .into_iter()
        .map(|(knob, ..)| knob)
        .collect()
}

/// Phase F.7: Apply post-substitution structural overrides to a resolved
/// `TraitDefinition`. These differ from `apply_overrides_to_trait` in that
/// they do NOT rewrite identifiers — they replace whole structural fields
/// with caller-supplied values:
///
/// - **`listens`** — replace the trait's `listens` array entirely with the
///   caller-supplied list. The common case in agent molecules is `[]`,
///   which clears the upstream's external event listeners. The inliner
///   accepts any caller list, including non-empty ones, but does not merge
///   with the upstream — it's a full replacement.
///
/// - **`emits_scope`** — set the `scope` field on every existing emit to
///   the caller-supplied value (`Internal` or `External`). The common case
///   in agent molecules is `Internal`, which confines the trait's emits to
///   the orbital's internal bus instead of broadcasting them. The inliner
///   does NOT add new emit declarations; it only modifies the scope of
///   what's already there.
///
/// Both overrides are applied AFTER `apply_overrides_to_trait` has run, so
/// any rename / linkedEntity / events substitutions are already in effect
/// by the time these structural changes apply.
fn apply_post_subst_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    listens: Option<&Vec<orbital_core::schema::ListenDefinition>>,
    emits_scope: Option<&orbital_core::schema::EventScope>,
) {
    if let Some(new_listens) = listens {
        trait_def.listens = Some(new_listens.clone());
    }
    if let Some(scope) = emits_scope {
        if let Some(emits) = trait_def.emits.as_mut() {
            for emit in emits.iter_mut() {
                emit.scope = Some(scope.clone());
            }
        }
    }
}

/// Walk an SExpression and collect every string literal of the exact form
/// `@trait.<SiblingName>` into `out`. App atoms embed a curated variation as
/// a sibling trait and reference it inside the main trait's render-ui as the
/// literal `"@trait.SiblingView"`. The form is the full string equal to
/// `@trait.<Name>` (no trailing path segment) — that's how a render node
/// addresses a sibling trait's projected view.
///
/// Also the single owner of the `@trait.X`-collection contract for the
/// lambda-splice pass (inline/splice.rs) and the shared-entity render-
/// authority attribution (validation/entity_mutation.rs) — no second
/// `@trait` walker exists.
pub(crate) fn collect_sibling_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(name) = s.strip_prefix("@trait.") {
                if !name.is_empty() && !name.contains('.') {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_sibling_trait_refs(item, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_sibling_trait_refs(v, out);
            }
        }
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// Scan a resolved trait's render trees (every SExpr inside transition effects
/// + guards, and tick effects + guards) for `@trait.<Sibling>` references.
///
/// Also scans every config-field DEFAULT value. A `@trait.<Sibling>` reference
/// can live inside a config-field's default render tree (e.g. std-browse's
/// `denseBodyContent`/`feedBodyContent` bodies, selected by a `browseLook`
/// guard) rather than directly in a transition effect. When the effect that
/// consumes such a field is still an unsubstituted `@config.<field>` binding
/// — as it is during a deferred import-load inline — the transition walk alone
/// misses those refs, so the siblings never get pulled into the intermediate
/// clone and dangle once a downstream consumer substitutes the binding.
/// Scanning config defaults makes sibling discovery independent of whether the
/// config binding has been substituted yet.
fn sibling_trait_refs_of(trait_def: &TraitDefinition) -> HashSet<String> {
    let mut out = HashSet::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = transition.guard.as_ref() {
            collect_sibling_trait_refs(guard, &mut out);
        }
        if let Some(effects) = transition.effects.as_ref() {
            for effect in effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_ref() {
        for tick in ticks {
            if let Some(guard) = tick.guard.as_ref() {
                collect_sibling_trait_refs(guard, &mut out);
            }
            for effect in &tick.effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    if let Some(config) = trait_def.config.as_ref() {
        for field in config.values() {
            if let Some(default) = field.default.as_ref() {
                collect_sibling_trait_refs(default, &mut out);
            }
        }
    }
    out
}

/// One trait Reference whose call-site `config` doesn't exactly match the
/// referenced trait's declared config knobs. Collected during inlining (the
/// only point where both the raw call-site key set and the atom's untouched
/// declaration — including cross-orbital `std/behaviors/*` atoms resolved
/// via the embedded phf registry — are visible together) and converted into
/// `ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY` / `ORB_T_CONFIG_MISSING_REQUIRED_KEY`
/// validation errors by `phases::validation::trait_ref_config`.
#[derive(Debug, Clone)]
pub struct TraitRefConfigDiagnostic {
    pub path: String,
    pub reference: String,
    pub unknown_keys: Vec<String>,
    pub missing_required_keys: Vec<String>,
    pub declared_keys: Vec<String>,
}

pub(crate) type InlinedTraits = (
    Vec<TraitReference>,
    HashMap<String, String>,
    Vec<EntityDefinition>,
    // Names of traits added by the auto-pull loop (siblings referenced via
    // `@trait.<Sibling>`). These keep their source name within an orbital so
    // the `@trait` ref resolves, but identical pulls in two orbitals collide
    // app-wide; `inline_schema` uniquifies those after all orbitals resolve.
    Vec<String>,
    Vec<TraitRefConfigDiagnostic>,
);

/// Inline trait references.
///
/// For each `TraitReference::Reference { ref, linkedEntity, config, .. }`,
/// this function resolves the imported `TraitDefinition`, applies any
/// `linkedEntity` (and, eventually, `config`) overrides via
/// `apply_overrides_to_trait`, and pushes a single
/// `TraitReference::Inline(merged_trait)` to the result. The downstream
/// `compose` and `resolve` phases only handle `TraitReference::Inline`, so
/// emitting a single merged Inline is what makes the override actually
/// reach the lowered OIR.
///
/// Phase 3.5.G.A.3 — Collision detection. After A.1 + A.2 produce the
/// merged `TraitDefinition` (with any `name` override applied), this
/// function tracks the resulting trait names in a `HashSet` and rejects
/// duplicates. The check covers three cases:
///
/// 1. Two `TraitReference::Reference` entries that both resolve to the same
///    upstream trait WITHOUT a `name` override on either: both end up with
///    the same `trait_def.name` (the upstream original) → collision.
/// 2. Two `Reference` entries where exactly one has a distinguishing `name`
///    override: the renamed one and the original-named one differ →
///    NO collision, both inlined.
/// 3. A literal `TraitReference::Inline` and a `Reference` that resolves
///    to the same name: also rejected as a collision.
///
/// On collision, the function returns
/// [`InlineError::DuplicateTraitName`] with the offending name and a
/// message that tells the molecule author exactly how to fix it: provide a
/// `name` override on at least one reference. The error fires AFTER the
/// rename has been applied, so users with two distinct `name` overrides
/// (e.g. `CartItemAddItem` and `CartItemEditItem`) are unaffected.
pub(crate) fn inline_traits(
    traits: &[TraitReference],
    ctx: &InlineContext,
    path_prefix: &str,
    // Consumer-scope entity name → id map, used to keep every rebound
    // `linkedEntity` name's dual-carry id sibling pointing at the consumer's
    // entity node (see `apply_overrides_to_trait`).
    entity_ids: &HashMap<String, String>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) -> Result<InlinedTraits, InlineError> {
    let mut result = Vec::new();
    let mut config_diagnostics: Vec<TraitRefConfigDiagnostic> = Vec::new();
    // Auxiliary entities surfaced by no-rebind sibling-pulls (see the pull loop).
    let mut pulled_aux_entities: Vec<EntityDefinition> = Vec::new();
    // Track every trait NAME that has already been pushed to `result`.
    // Names are unique across the orbital, so a second push of the same
    // name is a collision regardless of whether it came from a literal
    // Inline or a resolved Reference.
    let mut seen_names: HashSet<String> = HashSet::new();
    // Capture each `name:` override applied to a TraitReference so callers
    // (specifically `inline_orbital`) can propagate the rename through the
    // orbital's pages. Without this, `pages[].traits: [{ref: "OldName"}]`
    // entries become stale after the rename and fail validation with
    // `ORB_P_INVALID_TRAIT_REF`.
    let mut rename_map: HashMap<String, String> = HashMap::new();
    // App atoms keep a hand-rolled main trait and embed a curated variation as
    // a SIBLING trait, referencing it inside the main trait's render-ui as the
    // string literal `@trait.<SiblingName>`. When an organism composes ONLY the
    // main trait via a trait ref, the sibling is never instantiated and the
    // `@trait.<SiblingName>` dangles (`ORB_BINDING_TRAIT_UNKNOWN`). For each
    // resolved Reference trait we record the source alias + the linkedEntity
    // rebind it used, so a post-pass can auto-instantiate any same-alias
    // siblings the cloned body references — applying the SAME rebind a
    // hand-written sibling ref would have, keeping the sibling's original name
    // so `@trait.<SiblingName>` resolves.
    let mut sibling_pull_work: Vec<(String, String, Option<String>)> = Vec::new();

    for (idx, trait_ref) in traits.iter().enumerate() {
        let trait_path = format!("{path_prefix}.traits[{idx}]");
        match trait_ref {
            TraitReference::Inline(trait_def) => {
                if !seen_names.insert(trait_def.name.clone()) {
                    return Err(InlineError::DuplicateTraitName {
                        name: trait_def.name.clone(),
                        path: trait_path,
                    });
                }
                // Atom-scope config-default substitution. When an inline
                // trait carries `@config.X` bindings (e.g. `limit:
                // @config.pageSize`) and is compiled standalone (no import
                // call site supplying overrides), the codegen path emits
                // `Number(undefined)` for those bindings — see codegen/guard.rs
                // OirBindingRoot::Config which assumes the inline phase has
                // already substituted every @config.X with a literal.
                //
                // Only do this when the orbital is NOT being loaded as an
                // import for a downstream consumer (`ctx.defer_config_subst`).
                // On an import load, folding here would substitute `@config.X`
                // with this atom's OWN declared default BEFORE the consumer's
                // `apply_overrides_to_trait` pass (which runs afterwards on the
                // caller's `TraitReference`) can apply the consumer's override
                // — permanently clobbering the override for any knob that has a
                // declared default. That is the guard-fold bug: a guard
                // `["=", "@config.browseLook", "dense"]` was baked to the atom
                // default `["=", "table", "dense"]` instead of the consumer
                // override `["=", "dense", "dense"]`. Deferring leaves
                // `@config.X` live so the override substitution reaches it; the
                // consumer's own top-level resolution (Reference arm / sibling
                // pull, both defer=false) then folds it to the effective value.
                let mut td = trait_def.clone();
                if !ctx.defer_config_subst {
                    apply_inline_config_defaults(&mut td, recorder);
                }
                result.push(TraitReference::Inline(td));
            }
            TraitReference::Reference {
                reference,
                name: name_override,
                linked_entity,
                config,
                events: events_override,
                fields: fields_override,
                listens: listens_override,
                emits_scope: emits_scope_override,
                ..
            } => {
                if let Some(ref_str) = reference {
                    let parsed = parse_reference(ref_str)?;
                    match parsed {
                        ParsedReference::Trait { alias, name } => {
                            let inlined = ctx.get_trait(&alias, &name)?;
                            // STUDIO-1: capture provenance BEFORE override
                            // application. `source_label` is the imported
                            // behavior's human-readable name (set by
                            // `add_alias_multi`). `source_entity_def` is
                            // the entity from the orbital that owns this
                            // trait — already plumbed for the auto-merge
                            // path, reused here so the client-side mock
                            // generator (STUDIO-2) can produce rows for
                            // the trait's linkedEntity without walking
                            // the import graph at runtime.
                            let source_label = ctx.get_entry(&alias)?.source_label.clone();
                            let source_entity_def = ctx.get_entity_for_trait_orbital(&alias, &name);
                            let provenance = SourceBehaviorMetadata {
                                behavior: source_label,
                                alias: alias.clone(),
                                original_name: name.clone(),
                            };
                            match inlined {
                                TraitReference::Inline(mut trait_def) => {
                                    // Apply name / linkedEntity / events /
                                    // config overrides to the cloned trait
                                    // so the single Inline pushed below
                                    // carries the caller's intent
                                    // end-to-end.
                                    // Capture the original trait name (the
                                    // ref's `name` segment, BEFORE override)
                                    // so the rename map below records the
                                    // before/after pair when a `name:`
                                    // override is supplied.
                                    let original_name = trait_def.name.clone();
                                    // RULE 3: validate the events-rename SOURCE
                                    // keys against the imported trait's event
                                    // set BEFORE the override is applied (after
                                    // it, the original keys are gone). A `from`
                                    // key that names nothing silently no-ops —
                                    // surface it instead. The event set is the
                                    // union of declared emits and transition
                                    // triggers (`ORB_T_EVENT_RENAME_UNKNOWN_KEY`).
                                    if let Some(map) = events_override.as_ref() {
                                        // Build the same effective per-instance
                                        // config `apply_overrides_to_trait` will
                                        // use below (declared defaults, then
                                        // call-site overrides — but only when
                                        // this isn't a deferred import load,
                                        // where the consumer's own override
                                        // pass hasn't run yet) so a rename's
                                        // source key is checked against the
                                        // RESOLVED `@config.<knob>` emit name,
                                        // not the raw unresolved token.
                                        let mut effective_config: HashMap<String, SExpression> =
                                            HashMap::new();
                                        if let Some(declared) = trait_def.config.as_ref() {
                                            for (k, field) in declared.iter() {
                                                if let Some(default) = field.default.as_ref() {
                                                    effective_config
                                                        .insert(k.clone(), default.clone());
                                                }
                                            }
                                        }
                                        if !ctx.defer_config_subst {
                                            if let Some(cfg) = config.as_ref() {
                                                for (k, cf) in cfg.iter() {
                                                    if let Some(v) = cf.default.as_ref() {
                                                        effective_config
                                                            .insert(k.clone(), v.clone());
                                                    }
                                                }
                                            }
                                        }
                                        let mut known: HashSet<String> = HashSet::new();
                                        if let Some(emits) = &trait_def.emits {
                                            for e in emits {
                                                if orbital_core::schema::config_ref_event_knob(
                                                    &e.event,
                                                )
                                                .is_some()
                                                {
                                                    match orbital_core::schema::resolve_config_ref_event_name(
                                                        &e.event,
                                                        &effective_config,
                                                        trait_def.config.as_ref(),
                                                    ) {
                                                        Ok(resolved) => {
                                                            known.insert(resolved);
                                                        }
                                                        // Unresolvable ref (unknown knob /
                                                        // non-string / no default): keep the
                                                        // raw token so the EmitConfigRefInvalid
                                                        // validation check (not this one)
                                                        // reports the real cause, and a rename
                                                        // targeting a genuinely different key
                                                        // still errors here.
                                                        Err(_) => {
                                                            known.insert(e.event.clone());
                                                        }
                                                    }
                                                } else {
                                                    known.insert(e.event.clone());
                                                }
                                            }
                                        }
                                        for t in &trait_def.state_machine.transitions {
                                            known.insert(t.event.clone());
                                        }
                                        for from in map.keys() {
                                            // Skip self-maps / empties — they
                                            // are pruned as no-ops by the
                                            // override builder anyway.
                                            if from.is_empty() {
                                                continue;
                                            }
                                            if !known.contains(from.as_str()) {
                                                return Err(InlineError::EventRenameUnknownKey {
                                                    from: from.clone(),
                                                    trait_name: original_name.clone(),
                                                    path: trait_path.clone(),
                                                });
                                            }
                                        }
                                    }
                                    // E1/E2: check the call-site `config`
                                    // keys against the referenced trait's
                                    // DECLARED config BEFORE
                                    // `apply_overrides_to_trait` folds
                                    // matched keys into `trait_def.config`
                                    // and silently drops unmatched ones —
                                    // this is the only point where both the
                                    // raw call-site key set and the atom's
                                    // untouched declaration are visible
                                    // together, even for cross-orbital
                                    // std/behaviors refs (resolved above via
                                    // `ctx.get_trait`, which reaches the
                                    // embedded phf registry).
                                    {
                                        let empty_cfg: HashMap<String, ConfigField> =
                                            HashMap::new();
                                        let cfg = config.as_ref().unwrap_or(&empty_cfg);
                                        let mut declared_keys: std::collections::BTreeSet<String> =
                                            trait_def
                                                .config
                                                .as_ref()
                                                .map(|m| m.keys().cloned().collect())
                                                .unwrap_or_default();
                                        // A `.lolo` alias trait can declare its OWN typed
                                        // knobs inside the reference's config block (e.g.
                                        // `trait Alias = Wrapped -> X { config { steps :
                                        // [StepSpec] = [...] @label "Steps" } }`) — those
                                        // are NEW knobs on the alias, not overrides of
                                        // `Wrapped`'s declared config, so they must not be
                                        // flagged unknown. `is_declaration_form` is the
                                        // structural signal the lowerer already uses to
                                        // distinguish a re-declaration from a plain wiring
                                        // override (`field_type == "unknown"`, no
                                        // annotations) — no key-name or value sniffing.
                                        declared_keys.extend(
                                            cfg.iter()
                                                .filter(|(_, f)| f.is_declaration_form())
                                                .map(|(k, _)| k.clone()),
                                        );
                                        let mut unknown_keys: Vec<String> = cfg
                                            .keys()
                                            .filter(|k| !declared_keys.contains(*k))
                                            .cloned()
                                            .collect();
                                        unknown_keys.sort();
                                        let mut missing_required_keys: Vec<String> = trait_def
                                            .config
                                            .as_ref()
                                            .map(|m| {
                                                m.iter()
                                                    .filter(|(k, f)| {
                                                        f.required.unwrap_or(false)
                                                            && !cfg.contains_key(k.as_str())
                                                    })
                                                    .map(|(k, _)| k.clone())
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        missing_required_keys.sort();
                                        if !unknown_keys.is_empty()
                                            || !missing_required_keys.is_empty()
                                        {
                                            config_diagnostics.push(TraitRefConfigDiagnostic {
                                                path: trait_path.clone(),
                                                reference: original_name.clone(),
                                                unknown_keys,
                                                missing_required_keys,
                                                declared_keys: declared_keys.into_iter().collect(),
                                            });
                                        }
                                    }
                                    apply_overrides_to_trait(
                                        &mut trait_def,
                                        linked_entity.as_ref(),
                                        entity_ids,
                                        config.as_ref(),
                                        name_override.as_deref(),
                                        events_override.as_ref(),
                                        fields_override.as_ref(),
                                        ctx.defer_config_subst,
                                        recorder,
                                    );
                                    if trait_def.name != original_name {
                                        rename_map.insert(original_name, trait_def.name.clone());
                                    }
                                    // Phase F.7: replace the trait's
                                    // listens and/or set every emit's scope
                                    // to the caller-supplied values. These
                                    // are STRUCTURAL overrides applied
                                    // after the substitution pass; they do
                                    // not require identifier rewriting.
                                    apply_post_subst_overrides_to_trait(
                                        &mut trait_def,
                                        listens_override.as_ref(),
                                        emits_scope_override.as_ref(),
                                    );
                                    // STUDIO-1/2: stamp provenance onto the
                                    // resolved trait. Runtime + codegen
                                    // ignore both fields; only Studio's L2
                                    // canvas reads them to bucket cards by
                                    // alias and populate mock rows for
                                    // imported entities.
                                    trait_def.source_behavior = Some(provenance);
                                    trait_def.source_entity_definition = source_entity_def;
                                    // Collision check happens AFTER
                                    // override application so
                                    // post-rename names are what we
                                    // compare. Two references with
                                    // distinct `name` overrides will
                                    // never collide here.
                                    if !seen_names.insert(trait_def.name.clone()) {
                                        return Err(InlineError::DuplicateTraitName {
                                            name: trait_def.name.clone(),
                                            path: trait_path.clone(),
                                        });
                                    }
                                    // Record this resolved ref so the sibling-pull
                                    // pass below can instantiate any same-alias
                                    // `@trait.X` siblings its body references,
                                    // rebinding them with the SAME linkedEntity.
                                    for sibling in sibling_trait_refs_of(&trait_def) {
                                        sibling_pull_work.push((
                                            alias.clone(),
                                            sibling,
                                            linked_entity.clone(),
                                        ));
                                    }
                                    result.push(TraitReference::Inline(trait_def));
                                }
                                other => {
                                    // Defensive: `ctx.get_trait` is
                                    // expected to return Inline. If it
                                    // returns a Reference (e.g. the
                                    // imported orbital had not been
                                    // inlined yet), pass it through
                                    // unchanged so a later pass can
                                    // resolve it. We still record the
                                    // name (via `trait_name()`) so a
                                    // subsequent collision against the
                                    // same name is rejected.
                                    if let Some(name) = other.trait_name() {
                                        if !seen_names.insert(name.to_string()) {
                                            return Err(InlineError::DuplicateTraitName {
                                                name: name.to_string(),
                                                path: trait_path.clone(),
                                            });
                                        }
                                    }
                                    result.push(other);
                                }
                            }
                        }
                        _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                    }
                } else {
                    // No reference string, keep as-is (named trait
                    // reference). Track the name if available so a
                    // collision against an Inline or a resolved
                    // Reference with the same name is still rejected.
                    if let Some(name) = trait_ref.trait_name() {
                        if !seen_names.insert(name.to_string()) {
                            return Err(InlineError::DuplicateTraitName {
                                name: name.to_string(),
                                path: trait_path.clone(),
                            });
                        }
                    }
                    result.push(trait_ref.clone());
                }
            }
        }
    }

    // Sibling-trait auto-pull. Process the worklist gathered above: for each
    // `@trait.<Sibling>` a resolved trait references, instantiate the sibling
    // from the SAME source alias if it isn't already present in the consumer,
    // applying the SAME linkedEntity rebind the parent ref used. The pulled
    // sibling KEEPS its original name (so `@trait.<Sibling>` still resolves)
    // and gets NO page. Recurse: a pulled sibling's body may reference further
    // siblings. `visited` guards against cycles; `seen_names` dedups against
    // traits the consumer already declares (explicit composition) and against
    // siblings already pulled.
    let mut pulled_names: Vec<String> = Vec::new();
    {
        let mut worklist = sibling_pull_work;
        let mut visited: HashSet<(String, String)> = HashSet::new();
        while let Some((alias, sibling_name, linked_entity)) = worklist.pop() {
            // Already present in the consumer (explicit comp or prior pull): skip.
            if seen_names.contains(&sibling_name) {
                continue;
            }
            if !visited.insert((alias.clone(), sibling_name.clone())) {
                continue;
            }
            // The sibling must exist as a trait in the SAME source atom.
            let Ok(TraitReference::Inline(mut sibling_def)) = ctx.get_trait(&alias, &sibling_name)
            else {
                // Not a same-alias sibling (or not resolvable to an inline
                // trait) — leave the `@trait.X` for the validator to flag.
                continue;
            };
            // Apply the SAME rebind a hand-written sibling ref would: rewrite
            // every `@X.path` / `["ref", X]` / fetch/persist literal to the
            // consumer's entity. KEEP the original name (no `name` override) so
            // `@trait.<Sibling>` resolves against it.
            apply_overrides_to_trait(
                &mut sibling_def,
                linked_entity.as_ref(),
                entity_ids,
                None,
                None,
                None,
                None,
                ctx.defer_config_subst,
                recorder,
            );
            // Provenance + source entity, mirroring the main ref path so Studio
            // and the mock generator treat the pulled sibling like any ref.
            if let Ok(entry) = ctx.get_entry(&alias) {
                sibling_def.source_behavior = Some(SourceBehaviorMetadata {
                    behavior: entry.source_label.clone(),
                    alias: alias.clone(),
                    original_name: sibling_name.clone(),
                });
            }
            sibling_def.source_entity_definition =
                ctx.get_entity_for_trait_orbital(&alias, &sibling_name);
            // No-rebind sibling: it keeps its OWN bound entity (e.g. a
            // step-flow's `ProgressVerticalView`). That entity is auxiliary in
            // the source behavior; carry it into the consumer so the sibling's
            // `(fetch X)` / `linkedEntity` resolve. With a rebind, the sibling
            // was retargeted to the consumer's entity and needs nothing here.
            if linked_entity.is_none() {
                if let Some(bound) = &sibling_def.linked_entity {
                    if let Some(aux) = ctx.get_entity_by_name_for_alias(&alias, bound) {
                        pulled_aux_entities.push(aux);
                    }
                }
            }
            // Recurse into the rebound sibling's own `@trait.Y` references.
            for next in sibling_trait_refs_of(&sibling_def) {
                worklist.push((alias.clone(), next, linked_entity.clone()));
            }
            seen_names.insert(sibling_def.name.clone());
            pulled_names.push(sibling_def.name.clone());
            result.push(TraitReference::Inline(sibling_def));
        }
    }

    // Phase 9.5: drop source-scoped listeners whose source trait was not
    // imported into this orbital. When an atom's trait (e.g. the Browse trait
    // in std-agent-memory) listens for a sibling trait's emits
    // (`AgentMemoryAgent PINNED -> INIT`), and a consumer imports ONLY the
    // Browse trait via a trait ref, the sibling trait is not present in the
    // consumer's orbital, so the listen can never fire. Silently dropping it
    // prevents a false-positive validation error while preserving the
    // source-filter semantics wherever the sibling IS imported.
    //
    // `ListenSource::Orbital` entries are left alone — they point at a
    // trait in another orbital (cross-orbital filter) and are not validated
    // against the local trait set.
    let local_trait_names: HashSet<String> = result
        .iter()
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect();

    for trait_ref in result.iter_mut() {
        if let TraitReference::Inline(trait_def) = trait_ref {
            if let Some(listens) = trait_def.listens.as_mut() {
                listens.retain(|listen| match &listen.source {
                    Some(orbital_core::schema::ListenSource::Trait { trait_name, .. }) => {
                        local_trait_names.contains(trait_name)
                    }
                    _ => true,
                });
            }
        }
    }

    Ok((
        result,
        rename_map,
        pulled_aux_entities,
        pulled_names,
        config_diagnostics,
    ))
}
