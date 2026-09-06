//! Orbital-import materialization: `orbital X = Alias.orbitals.Y { … }`
//! (`docs/Almadar_Orbital_Import.md` §3-§6). Compiled-path twin of the JS
//! resolver's `materializeOrbitalRef`
//! (`packages/almadar-runtime/src/resolver/reference-resolver.ts:2854`) —
//! every rename/id/rewrite rule here is written to match that function
//! byte-for-byte in observable output (trait/entity/page names, ids, navigate
//! literals), since a schema resolved by either path must be
//! indistinguishable.
//!
//! `resolve_orbital_reference` flattens the (already fully inlined) upstream
//! orbital's whole trait/entity/page closure into the consumer orbital,
//! unconditionally prefixed by the LOCAL orbital name (§4.3 — never
//! collision-triggered, so two imports of one upstream are trivially
//! disjoint). It reuses three existing primitives at trait scope rather than
//! introducing a parallel rename pass: `rewrite_trait_embed_tokens`
//! (`@trait.` tokens), `rewrite_identifiers` (entity-root + event tokens),
//! and `rewrite_entity_field_paths` (`@entity.<field>` tokens, the `fields
//! {}` override surface).

use super::collect_sibling_trait_refs;
use super::context::{parse_reference, InlineContext, ParsedReference};
use super::identity_normalize::rewrite_trait_embed_tokens;
use super::r#trait::forwarded_sibling_config_from;
use super::rewrite::{
    rewrite_entity_field_paths, rewrite_identifiers, rewrite_payload_entity_refs,
    rewrite_trait_event_fields,
};
use super::InlineError;
use orbital_core::schema::{
    ConfigField, EntityDefinition, EntityRef, FieldDefinition, ListenSource, Orbital,
    OrbitalRefObject, PageRef, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Diagnostic collected while resolving one `orbital X = Alias.orbitals.Y
/// { … }` reference: a call-site `config {}` key the upstream orbital never
/// declared, and/or an `omit {}` trait a surviving trait still embeds via
/// `@trait.<X>`. Collected non-fatally at inline time (inlining still
/// completes on a best-effort basis) — W4-R's validators turn these into
/// hard `ORB_O_CONFIG_UNKNOWN_KEY` / `ORB_O_OMIT_EMBEDDED_TRAIT` errors.
#[derive(Debug, Clone)]
pub struct OrbitalRefDiagnostic {
    pub path: String,
    pub reference: String,
    pub unknown_config_keys: Vec<String>,
    /// (surviving trait name, omitted trait name it still embeds via `@trait.`).
    pub omitted_but_embedded: Vec<(String, String)>,
}

/// Diagnostic collected while resolving a `uses A from "…" { config { … } }`
/// call-site override: a key the imported behavior's app-level `config {}`
/// never declared. W4-R turns this into `ORB_U_CONFIG_UNKNOWN_KEY`.
#[derive(Debug, Clone)]
pub struct UsesConfigDiagnostic {
    pub path: String,
    pub alias: String,
    pub unknown_keys: Vec<String>,
}

/// Resolve one reference-form `orbital`, splicing the imported orbital's
/// whole closure into `orbital` in place. `orbital.reference` must already
/// be cleared by the caller (`inline_orbital` does this via `Option::take`
/// before calling in, matching the wire contract's transience rule).
///
/// `ctx`'s aliases were populated by the SAME `uses` loop every other
/// reference form in this orbital resolves through, so `Alias` here is
/// whatever `Alias.traits.X` / `Alias.pages.X` would also see — and every
/// orbital `ctx` holds is already fully inlined (`inline_orbital` recurses
/// into every loaded import before it's stashed), so a nested orbital-import
/// upstream resolves for free.
pub(crate) fn resolve_orbital_reference(
    orbital: &mut Orbital,
    obj: &OrbitalRefObject,
    ctx: &InlineContext,
    schema_config: Option<&HashMap<String, ConfigField>>,
    diagnostics: &mut Vec<OrbitalRefDiagnostic>,
) -> Result<(), InlineError> {
    let local_name = orbital.name.clone();
    let diag_path = format!("orbitals[name={local_name}]");

    let ParsedReference::Orbital { alias, name } = parse_reference(&obj.reference)? else {
        return Err(InlineError::InvalidReference(obj.reference.clone()));
    };
    let upstream = ctx.get_orbital(&alias, &name)?;

    let EntityRef::Inline(upstream_entity) = upstream.entity.clone() else {
        // The upstream orbital came back from `ctx.get_orbital`, which only
        // ever surfaces already-inlined orbitals — a non-Inline entity here
        // means the upstream's own inline pass never resolved it. Surface
        // the same not-found the plain `Alias.entity` reference form uses.
        return Err(InlineError::EntityNotFound(alias.clone()));
    };

    // 1. Trim the trait set (omit/only) BEFORE the unconditional prefix —
    // `subs` below only ever maps survivors. Names here are still upstream's.
    let only_set: Option<HashSet<&str>> =
        obj.only.as_ref().map(|v| v.iter().map(String::as_str).collect());
    let omit_set: HashSet<&str> = obj
        .omit
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut kept: Vec<TraitDefinition> = Vec::new();
    let mut removed_names: HashSet<String> = HashSet::new();
    for tr in &upstream.traits {
        // Upstream is fully inlined (see doc comment above) — every trait is
        // `Inline` by construction; a stray `Reference` is skipped rather
        // than propagated half-resolved.
        let TraitReference::Inline(td) = tr else {
            continue;
        };
        let keep = match &only_set {
            Some(set) => set.contains(td.name.as_str()),
            None => !omit_set.contains(td.name.as_str()),
        };
        if keep {
            kept.push(td.clone());
        } else {
            removed_names.insert(td.name.clone());
        }
    }

    // `ORB_O_OMIT_EMBEDDED_TRAIT`: a surviving trait's `@trait.<X>` embed
    // names a trait `omit`/`only` just dropped.
    let mut omitted_but_embedded: Vec<(String, String)> = Vec::new();
    for td in &kept {
        for embed in embedded_trait_refs_of(td) {
            if removed_names.contains(&embed) {
                omitted_but_embedded.push((td.name.clone(), embed));
            }
        }
    }

    // 2. Unconditional prefix (§4.3) — one combined rename map covering every
    // surviving trait + the primary + every auxiliary entity. Reused for
    // trait-embed rewriting, entity-root rewriting, AND listen-source
    // rewriting (matching the JS twin's single `subs` map).
    let aux_entities: Vec<EntityDefinition> = upstream
        .auxiliary_entities
        .iter()
        .flatten()
        .filter_map(|e| match e {
            EntityRef::Inline(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    let mut subs: HashMap<String, String> = HashMap::new();
    for td in &kept {
        subs.insert(td.name.clone(), format!("{local_name}{}", td.name));
    }
    let primary_final = obj
        .entity
        .clone()
        .unwrap_or_else(|| format!("{local_name}{}", upstream_entity.name));
    subs.insert(upstream_entity.name.clone(), primary_final.clone());
    for e in &aux_entities {
        subs.insert(e.name.clone(), format!("{local_name}{}", e.name));
    }
    let field_subs: HashMap<String, String> = obj.fields.clone().unwrap_or_default();

    // Embedder map for step 6's per-embedder config forward below: child
    // trait final name → the final name(s) of every surviving sibling that
    // embeds it via `@trait.<child>` (order = `kept` order, deterministic).
    // Built here — `subs` is final and `kept.name` is still upstream's
    // pre-rename name — so both sides translate through `subs` in one pass,
    // before step 4 overwrites `td.name` with the renamed value.
    let mut embedders_of: HashMap<String, Vec<String>> = HashMap::new();
    for td in &kept {
        let embedder_final = subs.get(&td.name).cloned().unwrap_or_else(|| td.name.clone());
        for child in embedded_trait_refs_of(td) {
            if let Some(child_final) = subs.get(&child) {
                embedders_of.entry(child_final.clone()).or_default().push(embedder_final.clone());
            }
        }
    }

    // 3. Config: upstream's DECLARED knobs folded with the call-site
    // override. An override of an undeclared knob is `ORB_O_CONFIG_UNKNOWN_KEY`.
    let declared_config = upstream.config.clone();
    let mut unknown_config_keys: Vec<String> = Vec::new();
    if let Some(overrides) = obj.config.as_ref() {
        for k in overrides.keys() {
            if declared_config.as_ref().is_none_or(|m| !m.contains_key(k)) {
                unknown_config_keys.push(k.clone());
            }
        }
    }
    let folded_config: Option<HashMap<String, ConfigField>> = declared_config.as_ref().map(|decl| {
        decl.iter()
            .map(|(k, field)| {
                let mut field = field.clone();
                if let Some(over) = obj.config.as_ref().and_then(|o| o.get(k)) {
                    field.default = over.default.clone();
                }
                (k.clone(), field)
            })
            .collect()
    });

    if !unknown_config_keys.is_empty() || !omitted_but_embedded.is_empty() {
        diagnostics.push(OrbitalRefDiagnostic {
            path: diag_path.clone(),
            reference: obj.reference.clone(),
            unknown_config_keys,
            omitted_but_embedded,
        });
    }

    // 4. Ids: every cloned node (trait AND entity) gets a FRESH derived id
    // rooted in its own source id when it has one, else a deterministic
    // fallback seed (`derive_materialized_id`) — so two imports of one
    // upstream are pairwise disjoint by id, not just by name. Renaming
    // `td.name` here (AFTER `subs` above was built off the OLD names) is
    // safe because nothing downstream re-derives `subs` from `kept`.
    let mut id_by_name: HashMap<String, String> = HashMap::new();
    for td in kept.iter_mut() {
        let final_name = subs
            .get(&td.name)
            .cloned()
            .expect("every kept trait has a subs entry (built from the same `kept` list above)");
        let final_id =
            derive_materialized_id(td.id.as_deref(), upstream.id.as_deref(), &td.name, &local_name);
        td.name = final_name.clone();
        td.id = Some(final_id.clone());
        id_by_name.insert(final_name, final_id);
    }

    let mut entity_ids: HashMap<String, String> = HashMap::new();
    let primary_id = derive_materialized_id(
        upstream_entity.id.as_deref(),
        upstream.id.as_deref(),
        &upstream_entity.name,
        &local_name,
    );
    entity_ids.insert(primary_final.clone(), primary_id.clone());
    let mut final_aux_entities: Vec<EntityDefinition> = Vec::new();
    for e in &aux_entities {
        let final_name = subs
            .get(&e.name)
            .cloned()
            .expect("every aux entity has a subs entry (built from the same `aux_entities` list above)");
        let final_id =
            derive_materialized_id(e.id.as_deref(), upstream.id.as_deref(), &e.name, &local_name);
        entity_ids.insert(final_name.clone(), final_id.clone());
        let mut ed = e.clone();
        ed.name = final_name;
        ed.id = Some(final_id);
        rewrite_entity_fields_in_entity(&mut ed, &field_subs);
        final_aux_entities.push(ed);
    }
    // (A) every auxiliary entity's own relation fields (e.g. a self-relation,
    // or a pointer to the primary entity or to ANOTHER aux) must resolve
    // inside the materialized closure — run as a SEPARATE pass, after the
    // loop above, so `entity_ids` is fully populated (primary + every aux)
    // before any relation's `entity_id` sibling is looked up; renaming
    // in-loop would see a partial map for a target declared later in
    // `aux_entities`.
    for ed in final_aux_entities.iter_mut() {
        rename_entity_refs_in_entity(ed, &subs, &entity_ids);
    }

    // 5. Pages: `obj.pages` remaps upstream→local path; an unmapped path
    // keeps its upstream value. Built before the per-trait pipeline below —
    // it does not depend on any trait mutation, only on `subs`/`id_by_name`.
    let mut path_map: HashMap<String, String> = HashMap::new();
    if let Some(pages) = &upstream.pages {
        for p in pages {
            if let PageRef::Inline(pd) = p {
                let mapped = obj
                    .pages
                    .as_ref()
                    .and_then(|m| m.get(&pd.path))
                    .cloned()
                    .unwrap_or_else(|| pd.path.clone());
                path_map.insert(pd.path.clone(), mapped);
            }
        }
    }
    let orbital_rename: Option<(String, String)> = if upstream.name != local_name {
        Some((upstream.name.clone(), local_name.clone()))
    } else {
        None
    };

    // Snapshot every kept trait's declared config, keyed by its FINAL
    // (post-rename) name, for the step-6 embedder lookup below — a clone
    // taken before the loop starts iter_mut over `kept`, since the loop
    // would otherwise need to borrow `kept` both mutably (the trait being
    // processed) and immutably (an embedder's config) at once.
    let parent_configs: HashMap<String, HashMap<String, ConfigField>> = kept
        .iter()
        .filter_map(|td| td.config.clone().map(|cfg| (td.name.clone(), cfg)))
        .collect();

    // 6. Per-trait pipeline, in the same order as the JS twin
    // (`materializeOrbitalRef`): embed tokens → entity roots → entity
    // fields → listen sources → navigate targets → config forward → events.
    for td in kept.iter_mut() {
        rewrite_trait_embed_tokens(td, &subs);
        rename_entities_in_trait(td, &subs, &entity_ids);
        // (B) the trait's own source-entity provenance copy must track the
        // same rename, or a trait whose atom-original entity coincided with
        // upstream's own entity name reads as a spurious rebind post-import.
        rename_source_entity_definition(td, &subs, &entity_ids);
        if !field_subs.is_empty() {
            rewrite_entity_fields_in_trait(td, &field_subs);
        }
        rewrite_listen_sources(
            td,
            &subs,
            &id_by_name,
            orbital_rename.as_ref().map(|(from, to)| (from.as_str(), to.as_str())),
        );
        if !path_map.is_empty() {
            rewrite_trait_navigate_targets(td, &path_map);
        }
        // (D) config-held page-path literals (e.g. an AppLayout's
        // `navItems[].href`) sit outside the navigate-effect surface above.
        rewrite_trait_config_page_paths(td, &path_map);
        // Config forward: resolve against this trait's own embedder(s) in
        // `kept` first (a splice DOES have embedders — a surviving sibling
        // that renders `@trait.<this>` — unlike the old `parent_name: None`
        // assumption), falling through to the orbital/schema rungs when no
        // embedder yields a delta (same precedence the pull site uses, one
        // rung further out). `td.name` is already the FINAL name here (step
        // 4 ran before this loop), matching `embedders_of`/`parent_configs`.
        let mut delta = None;
        if let Some(embedders) = embedders_of.get(&td.name) {
            for embedder in embedders {
                let Some(parent_cfg) = parent_configs.get(embedder) else {
                    continue;
                };
                delta = forwarded_sibling_config_from(
                    &*td,
                    Some(parent_cfg),
                    folded_config.as_ref(),
                    schema_config,
                );
                if delta.is_some() {
                    break;
                }
            }
        }
        if delta.is_none() {
            delta = forwarded_sibling_config_from(&*td, None, folded_config.as_ref(), schema_config);
        }
        if let Some(delta) = delta {
            let cfg = td.config.get_or_insert_with(HashMap::new);
            for (k, v) in delta {
                cfg.insert(k, v);
            }
        }
        if let Some(events) = obj.events.as_ref() {
            rename_events_in_trait(td, events);
        }
    }

    let mut final_pages: Vec<PageRef> = Vec::new();
    if let Some(pages) = &upstream.pages {
        for p in pages {
            let PageRef::Inline(page_def) = p else {
                continue;
            };
            let mut pd = page_def.clone();
            pd.path = path_map.get(&pd.path).cloned().unwrap_or_else(|| pd.path.clone());
            // Unconditional prefix (§4.3), same rule as traits/entities — a
            // materialized page's NAME is upstream vocabulary until here, so
            // two imports of one upstream (each with its own `pages {}` path
            // remap) would otherwise collide on `PageDefinition.name` even
            // though their paths are disjoint. JS twin: identical rule in
            // `materializeOrbitalRef`.
            pd.name = format!("{local_name}{}", pd.name);
            // Same rule as traits/entities: a page id is re-derived per
            // import so two imports of one upstream never share one (JS twin:
            // `asPageId(deriveMaterializedId(...))` in `materializeOrbitalRef`).
            pd.id = Some(derive_materialized_id(
                page_def.id.as_deref(),
                upstream.id.as_deref(),
                &page_def.name,
                &local_name,
            ));
            if let Some(traits) = pd.traits.as_mut() {
                traits.retain_mut(|tr| {
                    if let TraitReference::Reference {
                        reference,
                        ref_id,
                        name,
                        linked_entity,
                        ..
                    } = tr
                    {
                        let key = reference.clone().or_else(|| name.clone());
                        if let Some(k) = &key {
                            // A page mount naming a trait `omit`/`only` just
                            // dropped is dropped from the page too.
                            if removed_names.contains(k) {
                                return false;
                            }
                            if let Some(to) = subs.get(k) {
                                if reference.is_some() {
                                    *reference = Some(to.clone());
                                } else {
                                    *name = Some(to.clone());
                                }
                                *ref_id = id_by_name.get(to).cloned();
                            }
                        }
                        if let Some(le) = linked_entity.clone() {
                            if let Some(to) = subs.get(&le) {
                                *linked_entity = Some(to.clone());
                            }
                        }
                    }
                    true
                });
            }
            if let Some(pe) = pd.primary_entity.clone() {
                if let Some(to) = subs.get(&pe) {
                    pd.primary_entity = Some(to.clone());
                }
            }
            final_pages.push(PageRef::Inline(pd));
        }
    }

    // 7. Splice. `orbital.reference` was already cleared by the caller;
    // `orbital.name` stays the local name (never touched here).
    let mut primary_entity = upstream_entity;
    primary_entity.name = primary_final;
    primary_entity.id = Some(primary_id);
    rewrite_entity_fields_in_entity(&mut primary_entity, &field_subs);
    // (A) the primary entity's own relation fields (e.g. `authorId: Author`,
    // a self-relation `parentId: Note`, or an array relation `tagIds:
    // [NoteTagRef]`) still name upstream's un-prefixed entities. `entity_ids`
    // is fully populated by this point (primary + every aux, computed above).
    rename_entity_refs_in_entity(&mut primary_entity, &subs, &entity_ids);

    orbital.entity = EntityRef::Inline(primary_entity);
    orbital.auxiliary_entities = if final_aux_entities.is_empty() {
        None
    } else {
        Some(final_aux_entities.into_iter().map(EntityRef::Inline).collect())
    };
    orbital.traits = kept.into_iter().map(TraitReference::Inline).collect();
    orbital.pages = if final_pages.is_empty() { None } else { Some(final_pages) };
    orbital.config = folded_config;
    // Orbital-level `emits`/`listens`/`exposes` name UPSTREAM's pre-rename
    // vocabulary — carrying them through would be wrong, and nothing reads
    // them off a materialized orbital (matches the JS twin's drop; §Report).
    orbital.emits = None;
    orbital.listens = None;
    orbital.exposes = None;
    // (C) the LOCAL placeholder orbital is its own ledgered node — the
    // id-stamping phase (post-lowering, pre-inline) already gave it an id
    // that the ledger has a row for. Only its CONTENTS are derived from
    // upstream; overwriting `orbital.id` here orphans that ledger row
    // (`ORB_ID_LEDGER_ORPHAN`). Derive from upstream only when the local
    // orbital carries no id of its own.
    if orbital.id.is_none() {
        orbital.id =
            upstream.id.as_deref().map(|uid| orbital_core::identity::derive_id(uid, &local_name));
    }

    Ok(())
}

/// Deterministic id for a materialized trait/entity node: rooted in the
/// source node's own id when it has one (`derive_id` keeps that id's KIND —
/// trait stays trait, entity stays entity); when the source has none yet
/// (pre-Phase-7, common today), a seed string derived from the upstream
/// orbital id + the pre-rename source name instead — feeding `upstream.id`
/// (orbital-kind, `orb_`) straight into `derive_id` would stamp a
/// trait/entity with the wrong id KIND, since `derive_id` inherits its
/// parent's kind. The seed string never starts with a reserved id prefix, so
/// `derive_id` falls back to its own default (`trait`). JS twin:
/// `deriveMaterializedId` (`reference-resolver.ts:1547`) — same seed shape,
/// ported byte-for-byte.
fn derive_materialized_id(
    source_id: Option<&str>,
    upstream_orbital_id: Option<&str>,
    source_name: &str,
    local_name: &str,
) -> String {
    let seed;
    let parent: &str = match source_id {
        Some(id) => id,
        None => {
            seed = format!(
                "orbital-import-seed:{}:{source_name}",
                upstream_orbital_id.unwrap_or("no-orbital-id")
            );
            &seed
        }
    };
    orbital_core::identity::derive_id(parent, local_name)
}

/// `@trait.<Sibling>` refs a trait carries, scanned across the same surface
/// `collect_sibling_trait_refs`'s private wrapper (`trait.rs`'s
/// `sibling_trait_refs_of`) does: transition guards/effects, tick
/// guards/effects, and config-field defaults (`std-browse`'s
/// `bodyContent`-shaped knobs can carry an embed inside their default tree).
fn embedded_trait_refs_of(td: &TraitDefinition) -> HashSet<String> {
    let mut out = HashSet::new();
    for t in &td.state_machine.transitions {
        if let Some(guard) = t.guard.as_ref() {
            collect_sibling_trait_refs(guard, &mut out);
        }
        if let Some(effects) = t.effects.as_ref() {
            for e in effects {
                collect_sibling_trait_refs(e, &mut out);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_ref() {
        for tick in ticks {
            if let Some(guard) = tick.guard.as_ref() {
                collect_sibling_trait_refs(guard, &mut out);
            }
            for e in &tick.effects {
                collect_sibling_trait_refs(e, &mut out);
            }
        }
    }
    if let Some(cfg) = td.config.as_ref() {
        for field in cfg.values() {
            if let Some(default) = field.default.as_ref() {
                collect_sibling_trait_refs(default, &mut out);
            }
        }
    }
    out
}

/// Rename every entity NAME in `entity_subs` (old → new) across a trait's
/// `linkedEntity` + every entity-root/positional-argument token its guards
/// and effects carry (`rewrite_identifiers`'s `@<Entity>` prefix match plus
/// its `fetch`/`persist`/`ref`/render-ui-prop positional rewrites — a
/// superset of the JS twin's effects-only `renameEntityInEffects`, and the
/// SAME primitive every other entity-rebind call site in this crate already
/// uses, so this is the general case, not a new special-cased walker).
fn rename_entities_in_trait(
    td: &mut TraitDefinition,
    entity_subs: &HashMap<String, String>,
    entity_ids: &HashMap<String, String>,
) {
    if entity_subs.is_empty() {
        return;
    }
    if let Some(le) = td.linked_entity.clone() {
        if let Some(to) = entity_subs.get(&le) {
            td.linked_entity = Some(to.clone());
            td.linked_entity_id = entity_ids.get(to).cloned();
        }
    }
    let no_events: HashMap<String, String> = HashMap::new();
    for t in td.state_machine.transitions.iter_mut() {
        if let Some(guard) = t.guard.as_mut() {
            rewrite_identifiers(guard, entity_subs, &no_events);
        }
        if let Some(effects) = t.effects.as_mut() {
            for e in effects.iter_mut() {
                rewrite_identifiers(e, entity_subs, &no_events);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_identifiers(guard, entity_subs, &no_events);
            }
            for e in tick.effects.iter_mut() {
                rewrite_identifiers(e, entity_subs, &no_events);
            }
        }
    }
    rewrite_payload_entity_refs(td, entity_subs);
}

/// Apply an `OrbitalRefObject.fields` rename (`{ up: local }`) to every
/// `@entity.<field>` token a trait carries: transition guards/effects, tick
/// guards/effects, listen guards/payloadMapping, and config-field defaults —
/// matches the JS twin's `rewriteEntityFieldsInTrait` scope, plus the
/// `entity_contract.requires`/`provides` rewrite `apply_overrides_to_trait`
/// already does for the ordinary trait-ref `fields {}` surface (kept for
/// parity with that call site; not present in the JS twin, which has no
/// `entity_contract` concept).
fn rewrite_entity_fields_in_trait(td: &mut TraitDefinition, field_subs: &HashMap<String, String>) {
    if field_subs.is_empty() {
        return;
    }
    for t in td.state_machine.transitions.iter_mut() {
        if let Some(guard) = t.guard.as_mut() {
            rewrite_entity_field_paths(guard, field_subs);
        }
        if let Some(effects) = t.effects.as_mut() {
            for e in effects.iter_mut() {
                rewrite_entity_field_paths(e, field_subs);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_entity_field_paths(guard, field_subs);
            }
            for e in tick.effects.iter_mut() {
                rewrite_entity_field_paths(e, field_subs);
            }
        }
    }
    if let Some(listens) = td.listens.as_mut() {
        for l in listens.iter_mut() {
            if let Some(guard) = l.guard.as_mut() {
                rewrite_entity_field_paths(guard, field_subs);
            }
            if let Some(pm) = l.payload_mapping.as_mut() {
                for v in pm.values_mut() {
                    rewrite_entity_field_paths(v, field_subs);
                }
            }
        }
    }
    if let Some(cfg) = td.config.as_mut() {
        for f in cfg.values_mut() {
            if let Some(default) = f.default.as_mut() {
                rewrite_entity_field_paths(default, field_subs);
            }
        }
    }
    if let Some(contract) = td.entity_contract.as_mut() {
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

/// Rename `entity`-level fields on the entity itself: field keys plus any
/// `@entity.<field>` token in its access policies. JS twin:
/// `rewriteEntityFieldsInEntity`.
fn rewrite_entity_fields_in_entity(entity: &mut EntityDefinition, field_subs: &HashMap<String, String>) {
    if field_subs.is_empty() {
        return;
    }
    for f in entity.fields.iter_mut() {
        if let Some(name) = f.name.as_ref() {
            if let Some(to) = field_subs.get(name) {
                f.name = Some(to.clone());
            }
        }
    }
    if let Some(p) = entity.read_policy.as_mut() {
        rewrite_entity_field_paths(p, field_subs);
    }
    if let Some(p) = entity.create_policy.as_mut() {
        rewrite_entity_field_paths(p, field_subs);
    }
    if let Some(p) = entity.update_policy.as_mut() {
        rewrite_entity_field_paths(p, field_subs);
    }
    if let Some(p) = entity.delete_policy.as_mut() {
        rewrite_entity_field_paths(p, field_subs);
    }
}

/// Rename every relation TARGET (`FieldDefinition.relation.entity`, plus its
/// dual-carry `relation.entityId` sibling) reachable from `fields`, recursing
/// through array `items` and object `properties` — the only entity-name-
/// bearing slot `FieldDefinition` carries. Distinct from
/// `rewrite_entity_fields_in_entity`/`_in_trait`, which rename FIELD KEYS (the
/// `fields {}` override surface), not relation targets. Gap (A): an imported
/// entity's own `authorId`/`parentId`/`tagIds`-style relation fields still
/// named the upstream entity, tripping `ORB_E_INVALID_RELATION` /
/// `ORB_X_UNRESOLVED_RELATION_TARGET` (and the consequential
/// `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED` once the owner field's relation no
/// longer resolves to an `[identity]` entity). `entity_ids` (final entity name
/// → its OWN freshly materialized id) resets `entityId` to the RENAMED
/// target's own id rather than leaving it pointing at upstream's id — the
/// id-integrity dual-carry check (`id_integrity.rs`) reads `entityId` against
/// the ORIGINAL schema's ledger, and a stale upstream id there resolves to
/// upstream's un-prefixed `curName`, tripping `ORB_ID_NAME_MISMATCH` against
/// the just-renamed `.entity`. A target missing from `entity_ids` (should not
/// happen within one materialized closure) drops the id sibling rather than
/// leaving it dangling — `entityId` is optional (pre-Phase-7).
fn rename_relation_targets_in_fields(
    fields: &mut [FieldDefinition],
    entity_subs: &HashMap<String, String>,
    entity_ids: &HashMap<String, String>,
) {
    for f in fields.iter_mut() {
        if let Some(rel) = f.relation.as_mut() {
            if let Some(to) = entity_subs.get(&rel.entity) {
                rel.entity = to.clone();
                rel.entity_id = entity_ids.get(to).cloned();
            }
        }
        if let Some(items) = f.items.as_mut() {
            rename_relation_targets_in_fields(
                std::slice::from_mut(items.as_mut()),
                entity_subs,
                entity_ids,
            );
        }
        if let Some(props) = f.properties.as_mut() {
            for v in props.values_mut() {
                rename_relation_targets_in_fields(std::slice::from_mut(v), entity_subs, entity_ids);
            }
        }
    }
}

/// Rename every relation target on `entity.fields` in place. Thin wrapper
/// kept as its own fn (rather than inlining the `&mut entity.fields` call)
/// so every call site — primary entity, each auxiliary entity, and a trait's
/// `source_entity_definition` copy (see {@link rename_source_entity_definition})
/// — reads the same way. Callers must pass a FULLY populated `entity_ids`
/// (primary + every aux already computed) — see the call-site comments on why
/// the auxiliary-entity loop renames in a second pass rather than in-loop.
fn rename_entity_refs_in_entity(
    entity: &mut EntityDefinition,
    entity_subs: &HashMap<String, String>,
    entity_ids: &HashMap<String, String>,
) {
    rename_relation_targets_in_fields(&mut entity.fields, entity_subs, entity_ids);
}

/// Gap (B): a materialized trait's `source_entity_definition` is a COPY of
/// the atom's own entity, stamped when the trait was originally inlined
/// (`source_behavior` + `source_entity_definition` together record "this
/// trait came from an atom bound to entity X"). An orbital import is a CLONE
/// of the whole subsystem, not a re-host of it onto a foreign entity — so the
/// cloned trait's source entity must track the clone: rename its `.name` (and
/// any relation target it carries) through the SAME `subs` map used for
/// `linked_entity`, so a trait whose atom-original entity coincided with the
/// upstream orbital's own entity (the common `PageAtom.traits.X -> Note` shape,
/// where the atom's own entity is ALSO named `Note`) still has
/// `source_entity_definition.name == linked_entity` post-import, exactly as it
/// did pre-import. Left un-renamed, `validate_entity_rebind`
/// (`trait_validator.rs`) sees a spurious rebind (source now named
/// upstream's un-prefixed name, linked_entity already renamed) and fires
/// `ORB_T_ENTITY_NOT_REBINDABLE` on traits that were never a rebind at all.
/// Does NOT touch `entity_rebindable` or clear the field — provenance and any
/// genuine (pre-existing) rebind opt-in must survive untouched.
fn rename_source_entity_definition(
    td: &mut TraitDefinition,
    entity_subs: &HashMap<String, String>,
    entity_ids: &HashMap<String, String>,
) {
    let Some(src) = td.source_entity_definition.as_mut() else {
        return;
    };
    if let Some(to) = entity_subs.get(&src.name) {
        src.name = to.clone();
    }
    rename_entity_refs_in_entity(src, entity_subs, entity_ids);
}

/// Point a materialized trait's `listens[].source` at its renamed name. `Trait`
/// sources are looked up in the combined `subs` map; `Orbital`-kind sources
/// (`Orbital.Trait.EVENT`) additionally need `orbital_rename` (`Some((from,
/// to))` when the upstream orbital's own name differs from `local_name`) so a
/// source that named the upstream orbital now names the LOCAL one — the one
/// case sibling-pull's own listen rewrite never needed (a pulled sibling's
/// owner is always the SAME orbital).
fn rewrite_listen_sources(
    td: &mut TraitDefinition,
    subs: &HashMap<String, String>,
    id_by_name: &HashMap<String, String>,
    orbital_rename: Option<(&str, &str)>,
) {
    let Some(listens) = td.listens.as_mut() else {
        return;
    };
    for listen in listens.iter_mut() {
        match listen.source.as_mut() {
            Some(ListenSource::Trait { trait_name, trait_id }) => {
                if let Some(target) = subs.get(trait_name.as_str()) {
                    if target != trait_name {
                        *trait_id = id_by_name.get(target).cloned();
                        *trait_name = target.clone();
                    }
                }
            }
            Some(ListenSource::Orbital { orbital, trait_name, trait_id, orbital_id }) => {
                if let Some((from, to)) = orbital_rename {
                    if orbital.as_str() == from {
                        let target = subs.get(trait_name.as_str()).cloned().unwrap_or_else(|| trait_name.clone());
                        *trait_id = id_by_name.get(&target).cloned();
                        *trait_name = target;
                        *orbital = to.to_string();
                        // The old value named the UPSTREAM orbital; there is
                        // no reliable local-orbital id to carry here, so
                        // clear the dual-carry sibling (matches the JS twin,
                        // which never emits `orbitalId` on this rewrite).
                        *orbital_id = None;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Apply an `OrbitalRefObject.events` rename (`{ UP: LOCAL }`) to every
/// event-bearing position a trait carries: `(emit …)` effect forms and
/// event-valued render-ui props (via `rewrite_identifiers`'s `event_subs`
/// arm), the structured event-key fields (`stateMachine.events[*].key`,
/// `transitions[*].event`, `emits[*].event`, `listens[*].event/triggers` —
/// via `rewrite_trait_event_fields`), and a declared config knob typed
/// `event` whose literal default names a renamed event (the JS twin's
/// `renameEventsInDeclaredConfig` main branch — its struct-member/array form
/// is not replicated here, a documented gap since no W3-R test exercises it).
fn rename_events_in_trait(td: &mut TraitDefinition, event_subs: &HashMap<String, String>) {
    if event_subs.is_empty() {
        return;
    }
    let no_entities: HashMap<String, String> = HashMap::new();
    for t in td.state_machine.transitions.iter_mut() {
        if let Some(guard) = t.guard.as_mut() {
            rewrite_identifiers(guard, &no_entities, event_subs);
        }
        if let Some(effects) = t.effects.as_mut() {
            for e in effects.iter_mut() {
                rewrite_identifiers(e, &no_entities, event_subs);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_identifiers(guard, &no_entities, event_subs);
            }
            for e in tick.effects.iter_mut() {
                rewrite_identifiers(e, &no_entities, event_subs);
            }
        }
    }
    rewrite_trait_event_fields(td, event_subs);
    if let Some(cfg) = td.config.as_mut() {
        for f in cfg.values_mut() {
            if f.field_type == "event" {
                if let Some(SExpression::String(s)) = f.default.as_ref() {
                    if let Some(to) = event_subs.get(s) {
                        f.default = Some(SExpression::String(to.clone()));
                    }
                }
            }
        }
    }
}

/// Segments of a URL path, ignoring empty leading/trailing slashes.
fn navigate_path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// The prefix a `str/concat`-built navigate target can be compared against: a
/// page path with its trailing `:param` segment stripped. Only a TRAILING
/// param segment yields a prefix; a mid-path param has none. Mirrors
/// `navigateParamStrippedPrefix` (`reference-resolver.ts:1564`).
fn navigate_param_stripped_prefix(path: &str) -> Option<String> {
    let segments = navigate_path_segments(path);
    let last = segments.last()?;
    if !last.starts_with(':') {
        return None;
    }
    Some(format!("/{}/", segments[..segments.len() - 1].join("/")))
}

/// The local prefix a `str/concat`-built navigate target's literal head
/// rewrites to, given the upstream→local page-path map. Mirrors
/// `rewriteConcatPrefix` (`reference-resolver.ts:1578`).
fn rewrite_concat_prefix(literal_prefix: &str, path_map: &HashMap<String, String>) -> Option<String> {
    let normalized = if literal_prefix.ends_with('/') {
        literal_prefix.to_string()
    } else {
        format!("{literal_prefix}/")
    };
    for (upstream_path, final_path) in path_map {
        let Some(stripped) = navigate_param_stripped_prefix(upstream_path) else {
            continue;
        };
        if stripped != normalized {
            continue;
        }
        if let Some(final_stripped) = navigate_param_stripped_prefix(final_path) {
            return Some(final_stripped);
        }
    }
    None
}

/// Rewrite `(navigate "<upstream>")` literals — all three arities
/// (`(navigate path)`, `(navigate path payload)`, `(navigate path payload
/// crumb)`) — plus the `(navigate (str/concat "<prefix>" …) …)` form, against
/// an upstream→local page-path map. New: no existing Rust helper rewrites
/// navigate targets (the JS twin's `rewriteNavigateTargets`,
/// `reference-resolver.ts:1595`, is the only prior implementation — ported
/// here node-shape-for-node-shape since the compiled path had none yet).
fn rewrite_navigate_targets(expr: &mut SExpression, path_map: &HashMap<String, String>) {
    let SExpression::Array(items) = expr else {
        if let SExpression::Object(map) = expr {
            for v in map.values_mut() {
                rewrite_navigate_targets(v, path_map);
            }
        }
        return;
    };
    let is_navigate = items.len() >= 2
        && matches!(items.first(), Some(SExpression::String(op)) if op == "navigate");
    if is_navigate {
        if let SExpression::String(target) = &items[1] {
            if let Some(to) = path_map.get(target) {
                items[1] = SExpression::String(to.clone());
                for item in items.iter_mut().skip(2) {
                    rewrite_navigate_targets(item, path_map);
                }
                return;
            }
        }
        let concat_prefix = match &items[1] {
            SExpression::Array(concat) if concat.len() >= 2 => match (&concat[0], &concat[1]) {
                (SExpression::String(cop), SExpression::String(prefix))
                    if cop == "str/concat" && prefix.starts_with('/') =>
                {
                    Some(prefix.clone())
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(prefix) = concat_prefix {
            if let Some(rewritten) = rewrite_concat_prefix(&prefix, path_map) {
                if let SExpression::Array(concat) = &mut items[1] {
                    concat[1] = SExpression::String(rewritten);
                }
            }
            for item in items.iter_mut().skip(2) {
                rewrite_navigate_targets(item, path_map);
            }
            return;
        }
    }
    for item in items.iter_mut() {
        rewrite_navigate_targets(item, path_map);
    }
}

/// Apply {@link rewrite_navigate_targets} across a trait's whole effect
/// surface — transitions and ticks only; `navigate` is an effect, never a
/// guard.
fn rewrite_trait_navigate_targets(td: &mut TraitDefinition, path_map: &HashMap<String, String>) {
    if path_map.is_empty() {
        return;
    }
    for t in td.state_machine.transitions.iter_mut() {
        if let Some(effects) = t.effects.as_mut() {
            for e in effects.iter_mut() {
                rewrite_navigate_targets(e, path_map);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            for e in tick.effects.iter_mut() {
                rewrite_navigate_targets(e, path_map);
            }
        }
    }
}

/// Gap (D): rewrite a config-default STRING that EXACTLY equals an upstream
/// page path to its local remapped path — the config-held twin of
/// `rewrite_navigate_targets`'s literal-page-path rewrite, for values like an
/// `AppLayout`'s `navItems[].href` that hold a bare upstream path OUTSIDE any
/// `(navigate …)` s-expression (`std-notes.lolo:278-290`). Exact-match only —
/// a string that merely CONTAINS a path (e.g. a sentence mentioning "/notes"
/// in prose) is left alone; recurses through arrays/objects only, since a
/// bare page path never appears anywhere else in an s-expression tree.
fn rewrite_config_page_paths(expr: &mut SExpression, path_map: &HashMap<String, String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(to) = path_map.get(s.as_str()) {
                *s = to.clone();
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_config_page_paths(item, path_map);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_config_page_paths(v, path_map);
            }
        }
        _ => {}
    }
}

/// Apply {@link rewrite_config_page_paths} across every declared config
/// field's default value on one trait.
fn rewrite_trait_config_page_paths(td: &mut TraitDefinition, path_map: &HashMap<String, String>) {
    if path_map.is_empty() {
        return;
    }
    if let Some(cfg) = td.config.as_mut() {
        for f in cfg.values_mut() {
            if let Some(default) = f.default.as_mut() {
                rewrite_config_page_paths(default, path_map);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Upstream orbital fixture: `A` embeds `@trait.B` and listens on it via
    /// `ListenSource::Trait`; `B` and `C` are plain siblings. No config, no
    /// pages, no aux entities — the config/pages/aux cases get their own
    /// smaller fixtures below. The full-surface proof (config forward, page
    /// remap with all navigate arities, `ListenSource::Orbital`, disjoint
    /// double import) lives in `tests/orbital_import.rs` instead, exercised
    /// through the real `inline_schema` pipeline rather than this function.
    fn upstream_orbital(config: Option<serde_json::Value>) -> Orbital {
        let mut value = json!({
            "name": "UpOrbital",
            "id": "orb_UPSTREAMTESTORBITAL000000",
            "entity": {
                "name": "Widget",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": [
                {
                    "name": "A",
                    "linkedEntity": "Widget",
                    "category": "interaction",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN" } ],
                        "transitions": [
                            {
                                "from": "idle", "to": "idle", "event": "OPEN",
                                "effects": [
                                    ["render-ui", "main", { "type": "stack", "children": ["@trait.B"] }]
                                ]
                            }
                        ]
                    },
                    "listens": [
                        { "event": "PING", "triggers": "OPEN", "source": { "kind": "trait", "trait": "B" } }
                    ]
                },
                {
                    "name": "B",
                    "linkedEntity": "Widget",
                    "category": "interaction",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "PING" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "PING" } ]
                    },
                    "emits": [ { "event": "PING" } ]
                },
                {
                    "name": "C",
                    "linkedEntity": "Widget",
                    "category": "interaction",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "transitions": []
                    }
                }
            ]
        });
        if let Some(cfg) = config {
            value["config"] = cfg;
        }
        serde_json::from_value(value).expect("upstream orbital fixture must deserialize")
    }

    fn ctx_with_upstream(config: Option<serde_json::Value>) -> InlineContext {
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream_orbital(config));
        ctx
    }

    fn consumer_orbital(local_name: &str) -> Orbital {
        serde_json::from_value(json!({
            "name": local_name,
            "entity": "Up.orbitals.UpOrbital.entity",
            "traits": []
        }))
        .expect("consumer orbital fixture must deserialize")
    }

    fn kept_trait<'a>(orbital: &'a Orbital, name: &str) -> &'a TraitDefinition {
        orbital
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(td) if td.name == name => Some(td),
                _ => None,
            })
            .unwrap_or_else(|| panic!("trait {name} not found; got {:?}", trait_names(orbital)))
    }

    fn trait_names(orbital: &Orbital) -> Vec<String> {
        orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some(td.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unconditional_prefix_renames_traits_with_zero_collisions() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        let names = trait_names(&orbital);
        assert_eq!(names.len(), 3, "{names:?}");
        for expected in ["LocalA", "LocalB", "LocalC"] {
            assert!(names.contains(&expected.to_string()), "{names:?}");
        }
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn listen_source_trait_kind_is_rewritten_to_the_prefixed_name() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        let listens = a.listens.as_ref().expect("listens carried");
        match listens[0].source.as_ref().expect("source") {
            ListenSource::Trait { trait_name, trait_id } => {
                assert_eq!(trait_name, "LocalB");
                assert!(trait_id.is_some(), "renamed source must carry the target's derived id");
            }
            other => panic!("expected ListenSource::Trait, got {other:?}"),
        }
    }

    #[test]
    fn omit_of_a_trait_a_survivor_still_embeds_is_diagnosed() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: Some(vec!["B".to_string()]),
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        let names = trait_names(&orbital);
        assert!(!names.contains(&"LocalB".to_string()), "{names:?}");
        assert_eq!(diagnostics.len(), 1);
        // Diagnostic names are pre-rename (upstream) names, matching the JS
        // twin — the check runs before the unconditional-prefix step.
        assert_eq!(diagnostics[0].omitted_but_embedded, vec![("A".to_string(), "B".to_string())]);

        // The dangling `@trait.B` embed is left AS-IS (no rename target) for
        // the validator to flag via ORB_O_OMIT_EMBEDDED_TRAIT — never rewritten
        // to point at a trait that no longer exists in this orbital.
        let a = kept_trait(&orbital, "LocalA");
        let blob = serde_json::to_string(&a.state_machine.transitions).unwrap();
        assert!(blob.contains("@trait.B"), "{blob}");
    }

    #[test]
    fn unknown_config_override_key_is_diagnosed() {
        let ctx = ctx_with_upstream(Some(json!({
            "pageSize": { "type": "number", "default": 10 }
        })));
        let mut orbital = consumer_orbital("Local");
        let mut overrides = std::collections::HashMap::new();
        overrides.insert(
            "bogusKnob".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "number", "default": 99 })).unwrap(),
        );
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: Some(overrides),
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].unknown_config_keys, vec!["bogusKnob".to_string()]);
        // The declared (known) knob is untouched — the unknown key is dropped,
        // not silently accepted into the folded config.
        let folded = orbital.config.expect("declared config carried");
        assert!(!folded.contains_key("bogusKnob"));
        assert_eq!(
            folded.get("pageSize").and_then(|f| f.default.clone()),
            Some(SExpression::Number(10.0))
        );
    }

    #[test]
    fn schema_rung_resolves_a_forward_when_the_orbital_declares_no_config() {
        // Upstream orbital declares NO `config {}` of its own, but trait `A`
        // forwards `@config.pageSize` — with `orbital_config: None`, the
        // outermost (schema) rung must still resolve it.
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(td) = &mut upstream.traits[0] {
            td.config = Some(std::collections::HashMap::from([(
                "limit".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "number", "default": "@config.pageSize"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut schema_config = std::collections::HashMap::new();
        schema_config.insert(
            "pageSize".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "number", "default": 42 })).unwrap(),
        );

        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, Some(&schema_config), &mut diagnostics)
            .expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        assert_eq!(
            a.config.as_ref().and_then(|c| c.get("limit")).and_then(|f| f.default.clone()),
            Some(SExpression::Number(42.0)),
            "with no orbital-level config, the schema rung must still resolve the forward"
        );
    }

    #[test]
    fn ids_are_deterministic_across_two_runs() {
        let ctx = ctx_with_upstream(None);
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };

        let mut first = consumer_orbital("Local");
        resolve_orbital_reference(&mut first, &obj, &ctx, None, &mut Vec::new()).expect("resolves");
        let mut second = consumer_orbital("Local");
        resolve_orbital_reference(&mut second, &obj, &ctx, None, &mut Vec::new()).expect("resolves");

        assert_eq!(kept_trait(&first, "LocalA").id, kept_trait(&second, "LocalA").id);
        let EntityRef::Inline(e1) = &first.entity else { panic!("inline") };
        let EntityRef::Inline(e2) = &second.entity else { panic!("inline") };
        assert_eq!(e1.id, e2.id);
    }

    #[test]
    fn two_different_local_names_importing_one_upstream_are_disjoint() {
        let ctx = ctx_with_upstream(None);
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };

        let mut one = consumer_orbital("One");
        resolve_orbital_reference(&mut one, &obj, &ctx, None, &mut Vec::new()).expect("resolves");
        let mut two = consumer_orbital("Two");
        resolve_orbital_reference(&mut two, &obj, &ctx, None, &mut Vec::new()).expect("resolves");

        assert_ne!(trait_names(&one), trait_names(&two));
        assert_ne!(kept_trait(&one, "OneA").id, kept_trait(&two, "TwoA").id);
        let EntityRef::Inline(e1) = &one.entity else { panic!("inline") };
        let EntityRef::Inline(e2) = &two.entity else { panic!("inline") };
        assert_ne!(e1.id, e2.id);
    }

    /// `A` embeds `@trait.B` (fixture render effect) and declares `A`'s own
    /// `itemClickEvent` config default as the literal `"VIEW"`; `B` forwards
    /// `itemClickEvent` (`default "@config.itemClickEvent"`) — the
    /// `std-browse` `MasterListView` shape (Almadar_Orbital_Import.md §4.5
    /// / `docs/Almadar_LOLO_Gaps.md`). Before the fix, the splice's step 6
    /// always called `forwarded_sibling_config(td, &[], None, …)` — no
    /// `result` list, no `parent_name` — so a pulled sibling's forward
    /// never saw its embedder and fell straight through to the orbital/
    /// schema rungs, leaving `LocalB.config.itemClickEvent.default` an
    /// unresolved `"@config.itemClickEvent"` string.
    #[test]
    fn embedder_forwards_config_to_pulled_sibling() {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(a) = &mut upstream.traits[0] {
            a.config = Some(HashMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "VIEW"
                }))
                .unwrap(),
            )]));
        }
        if let TraitReference::Inline(b) = &mut upstream.traits[1] {
            b.config = Some(HashMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "@config.itemClickEvent"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        let b = kept_trait(&orbital, "LocalB");
        assert_eq!(
            b.config.as_ref().and_then(|c| c.get("itemClickEvent")).and_then(|f| f.default.clone()),
            Some(SExpression::String("VIEW".to_string())),
            "the pulled sibling's forward must resolve against its EMBEDDER's declared \
             config (A), not fall straight through to the orbital/schema rungs"
        );
    }

    /// Same shape as above, but the upstream ORBITAL also declares
    /// `itemClickEvent` (a lower-precedence rung) — the embedder's value
    /// must still win, matching the pull site's rung order (embedder →
    /// orbital → schema).
    #[test]
    fn embedder_config_forward_wins_over_orbital_rung() {
        let mut upstream = upstream_orbital(Some(json!({
            "itemClickEvent": { "type": "string", "default": "ORBITAL_DEFAULT" }
        })));
        if let TraitReference::Inline(a) = &mut upstream.traits[0] {
            a.config = Some(HashMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "VIEW"
                }))
                .unwrap(),
            )]));
        }
        if let TraitReference::Inline(b) = &mut upstream.traits[1] {
            b.config = Some(HashMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "@config.itemClickEvent"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(&mut orbital, &obj, &ctx, None, &mut diagnostics).expect("resolves");

        let b = kept_trait(&orbital, "LocalB");
        assert_eq!(
            b.config.as_ref().and_then(|c| c.get("itemClickEvent")).and_then(|f| f.default.clone()),
            Some(SExpression::String("VIEW".to_string())),
            "the embedder rung must win over the orbital rung, same precedence as the pull site"
        );
    }
}
