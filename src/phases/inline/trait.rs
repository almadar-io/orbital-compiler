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
    // Build the effective config map (key → concrete SExpression value) for
    // `@config.X` substitution. Sources in precedence order:
    //   1. Atom's declared defaults (lowest priority).
    //   2. Call-site overrides (highest priority — the `default` field of
    //      each `ConfigField` entry).
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
    }
    if !defer_config_subst && !effective_config.is_empty() {
        let cfg = &effective_config;
        record_config_provenance(trait_def, cfg, recorder);
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
fn collect_sibling_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
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
    out
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
    recorder: &mut Vec<ConfigProvenanceRecord>,
) -> Result<InlinedTraits, InlineError> {
    let mut result = Vec::new();
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
                // @config.pageSize`) and is compiled standalone (no
                // import call site supplying overrides), the codegen
                // path emits `Number(undefined)` for those bindings —
                // see codegen/guard.rs OirBindingRoot::Config which
                // assumes the inline phase has already substituted
                // every @config.X with a literal. For Reference traits
                // the substitution happens in `apply_overrides_to_trait`
                // (line ~1097); for Inline traits it didn't run at all.
                // Apply the trait's own declared defaults here so atom
                // standalone codegen produces `Number(10)` instead of
                // `Number(undefined)`. Cloning into a new Inline keeps
                // the rest of the pipeline (which expects a fresh
                // owned trait_def) unchanged.
                let mut td = trait_def.clone();
                apply_inline_config_defaults(&mut td, recorder);
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
                                        let mut known: HashSet<&str> = HashSet::new();
                                        if let Some(emits) = &trait_def.emits {
                                            for e in emits {
                                                known.insert(e.event.as_str());
                                            }
                                        }
                                        for t in &trait_def.state_machine.transitions {
                                            known.insert(t.event.as_str());
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
                                    apply_overrides_to_trait(
                                        &mut trait_def,
                                        linked_entity.as_ref(),
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
                    Some(orbital_core::schema::ListenSource::Trait { trait_name }) => {
                        local_trait_names.contains(trait_name)
                    }
                    _ => true,
                });
            }
        }
    }

    Ok((result, rename_map, pulled_aux_entities, pulled_names))
}
