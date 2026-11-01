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
use std::sync::Arc;
use super::context::{parse_reference, InlineContext, ParsedReference};
use super::identity_normalize::{rewrite_config_embed_tokens, rewrite_trait_embed_tokens};
use super::r#trait::{
    apply_overrides_to_trait, apply_post_subst_overrides_to_trait,
    forwarded_sibling_config_from_tracked,
};
use super::rewrite::{
    collect_entity_slots, fold_event_typed_members,
    rewrite_bare_field_name_config_default, rewrite_entity_field_paths, rewrite_entity_ref_ids,
    rewrite_entity_typed_config_values, rewrite_identifiers, rewrite_payload_entity_refs,
    resolve_event_scope_renames, rewrite_role_literals, rewrite_role_literals_in_config_lists,
    rewrite_trait_event_fields, EventScopeRenames,
};
use super::InlineError;
use crate::phases::validation::user_identity::{identity_entities_of, vocabulary};
use orbital_core_base::schema::{
    ConfigField, EntityDefinition, EntityRef, ExpectDeclaration, FieldDefinition, ListenSource, Orbital,
    OrbitalRefObject, PageRef, Persistence, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// An orbital that hosts an import takes the import's `os/` grants (EXEC-GRANTS-2): they stay on the orbital the
/// code runs in and never reach a sibling or the app level.
pub(crate) fn take_os_grants(orbital: &mut Orbital, grants: Option<&orbital_core_base::os_grants::OsGrants>) {
    let Some(grants) = grants.filter(|g| !g.is_empty()) else {
        return;
    };
    orbital.os_grants.get_or_insert_with(Default::default).union(grants);
}

/// Every entity name the orbital's `expects` declarations license, a named `expects entity [identity]` included.
pub(crate) fn expected_entity_names(orbital: &Orbital) -> Vec<&str> {
    orbital
        .expects
        .iter()
        .flatten()
        .filter_map(|e| match e {
            ExpectDeclaration::Entity { name, .. } => Some(name.as_str()),
            ExpectDeclaration::Identity { name: Some(name), .. } => Some(name.as_str()),
            ExpectDeclaration::Identity { name: None, .. } | ExpectDeclaration::Page { .. } => None,
            _ => None,
        })
        .collect()
}

/// What resolving one `orbital X = Alias.orbitals.Y { … }` reference hands the schema-level passes that run
/// after every reference is resolved. It carries no validation finding: the call-site checks are the std lint
/// atoms `std-orb-lint-orbital-ref-config` and `std-orb-lint-orbital-ref-closure`.
#[derive(Debug, Clone, Default)]
pub struct OrbitalRefDiagnostic {
    pub path: String,
    pub reference: String,
    /// G5 (`docs/Almadar_Compiler_Gaps.md` §82): this reference's `events {
    /// OLD: NEW }` map, carried out of `resolve_orbital_reference` (which has
    /// no `schema.ledger` access — it operates on one `Orbital`, not the
    /// whole schema) so a LATER, schema-level pass
    /// (`reconcile_event_id_renames` in `inline/mod.rs`) can
    /// re-derive the id of every event declaration this rename actually
    /// touched. Empty when the reference declares no `events {}`.
    pub event_subs: orbital_core_base::collections::IndexMap<String, String>,
    /// The materialized LOCAL orbital name this diagnostic's reference
    /// spliced into — lets the schema-level pass above find `orbital` by
    /// name (same by-name fallback `local_orbital_index` already uses when
    /// there is no schema index to give).
    pub local_name: String,
    /// Upstream entity name -> the name it carries in the materialized import (the primary and every
    /// auxiliary of the closure). The schema-level pass `bind_inherited_entity_expectations` reads it
    /// to point a sibling's inherited `expects entity` at the provider's renamed entity.
    pub entity_renames: orbital_core_base::collections::IndexMap<String, String>,
    /// The entity expectations this import inherited from its upstream orbital and left unmapped.
    pub inherited_entity_expects: Vec<String>,
    /// `entities { N: Target }` entries whose key is an entity the upstream orbital EXPECTS of a sibling:
    /// the explicit binding of that expectation to `Target`, applied by `bind_inherited_entity_expectations`.
    pub expected_entity_map: orbital_core_base::collections::IndexMap<String, String>,
}

/// A `uses A from "…" { config { … } }` call site, recorded once per use of the import. A shared import
/// replays the sites it recorded for every later user (the reuse test counts them); it carries no finding.
#[derive(Debug, Clone)]
pub struct UsesConfigSite {
    pub path: String,
    pub alias: String,
}

/// Fold a call-site config override onto a declared `config {}` map: every
/// DECLARED knob survives with its default replaced by the override's value
/// when one is supplied; an override key naming an UNDECLARED knob is
/// silently absent from the result (callers that need to diagnose it —
/// `ORB_O_CONFIG_UNKNOWN_KEY` / `ORB_U_CONFIG_UNKNOWN_KEY` — compute that
/// separately against the same `declared`/`overrides` pair). `None`
/// declared ⇒ `None` (nothing to fold onto). One owner for every
/// declared-config × call-site-override fold in this module: the
/// orbital-level rung (`obj.config` onto `upstream.config`, used by
/// `resolve_orbital_reference` itself) AND the schema (alias) rung
/// (`use_decl.config` onto the alias's loaded app-level `config {}}`,
/// built in `inline_orbital` alongside `ctx.add_alias_multi`). Two-path
/// twin: JS's `foldUseConfigOverride` (`reference-resolver.ts`).
pub(crate) fn fold_declared_config_override(
    declared: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
    overrides: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
) -> Option<orbital_core_base::collections::IndexMap<String, ConfigField>> {
    declared.map(|decl| {
        decl.iter()
            .map(|(k, field)| {
                let mut field = field.clone();
                if let Some(over) = overrides.and_then(|o| o.get(k)) {
                    field.default = over.default.clone();
                }
                (k.clone(), field)
            })
            .collect()
    })
}

/// Last hop of a `uses Alias { config { k: @config.j } }` override: a folded
/// alias app-knob whose default is exactly the consumer app-knob token
/// `@config.<j>` takes the consumer's declared default (provenance kept in
/// `forwarded_from`). Anything else — no such consumer knob, a knob without a
/// default — is left for `validate_dangling_config_refs` to judge.
pub(crate) fn forward_consumer_app_knobs(
    folded: &mut orbital_core_base::collections::IndexMap<String, ConfigField>,
    consumer: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
) {
    let Some(consumer) = consumer else { return };
    for field in folded.values_mut() {
        let Some(SExpression::String(token)) = field.default.as_ref() else {
            continue;
        };
        let Some(key) = token.strip_prefix("@config.") else {
            continue;
        };
        if key.contains('.') {
            continue;
        }
        let Some(source) = consumer.get(key) else {
            continue;
        };
        let Some(value) = source.default.clone() else {
            continue;
        };
        field.forwarded_from = Some(token.clone().into_inner());
        field.default = Some(value);
    }
}

/// The materialized NAME and derived ID of an `orbital X = Alias.orbitals.Y
/// { … }` reference's PRIMARY entity, computed BEFORE any other part of the
/// reference resolves — the same pair `resolve_orbital_reference` itself
/// needs for its `primary_final`/`entity_ids` insert (§4.3's unconditional
/// prefix + `derive_materialized_id`'s zero-churn clone id). One owner,
/// used by `resolve_orbital_reference` AND by (B)'s pre-pass
/// (`inline_schema_with_orbital_diagnostics`'s
/// `precompute_reference_form_primary_ids`), which needs this SAME pair for
/// every reference-form orbital in the file — INCLUDING ones not yet
/// resolved — so `consumer_entity_ids` can already `entities {}`-map onto
/// them (the approved two-import Project Friday shape is order-free: import
/// 1 maps an out-of-orbital primary onto import 2's materialized entity,
/// regardless of file order). `None` only when the upstream's own entity is
/// itself a non-Inline form (never arises on the real resolve path, since
/// `ctx.get_orbital` only ever returns already-inlined orbitals; the
/// pre-pass's shallow, non-recursive load can legitimately hit this).
/// The prefix an import's names take (§4.3): the local orbital name, except for an import that keeps the upstream
/// orbital's own name (`orbital X = Alias.orbitals.X`), which reproduces it as declared. A second import of the same
/// upstream needs another local name, so it is prefixed and stays disjoint.
pub(crate) fn import_prefix<'a>(local_name: &'a str, obj: &OrbitalRefObject) -> &'a str {
    if obj.reference.rsplit('.').next() == Some(local_name) {
        ""
    } else {
        local_name
    }
}

pub(crate) fn materialized_primary_entity(
    upstream: &Orbital,
    obj: &OrbitalRefObject,
    local_name: &str,
) -> Option<(String, String)> {
    let EntityRef::Inline(upstream_entity) = &upstream.entity else {
        return None;
    };
    let primary_final = obj
        .entity
        .clone()
        .unwrap_or_else(|| format!("{}{}", import_prefix(local_name, obj), upstream_entity.name));
    let primary_id = derive_materialized_id(
        upstream_entity.id.as_deref(),
        upstream.id.as_deref(),
        &upstream_entity.name,
        import_prefix(local_name, obj),
        orbital_core_base::identity::IdKind::Entity);
    Some((primary_final, primary_id))
}

/// One entry per reference-form orbital in a schema, built ONCE (before any
/// splice runs) by `precompute_reference_form_listen_targets` — G1
/// (`docs/Almadar_Compiler_Gaps.md` §82). Every value here is re-derived
/// straight off the UPSTREAM schema (`materialized_trait_subs`, same
/// derivation `resolve_orbital_reference`'s own step-4 trait loop uses), so
/// it never depends on another orbital of this schema having already
/// resolved — the map is safe to build order-independently and consult from
/// any orbital's splice, whichever was declared first.
#[derive(Debug, Clone, Default)]
pub(crate) struct SiblingImportInfo {
    pub local_name: String,
    /// upstream (pre-prefix) trait name -> (final local trait name, final derived id)
    pub trait_subs: HashMap<String, (String, String)>,
    /// G-ORB-031: one `(local orbital name, upstream path -> local path)` per
    /// import of this (alias, upstream orbital) that actually moves a page
    /// (identity entries dropped). A `Vec` because two imports of one upstream
    /// orbital share this map's key yet remap independently.
    pub page_remaps: Vec<(String, orbital_core_base::collections::IndexMap<String, String>)>,
    /// Every import of this (alias, upstream orbital), by local orbital name,
    /// with its own trait subs — `local_name`/`trait_subs` hold only the last.
    pub imports: Vec<(String, HashMap<String, (String, String)>)>,
}

/// The upstream→local page remaps of every OTHER import from `alias`,
/// restricted to paths this import's own upstream pages never declare (own
/// remap always wins). A path every other import maps to the SAME local path
/// is applied; a path they map DIFFERENTLY is ambiguous and left out — the
/// sibling's target stays the upstream path and the unreachable-navigate
/// check reports it, so no import is silently preferred over another.
fn sibling_page_remaps(
    alias: &str,
    local_name: &str,
    own_path_map: &orbital_core_base::collections::IndexMap<String, String>,
    listen_source_targets: &HashMap<(String, String), SiblingImportInfo>,
) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut targets: HashMap<String, HashSet<String>> = HashMap::new();
    for ((a, _), info) in listen_source_targets {
        if a != alias {
            continue;
        }
        for (importer, remap) in &info.page_remaps {
            if importer == local_name {
                continue;
            }
            for (from, to) in remap {
                if !own_path_map.contains_key(from) {
                    targets.entry(from.clone()).or_default().insert(to.clone());
                }
            }
        }
    }
    targets
        .into_iter()
        .filter_map(|(from, tos)| {
            let mut it = tos.into_iter();
            match (it.next(), it.next()) {
                (Some(to), None) => Some((from, to)),
                _ => None,
            }
        })
        .collect()
}

/// `obj.pages` entries that name a real upstream page and move it.
pub(crate) fn effective_page_remap(
    upstream: &Orbital,
    obj: &OrbitalRefObject,
) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    let (Some(pages), Some(map)) = (&upstream.pages, obj.pages.as_ref()) else {
        return out;
    };
    for p in pages {
        if let PageRef::Inline(pd) = p {
            if let Some(to) = map.get(&pd.path) {
                if to != &pd.path {
                    out.insert(pd.path.clone(), to.clone());
                }
            }
        }
    }
    out
}

/// Every KEPT (post `omit`/`only`) upstream trait a reference-form orbital
/// will bring in, keyed by its upstream declared name, mapped to its final
/// prefixed name + derived id. Computed straight off the UPSTREAM schema
/// (never off another orbital's own resolve output — trait declaration
/// names/ids don't change through inline, only CLONES get fresh ids), so
/// this is safe to call for every reference-form orbital in a schema up
/// front, before any of them has actually spliced. Mirrors the trimming
/// (`only`/`omit`) and id derivation `resolve_orbital_reference`'s own
/// step-1/step-4 loops perform for the orbital it is CURRENTLY splicing;
/// this is the same derivation exposed so the schema-wide precompute pass
/// can compute it for every OTHER reference-form orbital too (G1, §82).
/// Rewrite one upstream obligation by an orbital import's renames: its trait to the kept trait's final name, its
/// events by the import's `events`, its page by `pages`, and the entity fields its propositions and shape read by
/// `fields`. `false` when the import omits what the obligation is about.
fn rename_imported_obligation(
    expect: &mut orbital_core_base::schema::ExpectDeclaration,
    upstream: &Orbital,
    obj: &OrbitalRefObject,
    kept_traits: &HashMap<String, (String, String)>,
    omitted_pages: &HashSet<&str>,
) -> bool {
    use orbital_core_base::schema::ExpectDeclaration as E;
    let upstream_trait = |t: &str| upstream.traits.iter().any(|tr| tr.trait_name() == Some(t));
    let event = |e: &mut String| {
        if let Some(to) = obj.events.as_ref().and_then(|m| m.get(e.as_str())) {
            *e = to.clone();
        }
    };
    let fields = |e: &mut SExpression| {
        if let Some(subs) = obj.fields.as_ref() {
            super::rewrite::rewrite_entity_field_paths(e, subs);
        }
    };
    let retrait = |t: &mut String| -> bool {
        if !upstream_trait(t) {
            return true;
        }
        match kept_traits.get(t.as_str()) {
            Some((final_name, _)) => {
                *t = final_name.clone();
                true
            }
            None => false,
        }
    };
    match expect {
        E::State { trait_name, holds, .. } => {
            holds.iter_mut().for_each(fields);
            retrait(trait_name)
        }
        E::Transition { trait_name, event: ev, ensures, .. } => {
            event(ev);
            ensures.iter_mut().flatten().for_each(fields);
            retrait(trait_name)
        }
        E::Event { trait_name, event: ev, .. } => {
            event(ev);
            retrait(trait_name)
        }
        E::Page { path, traits, strict, .. } => {
            let own_page = upstream.pages.iter().flatten().any(|p| matches!(p, PageRef::Inline(pd) if pd.path == *path));
            if !*strict || !own_page {
                return true;
            }
            if omitted_pages.contains(path.as_str()) {
                return false;
            }
            if let Some(to) = obj.pages.as_ref().and_then(|m| m.get(path.as_str())) {
                *path = to.clone();
            }
            for t in traits.iter_mut().flatten() {
                if !retrait(t) {
                    return false;
                }
            }
            true
        }
        E::Entity { shape, invariants, read_policy, create_policy, update_policy, delete_policy, .. } => {
            if let Some(subs) = obj.fields.as_ref() {
                for f in shape.iter_mut().flatten() {
                    if let Some(to) = f.name.as_ref().and_then(|n| subs.get(n)) {
                        f.name = Some(to.clone());
                    }
                }
            }
            invariants.iter_mut().flatten().for_each(fields);
            for policy in [read_policy, create_policy, update_policy, delete_policy] {
                policy.iter_mut().for_each(fields);
            }
            true
        }
        _ => true,
    }
}

pub(crate) fn materialized_trait_subs(
    upstream: &Orbital,
    obj: &OrbitalRefObject,
    local_name: &str,
) -> HashMap<String, (String, String)> {
    let only_closed: Option<HashSet<String>> = obj.only.as_ref().map(|v| only_closure(upstream, v));
    let only_set: Option<HashSet<&str>> = only_closed
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let omit_set: HashSet<&str> = obj
        .omit
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut out: HashMap<String, (String, String)> = HashMap::new();
    for tr in &upstream.traits {
        let (name, id) = match tr {
            TraitReference::Inline(td) => (td.name.clone(), td.id.clone()),
            TraitReference::Reference {
                name: Some(name),
                id,
                ..
            } => (name.clone(), id.clone()),
            _ => continue,
        };
        let keep = match &only_set {
            Some(set) => set.contains(name.as_str()),
            None => !omit_set.contains(name.as_str()),
        };
        if !keep {
            continue;
        }
        let final_name = format!("{}{name}", import_prefix(local_name, obj));
        let final_id = derive_materialized_id(
            id.as_deref(),
            upstream.id.as_deref(),
            &name,
            import_prefix(local_name, obj),
            orbital_core_base::identity::IdKind::Trait);
        out.insert(name, (final_name, final_id));
    }
    out
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_orbital_reference(
    orbital: &mut Orbital,
    obj: &OrbitalRefObject,
    ctx: &InlineContext,
    // (A) fix: the outermost §4.5 config-forward rung for THIS reference's
    // materialized traits must be the ALIAS's (upstream FILE's) own
    // app-level `config {}`, folded with the `uses <Alias> { config {…} }`
    // call-site override — NEVER the consumer's own top-level `schema.config`
    // (a different file's app knobs entirely). Keyed by alias name so a
    // multi-`uses` orbital resolves each reference against ITS OWN alias's
    // schema, built once per alias alongside `ctx.add_alias_multi`
    // (`inline_orbital`'s `alias_schema_configs`, folded via the shared
    // `fold_declared_config_override` this function's OWN orbital-rung fold
    // (step 3 below) also uses). Two-path note: JS's twin is
    // `ResolvedImport.schemaConfig` (`foldUseConfigOverride`, B4-J2).
    alias_schema_configs: &orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>>,
    // Every CONSUMER orbital's own primary + auxiliary entity name → id
    // (built once, before any reference is resolved — Stage B's `entities
    // {}` fold needs an id for whatever consumer entity it repoints a
    // relation at, and that entity is never one materialized by THIS call).
    consumer_entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
    // G1 (`docs/Almadar_Compiler_Gaps.md` §82): every reference-form orbital
    // of THIS schema, keyed by (upstream alias, upstream orbital name),
    // computed ONCE before any orbital resolves (order-independent — see
    // `SiblingImportInfo`) — lets a listen source naming ANOTHER imported
    // orbital of the same alias re-point at that import's local name/trait,
    // not just the orbital currently being spliced.
    listen_source_targets: &HashMap<(String, String), SiblingImportInfo>,
    // §86 (`docs/Almadar_Compiler_Gaps.md`): the CONSUMER'S OWN top-level
    // `schema.config` (`inline_orbital`'s `schema_config` parameter at this
    // call site) — the last-hop rung `forward_consumer_app_knobs` folds
    // AFTER step 3's alias-rung fold, so a folded knob whose default is
    // literally the token `@config.<consumerKnob>` (a `uses Alias { config:
    // { k: @config.j } }` call-site override) resolves to the consumer's
    // OWN declared default before the no-embedder fallback, instead of
    // surviving into codegen as the raw unresolved token string. Distinct
    // from `alias_schema_configs` above (the ALIAS's own file-level config,
    // never the consumer's) — see the (A) fix note on that parameter.
    consumer_schema_config: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
    diagnostics: &mut Vec<OrbitalRefDiagnostic>,
    // `.lolo` spans (Stage C): `Some(i)` for a TOP-LEVEL consumer orbital
    // (its index into `schema.orbitals`, matching the index `orbital-lolo`
    // lowering already stamped into `LoloSourceMap` as `orbitals[{i}]` —
    // see `lower/orbital_ref.rs`'s `source_map.insert(base, …)`); `None`
    // when resolving an ALIAS's own reference-form orbital recursively (a
    // different file's index space, no source map to resolve against here
    // anyway). Threads `diag_path` from the digit-bracket indexed form
    // `LoloSourceMap::normalize` accepts, replacing the old
    // `orbitals[name=X]` shape it always rejected outright.
    orbital_index: Option<usize>,
    // An `entities { Up: Target }` retarget queues `Up`'s fields onto `Target`
    // (landed once every orbital is inlined, like any cross-orbital merge).
    deferred_merges: &mut Vec<super::DeferredMerge>,
) -> Result<(), InlineError> {
    let local_name = orbital.name.clone();
    let diag_path = match orbital_index {
        Some(i) => format!("orbitals[{i}]"),
        None => format!("orbitals[name={local_name}]"),
    };

    let ParsedReference::Orbital { alias, name } = parse_reference(&obj.reference)? else {
        return Err(InlineError::InvalidReference(obj.reference.clone()));
    };
    let mut upstream = ctx.get_orbital(&alias, &name)?;
    take_os_grants(orbital, upstream.os_grants.as_ref());
    if let Some(types) = upstream.types.as_ref() {
        let behavior = ctx.get_entry(&alias)?.behavior_name.clone();
        let local = super::qualify::LocalTypes::declared_by(&behavior, types);
        if !local.is_empty() {
            local.orbital(&mut upstream);
        }
    }
    if ctx.collect_references {
        let entry = ctx.get_entry(&alias)?;
        let orbitals = std::rc::Rc::clone(&entry.orbitals);
        let index = orbitals
            .iter()
            .position(|o| o.name == name)
            .ok_or_else(|| InlineError::OrbitalNotFound(name.clone(), entry.source_label.clone()))?;
        let file = entry.source_path.clone().ok_or_else(|| InlineError::ReferenceFileUnknown(entry.source_label.clone()))?;
        ctx.reference_pairs.borrow_mut().push(super::ReferencePair {
            kind: super::ReferenceKind::Orbital,
            alias: alias.clone(),
            name: name.clone(),
            path: diag_path.clone(),
            call_site: super::ReferenceCallSite::Orbital(obj.clone()),
            declaration: super::ReferenceDeclaration::Orbital(super::OrbitalDeclaration::new(orbitals, index, file)),
        });
    }
    let schema_config = alias_schema_configs.get(&alias);

    let EntityRef::Inline(upstream_entity) = upstream.entity.clone() else {
        // The upstream orbital came back from `ctx.get_orbital`, which only
        // ever surfaces already-inlined orbitals — a non-Inline entity here
        // means the upstream's own inline pass never resolved it. Surface
        // the same not-found the plain `Alias.entity` reference form uses.
        return Err(InlineError::EntityNotFound(alias.clone()));
    };
    let (primary_final, primary_id) = materialized_primary_entity(&upstream, obj, &local_name)
        .expect("upstream_entity is already Inline (checked above)");

    // 1. Trim the trait set (omit/only) BEFORE the unconditional prefix —
    // `subs` below only ever maps survivors. Names here are still upstream's.
    let only_closed: Option<HashSet<String>> =
        obj.only.as_ref().map(|v| only_closure(&upstream, v));
    let only_set: Option<HashSet<&str>> = only_closed
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
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
            kept.push(TraitDefinition::clone(td));
        } else {
            removed_names.insert(td.name.clone());
        }
    }

    // A trait removed by `omit`/`only` takes along what only it embedded (its
    // render children, which the emitter also mounts on the page): a kept
    // trait whose every embedder is removed is removed too — to a fixed point.
    if !removed_names.is_empty() {
        loop {
            let mut embedders: orbital_core_base::collections::IndexMap<String, Vec<String>> = orbital_core_base::collections::IndexMap::new();
            for tr in &upstream.traits {
                let TraitReference::Inline(td) = tr else {
                    continue;
                };
                for child in embedded_trait_refs_of(td) {
                    embedders.entry(child).or_default().push(td.name.clone());
                }
            }
            let orphaned: Vec<String> = kept
                .iter()
                .filter(|td| {
                    embedders
                        .get(&td.name)
                        .is_some_and(|es| es.iter().all(|e| removed_names.contains(e)))
                })
                .map(|td| td.name.clone())
                .collect();
            if orphaned.is_empty() {
                break;
            }
            kept.retain(|td| !orphaned.contains(&td.name));
            removed_names.extend(orphaned);
        }
    }
    // An import carries only the merged fields (`mergedFrom`) its own kept traits
    // reference: those a sibling orbital's or a removed trait merged leave with them.
    let unowned_merged_fields: HashSet<String> = {
        let kept_refs: HashSet<String> = kept
            .iter()
            .flat_map(|td| {
                let own = td
                    .linked_entity
                    .as_deref()
                    .is_none_or(|le| le == upstream_entity.name.as_str());
                super::entity::trait_referenced_fields_of_entity(td, &upstream_entity, own)
            })
            .collect();
        upstream_entity
            .fields
            .iter()
            .filter(|fd| fd.merged_from.is_some())
            .filter_map(|fd| fd.name.clone())
            .filter(|f| !kept_refs.contains(f))
            .collect()
    };

    // `traits { Up { … } }`: the §8 trait-reference override surface applied to one
    // imported trait, BEFORE the unconditional prefix (still upstream names), through
    // the same `apply_overrides_to_trait` a `trait X = A.traits.Y { … }` call site uses.
    if let Some(overrides) = obj.traits.as_ref() {
        let mut names: Vec<&String> = overrides.keys().collect();
        names.sort();
        let mut recorder = Vec::new();
        for name in names {
            let ov = &overrides[name];
            let in_upstream = upstream
                .traits
                .iter()
                .any(|tr| matches!(tr, TraitReference::Inline(td) if &td.name == name));
            if !in_upstream || removed_names.contains(name) {
                continue;
            }
            let Some(td) = kept.iter_mut().find(|td| &td.name == name) else {
                continue;
            };
            let no_ids = orbital_core_base::collections::IndexMap::new();
            apply_overrides_to_trait(
                td,
                ov.linked_entity.as_ref(),
                if ov.linked_entity.is_some() {
                    consumer_entity_ids
                } else {
                    &no_ids
                },
                ov.config.as_ref(),
                None,
                ov.events.as_ref(),
                ov.fields.as_ref(),
                true,
                &mut recorder,
                upstream.types.as_ref(),
            )?;
            apply_post_subst_overrides_to_trait(td, ov.listens.as_ref(), ov.emits_scope.as_ref())?;
            td.dispatch = ov.dispatch.or(td.dispatch);
        }
    }

    // 2. Unconditional prefix (§4.3) — one combined rename map covering every
    // surviving trait + the primary + every auxiliary entity. Reused for
    // trait-embed rewriting, entity-root rewriting, AND listen-source
    // rewriting (matching the JS twin's single `subs` map).
    // An auxiliary copy of a name another orbital of the alias owns as its primary is that
    // orbital's entity (one name is one entity in a program): it is out-of-orbital, mapped
    // through `entities {}`, never cloned into the import.
    let sibling_primaries: HashSet<String> = ctx
        .get_entry(&alias)?
        .orbitals
        .iter()
        .filter(|o| o.name != upstream.name)
        .filter_map(|o| match &o.entity {
            EntityRef::Inline(d) => Some(d.name.clone()),
            _ => None,
        })
        .collect();
    let mut aux_entities: Vec<EntityDefinition> = upstream
        .auxiliary_entities
        .iter()
        .flatten()
        .filter_map(|e| match e {
            EntityRef::Inline(d) if !sibling_primaries.contains(&d.name) => Some(d.clone()),
            _ => None,
        })
        .collect();
    // An auxiliary travels with the traits that read it: one a removed trait read and no surviving
    // trait, page or entity reads leaves with them.
    if !removed_names.is_empty() {
        let aux_names: HashSet<String> = aux_entities.iter().map(|e| e.name.clone()).collect();
        let read_by = |traits: &mut dyn Iterator<Item = &TraitDefinition>| -> HashSet<String> {
            traits
                .flat_map(|td| entity_names_read_by_trait(td, &aux_names))
                .collect()
        };
        let removed_reads = read_by(&mut upstream.traits.iter().filter_map(|tr| match tr {
            TraitReference::Inline(td) if removed_names.contains(&td.name) => Some(&**td),
            _ => None,
        }));
        let mut surviving = read_by(&mut kept.iter());
        let mut relation_sites: Vec<(String, String)> = Vec::new();
        collect_relation_targets_referenced(&upstream_entity.fields, "", &aux_names, &mut relation_sites);
        for page in upstream.pages.iter().flatten() {
            if let PageRef::Inline(pd) = page {
                if let Some(pe) = pd.primary_entity.as_ref() {
                    record_referenced("", pe, &aux_names, &mut relation_sites);
                }
            }
        }
        surviving.extend(relation_sites.drain(..).map(|(_, name)| name));
        loop {
            let before = surviving.len();
            for e in aux_entities.iter().filter(|e| surviving.contains(&e.name)) {
                collect_relation_targets_referenced(&e.fields, "", &aux_names, &mut relation_sites);
            }
            surviving.extend(relation_sites.drain(..).map(|(_, name)| name));
            if surviving.len() == before {
                break;
            }
        }
        aux_entities.retain(|e| surviving.contains(&e.name) || !removed_reads.contains(&e.name));
    }
    let mut subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for td in &kept {
        subs.insert(td.name.clone(), format!("{}{}", import_prefix(&local_name, obj), td.name));
    }
    subs.insert(upstream_entity.name.clone(), primary_final.clone());
    let mut entity_renames: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    entity_renames.insert(upstream_entity.name.clone(), primary_final.clone());
    // An in-closure auxiliary leaves the prefix when `entities {}` names it (it becomes the named
    // consumer entity) or when it is an `[identity]` roster (it is `@user`, never forked per import).
    let in_closure_targets: orbital_core_base::collections::IndexMap<String, String> = aux_entities
        .iter()
        .filter_map(|e| {
            let target = match obj.entities.as_ref().and_then(|m| m.get(&e.name)) {
                Some(target) => target.clone(),
                None if e.identity => e.name.clone(),
                None => return None,
            };
            Some((e.name.clone(), target))
        })
        .collect();
    for e in &aux_entities {
        let final_name = in_closure_targets
            .get(&e.name)
            .cloned()
            .unwrap_or_else(|| format!("{}{}", import_prefix(&local_name, obj), e.name));
        subs.insert(e.name.clone(), final_name.clone());
        entity_renames.insert(e.name.clone(), final_name);
    }
    let field_subs: orbital_core_base::collections::IndexMap<String, String> = obj.fields.clone().unwrap_or_default();

    // Embedder map for step 6's per-embedder config forward below: child
    // trait final name → the final name(s) of every surviving sibling that
    // embeds it via `@trait.<child>` (order = `kept` order, deterministic).
    // Built here — `subs` is final and `kept.name` is still upstream's
    // pre-rename name — so both sides translate through `subs` in one pass,
    // before step 4 overwrites `td.name` with the renamed value.
    let mut embedders_of: orbital_core_base::collections::IndexMap<String, Vec<String>> = orbital_core_base::collections::IndexMap::new();
    for td in &kept {
        let embedder_final = subs
            .get(&td.name)
            .cloned()
            .unwrap_or_else(|| td.name.clone());
        for child in embedded_trait_refs_of(td) {
            if let Some(child_final) = subs.get(&child) {
                embedders_of
                    .entry(child_final.clone())
                    .or_default()
                    .push(embedder_final.clone());
            }
        }
    }

    // 3. Config: upstream's DECLARED knobs folded with the call-site override.
    let declared_config = upstream.config.clone();
    let mut folded_config: Option<orbital_core_base::collections::IndexMap<String, ConfigField>> =
        fold_declared_config_override(declared_config.as_ref(), obj.config.as_ref());
    // §86 last hop: a folded knob whose default is still the bare token
    // `@config.<consumerKnob>` resolves against the CONSUMER's own top-level
    // declared default, before anything downstream falls back to treating
    // the raw token as a literal (`validate_dangling_config_refs` judges
    // whatever is left unresolved).
    if let Some(folded) = folded_config.as_mut() {
        forward_consumer_app_knobs(folded, consumer_schema_config);
        // G-CROSS-046: a consumer address (`Local.UpstreamTrait.EVENT`) in the
        // override lands on the sibling import's materialized trait HERE, so the
        // copy every `@config.<knob>` effect substitutes below is already final.
        super::rewrite::map_config_fields_event_addresses(folded, &mut |event, source| {
            let ListenSource::Orbital {
                orbital,
                trait_name,
                ..
            } = source
            else {
                return None;
            };
            let (final_name, final_id) = listen_source_targets
                .values()
                .flat_map(|info| info.imports.iter())
                .find(|(local, _)| local == orbital)
                .and_then(|(_, subs)| subs.get(trait_name.as_str()))?;
            Some((
                event.to_string(),
                ListenSource::Orbital {
                    orbital: orbital.clone(),
                    trait_name: final_name.clone(),
                    orbital_id: None,
                    trait_id: Some(final_id.clone()),
                },
            ))
        });
    }

    // 4. Ids: every cloned node (trait AND entity) gets a FRESH derived id
    // rooted in its own source id when it has one, else a deterministic
    // fallback seed (`derive_materialized_id`) — so two imports of one
    // upstream are pairwise disjoint by id, not just by name. Renaming
    // `td.name` here (AFTER `subs` above was built off the OLD names) is
    // safe because nothing downstream re-derives `subs` from `kept`.
    let mut id_by_name: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for td in kept.iter_mut() {
        let final_name = subs
            .get(&td.name)
            .cloned()
            .expect("every kept trait has a subs entry (built from the same `kept` list above)");
        let final_id = derive_materialized_id(
            td.id.as_deref(),
            upstream.id.as_deref(),
            &td.name,
            import_prefix(&local_name, obj),
            orbital_core_base::identity::IdKind::Trait);
        if let Some(mut origin) = ctx.trait_origin(td, &alias) {
            origin.root_callsite = orbital_index.map(|i| (i, td.name.clone()));
            if let Some(path) = ctx.current_source_path.as_ref() {
                ctx.trait_origins.borrow_mut().insert((path.clone(), final_id.clone()), origin);
            }
        }
        td.name = final_name.clone();
        td.id = Some(final_id.clone());
        id_by_name.insert(final_name, final_id);
    }

    let mut entity_ids: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    entity_ids.insert(primary_final.clone(), primary_id.clone());
    let mut final_aux_entities: Vec<EntityDefinition> = Vec::new();
    for e in &aux_entities {
        let final_name = subs.get(&e.name).cloned().expect(
            "every aux entity has a subs entry (built from the same `aux_entities` list above)",
        );
        let retargeted = in_closure_targets.contains_key(&e.name);
        if retargeted && consumer_entity_ids.contains_key(&final_name) {
            continue;
        }
        let final_id = match (retargeted, e.id.as_ref()) {
            (true, Some(own)) => own.clone(),
            _ => derive_materialized_id(
                e.id.as_deref(),
                upstream.id.as_deref(),
                &e.name,
                import_prefix(&local_name, obj),
                orbital_core_base::identity::IdKind::Entity),
        };
        entity_ids.insert(final_name.clone(), final_id.clone());
        let mut ed = e.clone();
        ed.name = final_name;
        ed.id = Some(final_id);
        rewrite_entity_fields_in_entity(&mut ed, &field_subs);
        final_aux_entities.push(ed);
    }
    // Stage B: `entities {}` — an out-of-orbital relation remap. "Out-of-
    // orbital" = named by the imported closure (a relation target, a
    // fetch/persist/`@Entity` token, a page's primary entity) but declared
    // by a DIFFERENT orbital of the SAME upstream alias — a decidable set
    // once every orbital the alias loaded is known (`ctx.get_entry`; Stage
    // A's own invariant is that every orbital an alias holds is already
    // fully inlined). `closure` is upstream's OWN vocabulary (primary + aux,
    // pre-rename, i.e. what `subs` already covers); `upstream_universe` is
    // everything ELSE the alias declares.
    let closure: HashSet<String> = std::iter::once(upstream_entity.name.clone())
        .chain(aux_entities.iter().map(|e| e.name.clone()))
        .collect();
    let alias_entry = ctx.get_entry(&alias)?;
    let mut upstream_universe: HashSet<String> = HashSet::new();
    // Full definitions of the same out-of-orbital set, keyed by name — needed
    // (below) to read `identity` and to clone an AUX-ONLY one into this
    // import as its own aux entity. Kept separate from `upstream_universe`
    // itself so `record_referenced`/`collect_relation_targets_referenced`/
    // `collect_entity_slots` (pure name-membership checks, shared with other
    // call sites) stay untouched.
    let mut upstream_universe_defs: HashMap<String, EntityDefinition> = HashMap::new();
    // Every out-of-orbital name that is the PRIMARY entity of at least one
    // OTHER orbital of this alias — author-declared domain data (e.g.
    // `Employee`), regardless of whether persistence happens to be runtime.
    // A name that never appears here (only ever an aux, materialized by
    // descent from some atom) is "aux-only" and auto-clones below.
    let mut primary_anywhere: HashSet<String> = HashSet::new();
    // Every field any upstream copy of an out-of-orbital entity carries: a merge the
    // alias landed sits on one copy only, and a retarget below draws from all of them.
    let mut upstream_universe_fields: HashMap<String, Vec<FieldDefinition>> = HashMap::new();
    let mut pool_fields = |def: &EntityDefinition| {
        let pool = upstream_universe_fields
            .entry(def.name.clone())
            .or_default();
        for f in &def.fields {
            if !pool.iter().any(|p| p.name == f.name) {
                pool.push(f.clone());
            }
        }
    };
    for other in alias_entry.orbitals.iter() {
        if other.name == upstream.name {
            continue;
        }
        if let EntityRef::Inline(def) = &other.entity {
            if !closure.contains(&def.name) {
                upstream_universe.insert(def.name.clone());
                upstream_universe_defs.insert(def.name.clone(), def.clone());
                primary_anywhere.insert(def.name.clone());
                pool_fields(def);
            }
        }
        for aux in other.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = aux {
                if !closure.contains(&def.name) {
                    upstream_universe.insert(def.name.clone());
                    upstream_universe_defs.insert(def.name.clone(), def.clone());
                    pool_fields(def);
                }
            }
        }
    }

    // `referenced`: every (site, out-of-orbital name) pair the kept closure
    // actually touches, computed BEFORE any rename below runs so the tokens
    // are still upstream's own vocabulary — relation targets on the primary
    // and every auxiliary entity, entity slots (`collect_entity_slots`,
    // the SAME position rule the rename walk uses) across every kept
    // trait's transition/tick guards+effects, listen guard/payloadMapping,
    // and config-field defaults, `source_entity_definition`'s own relation
    // targets + the trait's `linked_entity`, and every page's `primary_entity`.
    let mut referenced: Vec<(String, String)> = Vec::new();
    collect_relation_targets_referenced(
        &upstream_entity.fields,
        "primaryEntity",
        &upstream_universe,
        &mut referenced,
    );
    for e in &aux_entities {
        collect_relation_targets_referenced(
            &e.fields,
            &format!("auxEntity.{}", e.name),
            &upstream_universe,
            &mut referenced,
        );
    }
    for td in &kept {
        let site = format!("trait.{}", td.name);
        for name in entity_names_read_by_trait(td, &upstream_universe) {
            record_referenced(&site, &name, &upstream_universe, &mut referenced);
        }
    }
    if let Some(pages) = &upstream.pages {
        for p in pages {
            if let PageRef::Inline(pd) = p {
                if let Some(pe) = pd.primary_entity.as_ref() {
                    record_referenced(
                        &format!("page.{}", pd.path),
                        pe,
                        &upstream_universe,
                        &mut referenced,
                    );
                }
            }
        }
    }

    // Ruling (B4-R2, regression on B2-R's original "every out-of-orbital
    // reference must be mapped" rule): an out-of-orbital entity that is
    // AUX-ONLY in the upstream — never the PRIMARY entity of any other
    // orbital of the alias, only ever materialized by descent from an atom
    // (`std-app-layout`'s `AppLayoutData`, `std-notification-panel`'s
    // `NotificationRecord`) — is UI/component state, not a shared row: it
    // sits on whichever sibling orbital happened to pull it first, but every
    // OTHER orbital of the same alias that also embeds that sibling trait
    // needs its OWN copy, not a mapping to a row that doesn't conceptually
    // exist yet. So an aux-only entity auto-clones into this import exactly
    // like the closure's own aux entities (below) and is NEVER demanded in
    // `entities {}` — the consumer may still map it explicitly (to genuinely
    // share state across imports), that's just not required. PERSISTENCE is
    // NOT the discriminator (a pulled-sibling aux is routinely `persistence:
    // persistent` storage, e.g. `NotificationRecord`'s `collection:
    // notifications`) — an entity that IS the primary of another orbital
    // (author-declared domain data, e.g. `Employee`) keeps the original
    // rule: unmapped is `ORB_O_ENTITY_UNMAPPED`, even if it happens to be
    // `persistence: runtime`. EXCEPTION: an aux-only `[identity]` roster
    // (e.g. `Member`) is NEVER auto-cloned — cloning it would fork `@user`
    // role/identity comparisons away from the roster every other orbital
    // shares, so it keeps the "must be mapped" rule too (JS twin B4-J3
    // parity).
    let mut aux_only_referenced_order: Vec<String> = Vec::new();
    let mut aux_only_referenced_seen: HashSet<String> = HashSet::new();
    for (_, entity) in &referenced {
        let is_aux_only = !primary_anywhere.contains(entity)
            && upstream_universe_defs
                .get(entity)
                .is_some_and(|def| !def.identity);
        if is_aux_only {
            if aux_only_referenced_seen.insert(entity.clone()) {
                aux_only_referenced_order.push(entity.clone());
            }
            continue;
        }
    }
    // A target a sibling orbital declares, promised by this orbital's `expects entity`:
    // the rename applies (no id — an expectation carries none); compose link-checks it.
    let expected_targets: HashSet<String> =
        expected_entity_names(orbital)
            .into_iter()
            .map(str::to_string)
            .collect();
    // An identity the alias EXPECTS (declared by no orbital of it) is out-of-orbital
    // too: `entities { Person: Customer }` retargets it onto the consumer's roster.
    let expected_identities: HashSet<String> = alias_entry
        .orbitals
        .iter()
        .flat_map(|o| o.expects.iter().flatten())
        .filter_map(|e| match e {
            orbital_core_base::schema::ExpectDeclaration::Identity { name: Some(n), .. } => {
                Some(n.clone())
            }
            _ => None,
        })
        .filter(|n| !closure.contains(n) && !upstream_universe.contains(n))
        .collect();
    let expected_entities: HashSet<String> = upstream
        .expects
        .iter()
        .flatten()
        .filter_map(|e| match e {
            orbital_core_base::schema::ExpectDeclaration::Entity { name, .. } => Some(name.clone()),
            _ => None,
        })
        .filter(|n| !closure.contains(n) && !upstream_universe.contains(n.as_str()))
        .collect();
    let mut expected_entity_map: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    let mut entities_to_map: orbital_core_base::collections::IndexMap<String, String> =
        obj.entities.clone().unwrap_or_default();
    for (name, target) in &in_closure_targets {
        entities_to_map.entry(name.clone()).or_insert_with(|| target.clone());
    }
    {
        for (up_name, target) in &entities_to_map {
            if expected_entities.contains(up_name) {
                expected_entity_map.insert(up_name.clone(), target.clone());
                continue;
            }
            let in_closure_aux = in_closure_targets.contains_key(up_name);
            if !in_closure_aux
                && !upstream_universe.contains(up_name.as_str())
                && !expected_identities.contains(up_name.as_str())
            {
                continue;
            }
            let mapped = match consumer_entity_ids.get(target) {
                Some(id) => {
                    // Fold into the SAME rename map every other closure
                    // entity goes through — an out-of-orbital rebind is not
                    // a special case downstream, it is one more `subs` entry.
                    subs.insert(up_name.clone(), target.clone());
                    entity_ids.insert(target.clone(), id.clone());
                    true
                }
                None if expected_targets.contains(target) => {
                    subs.insert(up_name.clone(), target.clone());
                    true
                }
                None if in_closure_aux && target == up_name => false,
                None => false,
            };
            // The retarget shares the consumer's entity, so it must carry the
            // fields this import's traits use: queue the upstream definition's
            // fields onto the target (missing names only, the owner's win).
            let def = upstream_universe_defs
                .get(up_name)
                .or_else(|| aux_entities.iter().find(|e| &e.name == up_name));
            if let (true, Some(def)) = (mapped, def) {
                let mut imported = def.clone();
                if let Some(pool) = upstream_universe_fields.get(up_name) {
                    imported.fields = pool.clone();
                }
                let used: HashSet<String> = kept
                    .iter()
                    .flat_map(|td| {
                        let own = td.linked_entity.as_deref() == Some(def.name.as_str());
                        super::entity::trait_referenced_fields_of_entity(td, &imported, own)
                    })
                    .collect();
                imported.fields.retain(|f| {
                    f.merged_from.is_none() || f.name.as_ref().is_some_and(|n| used.contains(n))
                });
                rewrite_entity_fields_in_entity(&mut imported, &field_subs);
                let fields: HashSet<String> = imported
                    .fields
                    .iter()
                    .filter_map(|f| f.name.clone())
                    .collect();
                deferred_merges.push(super::DeferredMerge {
                    target: target.clone(),
                    imported,
                    intrinsic_only: false,
                    required_data: HashSet::new(),
                    referenced_fields: fields,
                });
            }
        }
    }
    // Auto-clone every referenced AUX-ONLY out-of-orbital entity the
    // consumer did NOT explicitly map above — same treatment as
    // `aux_entities`/`final_aux_entities` (fresh derived id, `subs` +
    // `entity_ids` entry, field-rename through `field_subs`), so two imports
    // of the same upstream still yield two disjoint clones (each prefixed by
    // its own `local_name`).
    for name in &aux_only_referenced_order {
        if obj
            .entities
            .as_ref()
            .is_some_and(|m| m.contains_key(name.as_str()))
        {
            continue;
        }
        let Some(def) = upstream_universe_defs.get(name) else {
            continue;
        };
        let final_name = format!("{}{name}", import_prefix(&local_name, obj));
        let final_id = derive_materialized_id(
            def.id.as_deref(),
            upstream.id.as_deref(),
            name,
            import_prefix(&local_name, obj),
            orbital_core_base::identity::IdKind::Entity);
        subs.insert(name.clone(), final_name.clone());
        entity_renames.insert(name.clone(), final_name.clone());
        entity_ids.insert(final_name.clone(), final_id.clone());
        let mut ed = def.clone();
        ed.name = final_name;
        ed.id = Some(final_id);
        rewrite_entity_fields_in_entity(&mut ed, &field_subs);
        final_aux_entities.push(ed);
    }

    // Stage B: `roles {}` — `role_fields` is every UPSTREAM identity entity's
    // own vocabulary-bearing field (field name → declared values), scoped to
    // the alias's own orbitals (an import's roles remap always resolves
    // against the UPSTREAM's own words, never the consumer's roster).
    // `identity_entities_of` applies the same primary-shadows-import rule
    // `identity_entities(schema)` does, just over one alias's orbitals.
    let identity_entities = identity_entities_of(&alias_entry.orbitals);
    let mut role_fields: orbital_core_base::collections::IndexMap<String, Vec<String>> = orbital_core_base::collections::IndexMap::new();
    for identity in &identity_entities {
        for field in &identity.fields {
            let Some(name) = field.name.as_ref() else {
                continue;
            };
            if let Some(values) = vocabulary(field) {
                role_fields.insert(name.clone(), values.clone());
            }
        }
    }
    // An upstream with no roster of its own declares its viewer by `expects entity [identity]`; the
    // expectation's declared role values are then the upstream vocabulary.
    if role_fields.is_empty() {
        for expect in upstream.expects.iter().flatten() {
            if let orbital_core_base::schema::ExpectDeclaration::Identity {
                shape: Some(shape), ..
            } = expect
            {
                for field in shape {
                    if let (Some(name), Some(values)) = (field.name.as_ref(), vocabulary(field)) {
                        role_fields.insert(name.clone(), values.clone());
                    }
                }
            }
        }
    }
    let roles: orbital_core_base::collections::IndexMap<String, Vec<String>> = obj.roles.clone().unwrap_or_default();
    // (A) every auxiliary entity's own relation fields (e.g. a self-relation,
    // or a pointer to the primary entity or to ANOTHER aux) must resolve
    // inside the materialized closure — run as a SEPARATE pass, after the
    // loop above, so `entity_ids` is fully populated (primary + every aux)
    // before any relation's `entity_id` sibling is looked up; renaming
    // in-loop would see a partial map for a target declared later in
    // `aux_entities`. Role-literal rewriting rides along — same "every
    // materialized entity" scope `rewrite_entity_fields_in_entity` gets.
    for ed in final_aux_entities.iter_mut() {
        rename_entity_refs_in_entity(ed, &subs, &entity_ids);
        rewrite_role_literals_in_entity(ed, &role_fields, &roles);
    }

    // 5. Pages: `obj.pages` remaps upstream→local path; an unmapped path
    // keeps its upstream value. Built before the per-trait pipeline below —
    // it does not depend on any trait mutation, only on `subs`/`id_by_name`.
    let mut path_map: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
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
    let nav_path_map: orbital_core_base::collections::IndexMap<String, String> = {
        let mut m = path_map.clone();
        m.extend(sibling_page_remaps(
            &alias,
            &local_name,
            &path_map,
            listen_source_targets,
        ));
        m
    };
    // `omit { "/path" }` drops that upstream page (a quoted entry names a page).
    let omitted_pages: HashSet<&str> = obj
        .omit
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|e| e.starts_with('/'))
        .collect();
    // Snapshot every kept trait's declared config, keyed by its FINAL
    // (post-rename) name, for the step-6 embedder lookup below — a clone
    // taken before the loop starts iter_mut over `kept`, since the loop
    // would otherwise need to borrow `kept` both mutably (the trait being
    // processed) and immutably (an embedder's config) at once.
    let parent_configs: orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>> = kept
        .iter()
        .filter_map(|td| td.config.clone().map(|cfg| (td.name.clone(), cfg)))
        .collect();
    // Snapshotted alongside `parent_configs` — the loop below borrows `kept`
    // mutably, so its length must be read before that borrow starts.
    let kept_len = kept.len();

    // 6. Per-trait pipeline, in the same order as the JS twin
    // (`materializeOrbitalRef`): embed tokens → entity roots → entity
    // fields → listen sources → navigate targets → config forward → PAGE
    // PATHS → events. (i) Page-path rewriting runs AFTER the config forward
    // (planning find i): a knob like `navItems: "@config.navItems"` only
    // HOLDS its literal hrefs once the forward resolves it — rewriting
    // page-path literals first (the old order) missed every href sitting
    // behind an unresolved forward.
    let event_scope = match obj.events.as_ref() {
        Some(events) => resolve_event_scope_renames(kept.iter(), events)?,
        None => EventScopeRenames::default(),
    };
    for td in kept.iter_mut() {
        // Keys whose config-forward resolution reached all the way out to
        // `schema_config` (the CONSUMER's single app-level `config {}`,
        // shared by every orbital import in the schema) — populated below,
        // consumed by (D)'s page-path rewrite so it never touches one of
        // these: that data isn't this trait's own, it's the consumer's
        // outer, possibly multi-orbital structure passing through.
        let mut schema_only_keys: HashSet<String> = HashSet::new();
        rewrite_trait_embed_tokens(td, &subs);
        rename_entities_in_trait(td, &subs, &entity_ids);
        // (B) the trait's own source-entity provenance copy must track the
        // same rename, or a trait whose atom-original entity coincided with
        // upstream's own entity name reads as a spurious rebind post-import.
        rename_source_entity_definition(td, &subs, &entity_ids);
        // Stage B: role literals in policies/guards/effects/listens/config
        // defaults, and (inside) `source_entity_definition`'s own policies —
        // same scope `rewrite_entity_fields_in_trait` covers, run right
        // after the source-entity rename above per §5 of the plan.
        rewrite_role_literals_in_trait(td, &role_fields, &roles);
        if !field_subs.is_empty() {
            rewrite_entity_fields_in_trait(td, &field_subs);
        }
        rewrite_listen_sources(
            td,
            &subs,
            &id_by_name,
            &upstream.name,
            &local_name,
            &alias,
            listen_source_targets,
        );
        if !nav_path_map.is_empty() {
            rewrite_trait_navigate_targets(td, &nav_path_map);
        }
        // Config forward: resolve against EVERY surviving embedder in
        // `kept`, in order, merging each embedder's delta into `td.config`
        // immediately so a LATER embedder's lookup sees the knobs an
        // earlier one already resolved (JS semantic,
        // `reference-resolver.ts` ~L3170-3179) — replaces the old
        // first-embedder-wins `break`, which silently dropped every other
        // embedder's forward on a trait with more than one. `td.name` is
        // already the FINAL name here (step 4 ran before this loop),
        // matching `embedders_of`/`parent_configs`.
        if let Some(embedders) = embedders_of.get(&td.name) {
            for embedder in embedders {
                let Some(parent_cfg) = parent_configs.get(embedder) else {
                    continue;
                };
                if let Some(delta) = forwarded_sibling_config_from_tracked(
                    td.config.as_ref(),
                    &[parent_cfg],
                    folded_config.as_ref(),
                    None,
                    None,
                    schema_config,
                    Some(&mut schema_only_keys),
                ) {
                    let cfg = td.config.get_or_insert_with(orbital_core_base::collections::IndexMap::new);
                    for (k, v) in delta {
                        cfg.insert(k, v);
                    }
                }
            }
        }
        // Hoisted multi-hop forward: a knob none of the DIRECT embedders
        // above resolved may still resolve through an embedder's own
        // embedder chain — JSX-hoisted chrome nests several trait layers
        // deep (`Feed` embeds `Stack8` embeds `Stack5` embeds `Typography4`,
        // and only `Feed` declares the knob `Typography4` forwards). Walks
        // out from the first direct embedder via `embedders_of`'s
        // first-embedder-per-hop (see `embedder_chain_via_first_hop`); a
        // key already resolved by the direct-embedder loop above is a no-op
        // here (its default is no longer a forward string).
        if let Some(first_embedder) = embedders_of.get(&td.name).and_then(|v| v.first()) {
            let chain = embedder_chain_via_first_hop(
                first_embedder,
                &embedders_of,
                &parent_configs,
                kept_len,
            );
            if let Some(delta) = forwarded_sibling_config_from_tracked(
                td.config.as_ref(),
                &chain,
                folded_config.as_ref(),
                None,
                None,
                schema_config,
                Some(&mut schema_only_keys),
            ) {
                let cfg = td.config.get_or_insert_with(orbital_core_base::collections::IndexMap::new);
                for (k, v) in delta {
                    cfg.insert(k, v);
                }
            }
        }
        // No-embedder fallback (orbital/schema rungs only) — also catches
        // any knob no embedder above resolved.
        if let Some(delta) = forwarded_sibling_config_from_tracked(
            td.config.as_ref(),
            &[],
            folded_config.as_ref(),
            None,
            None,
            schema_config,
            Some(&mut schema_only_keys),
        ) {
            let cfg = td.config.get_or_insert_with(orbital_core_base::collections::IndexMap::new);
            for (k, v) in delta {
                cfg.insert(k, v);
            }
        }
        // (E) G3 (`docs/Almadar_Compiler_Gaps.md` §82): a config-forward
        // step just above can hand this trait a brand-new literal (e.g. a
        // JSX-hoisted render's `columns: @config.columns` resolving to its
        // embedder's declared array) that still names a PRE-rename field —
        // this trait's own field rewrite already ran, earlier in this same
        // loop iteration, before the forward existed — see
        // `rewrite_entity_fields_in_config`'s doc.
        if !field_subs.is_empty() {
            if let Some(cfg) = td.config.as_mut() {
                rewrite_entity_fields_in_config(cfg, &field_subs);
            }
        }
        rewrite_entity_typed_config_values(td, &subs, &entity_ids);
        rewrite_role_literals_in_config_lists(td, &role_fields, &roles, &schema_only_keys);
        if let Some(events) = obj.events.as_ref() {
            rename_events_in_trait(td, events, &subs, &event_scope)?;
        }
    }

    // Planning find (iii): drop a folded orbital knob no SURVIVING trait
    // forwards. An `omit {}` can silently strand a knob whose only forwarder
    // was the omitted trait — `ORB_O_CONFIG_DEAD_KNOB` on the consumer.
    // Checked against `parent_configs` (every KEPT trait's declared config,
    // snapshotted BEFORE the per-trait loop above resolved its own
    // forwards) — the just-run loop already overwrote a resolving trait's
    // `@config.<key>` default with its resolved value, so `kept`'s current
    // config no longer carries the literal forward string to match against.
    let folded_config: Option<orbital_core_base::collections::IndexMap<String, ConfigField>> = folded_config.map(|decl| {
        decl.into_iter()
            .filter(|(key, _)| {
                let token = format!("@config.{key}");
                let forward = SExpression::string(token.clone());
                parent_configs.values().any(|cfg| {
                    cfg.values().any(|f| {
                        f.default.as_ref() == Some(&forward)
                            || f.forwarded_from.as_deref() == Some(token.as_str())
                            || f.nested_forwarded_from.contains(&token)
                    })
                })
            })
            .collect()
    });

    let mut final_pages: Vec<PageRef> = Vec::new();
    if let Some(pages) = &upstream.pages {
        for p in pages {
            let PageRef::Inline(page_def) = p else {
                continue;
            };
            let route = page_def.is_route();
            if route && omitted_pages.contains(page_def.path.as_str()) {
                continue;
            }
            let mut pd = page_def.clone();
            if route {
                pd.path = path_map
                    .get(&pd.path)
                    .cloned()
                    .unwrap_or_else(|| pd.path.clone());
            }
            // Which upstream page this is: kept through nested imports, so the
            // per-locale imports of one page share it (their hreflang alternates).
            pd.source_page = page_def.source_page.clone().or_else(|| page_def.id.clone());
            if let Some(mods) = obj
                .page_modifiers
                .as_ref()
                .filter(|_| route)
                .and_then(|m| m.get(&page_def.path))
            {
                let local_path = pd.path.clone();
                super::page::apply_page_modifiers(&mut pd, mods, &obj.reference, &local_path)?;
            }
            // Unconditional prefix (§4.3), same rule as traits/entities — a
            // materialized page's NAME is upstream vocabulary until here, so
            // two imports of one upstream (each with its own `pages {}` path
            // remap) would otherwise collide on `PageDefinition.name` even
            // though their paths are disjoint. JS twin: identical rule in
            // `materializeOrbitalRef`.
            pd.name = format!("{}{}", import_prefix(&local_name, obj), pd.name);
            // Same rule as traits/entities: a page id is re-derived per
            // import so two imports of one upstream never share one (JS twin:
            // `asPageId(deriveMaterializedId(...))` in `materializeOrbitalRef`).
            pd.id = Some(derive_materialized_id(
                page_def.id.as_deref(),
                upstream.id.as_deref(),
                &page_def.name,
                import_prefix(&local_name, obj),
                orbital_core_base::identity::IdKind::Page));
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
            // Stage B: `mounts {upstreamPath: [siblingTraitName, …]}` — a
            // sibling trait declared in the reference body, mounted on this
            // (already-remapped) page. Siblings are consumer vocabulary and
            // are NEVER prefixed, so the mount is a plain local-name
            // reference (matches the local-trait-mount shape
            // `orbital-lolo`'s own page lowering already produces —
            // `reference: Some(name)`, no alias qualification).
            if let Some(mount_names) = obj
                .mounts
                .as_ref()
                .filter(|_| route)
                .and_then(|m| m.get(&page_def.path))
            {
                let sibling_names: HashSet<&str> = orbital
                    .traits
                    .iter()
                    .filter_map(|t| match t {
                        TraitReference::Inline(td) => Some(td.name.as_str()),
                        TraitReference::Reference { name, .. } => name.as_deref(),
                    })
                    .collect();
                let traits = pd.traits.get_or_insert_with(Vec::new);
                for name in mount_names {
                    // A mount naming a kept imported trait follows its prefix; a sibling keeps its name.
                    let name = match subs.get(name.as_str()) {
                        Some(to)
                            if !sibling_names.contains(name.as_str())
                                && kept.iter().any(|td| &td.name == to) =>
                        {
                            to.clone()
                        }
                        _ => name.clone(),
                    };
                    let already = traits.iter().any(|t| match t {
                        TraitReference::Reference {
                            reference, name: n, ..
                        } => {
                            reference.as_deref() == Some(name.as_str())
                                || n.as_deref() == Some(name.as_str())
                        }
                        TraitReference::Inline(td) => td.name == name,
                    });
                    if already {
                        continue;
                    }
                    traits.push(TraitReference::Reference {
                        dispatch: None,
                        reference: Some(name.clone()),
                        ref_id: None,
                        id: None,
                        name: None,
                        linked_entity: None,
                        linked_entity_id: None,
                        config: None,
                        events: None,
                        event_ids: None,
                        fields: None,
                        listens: None,
                        emits_scope: None,
                        type_args: None,
                    });
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
    rewrite_role_literals_in_entity(&mut primary_entity, &role_fields, &roles);

    // Stage C: `extend { … }` — append fields declared in the reference
    // body onto the materialized primary entity, AFTER `fields {}` renames
    // apply (already run above). A relation-typed added field resolves its
    // target in the CONSUMER scope (`consumer_entity_ids` — never `subs`/
    // `entity_ids`, which only cover upstream-renamed names): `Workspace!`
    // means the CONSUMER's `Workspace`, never an upstream entity of the
    // same name.
    if let Some(extend_fields) = obj.extend.as_ref() {
        let existing_names: HashSet<String> = primary_entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect();
        let rename_targets: HashSet<&str> = field_subs.values().map(String::as_str).collect();

        for ef in extend_fields {
            let Some(name) = ef.name.clone() else {
                continue;
            };
            if existing_names.contains(&name) || rename_targets.contains(name.as_str()) {
                continue;
            }
            let mut field = ef.clone();
            if let Some(rel) = field.relation.as_mut() {
                if let Some(id) = consumer_entity_ids.get(&rel.entity) {
                    rel.entity_id = Some(id.clone());
                }
            }
            primary_entity.fields.push(field);
        }
    }

    // `retype { … }`: replace the type/default of fields the upstream primary already
    // declares (post-rename). Identity, provenance, docs and key/unique marks carry over.
    if let Some(retype) = obj.retype.as_ref() {
        let extended: HashSet<&str> = obj
            .extend
            .iter()
            .flatten()
            .filter_map(|f| f.name.as_deref())
            .collect();
        let mut seen: HashSet<String> = HashSet::new();
        for rf in retype {
            let Some(name) = rf.name.clone() else {
                continue;
            };
            if extended.contains(name.as_str()) || !seen.insert(name.clone()) {
                continue;
            }
            let Some(slot) = primary_entity
                .fields
                .iter_mut()
                .find(|f| f.name.as_deref() == Some(name.as_str()))
            else {
                continue;
            };
            let mut field = rf.clone();
            if let Some(rel) = field.relation.as_mut() {
                if let Some(id) = consumer_entity_ids.get(&rel.entity) {
                    rel.entity_id = Some(id.clone());
                }
            }
            field.mock = field.mock.or_else(|| slot.mock.take());
            field.description = field.description.or_else(|| slot.description.take());
            field.synonyms = field.synonyms.or_else(|| slot.synonyms.take());
            field.primary_key = field.primary_key.or(slot.primary_key);
            field.unique = field.unique.or(slot.unique);
            field.intrinsic = field.intrinsic.or(slot.intrinsic);
            field.merged_from = slot.merged_from.take();
            *slot = field;
        }
    }

    // `entity Name [persistent: tbl, …]`: the factory's persistence + collection overrides,
    // allowed only where the factory allowed them (a `persistent` upstream entity).
    if obj.persistence.is_some() || obj.collection.is_some() {
        if matches!(
            primary_entity.persistence,
            None | Some(Persistence::Persistent)
        ) {
            if let Some(mode) = obj.persistence.clone() {
                primary_entity.persistence = Some(mode);
            }
            if let Some(collection) = obj.collection.clone() {
                primary_entity.collection = Some(collection);
            }
        }
    }
    primary_entity.shared |= obj.shared;
    primary_entity.identity |= obj.identity;
    primary_entity.browser |= obj.browser;

    if let Some(mock) = obj.mock.as_ref() {
        let mut keys: Vec<&String> = mock.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(field) = primary_entity
                .fields
                .iter_mut()
                .find(|f| f.name.as_deref() == Some(key.as_str()))
            {
                field.mock = Some(mock[key].clone());
            }
        }
    }

    if !unowned_merged_fields.is_empty() {
        primary_entity.fields.retain(|f| {
            f.merged_from.is_none()
                || f.name.as_deref().is_none_or(|n| {
                    let upstream_name = field_subs
                        .iter()
                        .find(|(_, to)| to.as_str() == n)
                        .map(|(from, _)| from.as_str())
                        .unwrap_or(n);
                    !unowned_merged_fields.contains(upstream_name)
                })
        });
    }
    orbital.entity = EntityRef::Inline(primary_entity);
    orbital.auxiliary_entities = if final_aux_entities.is_empty() {
        None
    } else {
        Some(
            final_aux_entities
                .into_iter()
                .map(EntityRef::Inline)
                .collect(),
        )
    };
    // Siblings (Stage B): trait declarations authored directly in the
    // reference body — never prefixed, `omit`/`only` never apply — ride
    // along as `orbital.traits` (lowered there by `orbital-lolo`, no longer
    // forced empty). They are CONSUMER-scope, so they splice in AFTER the
    // materialized (upstream) closure and are resolved by the same
    // `inline_traits` pass that runs on this orbital right after this
    // function returns (`inline_orbital` calls `resolve_orbital_reference`
    // before its own `inline_traits` call — verified in `mod.rs`), exactly
    // like any other trait this orbital declares.
    // `listens {}`: declared routes INTO the imported set. The target is an
    // upstream trait name; the source is another imported trait (follows the
    // prefix) or a host sibling (keeps its name). The trigger is written in
    // the post-`events {}` vocabulary the author sees.
    let body_trait_names: HashSet<String> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            TraitReference::Reference { name, .. } => name.clone(),
        })
        .collect();
    for entry in obj.listens.iter().flatten() {
        if let Some(orbital_core_base::schema::ListenSource::Trait { trait_name, .. }) =
            entry.listen.source.as_ref()
        {
            let imported = subs
                .get(trait_name.as_str())
                .is_some_and(|n| kept.iter().any(|td| &td.name == n));
            if !imported && !body_trait_names.contains(trait_name) {
                let mut candidates: Vec<&str> = kept
                    .iter()
                    .filter_map(|td| {
                        subs.iter()
                            .find(|(_, to)| *to == &td.name)
                            .map(|(from, _)| from.as_str())
                    })
                    .collect();
                candidates.sort_unstable();
                return Err(InlineError::OrbitalRefListenSourceUnknown {
                    from_trait: trait_name.clone(),
                    candidates: candidates.join(", "),
                    path: diag_path.clone(),
                });
            }
        }
        let final_name = subs
            .get(&entry.trait_name)
            .filter(|n| kept.iter().any(|td| &td.name == *n))
            .ok_or_else(|| InlineError::OrbitalRefListenTargetUnknown {
                target: entry.trait_name.clone(),
                path: diag_path.clone(),
            })?;
        let mut listen = entry.listen.clone();
        if let Some(orbital_core_base::schema::ListenSource::Trait {
            trait_name,
            trait_id,
        }) = listen.source.as_mut()
        {
            if let Some(imported) = subs.get(trait_name.as_str()) {
                *trait_name = imported.clone();
                *trait_id = None;
            }
        }
        if let Some(td) = kept.iter_mut().find(|td| &td.name == final_name) {
            td.listens.get_or_insert_with(Default::default).push(listen);
        }
    }

    let mut siblings = std::mem::take(&mut orbital.traits);
    let imported: HashSet<&str> = kept.iter().map(|td| td.name.as_str()).collect();
    let sibling_names: HashSet<String> = siblings
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.clone()),
            TraitReference::Reference { name, .. } => name.clone(),
        })
        .collect();
    let embed_subs: orbital_core_base::collections::IndexMap<String, String> = subs
        .iter()
        .filter(|(from, to)| imported.contains(to.as_str()) && !sibling_names.contains(*from))
        .map(|(from, to)| (from.clone(), to.clone()))
        .collect();
    for sibling in siblings.iter_mut() {
        match sibling {
            TraitReference::Inline(td) => rewrite_trait_embed_tokens(Arc::make_mut(td), &embed_subs),
            TraitReference::Reference { config, .. } => {
                if let Some(config) = config.as_mut() {
                    rewrite_config_embed_tokens(config, &embed_subs);
                }
            }
        }
    }
    for sibling in siblings.iter_mut() {
        let renamed = |listen: &orbital_core_base::schema::ListenDefinition| match &listen.source {
            Some(orbital_core_base::schema::ListenSource::Trait { trait_name, .. }) => subs
                .get(trait_name.as_str())
                .filter(|to| imported.contains(to.as_str()))
                .cloned(),
            _ => None,
        };
        let rewrite = |listens: &mut [orbital_core_base::schema::ListenDefinition]| {
            for listen in listens.iter_mut() {
                if let Some(to) = renamed(listen) {
                    if let Some(orbital_core_base::schema::ListenSource::Trait {
                        trait_name,
                        trait_id,
                    }) = listen.source.as_mut()
                    {
                        *trait_name = to;
                        *trait_id = None;
                    }
                }
            }
        };
        match sibling {
            TraitReference::Inline(td) => {
                if td.listens.iter().flat_map(|l| l.iter()).any(|l| renamed(l).is_some()) {
                    if let Some(listens) = Arc::make_mut(td).listens.as_mut() {
                        rewrite(listens);
                    }
                }
            }
            TraitReference::Reference { listens, .. } => {
                if let Some(listens) = listens.as_mut() {
                    rewrite(listens);
                }
            }
        }
    }
    orbital.traits = kept
        .into_iter()
        .map(|td| TraitReference::Inline(Arc::new(td)))
        .chain(siblings)
        .collect();
    orbital.pages = if final_pages.is_empty() {
        None
    } else {
        Some(final_pages)
    };
    orbital.config = folded_config;
    // The imported traits read the aliases the upstream declares (a `json/parse x Name` shape), so the composing
    // orbital knows the same names; one it declares itself wins.
    if let Some(upstream_types) = upstream.types.as_ref() {
        let own = orbital.types.get_or_insert_with(Default::default);
        let declaring_file = ctx.aliases.get(&alias).and_then(|entry| entry.source_path.clone());
        for (name, fields) in upstream_types {
            if let std::collections::btree_map::Entry::Vacant(slot) = own.entry(name.clone()) {
                slot.insert(fields.clone());
                if let Some(file) = &declaring_file {
                    ctx.pulled_type_files.borrow_mut().push((name.clone(), file.clone()));
                }
            }
        }
    }
    // Orbital-level `emits`/`listens`/`exposes` name UPSTREAM's pre-rename
    // vocabulary — carrying them through would be wrong, and nothing reads
    // them off a materialized orbital (matches the JS twin's drop; §Report).
    orbital.emits = None;
    orbital.listens = None;
    orbital.exposes = None;
    // The imported traits still read what the upstream declared it reads, so the
    // composing program owes the same — its expectations travel with the import.
    let mut inherited_entity_expects: Vec<String> = Vec::new();
    if let Some(upstream_expects) = upstream.expects.as_ref() {
        let kept_traits = materialized_trait_subs(&upstream, obj, &local_name);
        let own = orbital.expects.get_or_insert_with(Vec::new);
        let mut shape_subs = subs.clone();
        shape_subs.extend(expected_entity_map.iter().map(|(from, to)| (from.clone(), to.clone())));
        for expect in upstream_expects {
            let mut expect = expect.clone();
            // The obligations about the upstream's own traits and pages are owed under the import's renames, and
            // not at all for what the import omits (G-ORB-378).
            if !rename_imported_obligation(&mut expect, &upstream, obj, &kept_traits, &omitted_pages) {
                continue;
            }
            if let orbital_core_base::schema::ExpectDeclaration::Entity { shape: Some(fields), .. }
            | orbital_core_base::schema::ExpectDeclaration::Identity { shape: Some(fields), .. } = &mut expect
            {
                rename_relation_targets_in_fields(fields, &shape_subs, &entity_ids);
            }
            if let orbital_core_base::schema::ExpectDeclaration::Entity { name, strict: true, .. } = &mut expect {
                if let Some(local) = entity_renames.get(name.as_str()) {
                    *name = local.clone();
                }
            }
            let bound_by_pass = matches!(
                &expect,
                orbital_core_base::schema::ExpectDeclaration::Entity { name, .. }
                    if expected_entity_map.contains_key(name)
            );
            match &mut expect {
                _ if bound_by_pass => {}
                orbital_core_base::schema::ExpectDeclaration::Entity { name, .. }
                | orbital_core_base::schema::ExpectDeclaration::Identity {
                    name: Some(name), ..
                } => {
                    if let Some(local) = obj.entities.as_ref().and_then(|m| m.get(name.as_str())) {
                        *name = local.clone();
                    }
                }
                _ => {}
            }
            if let orbital_core_base::schema::ExpectDeclaration::Entity { name, .. } = &expect {
                if !entity_renames.contains_key(name) && !inherited_entity_expects.contains(name) {
                    inherited_entity_expects.push(name.clone());
                }
            }
            // The import's `roles {}` remap carries the upstream role vocabulary into the consumer's.
            if let orbital_core_base::schema::ExpectDeclaration::Identity {
                shape: Some(fields),
                ..
            } = &mut expect
            {
                if let Some(role_remap) = obj.roles.as_ref() {
                    for field in fields.iter_mut() {
                        if let Some(values) = field.values.as_mut() {
                            let mut mapped: Vec<String> = Vec::new();
                            for value in values.iter() {
                                for target in role_remap
                                    .get(value)
                                    .cloned()
                                    .unwrap_or_else(|| vec![value.clone()])
                                {
                                    if !mapped.contains(&target) {
                                        mapped.push(target);
                                    }
                                }
                            }
                            *values = mapped;
                        }
                    }
                }
            }
            if let orbital_core_base::schema::ExpectDeclaration::Identity {
                name: Some(upstream_name),
                ..
            } = &expect
            {
                if let Some(orbital_core_base::schema::ExpectDeclaration::Identity {
                    name: own_name @ None,
                    ..
                }) = own
                    .iter_mut()
                    .find(|e| matches!(e, orbital_core_base::schema::ExpectDeclaration::Identity { .. }))
                {
                    *own_name = Some(upstream_name.clone());
                }
            }
            super::merge_expectation(own, expect);
        }
    }
    // (C) the LOCAL placeholder orbital is its own ledgered node — the
    // id-stamping phase (post-lowering, pre-inline) already gave it an id
    // that the ledger has a row for. Only its CONTENTS are derived from
    // upstream; overwriting `orbital.id` here orphans that ledger row
    // (`ORB_ID_LEDGER_ORPHAN`). Derive from upstream only when the local
    // orbital carries no id of its own.
    if orbital.id.is_none() {
        orbital.id = upstream
            .id
            .as_deref()
            .map(|uid| orbital_core_base::identity::derive_id(uid, &local_name));
    }

    diagnostics.push(OrbitalRefDiagnostic {
        path: diag_path.clone(),
        reference: obj.reference.clone(),
        event_subs: obj.events.clone().unwrap_or_default(),
        local_name: local_name.clone(),
        entity_renames,
        inherited_entity_expects,
        expected_entity_map,
    });

    Ok(())
}

/// Deterministic id for a materialized trait/entity/page node: rooted in the
/// source node's own id when it has one (`derive_id` keeps that id's KIND —
/// trait stays trait, entity stays entity); when the source has none yet
/// (pre-Phase-7, common today), a seed string derived from the upstream
/// orbital id + the pre-rename source name instead — feeding `upstream.id`
/// (orbital-kind, `orb_`) straight into `derive_id` would stamp a
/// trait/entity with the wrong id KIND, since `derive_id` inherits its
/// parent's kind. The seed is prefixed with `kind.prefix()` so `derive_id`'s
/// own kind classification (`id_kind_of`, which just checks the prefix)
/// lands on the CALLER's intended kind instead of always falling back to its
/// default (`trait`) — an entity/page materialized from a source with no
/// real id used to get a `trt_` id regardless. JS twin: `deriveMaterializedId`
/// (`reference-resolver.ts:1547`) threads the same explicit kind through
/// `asEntityId`/`asPageId`/`asTraitId`.
pub(crate) fn derive_materialized_id(
    source_id: Option<&str>,
    upstream_orbital_id: Option<&str>,
    source_name: &str,
    local_name: &str,
    kind: orbital_core_base::identity::IdKind,
) -> String {
    // An import that keeps the upstream's name (an empty prefix) reproduces its ids as declared.
    if let (true, Some(id)) = (local_name.is_empty(), source_id) {
        return id.to_string();
    }
    let seed;
    let parent: &str = match source_id {
        Some(id) => id,
        None => {
            seed = format!(
                "{}orbital-import-seed:{}:{source_name}",
                kind.prefix(),
                upstream_orbital_id.unwrap_or("no-orbital-id")
            );
            &seed
        }
    };
    orbital_core_base::identity::derive_id(parent, local_name)
}

/// Multi-hop embedder chain for the config-forward step-6 fallback: starting
/// at `start` (a direct embedder of the trait being resolved), walks OUT
/// through the FIRST embedder recorded for each hop (`embedders_of`'s
/// `kept`-order list — deterministic; a trait embedded by more than one
/// surviving sibling picks its first for chain-continuation purposes only,
/// mirroring `inline/trait.rs`'s pull-site `parent_of` walk). Returns
/// `parent_configs` snapshots, innermost first, matching
/// `forwarded_sibling_config_from`'s `embedder_chain` contract. Bounded by
/// `bound` (the number of kept traits) with a `visited` guard — defensive,
/// `embedders_of` reflects the acyclic composition graph.
fn embedder_chain_via_first_hop<'a>(
    start: &str,
    embedders_of: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    parent_configs: &'a orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>>,
    bound: usize,
) -> Vec<&'a orbital_core_base::collections::IndexMap<String, ConfigField>> {
    let mut chain = Vec::new();
    let mut current = start.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    let bound = bound.max(1);
    while chain.len() < bound && visited.insert(current.clone()) {
        if let Some(cfg) = parent_configs.get(&current) {
            chain.push(cfg);
        }
        match embedders_of.get(&current).and_then(|v| v.first()) {
            Some(next) => current = next.clone(),
            None => break,
        }
    }
    chain
}

/// `only { … }` keeps the named traits and everything they embed (their render
/// children), transitively — a kept view without its children renders nothing.
pub(crate) fn only_closure(upstream: &Orbital, only: &[String]) -> HashSet<String> {
    let mut keep: HashSet<String> = only.iter().cloned().collect();
    loop {
        let mut grew = false;
        for tr in &upstream.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            if !keep.contains(&td.name) {
                continue;
            }
            for child in embedded_trait_refs_of(td) {
                grew |= keep.insert(child);
            }
        }
        if !grew {
            return keep;
        }
    }
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
/// An orbital import renames every entity of its closure to `<Local><Upstream>`, and the entities its
/// upstream `expects` of a sibling are named in the upstream's own words. An `entities { N: Target }` entry
/// binds the expectation `N` to `Target` explicitly. Without one, when no entity of the program carries `N`
/// and exactly one sibling import provides an entity of that upstream name, the expectation follows that
/// provider; several providers are `ORB_O_ENTITY_EXPECT_AMBIGUOUS`. The expectation and every reference to
/// it in the expecting import are rewritten, so a writer and a reader address one collection.
pub(crate) fn bind_inherited_entity_expectations(
    schema: &mut orbital_core_base::schema::OrbitalSchema,
    diagnostics: &mut [OrbitalRefDiagnostic],
) {
    let mut entity_ids: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for orbital in &schema.orbitals {
        for er in std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten()) {
            if let EntityRef::Inline(def) = er {
                if let Some(id) = def.id.as_ref() {
                    entity_ids.entry(def.name.clone()).or_insert_with(|| id.clone());
                }
            }
        }
    }
    let existing: HashSet<String> = entity_ids.keys().cloned().collect();
    type Plan = (usize, orbital_core_base::collections::IndexMap<String, String>);
    let mut plans: Vec<Plan> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (at, diag) in diagnostics.iter().enumerate() {
        if diag.inherited_entity_expects.is_empty() || !seen.insert(diag.local_name.clone()) {
            continue;
        }
        let mut subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
        for expected in &diag.inherited_entity_expects {
            if let Some(target) = diag.expected_entity_map.get(expected) {
                if existing.contains(target) {
                    subs.insert(expected.clone(), target.clone());
                }
                continue;
            }
            if existing.contains(expected) {
                continue;
            }
            let mut providers: Vec<String> = diagnostics
                .iter()
                .filter(|p| p.local_name != diag.local_name)
                .filter_map(|p| p.entity_renames.get(expected))
                .filter(|renamed| existing.contains(*renamed))
                .cloned()
                .collect();
            providers.sort();
            providers.dedup();
            match providers.len() {
                0 => {}
                1 => {
                    subs.insert(expected.clone(), providers.remove(0));
                }
                _ => {}
            }
        }
        plans.push((at, subs));
    }
    for (at, subs) in plans {
        let local_name = diagnostics[at].local_name.clone();
        if subs.is_empty() {
            continue;
        }
        let Some(orbital) = schema.orbitals.iter_mut().find(|o| o.name == local_name) else {
            continue;
        };
        for expect in orbital.expects.iter_mut().flatten() {
            if let orbital_core_base::schema::ExpectDeclaration::Entity { name, .. } = expect {
                if let Some(to) = subs.get(name.as_str()) {
                    *name = to.clone();
                }
            }
        }
        for td in orbital.traits.iter_mut() {
            if let TraitReference::Inline(td) = td {
                let td = Arc::make_mut(td);
                rename_entities_in_trait(td, &subs, &entity_ids);
                rename_source_entity_definition(td, &subs, &entity_ids);
            }
        }
        if let EntityRef::Inline(def) = &mut orbital.entity {
            rename_entity_refs_in_entity(def, &subs, &entity_ids);
        }
        for er in orbital.auxiliary_entities.iter_mut().flatten() {
            if let EntityRef::Inline(def) = er {
                rename_entity_refs_in_entity(def, &subs, &entity_ids);
            }
        }
        super::effect_row::recompute_effect_rows(orbital);
    }
}

fn rename_entities_in_trait(
    td: &mut TraitDefinition,
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
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
    let no_events: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    td.state_machine.edit(|machine| {
        for p in machine.propositions_mut() {
            rewrite_identifiers(p, entity_subs, &no_events);
        }
        for t in machine.transitions.iter_mut() {
            if let Some(guard) = t.guard.as_mut() {
                rewrite_identifiers(guard, entity_subs, &no_events);
            }
            if let Some(effects) = t.effects.as_mut() {
                for e in effects.iter_mut() {
                    rewrite_identifiers(e, entity_subs, &no_events);
                }
            }
        }
    });
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
    rewrite_entity_ref_ids(td, entity_subs, entity_ids);
    // G2 (`docs/Almadar_Compiler_Gaps.md` §82): a config knob declared
    // `: entity` (`std-calendar-sync`'s `targetEntity`) holds an entity-name
    // VALUE, not a token the identifier walk above reaches.
    rewrite_entity_typed_config_values(td, entity_subs, entity_ids);
}

/// Rewrite every config default in `cfg` through the field-rename map — the
/// two-rewriter pair (`rewrite_entity_field_paths` +
/// `rewrite_bare_field_name_config_default`) `rewrite_entity_fields_in_trait`
/// applies to a trait's OWN declared config. Factored out so the SAME pass
/// can ALSO run a second time, AFTER the per-trait loop's config-forward
/// steps resolve a knob's `@config.<parent>` reference into a literal value
/// (§6 of that loop, three call sites: embedder rung, multi-hop fallback,
/// no-embedder fallback) — those steps read from `parent_configs`, a
/// snapshot taken BEFORE this loop's own field rewrite ran (comment above
/// `parent_configs`'s declaration), so a forward can hand a trait a
/// brand-new literal that still names the PRE-rename field, invisible to
/// this trait's OWN earlier rewrite pass (G3, `docs/
/// Almadar_Compiler_Gaps.md` §82: a JSX-hoisted `InlineTableViewRender6`'s
/// `columns: @config.columns` forward pulled in `[{field: hourlyRate, key:
/// hourlyRate, …}]` from its embedder's declared default, verbatim,
/// AFTER `InlineTableViewRender6` itself had already been field-rewritten —
/// field renames). Idempotent — re-running it on already-correct config is
/// a no-op — so calling it twice in the loop is safe.
fn rewrite_entity_fields_in_config(
    cfg: &mut orbital_core_base::collections::IndexMap<String, ConfigField>,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if field_subs.is_empty() {
        return;
    }
    for f in cfg.values_mut() {
        if let Some(default) = f.default.as_mut() {
            rewrite_entity_field_paths(default, field_subs);
            // G3: the knob's OWN default value IS the field-name reference
            // for a `string`/`[string]`-shaped knob (`searchField`/
            // `formFields`-style) — see the function doc for why this
            // dispatches on VALUE SHAPE, not the (post-override-fold, often
            // `"unknown"`) declared type.
            rewrite_bare_field_name_config_default(default, field_subs);
        }
    }
}

/// Apply an `OrbitalRefObject.fields` rename (`{ up: local }`) to every
/// `@entity.<field>` token a trait carries: transition guards/effects, tick
/// guards/effects, listen guards/payloadMapping, and config-field defaults —
/// matches the JS twin's `rewriteEntityFieldsInTrait` scope, plus the
/// `entity_contract.requires`/`provides` rewrite `apply_overrides_to_trait`
/// already does for the ordinary trait-ref `fields {}` surface (kept for
/// parity with that call site; not present in the JS twin, which has no
/// `entity_contract` concept).
fn rewrite_entity_fields_in_trait(td: &mut TraitDefinition, field_subs: &orbital_core_base::collections::IndexMap<String, String>) {
    if field_subs.is_empty() {
        return;
    }
    for p in td.propositions_mut() {
        rewrite_entity_field_paths(p, field_subs);
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
        rewrite_entity_fields_in_config(cfg, field_subs);
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
fn rewrite_entity_fields_in_entity(
    entity: &mut EntityDefinition,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
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
    for index in entity.indexes.iter_mut().flatten() {
        for field in index.fields.iter_mut() {
            if let Some(to) = field_subs.get(field) {
                *field = to.clone();
            }
        }
        if let Some(order) = index.order_by.as_mut() {
            let (name, direction) = match order.split_once(':') {
                Some((name, direction)) => (name, Some(direction)),
                None => (order.as_str(), None),
            };
            if let Some(to) = field_subs.get(name) {
                *order = match direction {
                    Some(direction) => format!("{to}:{direction}"),
                    None => to.clone(),
                };
            }
        }
    }
}

/// Stage B `roles {}`: rewrite role literals across an entity's four access
/// policies — the same four `rewrite_entity_fields_in_entity` covers, just
/// for role-literal comparisons instead of `@entity.<field>` reads. Applied
/// to the primary entity, every auxiliary entity, and (via
/// `rewrite_role_literals_in_trait`) a trait's `source_entity_definition`
/// copy.
fn rewrite_role_literals_in_entity(
    entity: &mut EntityDefinition,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) {
    if roles.is_empty() {
        return;
    }
    if let Some(p) = entity.read_policy.as_mut() {
        rewrite_role_literals(p, role_fields, roles);
    }
    if let Some(p) = entity.create_policy.as_mut() {
        rewrite_role_literals(p, role_fields, roles);
    }
    if let Some(p) = entity.update_policy.as_mut() {
        rewrite_role_literals(p, role_fields, roles);
    }
    if let Some(p) = entity.delete_policy.as_mut() {
        rewrite_role_literals(p, role_fields, roles);
    }
}

/// Stage B `roles {}`: rewrite role literals across a trait's whole
/// guard/effect surface — transition + tick guards/effects, listen
/// guard/payloadMapping, config-field defaults, and (recursively)
/// `source_entity_definition`'s own four access policies. Same scope
/// `rewrite_entity_fields_in_trait` covers for the `fields {}` surface.
fn rewrite_role_literals_in_trait(
    td: &mut TraitDefinition,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) {
    if roles.is_empty() {
        return;
    }
    for p in td.propositions_mut() {
        rewrite_role_literals(p, role_fields, roles);
    }
    for t in td.state_machine.transitions.iter_mut() {
        if let Some(guard) = t.guard.as_mut() {
            rewrite_role_literals(guard, role_fields, roles);
        }
        if let Some(effects) = t.effects.as_mut() {
            for e in effects.iter_mut() {
                rewrite_role_literals(e, role_fields, roles);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_role_literals(guard, role_fields, roles);
            }
            for e in tick.effects.iter_mut() {
                rewrite_role_literals(e, role_fields, roles);
            }
        }
    }
    if let Some(listens) = td.listens.as_mut() {
        for l in listens.iter_mut() {
            if let Some(guard) = l.guard.as_mut() {
                rewrite_role_literals(guard, role_fields, roles);
            }
            if let Some(pm) = l.payload_mapping.as_mut() {
                for v in pm.values_mut() {
                    rewrite_role_literals(v, role_fields, roles);
                }
            }
        }
    }
    if let Some(cfg) = td.config.as_mut() {
        for f in cfg.values_mut() {
            if let Some(default) = f.default.as_mut() {
                rewrite_role_literals(default, role_fields, roles);
            }
        }
    }
    if let Some(src) = td.source_entity_definition.as_mut() {
        rewrite_role_literals_in_entity(Arc::make_mut(src), role_fields, roles);
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
/// id-integrity dual-carry check (the std `id-refs` lint) reads `entityId` against
/// the ORIGINAL schema's ledger, and a stale upstream id there resolves to
/// upstream's un-prefixed `curName`, tripping `ORB_ID_NAME_MISMATCH` against
/// the just-renamed `.entity`. A target missing from `entity_ids` (should not
/// happen within one materialized closure) drops the id sibling rather than
/// leaving it dangling — `entityId` is optional (pre-Phase-7).
fn rename_relation_targets_in_fields(
    fields: &mut [FieldDefinition],
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
) {
    for f in fields.iter_mut() {
        if let Some(rel) = f.relation.as_mut() {
            if let Some(to) = entity_subs.get(&rel.entity) {
                rel.entity = to.clone();
                rel.entity_id = entity_ids.get(to).cloned();
            }
        }
        if let Some(projection) = f.projected_from.as_mut() {
            if let Some(to) = entity_subs.get(&projection.type_name) {
                projection.type_name = to.clone();
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

/// Every name of `candidates` a trait reads: its linked entity, the entity its
/// source definition carries (with that definition's relation targets), and each
/// entity slot of its guards, effects, ticks, listens and config defaults.
/// Sorted, so callers see one order.
fn entity_names_read_by_trait(td: &TraitDefinition, candidates: &HashSet<String>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut sink: Vec<(String, String)> = Vec::new();
    if let Some(src) = &td.source_entity_definition {
        record_referenced("", &src.name, candidates, &mut sink);
        collect_relation_targets_referenced(&src.fields, "", candidates, &mut sink);
    }
    if let Some(le) = &td.linked_entity {
        record_referenced("", le, candidates, &mut sink);
    }
    names.extend(sink.into_iter().map(|(_, name)| name));
    let mut slots: HashSet<String> = HashSet::new();
    for t in &td.state_machine.transitions {
        if let Some(g) = t.guard.as_ref() {
            collect_entity_slots(g, candidates, &mut slots);
        }
        for e in t.effects.iter().flatten() {
            collect_entity_slots(e, candidates, &mut slots);
        }
    }
    for tick in td.ticks.iter().flatten() {
        if let Some(g) = tick.guard.as_ref() {
            collect_entity_slots(g, candidates, &mut slots);
        }
        for e in &tick.effects {
            collect_entity_slots(e, candidates, &mut slots);
        }
    }
    for l in td.listens.iter().flatten() {
        if let Some(g) = l.guard.as_ref() {
            collect_entity_slots(g, candidates, &mut slots);
        }
        for v in l.payload_mapping.iter().flat_map(|pm| pm.values()) {
            collect_entity_slots(v, candidates, &mut slots);
        }
    }
    for f in td.config.iter().flat_map(|cfg| cfg.values()) {
        if let Some(d) = f.default.as_ref() {
            collect_entity_slots(d, candidates, &mut slots);
        }
    }
    names.extend(slots);
    names.sort();
    names.dedup();
    names
}

/// Stage B `entities {}`: record `(site, name)` when `name` is a member of
/// `upstream_universe` — the out-of-orbital candidate set — deduping
/// identical pairs. One tiny shared guard so every `referenced`-collection
/// call site in `resolve_orbital_reference` applies the same "is this worth
/// recording" rule.
fn record_referenced(
    site: &str,
    name: &str,
    upstream_universe: &HashSet<String>,
    referenced: &mut Vec<(String, String)>,
) {
    if upstream_universe.contains(name) && !referenced.iter().any(|(s, n)| s == site && n == name) {
        referenced.push((site.to_string(), name.to_string()));
    }
}

/// Read-only twin of `rename_relation_targets_in_fields`, scoped to Stage B's
/// `entities {}` diagnostic: record every relation target reachable from
/// `fields` (recursing through `items`/`properties`, the same positions the
/// rename walker covers) that names a member of `upstream_universe`.
fn collect_relation_targets_referenced(
    fields: &[FieldDefinition],
    site: &str,
    upstream_universe: &HashSet<String>,
    referenced: &mut Vec<(String, String)>,
) {
    for f in fields {
        if let Some(rel) = f.relation.as_ref() {
            record_referenced(site, &rel.entity, upstream_universe, referenced);
        }
        if let Some(items) = f.items.as_ref() {
            collect_relation_targets_referenced(
                std::slice::from_ref(items.as_ref()),
                site,
                upstream_universe,
                referenced,
            );
        }
        if let Some(props) = f.properties.as_ref() {
            for v in props.values() {
                collect_relation_targets_referenced(
                    std::slice::from_ref(v),
                    site,
                    upstream_universe,
                    referenced,
                );
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
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
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
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
) {
    let Some(src) = td.source_entity_definition.as_mut() else {
        return;
    };
    let src = Arc::make_mut(src);
    if let Some(to) = entity_subs.get(&src.name) {
        src.name = to.clone();
    }
    rename_entity_refs_in_entity(src, entity_subs, entity_ids);
}

/// Point a materialized trait's `listens[].source` at its renamed name.
/// `Trait` sources are looked up in the combined `subs` map (same orbital).
/// `Orbital`-kind sources (`Orbital.Trait.EVENT`) name a DIFFERENT orbital,
/// two ways: (1) the SAME upstream orbital currently being spliced
/// (`own_upstream_name` → `own_local_name`) — resolved directly through THIS
/// call's own `subs`/`id_by_name`, exactly as before; (2) ANOTHER
/// reference-form orbital of the same upstream `alias` also imported
/// elsewhere in this schema (G1, `docs/Almadar_Compiler_Gaps.md` §82,
/// order-independent) — resolved through the schema-wide
/// `listen_source_targets` precompute. (1) is checked FIRST and is
/// authoritative for `own_upstream_name`: two imports of the SAME upstream
/// orbital under different local names each own their SELF case this way,
/// so neither can clobber the other in the shared map (which, keyed only by
/// (alias, upstream name), cannot hold two different local names for one
/// key at once). A source naming an upstream orbital that is imported
/// NOWHERE in this schema matches neither and is left exactly as-is, so
/// `ORB_X_LISTEN_SOURCE_UNRESOLVED` still fires correctly for it (rung 2).
fn rewrite_listen_sources(
    td: &mut TraitDefinition,
    subs: &orbital_core_base::collections::IndexMap<String, String>,
    id_by_name: &orbital_core_base::collections::IndexMap<String, String>,
    own_upstream_name: &str,
    own_local_name: &str,
    alias: &str,
    listen_source_targets: &HashMap<(String, String), SiblingImportInfo>,
) {
    let rewrite = |source: &mut ListenSource| {
        rewrite_import_source(
            source,
            subs,
            id_by_name,
            own_upstream_name,
            own_local_name,
            alias,
            listen_source_targets,
        )
    };
    if let Some(listens) = td.listens.as_mut() {
        listens.edit(|listens| {
            for listen in listens.iter_mut() {
                if let Some(source) = listen.source.as_mut() {
                    rewrite(source);
                }
            }
        });
    }
    // A `listens`-addressed event value in config is the same address.
    let mut move_address = |event: &str, source: &ListenSource| {
        let mut moved = source.clone();
        rewrite(&mut moved);
        (moved != *source).then(|| (event.to_string(), moved))
    };
    super::rewrite::map_config_event_addresses(td, &mut move_address);
    super::rewrite::map_effect_event_addresses(td, &mut move_address);
}

/// The one rewrite of a `listens` address (a listen source, or the source part
/// of an event-typed config value) when its trait is materialized by import.
fn rewrite_import_source(
    source: &mut ListenSource,
    subs: &orbital_core_base::collections::IndexMap<String, String>,
    id_by_name: &orbital_core_base::collections::IndexMap<String, String>,
    own_upstream_name: &str,
    own_local_name: &str,
    alias: &str,
    listen_source_targets: &HashMap<(String, String), SiblingImportInfo>,
) {
    match source {
        ListenSource::Trait {
            trait_name,
            trait_id,
        } => {
            if let Some(target) = subs.get(trait_name.as_str()) {
                if target != trait_name {
                    *trait_id = id_by_name.get(target).cloned();
                    *trait_name = target.clone();
                }
            }
        }
        ListenSource::Orbital {
            orbital,
            trait_name,
            trait_id,
            orbital_id,
        } => {
            if orbital.as_str() == own_upstream_name {
                let target = subs
                    .get(trait_name.as_str())
                    .cloned()
                    .unwrap_or_else(|| trait_name.clone());
                *trait_id = id_by_name.get(&target).cloned();
                *trait_name = target;
                *orbital = own_local_name.to_string();
                // The old value named the UPSTREAM orbital; there is no
                // reliable local-orbital id to carry here, so clear the
                // dual-carry sibling (matches the JS twin, which never
                // emits `orbitalId` on this rewrite).
                *orbital_id = None;
            } else if let Some(info) =
                listen_source_targets.get(&(alias.to_string(), orbital.clone()))
            {
                match info.trait_subs.get(trait_name.as_str()) {
                    Some((final_trait, final_id)) => {
                        *trait_name = final_trait.clone();
                        *trait_id = Some(final_id.clone());
                    }
                    // The target import exists but never kept a trait by
                    // this name (e.g. `omit {}`'d) — no reliable id to
                    // carry, so clear it, same as the self case above on
                    // an unresolved trait name.
                    None => *trait_id = None,
                }
                *orbital = info.local_name.clone();
                *orbital_id = None;
            }
        }
        ListenSource::Any => {}
    }
}

/// Apply an `OrbitalRefObject.events` rename (`{ UP: LOCAL }`) to every
/// event-bearing position a trait carries: `(emit …)` effect forms and
/// event-valued render-ui props (via `rewrite_identifiers`'s `event_subs`
/// arm), the structured event-key fields (`stateMachine.events[*].key`,
/// `transitions[*].event`, `emits[*].event`, `listens[*].event/triggers` —
/// via `rewrite_trait_event_fields`), a declared config knob typed `event`
/// whose literal default names a renamed event (the JS twin's
/// `renameEventsInDeclaredConfig` main branch — its struct-member/array form
/// is not replicated here, a documented gap since no W3-R test exercises it),
/// and (G3, `docs/Almadar_Compiler_Gaps.md` §82) a SOURCED listen's `event`
/// field (the upstream key a cross-trait/cross-orbital `listens` reads,
/// distinct from `triggers`, the LOCAL key it fires) when the source trait
/// is itself one of THIS import's kept traits — `event_subs` is scoped to
/// one orbital reference, so within it every trait shares the SAME renamed
/// vocabulary, unlike the ordinary (non-import) trait-ref path's
/// `rewrite_trait_event_fields`, which deliberately leaves a sourced
/// listen's `event` alone because there the source trait's rename is an
/// INDEPENDENT call site's own `events {}` (reconciled instead via
/// `event_renames_by_trait` in `inline_traits`). `kept_final_names` is
/// `subs`'s value set (every kept trait's post-prefix name) — a listen
/// sourced from a SIBLING trait (Stage B, never prefixed, never subject to
/// this rename) is correctly excluded since its name can never collide with
/// a `{local_name}{upstream_name}` value.
fn rename_events_in_trait(
    td: &mut TraitDefinition,
    event_subs: &orbital_core_base::collections::IndexMap<String, String>,
    subs: &orbital_core_base::collections::IndexMap<String, String>,
    event_scope: &EventScopeRenames,
) -> Result<(), InlineError> {
    if event_subs.is_empty() {
        return Ok(());
    }
    {
        let kept_final_names: HashSet<&str> = subs.values().map(String::as_str).collect();
        super::rewrite::map_config_event_addresses(td, &mut |event, source| {
            let trait_name = match source {
                ListenSource::Trait { trait_name, .. }
                | ListenSource::Orbital { trait_name, .. } => trait_name,
                ListenSource::Any => return None,
            };
            if !kept_final_names.contains(trait_name.as_str()) {
                return None;
            }
            event_subs.get(event).map(|to| (to.clone(), source.clone()))
        });
    }
    let no_entities: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for p in td.propositions_mut() {
        rewrite_identifiers(p, &no_entities, event_subs);
    }
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
    let kept_final_names: HashSet<&str> = subs.values().map(String::as_str).collect();
    rewrite_trait_event_fields(td, event_scope, Some(&kept_final_names))?;
    if let Some(cfg) = td.config.as_mut() {
        for f in cfg.values_mut() {
            if f.field_type == "event" {
                if let Some(SExpression::String(s)) = f.default.as_ref() {
                    if let Some(to) = event_subs.get(s.as_str()) {
                        f.default = Some(SExpression::string(to.clone()));
                    }
                }
            }
            // (f): the STRUCT-MEMBER twin of the scalar fold above — a knob
            // declared `[RecordAction]` whose element schema types a member
            // `event` holds event-name REFERENCES too. Exact port of
            // `trait.rs:404-445`'s member-fold, reused (not copied) via the
            // shared `fold_event_typed_members`.
            if f.items.is_some() {
                if let Some(default) = f.default.as_mut() {
                    fold_event_typed_members(default, f.items.as_deref(), event_subs);
                }
            }
        }
    }
    Ok(())
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
fn rewrite_concat_prefix(
    literal_prefix: &str,
    path_map: &orbital_core_base::collections::IndexMap<String, String>,
) -> Option<String> {
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
fn rewrite_navigate_targets(expr: &mut SExpression, path_map: &orbital_core_base::collections::IndexMap<String, String>) {
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
            if let Some(to) = path_map.get(target.as_str()) {
                items[1] = SExpression::string(to.clone());
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
                    concat[1] = SExpression::string(rewritten);
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
fn rewrite_trait_navigate_targets(td: &mut TraitDefinition, path_map: &orbital_core_base::collections::IndexMap<String, String>) {
    if path_map.is_empty() {
        return;
    }
    td.state_machine.edit(|machine| {
        for t in machine.transitions.iter_mut() {
            if let Some(effects) = t.effects.as_mut() {
                for e in effects.iter_mut() {
                    rewrite_navigate_targets(e, path_map);
                }
            }
        }
    });
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            for e in tick.effects.iter_mut() {
                rewrite_navigate_targets(e, path_map);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{OrbitalSchema, UseDeclaration};
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
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN", "id": "evt_UPSTREAMOPEN00000000000000" } ],
                        "transitions": [
                            {
                                "from": "idle", "to": "idle", "event": "OPEN", "eventId": "evt_UPSTREAMOPEN00000000000000",
                                "effects": [
                                    ["render-ui", "main", { "type": "stack", "children": ["@trait.B"] }]
                                ]
                            }
                        ]
                    },
                    "listens": [
                        { "event": "PING", "eventId": "evt_UPSTREAMPING00000000000000", "triggers": "OPEN", "triggersId": "evt_UPSTREAMOPEN00000000000000", "source": { "kind": "trait", "trait": "B" } }
                    ]
                },
                {
                    "name": "B",
                    "linkedEntity": "Widget",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "PING", "id": "evt_UPSTREAMPING00000000000000" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "PING", "eventId": "evt_UPSTREAMPING00000000000000" } ]
                    },
                    "emits": [ { "event": "PING", "eventId": "evt_UPSTREAMPING00000000000000" } ]
                },
                {
                    "name": "C",
                    "linkedEntity": "Widget",
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let names = trait_names(&orbital);
        assert_eq!(names.len(), 3, "{names:?}");
        for expected in ["LocalA", "LocalB", "LocalC"] {
            assert!(names.contains(&expected.to_string()), "{names:?}");
        }
    }

    #[test]
    fn an_import_that_keeps_the_upstream_name_reproduces_it_unprefixed() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("UpOrbital");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let mut names = trait_names(&orbital);
        names.sort();
        assert_eq!(names, vec!["A", "B", "C"], "the same-name import keeps every trait name");
        let a = kept_trait(&orbital, "A");
        match a.listens.as_ref().expect("listens carried")[0].source.as_ref().expect("source") {
            ListenSource::Trait { trait_name, .. } => assert_eq!(trait_name, "B"),
            other => panic!("expected ListenSource::Trait, got {other:?}"),
        }
        let EntityRef::Inline(entity) = &orbital.entity else { panic!("the primary entity is inlined") };
        assert_eq!(entity.name, "Widget", "the same-name import keeps the entity name");
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        let listens = a.listens.as_ref().expect("listens carried");
        match listens[0].source.as_ref().expect("source") {
            ListenSource::Trait {
                trait_name,
                trait_id,
            } => {
                assert_eq!(trait_name, "LocalB");
                assert!(
                    trait_id.is_some(),
                    "renamed source must carry the target's derived id"
                );
            }
            other => panic!("expected ListenSource::Trait, got {other:?}"),
        }
    }

    fn resolve_with_trait_modes(
        modes: &[(&str, orbital_core_base::schema::DispatchMode)],
    ) -> (Orbital, Vec<OrbitalRefDiagnostic>) {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(td) = &mut upstream.traits[2] {
            let td = Arc::make_mut(td);
            td.dispatch = Some(orbital_core_base::schema::DispatchMode::Client);
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            traits: Some(
                modes
                    .iter()
                    .map(|(name, mode)| {
                        (
                            name.to_string(),
                            orbital_core_base::schema::OrbitalRefTraitOverride {
                                dispatch: Some(*mode),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        (orbital, diagnostics)
    }

    #[test]
    fn an_import_traits_override_sets_the_named_traits_dispatch_only() {
        use orbital_core_base::schema::DispatchMode;
        let (orbital, _) = resolve_with_trait_modes(&[
            ("A", DispatchMode::Optimistic),
            ("C", DispatchMode::Server),
        ]);
        assert_eq!(kept_trait(&orbital, "LocalA").dispatch, Some(DispatchMode::Optimistic));
        assert_eq!(kept_trait(&orbital, "LocalB").dispatch, None, "an unnamed trait is untouched");
        assert_eq!(
            kept_trait(&orbital, "LocalC").dispatch,
            Some(DispatchMode::Server),
            "the override beats the atom's declared [client]"
        );
    }

    #[test]
    fn an_import_traits_override_without_a_mode_keeps_the_atoms_declared_mode() {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(td) = &mut upstream.traits[2] {
            let td = Arc::make_mut(td);
            td.dispatch = Some(orbital_core_base::schema::DispatchMode::Client);
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            traits: Some(orbital_core_base::collections::IndexMap::from([(
                "C".to_string(),
                orbital_core_base::schema::OrbitalRefTraitOverride::default(),
            )])),
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        assert_eq!(
            kept_trait(&orbital, "LocalC").dispatch,
            Some(orbital_core_base::schema::DispatchMode::Client)
        );
    }

    #[test]
    fn omit_of_a_trait_a_survivor_still_embeds_leaves_the_embed_unrewritten() {
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let names = trait_names(&orbital);
        assert!(!names.contains(&"LocalB".to_string()), "{names:?}");
        assert_eq!(diagnostics.len(), 1);

        // The dangling `@trait.B` embed is left AS-IS (no rename target) for
        // the std lint to flag via ORB_O_OMIT_EMBEDDED_TRAIT — never rewritten
        // to point at a trait that no longer exists in this orbital.
        let a = kept_trait(&orbital, "LocalA");
        let blob = serde_json::to_string(&a.state_machine.transitions).unwrap();
        assert!(blob.contains("@trait.B"), "{blob}");
    }

    #[test]
    fn unknown_config_override_key_is_dropped_from_the_folded_config() {
        // Trait A forwards the orbital's `pageSize` knob — without a
        // forwarder, the dead-knob drop (planning find iii) would strip
        // `pageSize` from the folded config entirely, since nothing in
        // `kept` still names it.
        let mut upstream = upstream_orbital(Some(json!({
            "pageSize": { "type": "number", "default": 10 }
        })));
        if let TraitReference::Inline(a) = &mut upstream.traits[0] {
            let a = Arc::make_mut(a);
            a.config = Some(orbital_core_base::collections::IndexMap::from([(
                "limit".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "number", "default": "@config.pageSize"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);
        let mut orbital = consumer_orbital("Local");
        let mut overrides = orbital_core_base::collections::IndexMap::new();
        overrides.insert(
            "bogusKnob".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "number", "default": 99 }))
                .unwrap(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        assert_eq!(diagnostics.len(), 1);
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
        // outermost (schema) rung must still resolve it, from the ALIAS's
        // (upstream FILE's) own app-level config — never the consumer's own
        // `schema.config` (the (A) bug this fixture pins: `resolve_orbital_
        // reference` used to receive the consumer's OWN `schema_config`
        // verbatim, so an import's `@config.<appKnob>` forward against the
        // UPSTREAM's app config never resolved).
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(td) = &mut upstream.traits[0] {
            let td = Arc::make_mut(td);
            td.config = Some(orbital_core_base::collections::IndexMap::from([(
                "limit".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "number", "default": "@config.pageSize"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut upstream_schema_config = orbital_core_base::collections::IndexMap::new();
        upstream_schema_config.insert(
            "pageSize".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "number", "default": 42 }))
                .unwrap(),
        );
        let alias_schema_configs: orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>> =
            orbital_core_base::collections::IndexMap::from([("Up".to_string(), upstream_schema_config)]);

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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &alias_schema_configs,
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        assert_eq!(
            a.config
                .as_ref()
                .and_then(|c| c.get("limit"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::Number(42.0)),
            "with no orbital-level config, the ALIAS's schema rung must still resolve the forward"
        );
    }

    /// (A) regression: the CONSUMER's own top-level `schema.config` must
    /// NEVER be consulted as the schema rung for an orbital import — only
    /// the ALIAS's (upstream file's) own app-level config, keyed by the
    /// alias the reference names. A consumer-scope entry for an UNRELATED
    /// alias key (or none at all) must leave the forward unresolved (still
    /// the raw `@config.<knob>` string) rather than silently borrow it.
    #[test]
    fn schema_rung_never_falls_back_to_the_consumers_own_schema_config() {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(td) = &mut upstream.traits[0] {
            let td = Arc::make_mut(td);
            td.config = Some(orbital_core_base::collections::IndexMap::from([(
                "limit".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "number", "default": "@config.pageSize"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        // A schema config IS present, but keyed under a DIFFERENT alias
        // ("SomeOtherAlias") — simulating the consumer's own unrelated
        // `schema.config` reaching this call under the old (buggy) wiring.
        let mut other_alias_config = orbital_core_base::collections::IndexMap::new();
        other_alias_config.insert(
            "pageSize".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "number", "default": 999 }))
                .unwrap(),
        );
        let alias_schema_configs: orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>> =
            orbital_core_base::collections::IndexMap::from([("SomeOtherAlias".to_string(), other_alias_config)]);

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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &alias_schema_configs,
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        assert_eq!(
            a.config
                .as_ref()
                .and_then(|c| c.get("limit"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("@config.pageSize".to_string()).into())),
            "an unrelated alias's schema config must never resolve THIS reference's forward"
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };

        let mut first = consumer_orbital("Local");
        resolve_orbital_reference(
            &mut first,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let mut second = consumer_orbital("Local");
        resolve_orbital_reference(
            &mut second,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        assert_eq!(
            kept_trait(&first, "LocalA").id,
            kept_trait(&second, "LocalA").id
        );
        let EntityRef::Inline(e1) = &first.entity else {
            panic!("inline")
        };
        let EntityRef::Inline(e2) = &second.entity else {
            panic!("inline")
        };
        assert_eq!(e1.id, e2.id);
    }

    /// `derive_materialized_id`'s seed fallback (no real source id — `Widget`
    /// in `upstream_orbital` declares none) must carry the CALLER's kind, not
    /// always fall back to `derive_id`'s own default (`trait`) because the
    /// unprefixed seed string classifies as no known kind. A materialized
    /// PRIMARY ENTITY must get an `ent_`-prefixed id, and a materialized
    /// TRAIT must get a `trt_`-prefixed id, from the exact same seed shape.
    #[test]
    fn seed_derived_ids_carry_the_callers_kind() {
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut orbital = consumer_orbital("Local");
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("primary must be inline")
        };
        let entity_id = entity.id.as_deref().expect("entity must have a derived id");
        assert!(
            entity_id.starts_with("ent_"),
            "seed-derived primary entity id must carry the entity prefix, got {entity_id}"
        );

        let trait_id = kept_trait(&orbital, "LocalA")
            .id
            .as_deref()
            .expect("trait must have a derived id");
        assert!(
            trait_id.starts_with("trt_"),
            "seed-derived trait id must carry the trait prefix, got {trait_id}"
        );
    }

    /// `MainOrbital` (being imported) fetches `PanelData`, which is declared
    /// only as an AUXILIARY entity of a DIFFERENT orbital of the same alias
    /// (`SiblingOrbital`) — never anyone's primary. `persistence: persistent`
    /// on purpose: persistence is NOT the discriminator, aux-vs-primary is.
    fn aux_only_main_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "MainOrbital",
            "id": "orb_AUXONLYMAINORBITAL0000000",
            "entity": {
                "name": "Widget",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": [
                {
                    "name": "PanelFetcher",
                    "linkedEntity": "Widget",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "LOAD" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD",
                            "effects": [ ["fetch", "PanelData", {}] ] } ]
                    }
                }
            ]
        }))
        .expect("aux-only main orbital fixture must deserialize")
    }

    fn aux_only_sibling_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "SiblingOrbital",
            "entity": {
                "name": "SiblingWidget",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "auxiliaryEntities": [
                {
                    "name": "PanelData",
                    "persistence": "persistent",
                    "fields": [ { "name": "id", "type": "string", "required": true } ]
                }
            ],
            "traits": []
        }))
        .expect("aux-only sibling orbital fixture must deserialize")
    }

    /// Same shape, but the out-of-orbital entity `Employee` is declared as
    /// the PRIMARY of `EmployeeOrbital` — author-declared domain data, still
    /// `persistence: persistent` (matches `aux_only_*`, isolating persistence
    /// as NOT the discriminator).
    fn primary_referenced_main_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "MainOrbital",
            "id": "orb_PRIMARYREFMAINORBITAL000",
            "entity": {
                "name": "Widget",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": [
                {
                    "name": "EmployeeFetcher",
                    "linkedEntity": "Widget",
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "LOAD" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD",
                            "effects": [ ["fetch", "Employee", {}] ] } ]
                    }
                }
            ]
        }))
        .expect("primary-referenced main orbital fixture must deserialize")
    }

    fn employee_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "EmployeeOrbital",
            "entity": {
                "name": "Employee",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("employee orbital fixture must deserialize")
    }

    #[test]
    fn aux_only_out_of_orbital_entity_auto_clones_even_when_persistent() {
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Up",
            vec![aux_only_main_orbital(), aux_only_sibling_orbital()],
            "aux-only-test".to_string(),
        );
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let aux = orbital
            .auxiliary_entities
            .as_ref()
            .expect("auxiliary entities carried");
        let cloned = aux.iter().find_map(|e| match e {
            EntityRef::Inline(def) if def.name == "LocalPanelData" => Some(def),
            _ => None,
        });
        assert!(
            cloned.is_some(),
            "PanelData must be auto-cloned as this import's own LocalPanelData aux; got {:?}",
            aux.iter()
                .filter_map(|e| match e {
                    EntityRef::Inline(def) => Some(def.name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
    }

    /// Pins the defect the coordinator found on `orbital_import_disjoint.lolo`:
    /// a kept trait's `entityRefIds` side-map (stamped upstream, key =
    /// "PanelData" naming the aux-only out-of-orbital entity that
    /// auto-clones as `LocalPanelData`, value = the UPSTREAM entity's id)
    /// must be rewritten to key the CONSUMER name and carry the CLONE's own
    /// freshly-derived id — never left naming an entity/id that doesn't
    /// exist in the materialized schema. Uses the disjoint shape (an
    /// upstream aux entity referenced by a non-`linkedEntity`,
    /// positional-argument token) per the plan's test instruction.
    #[test]
    fn orbital_import_rewrites_entity_ref_ids_for_cloned_aux_entity() {
        let mut main = aux_only_main_orbital();
        // Stamp the trait's side-map the way `orbital_core::stamp` would:
        // key = the body token's name at stamp time, value = that entity's
        // id in the UPSTREAM stamp's own id space (never valid in any other
        // schema).
        if let TraitReference::Inline(td) = &mut main.traits[0] {
            let td = Arc::make_mut(td);
            td.entity_ref_ids = Some(orbital_core_base::collections::IndexMap::from([(
                "PanelData".to_string(),
                "ent_UPSTREAMPANELDATA00000000".to_string(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Up",
            vec![main, aux_only_sibling_orbital()],
            "aux-only-entity-ref-ids-test".to_string(),
        );
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let cloned_id = orbital
            .auxiliary_entities
            .as_ref()
            .expect("auxiliary entities carried")
            .iter()
            .find_map(|e| match e {
                EntityRef::Inline(def) if def.name == "LocalPanelData" => def.id.clone(),
                _ => None,
            })
            .expect("LocalPanelData clone carries its own derived id");

        let fetcher = kept_trait(&orbital, "LocalPanelFetcher");
        let refs = fetcher
            .entity_ref_ids
            .as_ref()
            .expect("entityRefIds side-map survives materialization");
        assert_eq!(
            refs.get("LocalPanelData"),
            Some(&cloned_id),
            "entityRefIds must key the CONSUMER clone's name and carry its CURRENT id, \
             not the upstream's: {refs:?}"
        );
        assert!(
            !refs.contains_key("PanelData"),
            "the stale upstream key must not survive: {refs:?}"
        );
        assert!(
            !refs.values().any(|v| v == "ent_UPSTREAMPANELDATA00000000"),
            "the stale upstream id must not survive: {refs:?}"
        );
    }

    #[test]
    fn primary_out_of_orbital_entity_is_not_auto_cloned_even_when_persistent() {
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Up",
            vec![primary_referenced_main_orbital(), employee_orbital()],
            "primary-ref-test".to_string(),
        );
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        assert_eq!(diagnostics.len(), 1);
        assert!(
            orbital
                .auxiliary_entities
                .iter()
                .flatten()
                .all(|e| !matches!(e, EntityRef::Inline(def) if def.name.contains("Employee"))),
            "a primary out-of-orbital entity must NOT be auto-cloned"
        );
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };

        let mut one = consumer_orbital("One");
        resolve_orbital_reference(
            &mut one,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let mut two = consumer_orbital("Two");
        resolve_orbital_reference(
            &mut two,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        assert_ne!(trait_names(&one), trait_names(&two));
        assert_ne!(kept_trait(&one, "OneA").id, kept_trait(&two, "TwoA").id);
        let EntityRef::Inline(e1) = &one.entity else {
            panic!("inline")
        };
        let EntityRef::Inline(e2) = &two.entity else {
            panic!("inline")
        };
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
            let a = Arc::make_mut(a);
            a.config = Some(orbital_core_base::collections::IndexMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "VIEW"
                }))
                .unwrap(),
            )]));
        }
        if let TraitReference::Inline(b) = &mut upstream.traits[1] {
            let b = Arc::make_mut(b);
            b.config = Some(orbital_core_base::collections::IndexMap::from([(
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let b = kept_trait(&orbital, "LocalB");
        assert_eq!(
            b.config
                .as_ref()
                .and_then(|c| c.get("itemClickEvent"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("VIEW".to_string()).into())),
            "the pulled sibling's forward must resolve against its EMBEDDER's declared \
             config (A), not fall straight through to the orbital/schema rungs"
        );
    }

    /// Splice-side twin of `config_override_round_trip.rs`'s
    /// `sibling_forward_resolves_through_three_deep_hoisted_chain` (pull-site
    /// coverage): the orbital-import splice must ALSO walk a multi-hop
    /// embedder chain when a knob's IMMEDIATE embedder doesn't declare it.
    /// `A` embeds `@trait.B` embeds `@trait.C`; only `A` declares
    /// `itemClickEvent`; `B` declares nothing relevant; `C` forwards it.
    #[test]
    fn embedder_forwards_config_through_hoisted_splice_chain() {
        let upstream: Orbital = serde_json::from_value(json!({
            "name": "HoistedSpliceOrbital",
            "id": "orb_HOISTEDSPLICEORBITAL0000",
            "entity": {
                "name": "Widget",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": [
                {
                    "name": "A", "linkedEntity": "Widget", 
                    "config": { "itemClickEvent": { "type": "string", "default": "VIEW" } },
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
                            "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.B"] }] ] } ]
                    }
                },
                {
                    "name": "B", "linkedEntity": "Widget", 
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN",
                            "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.C"] }] ] } ]
                    }
                },
                {
                    "name": "C", "linkedEntity": "Widget", 
                    "config": { "itemClickEvent": { "type": "string", "default": "@config.itemClickEvent" } },
                    "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
                }
            ]
        }))
        .expect("fixture deserializes");
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.HoistedSpliceOrbital".to_string(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let c = kept_trait(&orbital, "LocalC");
        assert_eq!(
            c.config
                .as_ref()
                .and_then(|m| m.get("itemClickEvent"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("VIEW".to_string()).into())),
            "C's forward must resolve through B (which declares nothing) out to \
             A's declared value — the direct-embedder loop alone only sees B"
        );
    }

    /// `embedder_chain_via_first_hop`'s `visited` guard must terminate on a
    /// malformed cyclic `embedders_of` (defensive — the composition graph is
    /// acyclic by construction) rather than loop forever.
    #[test]
    fn embedder_chain_via_first_hop_terminates_on_cycle() {
        let mut embedders_of: orbital_core_base::collections::IndexMap<String, Vec<String>> = orbital_core_base::collections::IndexMap::new();
        embedders_of.insert("A".to_string(), vec!["B".to_string()]);
        embedders_of.insert("B".to_string(), vec!["A".to_string()]);
        let mut parent_configs: orbital_core_base::collections::IndexMap<String, orbital_core_base::collections::IndexMap<String, ConfigField>> = orbital_core_base::collections::IndexMap::new();
        parent_configs.insert("A".to_string(), orbital_core_base::collections::IndexMap::new());
        parent_configs.insert("B".to_string(), orbital_core_base::collections::IndexMap::new());

        let chain = embedder_chain_via_first_hop("A", &embedders_of, &parent_configs, 2);
        assert_eq!(
            chain.len(),
            2,
            "A <-> B must each be visited exactly once, then stop: {chain:?}"
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
            let a = Arc::make_mut(a);
            a.config = Some(orbital_core_base::collections::IndexMap::from([(
                "itemClickEvent".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "string", "default": "VIEW"
                }))
                .unwrap(),
            )]));
        }
        if let TraitReference::Inline(b) = &mut upstream.traits[1] {
            let b = Arc::make_mut(b);
            b.config = Some(orbital_core_base::collections::IndexMap::from([(
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let b = kept_trait(&orbital, "LocalB");
        assert_eq!(
            b.config
                .as_ref()
                .and_then(|c| c.get("itemClickEvent"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("VIEW".to_string()).into())),
            "the embedder rung must win over the orbital rung, same precedence as the pull site"
        );
    }

    // ------------------------------------------------------------------
    // Stage B: roles/entities/mounts/siblings/ordering-fix tests.
    // ------------------------------------------------------------------

    /// `MainOrbital` (primary `Widget`, aux identity `ThingOwner` with a
    /// `role : "employee"|"approver"` vocabulary): `Widget.otherId` relates
    /// to `Other` — an entity declared by the SECOND upstream orbital under
    /// the same alias (`OtherOrbital`), the "out-of-orbital" shape
    /// `entities {}` remaps. `Embedder1`/`Embedder2` both embed `Sibling`
    /// via `@trait.Sibling` and each forward a DIFFERENT config knob to it
    /// (every-embedder). `OtherFetcher` fetches `Other` and reads a bare
    /// `@Other.name` token, and forwards the orbital's `deadKnobOrbital`
    /// knob (its only forwarder — dead-knob-drop fixture). `AppLayoutTrait`
    /// forwards `navItems` from the orbital's own literal default
    /// (page-path-rewrite-after-forward fixture). `Widget`'s `read_policy`
    /// carries a role-literal comparison.
    fn stage_b_main_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "MainOrbital",
            "id": "orb_STAGEBMAINORBITAL0000000",
            "config": {
                "navItems": { "type": "array", "default": [ { "href": "/main", "label": "Main" } ] },
                "deadKnobOrbital": { "type": "string", "default": "only-forwarder-value" }
            },
            "entity": {
                "name": "Widget",
                "persistence": "persistent",
                "read_policy": ["=", "@user.role", "approver"],
                "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "otherId", "type": "relation", "relation": { "entity": "Other", "cardinality": "one" } }
                ]
            },
            "auxiliaryEntities": [
                {
                    "name": "ThingOwner",
                    "identity": true,
                    "persistence": "persistent",
                    "fields": [
                        { "name": "id", "type": "string", "required": true },
                        { "name": "role", "type": "string", "values": ["employee", "approver"] }
                    ]
                }
            ],
            "traits": [
                {
                    "name": "Embedder1", "linkedEntity": "Widget", 
                    "config": { "embedKnob1": { "type": "string", "default": "VALUE1" } },
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN1" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN1",
                            "effects": [ ["render-ui", "main", { "type": "stack", "children": ["@trait.Sibling"] }] ] } ]
                    }
                },
                {
                    "name": "Embedder2", "linkedEntity": "Widget", 
                    "config": { "embedKnob2": { "type": "string", "default": "VALUE2" } },
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "OPEN2" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "OPEN2",
                            "effects": [ ["render-ui", "aux", { "type": "stack", "children": ["@trait.Sibling"] }] ] } ]
                    }
                },
                {
                    "name": "Sibling", "linkedEntity": "Widget", 
                    "config": {
                        "embedKnob1": { "type": "string", "default": "@config.embedKnob1" },
                        "embedKnob2": { "type": "string", "default": "@config.embedKnob2" }
                    },
                    "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
                },
                {
                    "name": "OtherFetcher", "linkedEntity": "Widget", 
                    "config": { "deadKnob": { "type": "string", "default": "@config.deadKnobOrbital" } },
                    "stateMachine": {
                        "states": [ { "name": "idle", "isInitial": true } ],
                        "events": [ { "key": "LOAD" } ],
                        "transitions": [ { "from": "idle", "to": "idle", "event": "LOAD",
                            "guard": ["!=", "@Other.name", ""],
                            "effects": [ ["fetch", "Other", {}] ] } ]
                    }
                },
                {
                    "name": "AppLayoutTrait", "linkedEntity": "Widget", 
                    "config": { "navItems": { "type": "array", "default": "@config.navItems" } },
                    "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
                }
            ],
            "pages": [
                { "name": "MainPage", "path": "/main", "primaryEntity": "Widget", "traits": [ { "ref": "AppLayoutTrait" } ] }
            ]
        }))
        .expect("MainOrbital fixture must deserialize")
    }

    /// Second orbital of the `Up` alias — declares `Other`, the
    /// out-of-orbital entity `MainOrbital.Widget.otherId` relates to and
    /// `OtherFetcher` fetches/reads by bare token.
    fn stage_b_other_orbital() -> Orbital {
        serde_json::from_value(json!({
            "name": "OtherOrbital",
            "entity": {
                "name": "Other",
                "persistence": "persistent",
                "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "name", "type": "string" }
                ]
            },
            "traits": []
        }))
        .expect("OtherOrbital fixture must deserialize")
    }

    #[test]
    fn an_entities_binding_renames_the_relation_targets_inside_inherited_expectation_shapes() {
        let upstream: Orbital = serde_json::from_value(json!({
            "name": "NotesOrbital",
            "entity": { "name": "Note", "persistence": "persistent", "fields": [
                { "name": "id", "type": "string", "required": true },
                { "name": "ownerId", "type": "relation", "relation": { "entity": "Learner", "cardinality": "one" } }
            ] },
            "expects": [
                { "kind": "identity", "name": "Learner", "shape": [{ "name": "id", "type": "string", "required": true }] },
                { "kind": "entity", "name": "Path", "shape": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "ownerId", "type": "relation", "relation": { "entity": "Learner", "cardinality": "one" } },
                    { "name": "topicId", "type": "relation", "relation": { "entity": "Topic", "cardinality": "one" } }
                ] }
            ],
            "traits": []
        }))
        .unwrap();
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi("Up", vec![upstream], "notes".to_string());
        let mut orbital: Orbital = serde_json::from_value(json!({ "name": "Local", "entity": "Up.orbitals.NotesOrbital.entity", "traits": [] })).unwrap();
        orbital.uses = Some(vec![UseDeclaration { lazy: None, from: "notes".to_string(), alias: "Up".to_string(), config: None }]);
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.NotesOrbital".to_string(),
            entities: Some(orbital_core_base::collections::IndexMap::from([("Learner".to_string(), "LearnerLearner".to_string())])),
            ..Default::default()
        };
        let consumer_ids = orbital_core_base::collections::IndexMap::from([("LearnerLearner".to_string(), "ent_LEARNERLEARNERID000000000".to_string())]);
        resolve_orbital_reference(&mut orbital, &obj, &ctx, &orbital_core_base::collections::IndexMap::new(), &consumer_ids, &HashMap::new(), None, &mut Vec::new(), None, &mut Vec::new())
            .expect("resolves");
        let target = |fields: &[FieldDefinition], name: &str| {
            fields.iter().find(|f| f.name.as_deref() == Some(name)).and_then(|f| f.relation.as_ref()).map(|r| r.entity.clone())
        };
        let EntityRef::Inline(entity) = &orbital.entity else { panic!("primary must be inline") };
        assert_eq!(target(&entity.fields, "ownerId").as_deref(), Some("LearnerLearner"), "control: the import's own relation follows the binding");
        let path_shape = orbital.expects.iter().flatten().find_map(|e| match e {
            orbital_core_base::schema::ExpectDeclaration::Entity { name, shape: Some(shape), .. } if name == "Path" => Some(shape.clone()),
            _ => None,
        }).expect("the Path expectation travels with the import");
        assert_eq!(target(&path_shape, "ownerId").as_deref(), Some("LearnerLearner"), "an expectation's relation to the bound entity follows the binding");
        assert_eq!(target(&path_shape, "topicId").as_deref(), Some("Topic"), "control: a relation to an unbound entity is left as declared");
    }

    #[test]
    fn an_entities_binding_renames_relation_targets_to_entities_the_import_only_expects() {
        let upstream: Orbital = serde_json::from_value(json!({
            "name": "MentorOrbital",
            "entity": { "name": "Flow", "persistence": "runtime", "fields": [
                { "name": "id", "type": "string", "required": true }
            ] },
            "expects": [
                { "kind": "entity", "name": "SharedCourse", "shape": [{ "name": "id", "type": "string", "required": true }] },
                { "kind": "entity", "name": "Enrollment", "shape": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "courseId", "type": "relation", "relation": { "entity": "SharedCourse", "cardinality": "one" } },
                    { "name": "otherId", "type": "relation", "relation": { "entity": "Elsewhere", "cardinality": "one" } }
                ] }
            ],
            "traits": []
        }))
        .unwrap();
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi("Up", vec![upstream], "mentor".to_string());
        let mut orbital: Orbital = serde_json::from_value(json!({ "name": "Local", "entity": "Up.orbitals.MentorOrbital.entity", "traits": [] })).unwrap();
        orbital.uses = Some(vec![UseDeclaration { lazy: None, from: "mentor".to_string(), alias: "Up".to_string(), config: None }]);
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MentorOrbital".to_string(),
            entities: Some(orbital_core_base::collections::IndexMap::from([
                ("SharedCourse".to_string(), "LearnSharedCourse".to_string()),
                ("Enrollment".to_string(), "LearnEnrollment".to_string()),
            ])),
            ..Default::default()
        };
        resolve_orbital_reference(&mut orbital, &obj, &ctx, &orbital_core_base::collections::IndexMap::new(), &orbital_core_base::collections::IndexMap::new(), &HashMap::new(), None, &mut Vec::new(), None, &mut Vec::new())
            .expect("resolves");
        let shape = orbital.expects.iter().flatten().find_map(|e| match e {
            orbital_core_base::schema::ExpectDeclaration::Entity { shape: Some(shape), .. } if shape.iter().any(|f| f.name.as_deref() == Some("courseId")) => Some(shape.clone()),
            _ => None,
        }).expect("the Enrollment expectation travels with the import");
        let target = |name: &str| {
            shape.iter().find(|f| f.name.as_deref() == Some(name)).and_then(|f| f.relation.as_ref()).map(|r| r.entity.clone())
        };
        assert_eq!(target("courseId").as_deref(), Some("LearnSharedCourse"), "a relation to an expected entity follows the binding");
        assert_eq!(target("otherId").as_deref(), Some("Elsewhere"), "control: a relation to an unbound entity is left as declared");
    }

    fn stage_b_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Up",
            vec![stage_b_main_orbital(), stage_b_other_orbital()],
            "stage-b".to_string(),
        );
        ctx
    }

    /// `orbital-compiler`'s `serde_json` builds with `preserve_order`
    /// (insertion order, not sorted), so a raw `to_value` of a
    /// `HashMap`-backed struct is NOT deterministic across runs.
    /// Canonicalizing before writing/comparing a fixture is what makes the
    /// byte-equality assertion meaningful — one production owner
    /// (`orbital_core::util::canonical_json`), shared with the CLI's schema
    /// serialization sites and the other three test-only copies this
    /// replaced.
    use orbital_core_eval::util::canonical_json;

    /// The `uses[].from` string shared between the synthesized
    /// pre-materialization `OrbitalSchema` (`stage_b_fixture_inputs`) and its
    /// `upstream` map key — arbitrary, but must be the SAME literal on both
    /// sides since the JS twin's fixture loader keys off it.
    const STAGE_B_UPSTREAM_FROM: &str = "stage-b";

    /// Build the two extra fixture inputs `assert_or_write_fixture` needs
    /// alongside the materialized result: the consumer `OrbitalSchema` AS IT
    /// STOOD BEFORE MATERIALIZATION (still carrying `reference` + a `uses`
    /// entry, not yet spliced), and the `upstream` map the JS resolver's
    /// loader stub serves that `uses[].from` from.
    ///
    /// `pre` is a clone of the consumer orbital taken right before
    /// `resolve_orbital_reference` ran (so it still has whatever the test
    /// set up — e.g. a sibling trait riding along — but not yet the
    /// materialized closure). `obj` is the same `OrbitalRefObject` passed to
    /// that call. The referenced orbital (`obj.reference`'s last path
    /// segment) becomes `upstream.orbital`; `stage_b_main_orbital` +
    /// `stage_b_other_orbital`, in that order, become `upstream.orbitals` —
    /// matching `stage_b_ctx`'s `add_alias_multi` call.
    fn stage_b_fixture_inputs(
        pre: &Orbital,
        obj: &OrbitalRefObject,
        // Same map `resolve_orbital_reference` was called with. The JS twin
        // has no such side-channel — it derives a consumer entity's id by
        // reading `schema.orbitals[].entity.id`, so every `entities {}`
        // target needs a declaring orbital in the fixture `schema`, carrying
        // whatever id this map supplies (absent stays id-less on both
        // sides, keeping an unresolved remap unresolved on both sides too).
        consumer_ids: &orbital_core_base::collections::IndexMap<String, String>,
    ) -> (OrbitalSchema, orbital_core_base::collections::IndexMap<String, serde_json::Value>) {
        let mut consumer = pre.clone();
        consumer.uses = Some(vec![UseDeclaration {
            lazy: None,
            from: STAGE_B_UPSTREAM_FROM.to_string(),
            alias: "Up".to_string(),
            config: None,
        }]);
        consumer.reference = Some(obj.clone());
        let consumer_name = consumer.name.clone();
        let mut orbitals = vec![consumer];
        // A schema is only a valid consumer input (what the JS resolver's
        // own pre-checks require) when every `entities {}` target is
        // declared somewhere in it — `resolve_orbital_reference` itself
        // trusts `consumer_ids` as an external fact and never checks this,
        // but a fixture destined for the JS twin has to stand on its own.
        let mut targets: Vec<&String> = obj
            .entities
            .as_ref()
            .map(|m| m.values().collect())
            .unwrap_or_default();
        targets.sort();
        targets.dedup();
        for target in targets {
            let owner: Orbital = serde_json::from_value(serde_json::json!({
                "name": format!("{target}Owner"),
                "entity": {
                    "name": target,
                    "id": consumer_ids.get(target),
                    "persistence": "persistent",
                    "fields": [ { "name": "id", "type": "string", "required": true } ]
                },
                "traits": []
            }))
            .expect("consumer entity-owner fixture orbital must deserialize");
            orbitals.push(owner);
        }
        let schema = OrbitalSchema {
            type_contract_version: None,
            resolved: None,            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            name: consumer_name,
            version: None,
            description: None,
            theme: None,
            config: None,
            orbitals,
            metadata: None,
            schema_version: None,
            ledger: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };
        let upstream_orbitals = vec![stage_b_main_orbital(), stage_b_other_orbital()];
        let referenced_name = obj
            .reference
            .rsplit('.')
            .next()
            .expect("reference names an orbital")
            .to_string();
        let referenced = upstream_orbitals
            .iter()
            .find(|o| o.name == referenced_name)
            .unwrap_or_else(|| panic!("stage-b upstream has no orbital named {referenced_name}"));
        let mut upstream = orbital_core_base::collections::IndexMap::new();
        upstream.insert(
            STAGE_B_UPSTREAM_FROM.to_string(),
            serde_json::json!({
                "orbital": referenced,
                "orbitals": upstream_orbitals,
            }),
        );
        (schema, upstream)
    }

    /// Shared-fixture contract (`docs` §B2-R plan, "the `derive_id.json`
    /// pattern"): write `{ schema, upstream, expected }` to
    /// `tests/fixtures/orbital_import_stage_b/<case>.json` when
    /// `UPDATE_FIXTURES=1`, else assert the current materialization is
    /// byte-identical to the committed fixture — B2-J's runtime materializer
    /// twin (`reference-resolver-stage-b-fixtures.test.ts`) reads the SAME
    /// file, resolves `schema` against `upstream` through its OWN
    /// `resolveOrbitalImports`, and asserts the result equals `expected`.
    fn assert_or_write_fixture(
        case: &str,
        schema: &OrbitalSchema,
        upstream: &orbital_core_base::collections::IndexMap<String, serde_json::Value>,
        orbital: &Orbital,
    ) {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/orbital_import_stage_b");
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        let path = dir.join(format!("{case}.json"));
        let combined = serde_json::json!({
            "schema": serde_json::to_value(schema).expect("schema serializes"),
            "upstream": serde_json::to_value(upstream).expect("upstream serializes"),
            "expected": serde_json::to_value(orbital).expect("orbital serializes"),
        });
        let canonical = canonical_json(&combined);
        let pretty = serde_json::to_string_pretty(&canonical).expect("pretty print fixture");
        if std::env::var("UPDATE_FIXTURES").as_deref() == Ok("1") {
            std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
            return;
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing fixture {path:?} — run with UPDATE_FIXTURES=1 to create it, \
                 then commit the file"
            )
        });
        assert_eq!(
            existing.trim_end(),
            pretty,
            "materialization for case {case} drifted from the committed fixture \
             ({path:?}) — the JS twin (B2-J) asserts the SAME file byte-equal; \
             re-run with UPDATE_FIXTURES=1 only after confirming the drift is intentional"
        );
    }

    #[test]
    fn an_entities_retarget_queues_the_upstream_fields_onto_its_target() {
        let ctx = stage_b_ctx();
        let run = |entities: Option<orbital_core_base::collections::IndexMap<String, String>>| {
            let mut orbital = consumer_orbital("Local");
            let obj = OrbitalRefObject {
                reference: "Up.orbitals.MainOrbital".to_string(),
                entities,
                ..Default::default()
            };
            let consumer_ids = orbital_core_base::collections::IndexMap::from([(
                "ConsumerOther".to_string(),
                "ent_CONSUMEROTHERID00000000".to_string(),
            )]);
            let mut merges = Vec::new();
            resolve_orbital_reference(
                &mut orbital,
                &obj,
                &ctx,
                &orbital_core_base::collections::IndexMap::new(),
                &consumer_ids,
                &HashMap::new(),
                None,
                &mut Vec::new(),
                None,
                &mut merges,
            )
            .map(|()| merges)
        };
        let merges = run(Some(orbital_core_base::collections::IndexMap::from([(
            "Other".to_string(),
            "ConsumerOther".to_string(),
        )])))
        .expect("resolves");
        assert_eq!(merges.len(), 1);
        assert_eq!(merges[0].target, "ConsumerOther");
        assert!(!merges[0].intrinsic_only);
        assert!(merges[0]
            .imported
            .fields
            .iter()
            .any(|f| f.name.as_deref() == Some("name")));
    }

    #[test]
    fn control_an_unmapped_entity_queues_no_retarget_merge() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ..Default::default()
        };
        let mut merges = Vec::new();
        let _ = resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut merges,
        );
        assert!(merges.is_empty());
    }

    #[test]
    fn entities_remap_rewrites_relation_fetch_token_and_page_primary_and_repoints_entity_id() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        // Real `inline_orbital` never clears `orbital.uses` after loading it —
        // the materialized result keeps declaring the SAME `uses` its
        // `reference` resolved through. Set it here (matching what
        // `stage_b_fixture_inputs` synthesizes onto the fixture's `schema`
        // copy) so `expected.uses` in the parity fixture isn't spuriously
        // absent relative to the input `schema` the JS twin resolves.
        orbital.uses = Some(vec![UseDeclaration {
            lazy: None,
            from: STAGE_B_UPSTREAM_FROM.to_string(),
            alias: "Up".to_string(),
            config: None,
        }]);
        let mut entities = orbital_core_base::collections::IndexMap::new();
        entities.insert("Other".to_string(), "ConsumerOther".to_string());
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: Some(entities),
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut consumer_ids = orbital_core_base::collections::IndexMap::new();
        consumer_ids.insert(
            "ConsumerOther".to_string(),
            "ent_CONSUMEROTHERID00000000".to_string(),
        );
        let pre = orbital.clone();
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &consumer_ids,
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        // Relation target + its dual-carry id sibling, re-pointed at the
        // CONSUMER entity's own id (not a freshly materialized one).
        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("primary must be inline")
        };
        let other_id_field = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("otherId"))
            .expect("otherId field carried");
        let rel = other_id_field.relation.as_ref().expect("relation carried");
        assert_eq!(rel.entity, "ConsumerOther");
        assert_eq!(
            rel.entity_id.as_deref(),
            Some("ent_CONSUMEROTHERID00000000")
        );

        // `["fetch", "Other", …]` token and the bare `@Other.name` guard
        // token both follow the same remap.
        let fetcher = kept_trait(&orbital, "LocalOtherFetcher");
        let blob = serde_json::to_string(&fetcher.state_machine.transitions).unwrap();
        assert!(blob.contains("\"ConsumerOther\""), "{blob}");
        assert!(blob.contains("@ConsumerOther.name"), "{blob}");
        assert!(!blob.contains("\"Other\""), "{blob}");
        assert!(!blob.contains("@Other."), "{blob}");

        // Page `primaryEntity` follows the PRIMARY rebind (unconditional
        // prefix), independent of the `entities {}` remap.
        let pages = orbital.pages.as_ref().expect("pages carried");
        let PageRef::Inline(page) = &pages[0] else {
            panic!("page must be inline")
        };
        assert_eq!(page.primary_entity.as_deref(), Some("LocalWidget"));

        let (schema, upstream) = stage_b_fixture_inputs(&pre, &obj, &consumer_ids);
        assert_or_write_fixture("entities_remap", &schema, &upstream, &orbital);
    }

    #[test]
    fn uses_config_override_forwards_consumer_app_knob_last_hop() {
        let field = |v: SExpression| -> ConfigField {
            serde_json::from_value(json!({ "type": "unknown", "default": v }))
                .expect("config field")
        };
        let mut folded: orbital_core_base::collections::IndexMap<String, ConfigField> = orbital_core_base::collections::IndexMap::from([
            (
                "navItems".to_string(),
                field(SExpression::string("@config.navItems")),
            ),
            (
                "appName".to_string(),
                field(SExpression::string("@config.missing")),
            ),
            (
                "title".to_string(),
                field(SExpression::string("literal")),
            ),
        ]);
        let consumer: orbital_core_base::collections::IndexMap<String, ConfigField> = orbital_core_base::collections::IndexMap::from([(
            "navItems".to_string(),
            field(SExpression::Array(vec![SExpression::String(
                "/tasks".into(),
            )].into())),
        )]);
        forward_consumer_app_knobs(&mut folded, Some(&consumer));
        let nav = &folded["navItems"];
        assert_eq!(nav.forwarded_from.as_deref(), Some("@config.navItems"));
        assert!(matches!(nav.default, Some(SExpression::Array(_))));
        // an unknown consumer knob and a literal are left for the validator / as-is
        assert_eq!(
            folded["appName"].default,
            Some(SExpression::string("@config.missing"))
        );
        assert_eq!(
            folded["title"].default,
            Some(SExpression::string("literal"))
        );
    }

    /// §86 (`docs/Almadar_Compiler_Gaps.md`): `forward_consumer_app_knobs` was only
    /// exercised by the unit test above — never wired into `resolve_orbital_reference`
    /// itself. Reproduces `TaskOrbital.calendarSyncEnabled`
    /// (`std-project-manager.lolo`): the upstream orbital declares the knob, the
    /// `uses Alias { config: { calendarSyncEnabled: @config.calendarSyncEnabled } }`
    /// call-site override forwards the CONSUMER's own top-level app knob token, and a
    /// kept trait's OWN `calendarSyncEnabled: @config.calendarSyncEnabled` config
    /// default resolves through the no-embedder fallback — the exact spot a
    /// composed trait's GUARD (`(= @config.calendarSyncEnabled true)`) reads its
    /// config default from. Before the fix `orbital.config` (and every trait
    /// forwarding from it) kept the raw `"@config.calendarSyncEnabled"` string —
    /// the literal text a `tsc` error surfaced verbatim in the emitted guard.
    #[test]
    fn uses_config_override_wires_consumer_app_knob_into_resolved_trait_config() {
        let mut upstream = upstream_orbital(Some(json!({
            "calendarSyncEnabled": { "type": "boolean", "default": false }
        })));
        if let TraitReference::Inline(td) = &mut upstream.traits[0] {
            let td = Arc::make_mut(td);
            td.config = Some(orbital_core_base::collections::IndexMap::from([(
                "calendarSyncEnabled".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "boolean", "default": "@config.calendarSyncEnabled"
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let consumer_schema_config: orbital_core_base::collections::IndexMap<String, ConfigField> = orbital_core_base::collections::IndexMap::from([(
            "calendarSyncEnabled".to_string(),
            serde_json::from_value::<ConfigField>(json!({ "type": "boolean", "default": true }))
                .unwrap(),
        )]);

        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: Some(orbital_core_base::collections::IndexMap::from([(
                "calendarSyncEnabled".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "boolean", "default": "@config.calendarSyncEnabled"
                }))
                .unwrap(),
            )])),
            events: None,
            roles: None,
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            Some(&consumer_schema_config),
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        assert_eq!(
            orbital
                .config
                .as_ref()
                .and_then(|c| c.get("calendarSyncEnabled"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::Bool(true)),
            "the orbital-rung fold must resolve the consumer app-knob token to its literal"
        );
        let a = kept_trait(&orbital, "LocalA");
        let guard_default = a
            .config
            .as_ref()
            .and_then(|c| c.get("calendarSyncEnabled"))
            .and_then(|f| f.default.clone());
        assert_eq!(
            guard_default,
            Some(SExpression::Bool(true)),
            "a kept trait's own config (what a guard's `@config.calendarSyncEnabled` \
             reads through the no-embedder fallback) must carry the resolved literal, \
             not the raw '@config.calendarSyncEnabled' token"
        );
    }

    #[test]
    fn extend_fields_appends_scalar_and_relation_and_writes_fixture() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        orbital.uses = Some(vec![UseDeclaration {
            lazy: None,
            from: STAGE_B_UPSTREAM_FROM.to_string(),
            alias: "Up".to_string(),
            config: None,
        }]);
        let obj = OrbitalRefObject {
            page_modifiers: None,
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            // The shared upstream's `Widget.otherId -> Other` must be mapped
            // or the fixture is an INVALID program (parity is defined for
            // valid inputs only; the JS twin refuses at resolve).
            entities: Some(orbital_core_base::collections::IndexMap::from([(
                "Other".to_string(),
                "ConsumerOther".to_string(),
            )])),
            mounts: None,
            extend: Some(vec![
                extend_field("overtimeHours", json!("number"), Some(json!(0))),
                serde_json::from_value(json!({
                    "name": "workspaceId",
                    "type": "relation",
                    "relation": { "entity": "Workspace", "cardinality": "one" }
                }))
                .unwrap(),
            ]),
            listens: None,
            mock: None,
            ..Default::default()
        };
        let mut consumer_ids = orbital_core_base::collections::IndexMap::new();
        consumer_ids.insert(
            "Workspace".to_string(),
            "ent_CONSUMERWORKSPACEID00000".to_string(),
        );
        consumer_ids.insert(
            "ConsumerOther".to_string(),
            "ent_CONSUMEROTHERID000000000".to_string(),
        );
        let pre = orbital.clone();
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &consumer_ids,
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("primary must be inline")
        };
        let hours = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("overtimeHours"))
            .expect("scalar extend field appended");
        assert_eq!(hours.field_type, orbital_core_base::schema::FieldType::Number);
        let workspace = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("workspaceId"))
            .expect("relation extend field appended");
        let rel = workspace.relation.as_ref().expect("relation carried");
        assert_eq!(rel.entity, "Workspace", "never renamed — consumer scope");
        assert_eq!(
            rel.entity_id.as_deref(),
            Some("ent_CONSUMERWORKSPACEID00000")
        );

        let (mut schema, upstream) = stage_b_fixture_inputs(&pre, &obj, &consumer_ids);
        // `stage_b_fixture_inputs` only auto-declares an owner orbital for
        // `obj.entities {}` targets — `extend {}`'s relation target is a
        // DIFFERENT consumer-scope surface (never routed through
        // `entities {}`), so this fixture declares its own `Workspace`
        // owner orbital, matching the SAME id `consumer_ids` supplies, so
        // the JS twin's schema-derived id lookup resolves identically.
        let workspace_owner: Orbital = serde_json::from_value(json!({
            "name": "WorkspaceOwner",
            "entity": {
                "name": "Workspace",
                "id": "ent_CONSUMERWORKSPACEID00000",
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("workspace owner fixture orbital must deserialize");
        schema.orbitals.push(workspace_owner);
        assert_or_write_fixture("extend_fields", &schema, &upstream, &orbital);
    }

    #[test]
    fn roles_single_target_is_diagnosed_and_recorded() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        let mut roles = orbital_core_base::collections::IndexMap::new();
        roles.insert("approver".to_string(), vec!["owner".to_string()]);
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: Some(roles),
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("primary must be inline")
        };
        assert_eq!(
            entity.read_policy,
            Some(SExpression::Array(vec![
                SExpression::String(("=".to_string()).into()),
                SExpression::String(("@user.role".to_string()).into()),
                SExpression::String(("owner".to_string()).into()),
            ].into())),
            "the primary entity's read_policy role literal must be remapped"
        );
        assert_eq!(diagnostics.len(), 1);
    }

    /// A forwarded `navItems` holding an upstream page path is config data:
    /// the `pages` remap leaves it verbatim.
    #[test]
    fn forwarded_config_page_path_is_not_rewritten_by_the_pages_remap() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        let mut pages = orbital_core_base::collections::IndexMap::new();
        pages.insert("/main".to_string(), "/local-main".to_string());
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: Some(pages),
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        // `Other` is out-of-orbital and unmapped in this test (it isn't
        // exercising `entities {}`) — expected, not what this test checks.

        let app_layout = kept_trait(&orbital, "LocalAppLayoutTrait");
        let nav_items = app_layout
            .config
            .as_ref()
            .and_then(|c| c.get("navItems"))
            .and_then(|f| f.default.clone())
            .expect("navItems default resolved");
        let blob = serde_json::to_string(&nav_items).unwrap();
        assert!(blob.contains("\"/main\""), "{blob}");
        assert!(!blob.contains("/local-main"), "{blob}");
    }

    /// Every surviving embedder forwards its OWN knob to the shared
    /// sibling, in order — the old first-embedder `break` would have
    /// dropped `Embedder2`'s forward.
    #[test]
    fn every_embedder_forwards_its_own_knob_to_the_shared_sibling() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
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
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        // `Other` is out-of-orbital and unmapped in this test (it isn't
        // exercising `entities {}`) — expected, not what this test checks.

        let sibling = kept_trait(&orbital, "LocalSibling");
        assert_eq!(
            sibling
                .config
                .as_ref()
                .and_then(|c| c.get("embedKnob1"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("VALUE1".to_string()).into())),
            "Embedder1's forward must resolve"
        );
        assert_eq!(
            sibling
                .config
                .as_ref()
                .and_then(|c| c.get("embedKnob2"))
                .and_then(|f| f.default.clone()),
            Some(SExpression::String(("VALUE2".to_string()).into())),
            "Embedder2's forward must ALSO resolve — every embedder, not just the first"
        );
    }

    /// Planning find (iii): an `omit {}` that drops the ONLY forwarder of an
    /// orbital knob must drop the knob from the folded config too, or the
    /// consumer keeps a dead `ORB_O_CONFIG_DEAD_KNOB` knob.
    #[test]
    fn omitting_the_only_forwarder_drops_the_orbital_knob() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: Some(vec!["OtherFetcher".to_string()]),
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let folded = orbital.config.expect("folded config carried");
        assert!(
            !folded.contains_key("deadKnobOrbital"),
            "deadKnobOrbital's only forwarder (OtherFetcher) was omitted — the knob must be dropped: {folded:?}"
        );
        assert!(
            folded.contains_key("navItems"),
            "navItems is still forwarded by the surviving AppLayoutTrait: {folded:?}"
        );
    }

    /// Siblings authored directly in the reference body ride along
    /// un-prefixed and mount on the remapped page; a `pages {}`/`mounts {}`
    /// key naming no upstream path is diagnosed.
    #[test]
    fn sibling_rides_along_unprefixed_and_mounts_on_the_remapped_page() {
        let ctx = stage_b_ctx();
        let mut orbital = consumer_orbital("Local");
        // See the SAME note in `entities_remap_rewrites_...` above: real
        // `inline_orbital` never clears `orbital.uses`, so the materialized
        // result keeps it — set it here to match `stage_b_fixture_inputs`'s
        // synthesized `schema` copy.
        orbital.uses = Some(vec![UseDeclaration {
            lazy: None,
            from: STAGE_B_UPSTREAM_FROM.to_string(),
            alias: "Up".to_string(),
            config: None,
        }]);
        // Simulate `orbital-lolo`'s lowering of a sibling trait declared in
        // the reference body: it lands in `orbital.traits` BEFORE the
        // splice runs, un-prefixed, consumer vocabulary.
        let sibling_td: TraitDefinition = serde_json::from_value(json!({
            "name": "LocalSiblingTrait",
            "linkedEntity": "LocalWidget",
            "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }))
        .unwrap();
        orbital.traits = vec![TraitReference::Inline(Arc::new(sibling_td))];

        let mut pages = orbital_core_base::collections::IndexMap::new();
        pages.insert("/main".to_string(), "/local-main".to_string());
        let mut mounts = orbital_core_base::collections::IndexMap::new();
        mounts.insert("/main".to_string(), vec!["LocalSiblingTrait".to_string()]);
        let mut unknown_pages = pages.clone();
        unknown_pages.insert("/nope".to_string(), "/x".to_string());
        // `MainOrbital.Widget.otherId` relates to the out-of-orbital `Other`
        // entity; mapping it here (rather than leaving `entities: None`)
        // keeps this fixture a VALID consumer input for the JS twin, which
        // refuses an unmapped out-of-orbital reference at resolve time
        // (`ORB_O_ENTITY_UNMAPPED`) instead of merely diagnosing it the way
        // `resolve_orbital_reference` does. Unrelated to what this test
        // actually exercises (sibling-trait mounting), so no assertion below
        // depends on the remap. `consumer_ids` supplies `ConsumerOther`'s id
        // (SAME id `entities_remap...` uses) so the retarget actually folds
        // in Rust AND the fixture's synthesized `ConsumerOtherOwner.entity`
        // carries an id the JS twin's own id-lookup can index — an id-less
        // target fixture reports `ORB_O_ENTITY_TARGET_UNKNOWN` on that side.
        let mut entities = orbital_core_base::collections::IndexMap::new();
        entities.insert("Other".to_string(), "ConsumerOther".to_string());
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.MainOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: Some(unknown_pages),
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: Some(entities),
            mounts: Some(mounts),
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        // The parity fixture must be a VALID consumer input (the JS twin
        // refuses `ORB_O_PAGE_UNKNOWN_PATH` at resolve time); only the
        // diagnostic assertion below needs the `/nope` key.
        let fixture_obj = OrbitalRefObject {
            pages: Some(pages.clone()),
            ..obj.clone()
        };
        let mut consumer_ids = orbital_core_base::collections::IndexMap::new();
        consumer_ids.insert(
            "ConsumerOther".to_string(),
            "ent_CONSUMEROTHERID00000000".to_string(),
        );
        let pre = orbital.clone();
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &consumer_ids,
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        // Sibling carried un-prefixed, alongside the (prefixed) upstream traits.
        let names = trait_names(&orbital);
        assert!(
            names.contains(&"LocalSiblingTrait".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"LocalAppLayoutTrait".to_string()),
            "{names:?}"
        );

        // Mounted on the remapped page, un-prefixed reference form.
        let pages = orbital.pages.as_ref().expect("pages carried");
        let PageRef::Inline(page) = &pages[0] else {
            panic!("page must be inline")
        };
        assert_eq!(page.path, "/local-main");
        let mounted = page
            .traits
            .as_ref()
            .expect("traits carried")
            .iter()
            .any(|tr| matches!(tr, TraitReference::Reference { reference: Some(r), .. } if r == "LocalSiblingTrait"));
        assert!(mounted, "{:?}", page.traits);

        assert_eq!(diagnostics.len(), 1);

        let (schema, upstream) = stage_b_fixture_inputs(&pre, &fixture_obj, &consumer_ids);
        assert_or_write_fixture("sibling_mount", &schema, &upstream, &orbital);
    }

    /// (f): a knob declared `[RecordAction]`-shaped (element schema types a
    /// member `event`) has that member renamed via `events {}` — the struct
    /// twin of the scalar `event`-typed knob fold.
    fn ref_with_listens(listens: serde_json::Value, omit: Option<Vec<String>>) -> OrbitalRefObject {
        OrbitalRefObject {
            page_modifiers: None,
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: Some(serde_json::from_value(listens).expect("listens fixture")),
            ..Default::default()
        }
    }

    fn resolve_with(obj: &OrbitalRefObject) -> Result<Orbital, InlineError> {
        resolve_with_siblings(obj, &[])
    }

    fn resolve_with_siblings(obj: &OrbitalRefObject, siblings: &[&str]) -> Result<Orbital, InlineError> {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        orbital.traits = siblings
            .iter()
            .map(|name| {
                serde_json::from_value(json!({
                    "name": name,
                    "linkedEntity": "Up",
                    "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
                }))
                .expect("sibling fixture")
            })
            .collect();
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )?;
        Ok(orbital)
    }

    /// project-friday: `ProjectLifecycle` (a host sibling) hands its row to
    /// the imported persistor — the route lands on the MATERIALIZED trait and
    /// the host source keeps its own name.
    #[test]
    fn import_listens_route_a_host_sibling_into_an_imported_trait() {
        let obj = ref_with_listens(
            json!([{ "trait": "B", "event": "GO", "source": { "kind": "trait", "trait": "Host" }, "triggers": "PING" }]),
            None,
        );
        let orbital = resolve_with_siblings(&obj, &["Host"]).expect("resolves");
        let b = kept_trait(&orbital, "LocalB");
        let routed = b.listens.as_deref().map(Vec::as_slice).unwrap_or(&[]).iter().any(|l| {
            l.event == "GO"
                && l.triggers.as_deref() == Some("PING")
                && matches!(&l.source, Some(orbital_core_base::schema::ListenSource::Trait { trait_name, .. }) if trait_name == "Host")
        });
        assert!(routed, "{:?}", b.listens);
    }

    #[test]
    fn import_listens_source_naming_an_imported_trait_follows_the_prefix() {
        let obj = ref_with_listens(
            json!([{ "trait": "B", "event": "OPEN", "source": { "kind": "trait", "trait": "A" }, "triggers": "PING" }]),
            None,
        );
        let orbital = resolve_with(&obj).expect("resolves");
        let b = kept_trait(&orbital, "LocalB");
        assert!(b.listens.as_deref().map(Vec::as_slice).unwrap_or(&[]).iter().any(|l| matches!(
            &l.source,
            Some(orbital_core_base::schema::ListenSource::Trait { trait_name, .. }) if trait_name == "LocalA"
        )), "{:?}", b.listens);
    }

    fn host_embedding(name: &str, embeds: &str) -> TraitReference {
        serde_json::from_value(json!({
            "name": name,
            "linkedEntity": "Up",
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "GO" } ],
                "transitions": [ { "from": "idle", "to": "idle", "event": "GO",
                    "effects": [ ["render-ui", "main", { "type": "stack", "children": [ format!("@trait.{embeds}") ] }] ] } ]
            }
        }))
        .expect("embedding sibling fixture")
    }

    fn embedded_tokens(orbital: &Orbital, host: &str) -> String {
        serde_json::to_string(kept_trait(orbital, host)).expect("serialises")
    }

    #[test]
    fn import_body_sibling_embedding_an_upstream_trait_name_lands_on_the_materialized_trait() {
        let obj = ref_with_listens(json!([]), None);
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        orbital.traits = vec![host_embedding("Host", "B")];
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let blob = embedded_tokens(&orbital, "Host");
        assert!(blob.contains("@trait.LocalB"), "{blob}");
        assert!(!blob.contains("@trait.B\""), "{blob}");
    }

    #[test]
    fn control_import_body_sibling_embedding_its_own_sibling_keeps_the_name() {
        let obj = ref_with_listens(json!([]), None);
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        orbital.traits = vec![host_embedding("Host", "Mine"), host_embedding("Mine", "Mine")];
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        assert!(embedded_tokens(&orbital, "Host").contains("@trait.Mine"));
    }

    #[test]
    fn import_listens_source_naming_no_trait_is_an_error_that_names_the_candidates() {
        for source in ["Typo", "C"] {
            let omit = (source == "C").then(|| vec!["C".to_string()]);
            let obj = ref_with_listens(
                json!([{ "trait": "B", "event": "GO", "source": { "kind": "trait", "trait": source }, "triggers": "PING" }]),
                omit,
            );
            match resolve_with_siblings(&obj, &["Host"]) {
                Err(e @ InlineError::OrbitalRefListenSourceUnknown { .. }) => {
                    assert_eq!(e.structured_code(), "ORB_O_LISTEN_SOURCE_UNKNOWN");
                    assert!(e.to_string().contains("A, B"), "{e}");
                }
                other => panic!("{source}: expected OrbitalRefListenSourceUnknown, got {:?}", other.map(|_| ())),
            }
        }
    }

    #[test]
    fn control_import_listens_from_an_orbital_kind_source_is_not_checked_here() {
        let obj = ref_with_listens(
            json!([{ "trait": "B", "event": "GO", "source": { "kind": "orbital", "orbital": "Elsewhere", "trait": "Anything" }, "triggers": "PING" }]),
            None,
        );
        resolve_with(&obj).expect("another orbital's trait is the validator's to resolve");
    }

    #[test]
    fn import_listens_target_outside_the_imported_set_is_an_error() {
        for (target, omit) in [("Z", None), ("C", Some(vec!["C".to_string()]))] {
            let obj = ref_with_listens(
                json!([{ "trait": target, "event": "GO", "source": { "kind": "trait", "trait": "Host" }, "triggers": "PING" }]),
                omit,
            );
            match resolve_with_siblings(&obj, &["Host"]) {
                Err(e @ InlineError::OrbitalRefListenTargetUnknown { .. }) => {
                    assert_eq!(e.structured_code(), "ORB_O_LISTEN_TARGET_UNKNOWN")
                }
                other => panic!(
                    "{target}: expected OrbitalRefListenTargetUnknown, got {:?}",
                    other.map(|_| ())
                ),
            }
        }
    }

    #[test]
    fn record_action_style_knob_event_member_is_renamed_via_events() {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(a) = &mut upstream.traits[0] {
            let a = Arc::make_mut(a);
            a.config = Some(orbital_core_base::collections::IndexMap::from([(
                "actions".to_string(),
                serde_json::from_value::<ConfigField>(json!({
                    "type": "array",
                    "default": [ { "event": "EDIT_ITEM", "label": "Edit" } ],
                    "items": {
                        "type": "object",
                        "properties": {
                            "event": { "type": "event" },
                            "label": { "type": "string" }
                        }
                    }
                }))
                .unwrap(),
            )]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);

        let mut orbital = consumer_orbital("Local");
        let mut events = orbital_core_base::collections::IndexMap::new();
        events.insert("EDIT_ITEM".to_string(), "EditRow".to_string());
        let obj = OrbitalRefObject {
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: None,
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: Some(events),
            roles: None,
            entities: None,
            mounts: None,
            extend: None,
            mock: None,
            listens: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let a = kept_trait(&orbital, "LocalA");
        let actions = a
            .config
            .as_ref()
            .and_then(|c| c.get("actions"))
            .and_then(|f| f.default.clone());
        let blob = serde_json::to_string(&actions).unwrap();
        assert!(blob.contains("EditRow"), "{blob}");
        assert!(!blob.contains("EDIT_ITEM"), "{blob}");
    }

    /// Import `UpOrbital` as `Local` with trait A holding `one : event = <one>`
    /// and `tools : [{ event }] = [{ event: <tool> }]`; returns A's two values.
    fn imported_event_values(
        one: &str,
        tool: &str,
        events: Option<orbital_core_base::collections::IndexMap<String, String>>,
    ) -> (String, String) {
        let mut upstream = upstream_orbital(None);
        if let TraitReference::Inline(a) = &mut upstream.traits[0] {
            let a = Arc::make_mut(a);
            a.config = Some(orbital_core_base::collections::IndexMap::from([
                (
                    "one".to_string(),
                    serde_json::from_value::<ConfigField>(json!({ "type": "event", "default": one }))
                        .unwrap(),
                ),
                (
                    "tools".to_string(),
                    serde_json::from_value::<ConfigField>(json!({
                        "type": "array",
                        "default": [{ "event": tool }],
                        "items": { "type": "object", "properties": { "event": { "type": "event" } } }
                    }))
                    .unwrap(),
                ),
            ]));
        }
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);
        let mut orbital = consumer_orbital("Local");
        let mut obj = ref_with_listens(json!([]), None);
        obj.listens = None;
        obj.events = events;
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        let cfg = kept_trait(&orbital, "LocalA")
            .config
            .clone()
            .unwrap_or_default();
        let one = match cfg.get("one").and_then(|f| f.default.clone()) {
            Some(SExpression::String(s)) => s,
            other => panic!("one: {other:?}"),
        };
        let tool = cfg
            .get("tools")
            .and_then(|f| f.default.clone())
            .and_then(|d| serde_json::to_value(d).ok())
            .and_then(|v| v[0]["event"].as_str().map(str::to_string))
            .expect("tools[0].event");
        (one.into_inner(), tool)
    }

    #[test]
    fn a_trait_addressed_event_value_follows_the_import_prefix() {
        let (one, tool) = imported_event_values("B.PING", "B.PING", None);
        assert_eq!(one, "LocalB.PING");
        assert_eq!(tool, "LocalB.PING");
    }

    #[test]
    fn an_orbital_addressed_event_value_naming_its_own_upstream_follows_the_import() {
        let (one, tool) = imported_event_values("UpOrbital.B.PING", "UpOrbital.B.PING", None);
        assert_eq!(one, "Local.LocalB.PING");
        assert_eq!(tool, "Local.LocalB.PING");
    }

    #[test]
    fn an_addressed_event_value_follows_an_events_rename_of_its_trait() {
        let events = orbital_core_base::collections::IndexMap::from([("PING".to_string(), "Ping".to_string())]);
        let (one, tool) = imported_event_values("B.PING", "UpOrbital.B.PING", Some(events));
        assert_eq!(one, "LocalB.Ping");
        assert_eq!(tool, "Local.LocalB.Ping");
    }

    #[test]
    fn control_an_address_into_another_orbital_is_left_alone() {
        let (one, tool) = imported_event_values("Elsewhere.B.PING", "Elsewhere.B.PING", None);
        assert_eq!(one, "Elsewhere.B.PING");
        assert_eq!(tool, "Elsewhere.B.PING");
    }

    // -------------------------------------------------------------------
    // Stage C: `extend { â¦ }` materialization + validators.
    // -------------------------------------------------------------------

    fn extend_field(
        name: &str,
        ty: serde_json::Value,
        default: Option<serde_json::Value>,
    ) -> FieldDefinition {
        let mut value = json!({ "name": name, "type": ty });
        if let Some(d) = default {
            value["default"] = d;
        }
        serde_json::from_value(value).expect("extend field fixture must deserialize")
    }

    #[test]
    fn extend_field_appends_a_new_field_onto_the_primary_entity() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            page_modifiers: None,
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
            extend: Some(vec![extend_field(
                "overtimeHours",
                json!("number"),
                Some(json!(0)),
            )]),
            listens: None,
            mock: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("expected inline entity");
        };
        let field = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("overtimeHours"))
            .expect("extend field appended");
        assert_eq!(field.field_type, orbital_core_base::schema::FieldType::Number);
    }

    #[test]
    fn extend_field_colliding_with_upstream_primary_field_is_not_appended() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let obj = OrbitalRefObject {
            page_modifiers: None,
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
            // `Widget` (the upstream primary entity, see `upstream_orbital`)
            // already declares `id`.
            extend: Some(vec![extend_field("id", json!("string"), None)]),
            listens: None,
            mock: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        // The colliding field is NOT appended a second time.
        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("expected inline entity");
        };
        let id_fields: Vec<_> = entity
            .fields
            .iter()
            .filter(|f| f.name.as_deref() == Some("id"))
            .collect();
        assert_eq!(id_fields.len(), 1, "collision must not duplicate the field");
    }

    #[test]
    fn extend_field_colliding_with_a_fields_rename_target_is_not_appended() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let mut fields = orbital_core_base::collections::IndexMap::new();
        fields.insert("id".to_string(), "widgetId".to_string());
        let obj = OrbitalRefObject {
            page_modifiers: None,
            reference: "Up.orbitals.UpOrbital".to_string(),
            ref_id: None,
            entity: None,
            fields: Some(fields),
            pages: None,
            omit: None,
            only: None,
            config: None,
            events: None,
            roles: None,
            entities: None,
            mounts: None,
            // `widgetId` is the RENAME TARGET `fields {}` just introduced.
            extend: Some(vec![extend_field("widgetId", json!("string"), None)]),
            listens: None,
            mock: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("expected inline entity");
        };
        let renamed: Vec<_> = entity
            .fields
            .iter()
            .filter(|f| f.name.as_deref() == Some("widgetId"))
            .collect();
        assert_eq!(renamed.len(), 1, "collision must not duplicate the field");
    }

    #[test]
    fn extend_relation_field_resolves_target_in_the_consumer_scope() {
        let ctx = ctx_with_upstream(None);
        let mut orbital = consumer_orbital("Local");
        let mut consumer_entity_ids = orbital_core_base::collections::IndexMap::new();
        // A CONSUMER-scope entity (declared by some OTHER orbital in this
        // schema) named `Workspace` — never part of the `Up` upstream at all.
        consumer_entity_ids.insert(
            "Workspace".to_string(),
            "ent_CONSUMERWORKSPACEID00000".to_string(),
        );
        let obj = OrbitalRefObject {
            page_modifiers: None,
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
            extend: Some(vec![serde_json::from_value(json!({
                "name": "workspaceId",
                "type": "relation",
                "relation": { "entity": "Workspace", "cardinality": "one" }
            }))
            .unwrap()]),
            listens: None,
            mock: None,
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &consumer_entity_ids,
            &HashMap::new(),
            None,
            &mut diagnostics,
            None,
            &mut Vec::new(),
        )
        .expect("resolves");

        let EntityRef::Inline(entity) = &orbital.entity else {
            panic!("expected inline entity");
        };
        let field = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("workspaceId"))
            .expect("relation extend field appended");
        let relation = field.relation.as_ref().expect("relation field");
        assert_eq!(
            relation.entity, "Workspace",
            "never renamed — consumer scope"
        );
        assert_eq!(
            relation.entity_id.as_deref(),
            Some("ent_CONSUMERWORKSPACEID00000"),
            "resolved against consumer_entity_ids, not the upstream's own ids"
        );
    }

    #[test]
    fn a_field_rename_follows_into_declared_index_paths_and_leaves_other_fields_alone() {
        let mut entity: EntityDefinition = serde_json::from_value(serde_json::json!({
            "name": "Edge",
            "fields": [{ "name": "graphId", "type": "string" }, { "name": "weight", "type": "number" }],
            "indexes": [
                { "fields": ["graphId", "relation"], "orderBy": "weight:desc" },
                { "fields": ["relation"], "orderBy": "weight" }
            ]
        }))
        .unwrap();
        let subs = orbital_core_base::collections::IndexMap::from([
            ("graphId".to_string(), "networkId".to_string()),
            ("weight".to_string(), "strength".to_string()),
        ]);
        rewrite_entity_fields_in_entity(&mut entity, &subs);
        let indexes = entity.indexes.as_ref().unwrap();
        assert_eq!(indexes[0].fields, vec!["networkId".to_string(), "relation".to_string()]);
        assert_eq!(indexes[0].order_by.as_deref(), Some("strength:desc"));
        assert_eq!(indexes[1].fields, vec!["relation".to_string()]);
        assert_eq!(indexes[1].order_by.as_deref(), Some("strength"));

        let mut untouched = entity.clone();
        rewrite_entity_fields_in_entity(&mut untouched, &orbital_core_base::collections::IndexMap::new());
        assert_eq!(untouched.indexes, entity.indexes, "control: no substitutions, no change");
    }

    fn typed_upstream() -> Orbital {
        serde_json::from_value(json!({
            "name": "UpOrbital",
            "id": "orb_UPSTREAMTYPEDORBITAL0000",
            "types": {"Badge": [{"name": "id", "type": "string"}]},
            "entity": {"name": "Widget", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true},
                {"name": "badge", "type": "union", "values": ["Badge"]}]},
            "traits": [{"name": "A", "linkedEntity": "Widget", "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}],
                "events": [{"key": "READ", "payloadSchema": [{"name": "badge", "type": "Badge"}]}],
                "transitions": [{"from": "idle", "to": "idle", "event": "READ"}]}}]
        }))
        .expect("typed upstream fixture")
    }

    fn resolve_typed(behavior: Option<&str>, consumer_types: serde_json::Value) -> Orbital {
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &typed_upstream());
        if let Some(behavior) = behavior {
            ctx.set_behavior_name("Up", behavior.to_string());
        }
        let mut orbital: Orbital = serde_json::from_value(json!({
            "name": "Local", "types": consumer_types, "entity": "Up.orbitals.UpOrbital.entity", "traits": []
        }))
        .expect("consumer");
        let obj = OrbitalRefObject { reference: "Up.orbitals.UpOrbital".to_string(), ..Default::default() };
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        orbital
    }

    #[test]
    fn an_orbital_level_uses_qualifies_the_behaviors_local_types() {
        let orbital = resolve_typed(Some("Atom"), json!(null));
        let types = orbital.types.as_ref().expect("types");
        assert!(types.contains_key("Atom::Badge") && !types.contains_key("Badge"), "{:?}", types.keys());
        let text = serde_json::to_string(&(&orbital.entity, &orbital.traits)).expect("json");
        assert!(text.contains(r#""values":["Atom::Badge"]"#) && text.contains(r#""type":"Atom::Badge""#), "{text}");
        assert!(!text.contains(r#""Badge""#), "{text}");
    }

    #[test]
    fn control_an_orbital_level_uses_keeps_the_consumers_own_same_named_type_unqualified() {
        let orbital = resolve_typed(Some("Atom"), json!({"Badge": [{"name": "code", "type": "string"}]}));
        let types = orbital.types.as_ref().expect("types");
        assert!(types.contains_key("Badge") && types.contains_key("Atom::Badge"), "{:?}", types.keys());
        assert_eq!(types["Badge"][0].name, "code");
    }
    fn parsing_upstream() -> Orbital {
        serde_json::from_value(json!({
            "name": "UpOrbital",
            "id": "orb_UPSTREAMPARSINGORBITAL00",
            "types": {"Badge": [{"name": "id", "type": "string"}]},
            "entity": {"name": "Widget", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true},
                {"name": "draft", "type": "string"}]},
            "traits": [{"name": "A", "linkedEntity": "Widget",
              "config": {"view": {"type": "render-ui", "default": {"type": "box", "schema": ["json/parse", "@entity.draft", "Badge"]}}},
              "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}],
                "events": [{"key": "READ"}],
                "transitions": [{"from": "idle", "to": "idle", "event": "READ", "effects": [
                    ["render-ui", "main", {"type": "box", "children": [["if", ["=", "@entity.draft", ""], {"type": "empty-state"}, {"type": "flow-canvas", "schema": ["json/parse", "@entity.draft", "Badge"]}]]}],
                    ["set", "@entity.draft", ["json/parse", "@entity.draft", "Badge"]]]}]}}]
        }))
        .expect("parsing upstream fixture")
    }

    fn resolve_parsing(behavior: &str, consumer_types: serde_json::Value) -> Orbital {
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &parsing_upstream());
        ctx.set_behavior_name("Up", behavior.to_string());
        let mut orbital: Orbital = serde_json::from_value(json!({
            "name": "Local", "types": consumer_types, "entity": "Up.orbitals.UpOrbital.entity", "traits": []
        }))
        .expect("consumer");
        let obj = OrbitalRefObject { reference: "Up.orbitals.UpOrbital".to_string(), ..Default::default() };
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        orbital
    }

    #[test]
    fn an_orbital_level_uses_qualifies_a_json_parse_shape_in_effects_and_pattern_props() {
        let orbital = resolve_parsing("Atom", json!(null));
        let text = serde_json::to_string(&orbital.traits).expect("json");
        assert_eq!(text.matches(r#""Atom::Badge""#).count(), 3, "{text}");
        assert_eq!(text.matches(r#""Badge""#).count(), 0, "{text}");
    }

    #[test]
    fn control_an_orbital_level_uses_leaves_a_json_parse_shape_of_an_undeclared_name_alone() {
        let mut upstream = parsing_upstream();
        let text = serde_json::to_string(&upstream).expect("json").replace("\"Badge\"]", "\"Nowhere\"]");
        upstream = serde_json::from_str(&text).expect("fixture");
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &upstream);
        ctx.set_behavior_name("Up", "Atom".to_string());
        let mut orbital: Orbital = serde_json::from_value(json!({
            "name": "Local", "entity": "Up.orbitals.UpOrbital.entity", "traits": []
        }))
        .expect("consumer");
        let obj = OrbitalRefObject { reference: "Up.orbitals.UpOrbital".to_string(), ..Default::default() };
        resolve_orbital_reference(
            &mut orbital, &obj, &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(), None, &mut Vec::new(), None, &mut Vec::new(),
        )
        .expect("resolves");
        let out = serde_json::to_string(&orbital.traits).expect("json");
        assert!(out.contains(r#""Nowhere""#) && !out.contains("Atom::Nowhere"), "{out}");
    }

    fn parameterised_upstream(with_params: bool) -> Orbital {
        let mut def = json!({
            "name": "UpOrbital",
            "id": "orb_UPSTREAMPARAMORBITAL0000",
            "types": {"Badge": [{"name": "id", "type": "string"}]},
            "entity": {"name": "Widget", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true}]},
            "traits": [{"name": "A", "linkedEntity": "Widget", "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}],
                "events": [{"key": "SEND", "payloadSchema": [{"name": "payload", "type": if with_params { "$p" } else { "Badge" }}]}],
                "transitions": [{"from": "idle", "to": "idle", "event": "SEND"}]}}]
        });
        if with_params {
            def["traits"][0]["typeParams"] = json!([{"name": "p", "kind": "Type"}]);
            def["traits"][0]["typeArgs"] = json!({"p": "Badge"});
        }
        serde_json::from_value(def).expect("parameterised upstream fixture")
    }

    fn resolve_parameterised(behavior: &str, with_params: bool) -> Orbital {
        let mut ctx = InlineContext::new();
        ctx.add_alias("Up", &parameterised_upstream(with_params));
        ctx.set_behavior_name("Up", behavior.to_string());
        let mut orbital: Orbital = serde_json::from_value(json!({
            "name": "Local", "entity": "Up.orbitals.UpOrbital.entity", "traits": []
        }))
        .expect("consumer");
        let obj = OrbitalRefObject { reference: "Up.orbitals.UpOrbital".to_string(), ..Default::default() };
        resolve_orbital_reference(
            &mut orbital,
            &obj,
            &ctx,
            &orbital_core_base::collections::IndexMap::new(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashMap::new(),
            None,
            &mut Vec::new(),
            None,
            &mut Vec::new(),
        )
        .expect("resolves");
        crate::phases::inline::rewrite::resolve_orbital_type_param_sentinels(&mut orbital);
        orbital
    }

    #[test]
    fn an_orbital_level_uses_keeps_a_traits_type_argument_bound_to_the_qualified_type() {
        let orbital = resolve_parameterised("Atom", true);
        let text = serde_json::to_string(&orbital.traits).expect("json");
        assert!(!text.contains("$p"), "{text}");
        assert!(text.contains(r#""payloadSchema":[{"name":"payload""#), "{text}");
        assert!(text.contains(r#""name":"id""#), "{text}");
    }

    #[test]
    fn control_an_orbital_level_uses_of_a_trait_without_type_parameters_is_unchanged() {
        let orbital = resolve_parameterised("Atom", false);
        let text = serde_json::to_string(&orbital.traits).expect("json");
        assert!(text.contains(r#""type":"Atom::Badge""#), "{text}");
        assert!(!text.contains("typeArgs"), "{text}");
    }
}
