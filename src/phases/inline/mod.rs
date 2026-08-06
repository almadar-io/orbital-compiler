//! Inline Phase
//!
//! Resolves `uses` declarations and inlines external references.
//!
//! This phase runs between Enrich and Resolve. It:
//! 1. Processes `uses` declarations in each orbital
//! 2. Loads external .orb files via the loader
//! 3. Replaces references with inline definitions
//!
//! Reference formats:
//! - `"Alias.entity"` → Entity from the aliased orbital
//! - `"Alias.traits.TraitName"` → Trait by name from aliased orbital
//! - `"Alias.pages.PageName"` → Page by name from aliased orbital
//! - `"Alias.theme"` → Theme from aliased schema
//! - `"Alias.services.ServiceName"` → Service by name from aliased orbital

mod context;
mod entity;
mod identity_normalize;
mod page;
mod rewrite;
mod service;
mod splice;
#[path = "trait.rs"]
mod r#trait;

// V4-W4 pre-inline identity normalization — rewrite id-carrying entity tokens to
// their current declaration name so a post-stamp rename resolves through the
// name-based inliner + validators (compiled-path twin of the runtime's
// `resolveEntityTokensById`). Called from `phases::resolve_schema_with_path`.
pub use identity_normalize::normalize_identity_tokens;
use identity_normalize::rewrite_trait_embed_tokens;

// Re-exports so the orchestration fns below — and the in-module
// `#[cfg(test)] mod tests { use super::*; }` block — keep resolving every helper
// under its original unqualified name after the split into submodules.
use context::{parse_reference, InlineContext, ParsedReference};
use entity::{inline_entity, merge_imported_entity_fields};
use page::inline_pages;
use r#trait::inline_traits;
// Re-exported at the `inline` namespace so validation passes (e.g.
// `validation::entity_mutation`) can reach it as
// `crate::phases::inline::collect_sibling_trait_refs`. The fn lives in the
// private `r#trait` submodule as `pub(crate)`; without this re-export the path
// is E0425 (latent — surfaces whenever orbital-compiler recompiles).
pub(crate) use r#trait::collect_sibling_trait_refs;
// Re-exported so `phases::validation::trait_ref_config` (and
// `inline_schema_with_config_diagnostics`'s public signature above) can name
// the type as `crate::phases::inline::TraitRefConfigDiagnostic` without
// reaching into the private `r#trait` submodule directly.
pub use r#trait::TraitRefConfigDiagnostic;
use rewrite::{apply_inline_config_defaults, trait_ref_name};
use service::inline_services;
use splice::splice_lambda_trait_refs;

// Test-only helpers exercised directly by the unit tests below; gated so a
// non-test build doesn't flag them as unused.
#[cfg(test)]
use r#trait::apply_overrides_to_trait;
#[cfg(test)]
use rewrite::{rewrite_entity_refs, rewrite_identifiers, rewrite_trait_event_fields};

use crate::loader::{
    discover_registry_roots, ExternalOrbitalLoader, ImportChain, LoaderConfig, LoaderError,
};
use orbital_core::schema::{
    ConfigProvenanceRecord, EntityDefinition, EntityRef, Orbital, OrbitalSchema, SchemaMetadata,
    ThemeRef, TraitReference,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use thiserror::Error;

// orbital_core schema types referenced only by the `tests` module via
// `use super::*`. Gated so a non-test build doesn't flag them as unused.
#[cfg(test)]
use orbital_core::schema::{
    EntityRefExtends, FieldDefinition, PageRef, SExpression, ServiceDefinition, ServiceRef,
    ServiceRefObject, TraitDefinition,
};

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum InlineError {
    #[error("Loader error: {0}")]
    Loader(#[from] LoaderError),

    #[error("Invalid reference format: {0}")]
    InvalidReference(String),

    #[error("Unknown alias: {0}")]
    UnknownAlias(String),

    #[error("Entity not found in orbital '{0}'")]
    EntityNotFound(String),

    #[error("Trait '{0}' not found in orbital '{1}'")]
    TraitNotFound(String, String),

    #[error("Page '{0}' not found in orbital '{1}'")]
    PageNotFound(String, String),

    #[error("Service '{0}' not found in orbital '{1}'")]
    ServiceNotFound(String, String),

    #[error("Theme not found in schema '{0}'")]
    ThemeNotFound(String),

    #[error(
        "Trait '{name}' has multiple references with conflicting names at {path}. \
         Provide a `name` override on at least one reference to disambiguate."
    )]
    DuplicateTraitName {
        /// The colliding trait name. Carried verbatim so callers can match
        /// on the conflict without reparsing the formatted message.
        name: String,
        /// Source-tagged path to the conflicting trait reference, e.g.
        /// `orbitals[name=CartItemOrbital].traits[5]`. Empty when the
        /// error originated outside an orbital context (test fixtures).
        /// Consumed by `orbital-cli`'s validation JSON output so the
        /// agent's dispatcher can route UPDATE_SPEC by orbital owner.
        path: String,
    },

    #[error(
        "events: {{ \"{from}\": ... }} override on trait reference at {path} names source key \
         '{from}' which does not exist in the imported trait '{trait_name}'. The rename silently \
         no-ops. Use one of the trait's declared event keys."
    )]
    EventRenameUnknownKey {
        /// The unknown source (`from`) key the rename map tried to rewrite.
        from: String,
        /// The imported trait's resolved name (post-`name:` rename, if any).
        trait_name: String,
        /// Source-tagged path to the offending trait reference.
        path: String,
    },

    #[error(
        "Trait '{name}' keeps its own state machine (multiple states, listens, ticks, guards, \
         or non-render effects) and cannot be expanded per list item at {path} — per-item \
         interactive traits are not supported; use the list pattern's item events"
    )]
    LambdaStatefulTrait {
        /// The referenced trait that failed the structural render-only test.
        name: String,
        /// Source-tagged path to the host trait whose lambda references it.
        path: String,
    },

    #[error(
        "Emit '{event}' from inline-expanded trait '{name}' collides with an emit already \
         declared on trait '{host}' with a different payload shape at {path} — rename one \
         or align the payload declarations"
    )]
    EmitMergeConflict {
        /// The colliding event name.
        event: String,
        /// The spliced wrapper trait contributing the emit.
        name: String,
        /// The host trait the emit was being merged into.
        host: String,
        /// Source-tagged path to the host trait.
        path: String,
    },
}

impl InlineError {
    /// Structured path to the source-position that produced this error,
    /// when the variant carries one. Returns an empty string for
    /// variants that don't yet have a path (gap closure ongoing).
    pub fn structured_path(&self) -> &str {
        match self {
            InlineError::DuplicateTraitName { path, .. } => path,
            InlineError::EventRenameUnknownKey { path, .. } => path,
            InlineError::LambdaStatefulTrait { path, .. } => path,
            InlineError::EmitMergeConflict { path, .. } => path,
            _ => "",
        }
    }

    /// Stable diagnostic code for this inline-phase error, mirroring the
    /// validator's `ErrorCode::as_str()` vocabulary. Variants without a
    /// dedicated code fall back to the generic `INLINE_ERROR` tag so the CLI
    /// JSON output carries a concrete `ORB_*` code where one exists.
    pub fn structured_code(&self) -> &'static str {
        match self {
            InlineError::EventRenameUnknownKey { .. } => "ORB_T_EVENT_RENAME_UNKNOWN_KEY",
            InlineError::DuplicateTraitName { .. } => "ORB_T_DUPLICATE_NAME",
            InlineError::LambdaStatefulTrait { .. } => "ORB_T_LAMBDA_STATEFUL_TRAIT",
            InlineError::EmitMergeConflict { .. } => "ORB_T_EMIT_MERGE_CONFLICT",
            _ => "INLINE_ERROR",
        }
    }
}

// ============================================================================
// Main Inline Function
// ============================================================================

/// Inline configuration
#[derive(Debug, Clone, Default)]
pub struct InlineConfig {
    /// Loader configuration
    pub loader_config: LoaderConfig,
    /// Whether to skip inlining (for testing)
    pub skip_inline: bool,
}

/// Inline all external references in a schema
pub fn inline_schema(
    schema: &mut OrbitalSchema,
    config: &InlineConfig,
    source_path: Option<&Path>,
) -> Result<(), InlineError> {
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(schema, config, source_path, &mut diagnostics)
}

/// Same as [`inline_schema`], but also collects every trait Reference whose
/// call-site `config` doesn't exactly match the referenced trait's declared
/// config knobs (unknown keys / missing `!`-required keys). Consumed by
/// `orb validate`'s call-site config rules
/// (`ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY` / `ORB_T_CONFIG_MISSING_REQUIRED_KEY`)
/// — collection has to happen during inlining because
/// `apply_overrides_to_trait` folds matched keys into the resolved trait and
/// silently drops unmatched ones, so the raw call-site key set is gone by
/// the time `validate_comprehensive_full` runs.
pub fn inline_schema_with_config_diagnostics(
    schema: &mut OrbitalSchema,
    config: &InlineConfig,
    source_path: Option<&Path>,
    diagnostics: &mut Vec<r#trait::TraitRefConfigDiagnostic>,
) -> Result<(), InlineError> {
    if config.skip_inline {
        return Ok(());
    }

    // Intrinsic identity, belt-and-suspenders: `resolve_schema_with_path`
    // already stamps before calling inline, but direct `inline_schema` callers
    // (unit tests, tooling) may hand in a ledger-less schema. Stamp here too so
    // the id side-maps (`trait_embed_ids`/`entity_ref_ids`) always cover their
    // tokens by the time the cross-orbital dedup rename runs — the invariant
    // that lets the name-fallback rewriters be deleted. No-op when stamped.
    orbital_core::stamp::ensure_stamped(schema);

    // V4-W4 identity normalization (compiled-path twin of the runtime's id-first
    // resolution). Rewrite every id-carrying reference token — entity tokens
    // (`@entity`/`fetch`/`persist`/`linkedEntity` via `entityRefIds`/
    // `linkedEntityId`), `@trait.<x>` embeds (via `traitEmbedIds`), and page trait
    // `ref`s (via `refId`) — to the CURRENT declaration name, so a post-stamp
    // rename (declaration + ledger updated, tokens kept canonical) resolves
    // through the name-based inline loop + the downstream validators. Runs on the
    // orbital's OWN inline traits before the import inline loop below (imports
    // carry their own rebind path). Additive: no side-map / no rename →
    // byte-identical passthrough. This is what lets the factory stop rewriting
    // tokens (`rebindInlineTraitEntity` / `applyTraitRenames`).
    normalize_identity_tokens(schema);

    // Auto-discover behavior-package registry roots from the source path when
    // none are configured, so bare imports like `almadar-behaviors/rpg-hero`
    // resolve against the workspace's emitted `behaviors/registry/` trees.
    let mut loader_config = config.loader_config.clone();
    if loader_config.auto_discover_registries && loader_config.behaviors_registry_roots.is_empty() {
        if let Some(src) = source_path {
            for (scope, root) in discover_registry_roots(src) {
                loader_config.behaviors_registry_roots.insert(scope, root);
            }
        }
    }

    let mut loader = ExternalOrbitalLoader::new(loader_config);
    let mut chain = ImportChain::new();

    // If we have a source path, add it to the chain
    if let Some(path) = source_path {
        chain.push(path);
    }

    // Process each orbital, recording the names the auto-pull loop added and
    // the config→knob provenance for substituted render-ui props.
    let mut pulled_by_orbital: Vec<(usize, Vec<String>)> = Vec::new();
    let mut all_provenance: HashMap<String, Vec<ConfigProvenanceRecord>> = HashMap::new();
    for (idx, orbital) in schema.orbitals.iter_mut().enumerate() {
        let mut pulled: Vec<String> = Vec::new();
        let mut provenance: Vec<ConfigProvenanceRecord> = Vec::new();
        inline_orbital(
            orbital,
            &mut loader,
            &mut chain,
            source_path,
            true,
            Some(&mut pulled),
            &mut provenance,
            diagnostics,
        )?;
        if !pulled.is_empty() {
            pulled_by_orbital.push((idx, pulled));
        }
        if !provenance.is_empty() {
            all_provenance.insert(orbital.name.clone(), provenance);
        }
    }

    uniquify_cross_orbital_pulled_siblings(schema, &pulled_by_orbital);

    // Preserve cross-file identity: copy the ledger rows of any source id that a
    // cloned node still references but the consumer arena/ledger doesn't carry.
    // Runs after all orbitals inline (so every reference position is final) and
    // uses the loader's caches as the source-ledger source of truth.
    merge_source_ledgers(schema, &loader.source_ledger_union());

    // Emit the config→knob provenance into `_metadata.configProvenance` so a
    // consumer (Studio contextual edit) can trace a substituted render-ui
    // literal back to the config knob it came from.
    if !all_provenance.is_empty() {
        match schema.metadata.as_mut() {
            Some(meta) => meta.config_provenance = Some(all_provenance),
            None => {
                schema.metadata = Some(SchemaMetadata {
                    version: None,
                    created_at: None,
                    source: None,
                    updated_at: None,
                    config_provenance: Some(all_provenance),
                });
            }
        }
    }

    Ok(())
}

/// Make auto-pulled sibling traits app-unique across orbitals.
///
/// An auto-pulled sibling keeps its source name within its orbital so the
/// pulling trait's `@trait.<Sibling>` reference still resolves. When two
/// orbitals compose the SAME source atom, each pulls a sibling with the same
/// name, colliding app-wide (`ORB_T_DUPLICATE_NAME`). Trait names must be
/// unique per app, so rename the colliding pulled siblings — prefixing with the
/// owning orbital name — and rewrite that orbital's `@trait.<old>` literals so
/// the reference tracks the rename. Only renames a pulled sibling when its name
/// also appears (pulled or explicit) in another orbital, so single-use pulls
/// keep their clean source name.
fn uniquify_cross_orbital_pulled_siblings(
    schema: &mut OrbitalSchema,
    pulled_by_orbital: &[(usize, Vec<String>)],
) {
    if pulled_by_orbital.is_empty() {
        return;
    }
    // Count how many orbitals declare each trait name (any kind).
    let mut name_orbital_count: HashMap<String, usize> = HashMap::new();
    for orbital in &schema.orbitals {
        let mut local: HashSet<String> = HashSet::new();
        for tr in &orbital.traits {
            if let Some(n) = trait_ref_name(tr) {
                local.insert(n.to_string());
            }
        }
        for n in local {
            *name_orbital_count.entry(n).or_default() += 1;
        }
    }
    // Stamp-before-rename: a display rename applies to the DECLARATION + the
    // ledger. The first copy of a stamped pulled sibling keeps its id (ledger
    // row renamed along); every ADDITIONAL copy of the same source trait is a
    // distinct declaration and gets its own deterministic identity
    // (`derive_id(source_id, orbital_name)` + a cloned ledger row) — one ledger
    // row cannot carry two curNames.
    let mut id_claimed: HashSet<String> = HashSet::new();
    for (orbital_idx, pulled) in pulled_by_orbital {
        let orbital_name = schema.orbitals[*orbital_idx].name.clone();
        let mut renames: HashMap<String, String> = HashMap::new();
        // (source_id, new_name, derived_copy_id) — applied to the ledger after
        // the trait borrow ends.
        let mut ledger_ops: Vec<(String, String, Option<String>)> = Vec::new();
        for old_name in pulled {
            if name_orbital_count.get(old_name).copied().unwrap_or(0) <= 1 {
                continue;
            }
            let new_name = format!("{}{}", orbital_name, old_name);
            if name_orbital_count.contains_key(&new_name) {
                continue;
            }
            let orbital = &mut schema.orbitals[*orbital_idx];
            for tr in orbital.traits.iter_mut() {
                if let TraitReference::Inline(td) = tr {
                    if td.name == *old_name {
                        td.name = new_name.clone();
                        if let Some(source_id) = td.id.clone() {
                            if id_claimed.insert(source_id.clone()) {
                                ledger_ops.push((source_id, new_name.clone(), None));
                            } else {
                                let copy_id =
                                    orbital_core::identity::derive_id(&source_id, &orbital_name);
                                td.id = Some(copy_id.clone());
                                ledger_ops.push((source_id, new_name.clone(), Some(copy_id)));
                            }
                        }
                    }
                    // A sibling-listening parent in the same orbital points its
                    // `listens` source at the old sibling name; track the rename.
                    if let Some(listens) = td.listens.as_mut() {
                        for listen in listens.iter_mut() {
                            if let Some(orbital_core::schema::ListenSource::Trait {
                                trait_name,
                                ..
                            }) = listen.source.as_mut()
                            {
                                if trait_name == old_name {
                                    *trait_name = new_name.clone();
                                }
                            }
                        }
                    }
                }
            }
            renames.insert(old_name.clone(), new_name);
        }
        if !ledger_ops.is_empty() {
            if let Some(ledger) = schema.ledger.as_mut() {
                let at = orbital_core::time_util::now_rfc3339();
                for (source_id, new_name, copy_id) in &ledger_ops {
                    match copy_id {
                        None => ledger.rename(source_id, new_name, &at),
                        Some(copy_id) => {
                            if let Some(src) = ledger.entries.get(source_id).cloned() {
                                let mut rename_hist = src.renames;
                                rename_hist.push(orbital_core::identity::RenameRecord {
                                    from: src.cur_name,
                                    to: new_name.clone(),
                                    at: at.clone(),
                                });
                                ledger.entries.insert(
                                    copy_id.clone(),
                                    orbital_core::identity::LedgerEntry {
                                        id: copy_id.clone(),
                                        kind: src.kind,
                                        baked_name: src.baked_name,
                                        cur_name: new_name.clone(),
                                        renames: rename_hist,
                                        owner: src.owner,
                                        parent: src.parent,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        // Rewrite the `@trait.<old>` literals this rename orphaned — in effect
        // trees AND config-field defaults. Id-first resolution cannot cover
        // these: a pulled sibling that entered the atom as a trait REFERENCE
        // (std-browse's `DataGrid1`/`DenseTableView` → ui-* atoms) has no local
        // declaration id at stamp time, so the referrer's `trait_embed_ids`
        // never carries the token and the name IS the only handle the binding
        // validator + runtime resolve by.
        if !renames.is_empty() {
            for tr in schema.orbitals[*orbital_idx].traits.iter_mut() {
                if let TraitReference::Inline(td) = tr {
                    rewrite_trait_embed_tokens(td, &renames);
                }
            }
        }
    }
}

/// Inline all references in an orbital.
///
/// `is_top_level` distinguishes the consumer-facing entry call (true) from
/// recursive calls used to load imports (false). When loading an import for a
/// caller, we MUST NOT pre-substitute the imported atom's `@config.X` bindings
/// with its own declared defaults — the caller's `apply_overrides_to_trait`
/// pass needs the raw `@config.X` strings present so its override values can
/// replace them. Eager default substitution at import-load time clobbers the
/// override surface for any field that has a default declared on the atom.
fn inline_orbital(
    orbital: &mut Orbital,
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
    is_top_level: bool,
    // When set (top-level orbitals only), receives the names of traits added by
    // the auto-pull loop so `inline_schema` can uniquify cross-orbital collisions.
    pulled_names_sink: Option<&mut Vec<String>>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
    config_diagnostics: &mut Vec<r#trait::TraitRefConfigDiagnostic>,
) -> Result<(), InlineError> {
    // Skip the import-loading + reference-substitution machinery when
    // the orbital has no `uses[]` declarations (e.g. an atom compiled
    // standalone). Inline traits still need their own declared
    // `@config.X` defaults substituted before codegen, since
    // codegen/guard.rs `OirBindingRoot::Config` emits `undefined` on
    // the assumption the inline phase resolved every config binding.
    // Run that one substitution pass ONLY when this orbital is the
    // top-level compilation target (atom-standalone codegen). When the
    // orbital is being loaded as an import, defer the substitution so
    // the consumer's overrides can replace `@config.X` strings first.
    let uses = match &orbital.uses {
        Some(uses) if !uses.is_empty() => uses.clone(),
        _ => {
            if is_top_level {
                for trait_ref in orbital.traits.iter_mut() {
                    if let TraitReference::Inline(trait_def) = trait_ref {
                        apply_inline_config_defaults(trait_def, recorder);
                    }
                }
                // Lambda-scope splice runs on the top-level target only —
                // AFTER config-default substitution so wrapper templates
                // carry the author's expressions, and only once every
                // `@config.X` is final (an import load defers substitution,
                // so splicing there would freeze pre-override templates).
                splice_lambda_trait_refs(orbital, &format!("orbitals[name={}]", orbital.name))?;
            }
            return Ok(());
        }
    };

    // Build context by loading all imports.
    let mut ctx = InlineContext::new();
    // When this orbital is itself an import (not the top-level compilation
    // target), defer `@config.X` substitution in resolved traits so a downstream
    // consumer can still override those bindings. Substituting here with this
    // import's own defaults would bake the value in and clobber the consumer's
    // override surface (the variation → base → consumer config-override path).
    ctx.defer_config_subst = !is_top_level;

    for use_decl in &uses {
        // Phase F.10: recursive composition. The loaded orbital may itself
        // declare `uses[]` and reference traits from yet another orbital
        // (e.g. organism → molecule → atom). Without recursive inlining
        // here, the consumer's `get_trait` lookup would find the loaded
        // orbital's traits as `TraitReference::Reference` (still pointing
        // at the deeper layer) and pass them through unchanged, breaking
        // the chain. We clone every loaded orbital + remember the source
        // path, then recursively call `inline_orbital` so each clone has
        // its own references already inlined before the consumer queries.
        //
        // The loader cache remains the source of truth for raw parsed
        // schemas; the inlined clones exist only in this consumer's
        // context.
        //
        // Multi-orbital fix: organisms (and any behavior that publishes
        // multiple orbitals in one `.orb`) declare traits/pages across
        // orbitals 0..N. Previously this loop took only `orbitals.first()`,
        // so any ref targeting a trait in orbital 1+ failed to resolve with
        // "Trait X not found in orbital Y". We now clone ALL orbitals and
        // stash them in the context, so `get_trait` / `get_page` /
        // `get_service` can iterate every orbital.
        let (mut inlined_orbitals, loaded_source) = {
            let loaded = loader.load(&use_decl.from, source_path, chain)?;
            if loaded.schema.orbitals.is_empty() {
                // Defensive: a valid imported `.orb` always has at least one
                // orbital (the parser rejects empty ones), so this branch
                // should be unreachable. Skip the alias rather than panic.
                continue;
            }
            (loaded.schema.orbitals.clone(), loaded.source_path.clone())
        };

        for inlined_orbital in &mut inlined_orbitals {
            inline_orbital(
                inlined_orbital,
                loader,
                chain,
                Some(&loaded_source),
                false,
                None,
                recorder,
                config_diagnostics,
            )?;
        }

        // The source label used in error messages is the `from` string from
        // the `uses` declaration (e.g. `"std/behaviors/std-arcade-game"`).
        // That is what the author wrote, so it is the actionable label when
        // a trait/page lookup fails.
        ctx.add_alias_multi(&use_decl.alias, inlined_orbitals, use_decl.from.clone());
    }

    // ----------------------------------------------------------------
    // Auto-merge imported entity fields (GAP-AGB-MOLECULE-ENTITY-CONTRACT)
    // ----------------------------------------------------------------
    // When the caller's orbital has trait refs pointing at imported behaviors
    // via `linkedEntity`, auto-merge the imported behavior's canonical entity
    // fields INTO the caller's entity. This runs BEFORE entity resolution and
    // trait inlining so the full field set is available when @entity bindings
    // are checked.
    //
    // Merge rule: for each field in the imported behavior's canonical entity,
    // if the caller's entity doesn't already have a field with that name, add
    // it. Caller's fields win on name collision.
    if let EntityRef::Inline(ref mut caller_entity) = orbital.entity {
        // Collect (alias, trait_name) pairs from trait refs of the form
        // "Alias.traits.TraitName". Use a set to avoid merging the same
        // import's entity twice when multiple traits reference it.
        //
        // Two cases carry the imported entity's fields onto the caller entity:
        //   (a) an explicit `linkedEntity` rebind to the caller entity, and
        //   (b) NO rebind, but the imported trait's own bound entity NAME
        //       equals the caller entity name (the consumer re-declares an
        //       entity with the same name and composes the trait over it).
        // Case (b) is the depth-3 propagation fix: an organism that imports a
        // self-contained widget atom WITHOUT rebinding still reads the trait's
        // INTRINSIC `@entity.X` fields against its own same-named entity, so
        // those machinery fields must travel onto it.
        let caller_entity_name = caller_entity.name.clone();
        let mut merged_aliases: HashSet<(String, String)> = HashSet::new();

        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity,
                ref_id,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    if let Some(imported_entity) =
                        ctx.get_entity_for_trait_orbital(&alias, &name, ref_id.as_deref())
                    {
                        let (carries_fields, intrinsic_only) = match linked_entity {
                            // (a) explicit rebind: merge the full field set
                            // (long-standing behavior, unchanged).
                            Some(_) => (true, false),
                            // (b) no rebind: carry only INTRINSIC machinery when
                            // the imported trait's bound entity name matches the
                            // caller's. Genuine DATA fields are NOT merged here —
                            // the consumer re-declared its own entity and owns
                            // them; a missing DATA field surfaces as an error.
                            None => (imported_entity.name == caller_entity_name, true),
                        };
                        if carries_fields && merged_aliases.insert((alias.clone(), name.clone())) {
                            // A required DATA field (in the trait's
                            // `entity_contract.requires`) must be satisfied by
                            // the target, NOT auto-merged from the source — that
                            // would mask a genuinely-missing field. Skip those so
                            // ORB_T_REBIND_MISSING_FIELDS can fire. Intrinsic +
                            // provided fields still carry.
                            let resolved_trait = ref_id
                                .as_deref()
                                .and_then(|rid| ctx.get_trait_by_ref_id(&alias, rid))
                                .or_else(|| ctx.get_trait(&alias, &name).ok());
                            let required_data = resolved_trait
                                .and_then(|tr| match tr {
                                    TraitReference::Inline(td) => td.entity_contract,
                                    _ => None,
                                })
                                .map(|c| c.requires.into_iter().collect::<HashSet<String>>())
                                .unwrap_or_default();
                            merge_imported_entity_fields(
                                caller_entity,
                                &imported_entity,
                                intrinsic_only,
                                &required_data,
                            );
                        }
                    }
                }
            }
        }
    }

    // ----------------------------------------------------------------
    // Auxiliary entities (Gap #22): when a trait reference imports an
    // atom WITHOUT rebinding linkedEntity (no `-> Entity` at the call
    // site), the imported atom's entity stays the trait's bound entity.
    // The molecule's primary `entity` field can only carry one entity,
    // so surface the imported atom entity in `auxiliary_entities` so
    // validators / codegen / runtime can resolve `linkedEntity =
    // "SearchResult"` lookups, register persistence, and emit the
    // TypeScript type. Dedup by entity name across all no-rebind imports
    // — a molecule that imports two no-rebind traits sharing the same
    // entity gets a single registration.
    {
        let mut seen_aux_names: HashSet<String> = HashSet::new();
        // Seed with the primary entity name so we never duplicate it as
        // an auxiliary (the primary is already in the orbital's entity
        // field — the auxiliary list is strictly for additional ones).
        if let EntityRef::Inline(ref primary_entity) = orbital.entity {
            seen_aux_names.insert(primary_entity.name.clone());
        }
        let mut aux_collected: Vec<EntityDefinition> = Vec::new();
        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity: None,
                ref_id,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    if let Some(imported_entity) =
                        ctx.get_entity_for_trait_orbital(&alias, &name, ref_id.as_deref())
                    {
                        if seen_aux_names.insert(imported_entity.name.clone()) {
                            aux_collected.push(imported_entity);
                        }
                    }
                }
            }
        }
        if !aux_collected.is_empty() {
            let existing = orbital.auxiliary_entities.take().unwrap_or_default();
            let mut combined = existing;
            for entity in aux_collected {
                combined.push(EntityRef::Inline(entity));
            }
            orbital.auxiliary_entities = Some(combined);
        }
    }

    // Inline entity
    orbital.entity = inline_entity(&orbital.entity, &ctx)?;

    // Inline traits
    // Source-tag every InlineError raised below with the orbital's
    // location in the composed schema so `orbital-cli` can populate the
    // ValidationJsonError's `path` field. Downstream rabit's
    // `dispatch-updates` walker reads this path to route UPDATE_SPEC to
    // the right subagent.
    let orbital_path_prefix = format!("orbitals[name={}]", orbital.name);
    // Consumer-scope entity name → dual-carry id map, so a `linkedEntity` rebind
    // rewrites the cloned trait's id sibling to the consumer's entity node (the
    // primary + any auxiliary entities declared on this orbital, all id-stamped
    // by L1 before the compiler runs).
    let entity_ids = collect_orbital_entity_ids(orbital);
    let (
        inlined_traits,
        // V4: the returned name→name map was consumed only by the deleted
        // `rewrite_page_trait_refs`; page refs now resolve by `ref_id`. The
        // trait-declaration renames still happen INSIDE `inline_traits`.
        _trait_rename_map,
        pulled_aux_entities,
        pulled_sibling_names,
        trait_config_diagnostics,
    ) = inline_traits(
        &orbital.traits,
        &ctx,
        &orbital_path_prefix,
        &entity_ids,
        recorder,
    )?;
    orbital.traits = inlined_traits;
    config_diagnostics.extend(trait_config_diagnostics);
    if let Some(sink) = pulled_names_sink {
        *sink = pulled_sibling_names;
    }

    // Register auxiliary entities surfaced by no-rebind SIBLING-PULLS. A
    // self-contained widget trait pulled transitively via `@trait.<Sibling>`
    // (e.g. a step-flow embedded in an app atom) keeps its own runtime view
    // entity (`ProgressVerticalView`); that entity is auxiliary in the source
    // behavior and must be carried here or the trait's `(fetch X)` /
    // `linkedEntity` dangle (ORB_EFF_FETCH_INVALID_ENTITY / ORB_E_INVALID_RELATION).
    // The pre-`inline_traits` aux pass only saw direct trait refs, so it missed
    // these. Dedup against the primary + already-registered auxiliaries.
    if !pulled_aux_entities.is_empty() {
        let mut known: HashSet<String> = HashSet::new();
        if let EntityRef::Inline(e) = &orbital.entity {
            known.insert(e.name.clone());
        }
        let mut combined = orbital.auxiliary_entities.take().unwrap_or_default();
        for a in &combined {
            if let EntityRef::Inline(e) = a {
                known.insert(e.name.clone());
            }
        }
        for e in pulled_aux_entities {
            if known.insert(e.name.clone()) {
                combined.push(EntityRef::Inline(e));
            }
        }
        if !combined.is_empty() {
            orbital.auxiliary_entities = Some(combined);
        }
    }

    // Inline pages. V4 intrinsic stamping: a `pages[].traits[]` ref that still
    // carries a trait's ORIGINAL name after an `inline_traits` rename is
    // resolved by its stable `ref_id` — the OIR page binding (resolve.rs) takes
    // the trait's current name by id, and the page validator accepts an id that
    // resolves in `trait_ids` — so no page-ref token rewrite is needed. The
    // former `rewrite_page_trait_refs` is deleted.
    orbital.pages = inline_pages(&orbital.pages, &ctx)?;

    // Inline services
    orbital.services = inline_services(&orbital.services, &ctx)?;

    // Lambda-scope splice for render-only trait references (host-context prop
    // evaluation). Runs AFTER trait inlining + sibling auto-pull so every
    // wrapper is a fully substituted `TraitReference::Inline`, and only on the
    // top-level target (imports defer `@config.X` substitution, so their
    // templates are not final yet — the consumer's own top-level pass splices
    // them after its overrides apply).
    if is_top_level {
        splice_lambda_trait_refs(orbital, &orbital_path_prefix)?;
    }

    // Inline theme if it's a reference. Only `Alias.theme` import-form
    // references are resolved here; bare built-in/inline theme names stay
    // as `Reference`s (they resolve at render via `@currentTheme`).
    if let Some(ThemeRef::Reference(ref_str)) = &orbital.theme {
        if let Ok(ParsedReference::Theme { alias }) = parse_reference(ref_str) {
            orbital.theme = Some(ctx.get_theme(&alias)?);
        }
    }

    Ok(())
}

/// Build the consumer-scope entity name → dual-carry id map for an orbital: its
/// primary inline entity plus every inline auxiliary entity. Used to keep a
/// rebound `linkedEntity` name's id sibling pointing at the consumer's own
/// entity node instead of the source file's.
fn collect_orbital_entity_ids(orbital: &Orbital) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    if let EntityRef::Inline(e) = &orbital.entity {
        if let Some(id) = &e.id {
            ids.insert(e.name.clone(), id.clone());
        }
    }
    for aux in orbital.auxiliary_entities.iter().flatten() {
        if let EntityRef::Inline(e) = aux {
            if let Some(id) = &e.id {
                ids.insert(e.name.clone(), id.clone());
            }
        }
    }
    ids
}

// ============================================================================
// Cross-file identity-ledger merge (Rabit V4)
// ============================================================================
//
// Inlining clones trait/entity nodes from imported source `.orb` schemas into
// the consumer. Those clones carry dual-carry ids minted in the SOURCE file,
// with ledger rows that live in the source schema's ledger — not the consumer's.
// Where a clone REFERENCES such a foreign id (e.g. a non-rebound `linkedEntity`
// still pointing at the source entity), the consumer's arena ∪ ledger doesn't
// know it and `ORB_ID_UNKNOWN_REF` fires. Preserve identity across the file
// seam by copying the referenced source ledger rows into the consumer ledger.
//
// Only ids that are actually referenced-but-unresolved are merged: an id that is
// a declared consumer node needs no ledger row, and merging an unreferenced
// source row would create an `ORB_ID_LEDGER_ORPHAN`. This keeps the merge
// orphan-free by construction (every merged row is carried by a live reference).

/// Copy referenced-but-unknown source ledger rows into the consumer schema's
/// ledger, preserving cross-file ids. `source_union` is the union of every
/// loaded import's ledger (`id → row`).
fn merge_source_ledgers(
    schema: &mut OrbitalSchema,
    source_union: &HashMap<String, orbital_core::identity::LedgerEntry>,
) {
    if source_union.is_empty() {
        return;
    }
    let declared = collect_declared_ids(schema);
    let referenced = collect_referenced_ids(schema);
    let have: HashSet<String> = schema
        .ledger
        .as_ref()
        .map(|l| l.entries.keys().cloned().collect())
        .unwrap_or_default();

    let mut to_add: Vec<orbital_core::identity::LedgerEntry> = Vec::new();
    for id in &referenced {
        if declared.contains(id) || have.contains(id) {
            continue;
        }
        if let Some(row) = source_union.get(id) {
            to_add.push(row.clone());
        }
    }
    if to_add.is_empty() {
        return;
    }
    let ledger = schema
        .ledger
        .get_or_insert_with(|| orbital_core::identity::IdentityLedger {
            schema_version: 1,
            entries: HashMap::new(),
        });
    for row in to_add {
        ledger.entries.entry(row.id.clone()).or_insert(row);
    }
}

/// Ids carried BY a declaration node (orbital/entity/trait/event/page/field) —
/// these are arena-known without a ledger row. Mirrors the declaration sites
/// that `validation::id_integrity::IdIndex::build` treats as arena nodes.
fn collect_declared_ids(schema: &OrbitalSchema) -> HashSet<String> {
    fn decl_entity(e: Option<&orbital_core::schema::EntityDefinition>, out: &mut HashSet<String>) {
        if let Some(id) = e.and_then(|e| e.id.as_ref()) {
            out.insert(id.clone());
        }
    }
    fn decl_trait(tr: &TraitReference, out: &mut HashSet<String>) {
        if let TraitReference::Inline(td) = tr {
            if let Some(id) = &td.id {
                out.insert(id.clone());
            }
            for emit in td.emits.iter().flatten() {
                if let Some(id) = &emit.event_id {
                    out.insert(id.clone());
                }
            }
            for de in td.data_entities.iter().flatten() {
                if let Some(id) = &de.id {
                    out.insert(id.clone());
                }
            }
        }
    }
    let mut out = HashSet::new();
    for orbital in &schema.orbitals {
        if let Some(id) = &orbital.id {
            out.insert(id.clone());
        }
        decl_entity(orbital.entity.as_inline(), &mut out);
        for aux in orbital.auxiliary_entities.iter().flatten() {
            decl_entity(aux.as_inline(), &mut out);
        }
        for tr in &orbital.traits {
            decl_trait(tr, &mut out);
        }
        for page in orbital.pages.iter().flatten() {
            if let orbital_core::schema::PageRef::Inline(p) = page {
                if let Some(id) = &p.id {
                    out.insert(id.clone());
                }
                for tr in p.traits.iter().flatten() {
                    decl_trait(tr, &mut out);
                }
            }
        }
    }
    out
}

/// Ids that appear in a REFERENCE (edge) position — the id siblings a validator
/// resolves against the arena ∪ ledger. Mirrors the reference walk in
/// `validation::id_integrity`; kept in step with it.
fn collect_referenced_ids(schema: &OrbitalSchema) -> HashSet<String> {
    use orbital_core::schema::{EntityRef, ListenSource, PageRef, TraitReference};
    let mut out = HashSet::new();
    let add = |id: &Option<String>, out: &mut HashSet<String>| {
        if let Some(id) = id {
            out.insert(id.clone());
        }
    };

    fn field_relations(field: &orbital_core::schema::FieldDefinition, out: &mut HashSet<String>) {
        if let Some(rel) = &field.relation {
            if let Some(id) = &rel.entity_id {
                out.insert(id.clone());
            }
        }
        if let Some(items) = &field.items {
            field_relations(items, out);
        }
        for (_, p) in field.properties.iter().flatten() {
            field_relations(p, out);
        }
    }

    let walk_trait_ref = |tr: &TraitReference, out: &mut HashSet<String>| match tr {
        TraitReference::Inline(td) => {
            add(&td.linked_entity_id, out);
            for listen in td.listens.iter().flatten() {
                add(&listen.event_id, out);
                add(&listen.triggers_id, out);
                match &listen.source {
                    Some(ListenSource::Trait { trait_id, .. }) => add(trait_id, out),
                    Some(ListenSource::Orbital {
                        orbital_id,
                        trait_id,
                        ..
                    }) => {
                        add(orbital_id, out);
                        add(trait_id, out);
                    }
                    Some(ListenSource::Any) | None => {}
                }
            }
            for de in td.data_entities.iter().flatten() {
                for f in &de.fields {
                    field_relations(f, out);
                }
            }
        }
        TraitReference::Reference {
            ref_id,
            linked_entity_id,
            ..
        } => {
            add(ref_id, out);
            add(linked_entity_id, out);
        }
    };

    for orbital in &schema.orbitals {
        match &orbital.entity {
            EntityRef::Extends(e) => add(&e.extends_id, &mut out),
            EntityRef::Inline(entity) => {
                for f in &entity.fields {
                    field_relations(f, &mut out);
                }
            }
            EntityRef::Reference(_) => {}
        }
        for aux in orbital.auxiliary_entities.iter().flatten() {
            match aux {
                EntityRef::Extends(e) => add(&e.extends_id, &mut out),
                EntityRef::Inline(entity) => {
                    for f in &entity.fields {
                        field_relations(f, &mut out);
                    }
                }
                EntityRef::Reference(_) => {}
            }
        }
        for tr in &orbital.traits {
            walk_trait_ref(tr, &mut out);
        }
        for page in orbital.pages.iter().flatten() {
            match page {
                PageRef::Inline(p) => {
                    for tr in p.traits.iter().flatten() {
                        walk_trait_ref(tr, &mut out);
                    }
                }
                PageRef::Object(o) => {
                    add(&o.ref_id, &mut out);
                    add(&o.linked_entity_id, &mut out);
                    if let Some(ref_ids) = &o.trait_ref_ids {
                        for rid in ref_ids {
                            if !rid.is_empty() {
                                out.insert(rid.clone());
                            }
                        }
                    }
                    for tr in o.traits.iter().flatten() {
                        walk_trait_ref(tr, &mut out);
                    }
                }
                PageRef::Reference(_) => {}
            }
        }
    }
    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures")
    }

    // ------------------------------------------------------------------
    // rewrite_entity_refs — Phase 3.5.G.A.1
    // ------------------------------------------------------------------

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }

    /// A cross-orbital pulled-sibling rename must drag every `@trait.<old>`
    /// literal with it — in transition effects AND config-field defaults. The
    /// pulled siblings enter as Reference-trait clones with no local id at
    /// stamp time, so `trait_embed_ids` cannot cover these tokens and the name
    /// is the only handle the binding validator + runtime resolve by
    /// (C-INLINE-TRAIT-CONFIG-AT-TRAIT-RENAME / C-PANEL-INLINE-TRAIT-UNKNOWN).
    #[test]
    fn uniquify_rename_rewrites_trait_tokens_in_effects_and_config_defaults() {
        let orbital = |name: &str| {
            serde_json::json!({
                "name": name,
                "entity": { "name": format!("{name}Item"), "fields": [] },
                "traits": [
                    {
                        "name": "DataGrid1",
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                        }
                    },
                    {
                        "name": format!("{name}Browse"),
                        "config": {
                            "bodyContent": {
                                "type": "render-ui",
                                "default": { "type": "stack", "children": ["@trait.DataGrid1"] }
                            }
                        },
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{
                                "from": "idle", "to": "idle", "event": "INIT",
                                "effects": [["render-ui", "main", { "type": "stack", "children": ["@trait.DataGrid1"] }]]
                            }]
                        }
                    }
                ]
            })
        };
        let mut schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "version": "2.0", "name": "T",
            "orbitals": [orbital("Alpha"), orbital("Beta")]
        }))
        .expect("valid schema");

        uniquify_cross_orbital_pulled_siblings(
            &mut schema,
            &[(0, vec!["DataGrid1".to_string()]), (1, vec!["DataGrid1".to_string()])],
        );

        let serialized = serde_json::to_string(&schema).unwrap();
        assert!(
            !serialized.contains("@trait.DataGrid1"),
            "no dangling pre-rename tokens may remain: {serialized}"
        );
        for (idx, orb_name) in [(0usize, "Alpha"), (1usize, "Beta")] {
            let renamed = format!("{orb_name}DataGrid1");
            let orb = &schema.orbitals[idx];
            assert!(
                orb.traits.iter().any(|t| matches!(t, TraitReference::Inline(td) if td.name == renamed)),
                "{orb_name}: sibling declaration renamed to {renamed}"
            );
            let host = orb
                .traits
                .iter()
                .find_map(|t| match t {
                    TraitReference::Inline(td) if td.name.ends_with("Browse") => Some(td),
                    _ => None,
                })
                .expect("host trait");
            let effects_json = serde_json::to_string(&host.state_machine.transitions[0].effects).unwrap();
            assert!(effects_json.contains(&format!("@trait.{renamed}")), "{orb_name}: effect token rewritten");
            let config_json = serde_json::to_string(&host.config).unwrap();
            assert!(config_json.contains(&format!("@trait.{renamed}")), "{orb_name}: config default rewritten");
        }
    }

    /// Stamped pulled siblings: the first renamed copy keeps its id and drags
    /// the ledger row's curName along; a second copy of the SAME source id is
    /// a distinct declaration — it gets a deterministic derived id + its own
    /// ledger row, since one row cannot carry two curNames
    /// (ORB_ID_NAME_MISMATCH storm on app organisms composing shared atoms).
    #[test]
    fn uniquify_rename_propagates_identity_per_copy() {
        let orbital = |name: &str| {
            serde_json::json!({
                "name": name,
                "entity": { "name": format!("{name}Item"), "fields": [] },
                "traits": [{
                    "name": "CloseButton",
                    "id": "trt_CLOSE0000000000000000000000",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                    }
                }]
            })
        };
        let mut schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "version": "2.0", "name": "T",
            "orbitals": [orbital("Alpha"), orbital("Beta")],
            "schemaVersion": 1,
            "ledger": {
                "schemaVersion": 1,
                "entries": {
                    "trt_CLOSE0000000000000000000000": {
                        "id": "trt_CLOSE0000000000000000000000",
                        "kind": "trait",
                        "bakedName": "CloseButton",
                        "curName": "CloseButton",
                        "renames": [],
                        "owner": "workspace"
                    }
                }
            }
        }))
        .expect("valid schema");

        uniquify_cross_orbital_pulled_siblings(
            &mut schema,
            &[
                (0, vec!["CloseButton".to_string()]),
                (1, vec!["CloseButton".to_string()]),
            ],
        );

        let ledger = schema.ledger.as_ref().expect("ledger kept");
        assert_eq!(
            ledger.cur_name("trt_CLOSE0000000000000000000000"),
            Some("AlphaCloseButton"),
            "first copy renames the source row"
        );
        let beta_td = match &schema.orbitals[1].traits[0] {
            TraitReference::Inline(td) => td,
            other => panic!("inline expected: {other:?}"),
        };
        assert_eq!(beta_td.name, "BetaCloseButton");
        let beta_id = beta_td.id.as_deref().expect("second copy re-minted");
        assert_ne!(beta_id, "trt_CLOSE0000000000000000000000");
        assert_eq!(
            ledger.cur_name(beta_id),
            Some("BetaCloseButton"),
            "second copy carries its own ledger row"
        );
        let alpha_td = match &schema.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            other => panic!("inline expected: {other:?}"),
        };
        assert_eq!(
            alpha_td.id.as_deref(),
            Some("trt_CLOSE0000000000000000000000"),
            "first copy keeps the source id"
        );
    }

    #[test]
    fn rewrite_entity_refs_rewrites_ref_atom() {
        // ["ref", "ModalRecord"] → ["ref", "ShoppingCart"]
        let mut expr = SExpression::Array(vec![s("ref"), s("ModalRecord")]);
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, SExpression::Array(vec![s("ref"), s("ShoppingCart")]));
    }

    #[test]
    fn rewrite_entity_refs_rewrites_binding_root() {
        // "@ModalRecord.name" → "@ShoppingCart.name"
        let mut expr = s("@ModalRecord.name");
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, s("@ShoppingCart.name"));

        // Bare "@ModalRecord" (no path) should also rewrite.
        let mut bare = s("@ModalRecord");
        rewrite_entity_refs(&mut bare, "ModalRecord", "ShoppingCart");
        assert_eq!(bare, s("@ShoppingCart"));
    }

    #[test]
    fn rewrite_entity_refs_does_not_rewrite_unrelated_entity() {
        // ["ref", "OtherEntity"] stays as-is.
        let mut expr = SExpression::Array(vec![s("ref"), s("OtherEntity")]);
        let snapshot = expr.clone();
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, snapshot);

        // "@ModalRecordOther" (prefix match without boundary) stays as-is.
        let mut prefixed = s("@ModalRecordOther");
        let snapshot2 = prefixed.clone();
        rewrite_entity_refs(&mut prefixed, "ModalRecord", "ShoppingCart");
        assert_eq!(prefixed, snapshot2);

        // "@ModalRecordOther.field" also stays as-is (not a boundary match).
        let mut prefixed_path = s("@ModalRecordOther.field");
        let snapshot3 = prefixed_path.clone();
        rewrite_entity_refs(&mut prefixed_path, "ModalRecord", "ShoppingCart");
        assert_eq!(prefixed_path, snapshot3);

        // Plain string "ModalRecord" without @ sigil stays as-is.
        let mut plain = s("ModalRecord");
        let snapshot4 = plain.clone();
        rewrite_entity_refs(&mut plain, "ModalRecord", "ShoppingCart");
        assert_eq!(plain, snapshot4);
    }

    #[test]
    fn rewrite_entity_refs_rewrites_nested_lists() {
        // Nested: ["render-ui", "main", { children: [ ["ref", "ModalRecord"] ] }]
        // plus a nested binding root inside a sibling array.
        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![SExpression::Array(vec![s("ref"), s("ModalRecord")])]),
        );
        obj.insert("title".to_string(), s("@ModalRecord.name"));

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);

        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");

        // Walk the result and assert both the ref atom and the binding root
        // were rewritten.
        if let SExpression::Array(items) = &expr {
            if let SExpression::Object(map) = &items[2] {
                let children = map.get("children").unwrap();
                assert_eq!(
                    children,
                    &SExpression::Array(vec![SExpression::Array(vec![
                        s("ref"),
                        s("ShoppingCart")
                    ])])
                );
                assert_eq!(map.get("title").unwrap(), &s("@ShoppingCart.name"));
            } else {
                panic!("expected object in slot 2");
            }
        } else {
            panic!("expected outer array");
        }
    }

    // ------------------------------------------------------------------
    // apply_overrides_to_trait — Phase 3.5.G.A.2 (trait renaming)
    // ------------------------------------------------------------------

    /// Build a minimal `TraitDefinition` with the given name so the rename
    /// tests don't have to hand-construct every field.
    fn make_trait(name: &str) -> TraitDefinition {
        use orbital_core::schema::StateMachine;
        TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("ModalRecord".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    #[test]
    fn inline_renames_trait_when_name_override_present() {
        // A Reference that supplies `name: Some("CartItemAddItem")` must
        // produce an inlined trait whose `name` is the override (not the
        // resolved upstream `ModalRecordModal`).
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(
            &mut trait_def,
            None,
            &HashMap::new(),
            None,
            Some("CartItemAddItem"),
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(
            trait_def.name, "CartItemAddItem",
            "name override must rename the trait"
        );
        // linked_entity is untouched because no linked_entity override was
        // supplied.
        assert_eq!(trait_def.linked_entity.as_deref(), Some("ModalRecord"));
    }

    #[test]
    fn inline_does_not_rename_when_name_matches_resolved() {
        // Passing the same name as the resolved trait's name is a no-op.
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(
            &mut trait_def,
            None,
            &HashMap::new(),
            None,
            Some("ModalRecordModal"),
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(trait_def.name, "ModalRecordModal");
    }

    #[test]
    fn inline_does_not_rename_when_name_is_none() {
        // With `name = None`, the resolved trait keeps its original name.
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(
            &mut trait_def,
            None,
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(trait_def.name, "ModalRecordModal");
    }

    #[test]
    fn inline_applies_rename_and_linked_entity_simultaneously() {
        // When BOTH `name` and `linked_entity` are set, both overrides
        // apply: the trait is renamed AND its linked_entity is rewritten
        // (which also walks the state machine — verified separately in the
        // rewrite_entity_refs tests). Here we just confirm both fields
        // change on the TraitDefinition.
        let mut trait_def = make_trait("ModalRecordModal");
        // Seed a transition effect that references the original entity so
        // we can assert the linked_entity rewrite still fires when the
        // rename is in play.
        use orbital_core::schema::{StringOrArray, TransitionDefinition};
        trait_def
            .state_machine
            .transitions
            .push(TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "open".to_string(),
                event: "OPEN".to_string(),
                guard: None,
                effects: Some(vec![SExpression::Array(vec![
                    SExpression::String("ref".to_string()),
                    SExpression::String("ModalRecord".to_string()),
                ])]),
            });

        let linked = "CartItem".to_string();
        apply_overrides_to_trait(
            &mut trait_def,
            Some(&linked),
            &HashMap::new(),
            None,
            Some("CartItemAddItem"),
            None,
            None,
            false,
            &mut Vec::new(),
        );

        assert_eq!(
            trait_def.name, "CartItemAddItem",
            "rename must apply alongside linked_entity"
        );
        assert_eq!(
            trait_def.linked_entity.as_deref(),
            Some("CartItem"),
            "linked_entity override must apply alongside rename"
        );

        // The seeded `["ref", "ModalRecord"]` effect must have been
        // rewritten to `["ref", "CartItem"]` by the linked_entity
        // override path.
        let effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("effects present")[0];
        assert_eq!(
            effect,
            &SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("CartItem".to_string()),
            ]),
            "rename path must not interfere with linked_entity rewrite"
        );
    }

    #[test]
    fn rebind_remaps_linked_entity_id_to_consumer_node() {
        // A trait cloned from a source file carries the SOURCE file's entity id
        // in `linked_entity_id`. When the call site rebinds `linkedEntity` to a
        // consumer entity, the id sibling MUST be rewritten to the consumer
        // entity's id in the same operation (dual-carry invariant) — leaving the
        // foreign id produces `ORB_ID_UNKNOWN_REF` on the composed schema.
        let mut trait_def = make_trait("ModalRecordModal");
        trait_def.linked_entity_id = Some("ent_SOURCEFOREIGN00000000000".to_string());

        let mut entity_ids = HashMap::new();
        entity_ids.insert(
            "CartItem".to_string(),
            "ent_CONSUMER0000000000000000".to_string(),
        );

        let linked = "CartItem".to_string();
        apply_overrides_to_trait(
            &mut trait_def,
            Some(&linked),
            &entity_ids,
            None,
            None,
            None,
            None,
            false,
            &mut Vec::new(),
        );

        assert_eq!(trait_def.linked_entity.as_deref(), Some("CartItem"));
        assert_eq!(
            trait_def.linked_entity_id.as_deref(),
            Some("ent_CONSUMER0000000000000000"),
            "id sibling must be remapped to the consumer node, not left stale"
        );
    }

    #[test]
    fn rebind_to_unknown_target_clears_stale_id_not_dangles() {
        // When the rebind target is not a node in the consumer arena (empty
        // map), the stale foreign id is CLEARED (dual-carry drops to name-only)
        // rather than left dangling — dropping is the correct carry only when the
        // target genuinely has no node in the composed graph.
        let mut trait_def = make_trait("ModalRecordModal");
        trait_def.linked_entity_id = Some("ent_SOURCEFOREIGN00000000000".to_string());
        let linked = "CartItem".to_string();
        apply_overrides_to_trait(
            &mut trait_def,
            Some(&linked),
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(trait_def.linked_entity.as_deref(), Some("CartItem"));
        assert_eq!(
            trait_def.linked_entity_id, None,
            "unresolvable rebind target must clear the stale foreign id"
        );
    }

    // ------------------------------------------------------------------
    // Cross-file ledger merge (Rabit V4) — foreign-node-not-merged
    // ------------------------------------------------------------------

    #[test]
    fn merge_source_ledgers_preserves_cross_file_id() {
        use crate::phases::validation::error::ErrorCode;
        use crate::phases::validation::{id_integrity, ValidationContext};
        use orbital_core::identity::{IdKind, LedgerEntry, LedgerOwner};

        // Consumer schema: one orbital whose inline trait references a FOREIGN
        // entity id (`ent_SRC...`) that is neither a declared consumer node nor
        // in the consumer ledger — the exact shape a non-rebound cross-file
        // clone leaves behind.
        let json = serde_json::json!({
            "name": "Consumer",
            "orbitals": [{
                "name": "ConsumerOrbital",
                "id": "orb_CON0000000000000000000000",
                "entity": {
                    "name": "Consumer",
                    "id": "ent_CON0000000000000000000000",
                    "fields": [{ "name": "title", "type": "string" }]
                },
                "traits": [{
                    "name": "ImportedWidget",
                    "id": "trt_IMP0000000000000000000000",
                    "linkedEntity": "SourceEntity",
                    "linkedEntityId": "ent_SRC0000000000000000000000",
                    "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] }
                }]
            }],
            "schemaVersion": 1,
            "ledger": {
                "schemaVersion": 1,
                "entries": {
                    "orb_CON0000000000000000000000": { "id": "orb_CON0000000000000000000000", "kind": "orbital", "bakedName": "ConsumerOrbital", "curName": "ConsumerOrbital", "renames": [], "owner": "workspace" },
                    "ent_CON0000000000000000000000": { "id": "ent_CON0000000000000000000000", "kind": "entity", "bakedName": "Consumer", "curName": "Consumer", "renames": [], "owner": "workspace" },
                    "trt_IMP0000000000000000000000": { "id": "trt_IMP0000000000000000000000", "kind": "trait", "bakedName": "ImportedWidget", "curName": "ImportedWidget", "renames": [], "owner": "workspace" }
                }
            }
        });
        let mut schema: OrbitalSchema = serde_json::from_value(json).expect("fixture parses");

        let id_codes = |schema: &OrbitalSchema| -> Vec<ErrorCode> {
            let mut ctx = ValidationContext::new();
            id_integrity::validate_id_integrity(schema, &mut ctx);
            ctx.into_result()
                .all_issues()
                .map(|e| e.code)
                .filter(|c| c.category() == "identity")
                .collect()
        };

        // Before the merge: the foreign id dangles.
        assert_eq!(
            id_codes(&schema),
            vec![ErrorCode::OrbIdUnknownRef],
            "pre-merge: the foreign linkedEntity id must be unknown"
        );

        // Source ledger union carrying the foreign entity's row.
        let mut source_union = HashMap::new();
        source_union.insert(
            "ent_SRC0000000000000000000000".to_string(),
            LedgerEntry {
                id: "ent_SRC0000000000000000000000".to_string(),
                kind: IdKind::Entity,
                baked_name: "SourceEntity".to_string(),
                cur_name: "SourceEntity".to_string(),
                renames: vec![],
                owner: LedgerOwner::Std,
                parent: None,
            },
        );

        super::merge_source_ledgers(&mut schema, &source_union);

        // The row is now in the consumer ledger and id integrity is clean.
        assert!(schema
            .ledger
            .as_ref()
            .unwrap()
            .entries
            .contains_key("ent_SRC0000000000000000000000"));
        assert!(
            id_codes(&schema).is_empty(),
            "post-merge: cross-file id must resolve, no ORB_ID_* diagnostics: {:?}",
            id_codes(&schema)
        );
    }

    // ------------------------------------------------------------------
    // inline_traits collision detection — Phase 3.5.G.A.3
    // ------------------------------------------------------------------

    /// Build an `Orbital` whose only trait is the supplied `TraitDefinition`,
    /// suitable for stashing in an `InlineContext` under an alias.
    fn make_orbital_with_trait(name: &str, trait_def: TraitDefinition) -> Orbital {
        use orbital_core::schema::EntityDefinition;
        Orbital {
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: format!("{}Entity", name),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(trait_def)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    /// Build an `InlineContext` with one alias `Modal` whose only trait is
    /// `ModalRecordModal`. This mirrors the embedded `std-modal` registry
    /// shape that the real fixtures rely on, without dragging the loader
    /// into the test.
    fn make_modal_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let modal_orbital = make_orbital_with_trait("Modal", make_trait("ModalRecordModal"));
        ctx.add_alias("Modal", &modal_orbital);
        ctx
    }

    /// Helper: build a `TraitReference::Reference` pointing at
    /// `Modal.traits.ModalRecordModal` with optional overrides.
    fn modal_ref(name_override: Option<&str>, linked_entity: Option<&str>) -> TraitReference {
        TraitReference::Reference {
            id: None,
            reference: Some("Modal.traits.ModalRecordModal".to_string()),
            name: name_override.map(|s| s.to_string()),
            linked_entity: linked_entity.map(|s| s.to_string()),
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        }
    }

    #[test]
    fn inline_collision_two_references_no_name_override_errors() {
        // Two references to Modal.traits.ModalRecordModal, neither carrying
        // a `name` override. After A.1 + A.2 each resolves to a trait whose
        // `name` is the upstream `ModalRecordModal`, so they collide on
        // push #2. The inliner must reject with DuplicateTraitName naming
        // the offending trait.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(None, Some("CartItem")),
            modal_ref(None, Some("CartItem")),
        ];

        let result = inline_traits(&traits, &ctx, "<test>", &HashMap::new(), &mut Vec::new());
        match result {
            Err(InlineError::DuplicateTraitName { name, .. }) => {
                assert_eq!(
                    name, "ModalRecordModal",
                    "DuplicateTraitName must carry the original (collided) trait name"
                );
            }
            other => panic!(
                "expected Err(DuplicateTraitName(\"ModalRecordModal\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn inline_collision_two_references_with_distinct_name_overrides_succeeds() {
        // Two references with two DIFFERENT `name` overrides. Post-rename
        // the trait names differ, so no collision: both inlined traits
        // appear in the result with their renamed names AND their
        // linked_entity overrides applied.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(Some("CartItemAddItem"), Some("CartItem")),
            modal_ref(Some("CartItemEditItem"), Some("CartItem")),
        ];

        let (result, _rename_map, _aux, _pulled, _cfg_diags) =
            inline_traits(&traits, &ctx, "<test>", &HashMap::new(), &mut Vec::new())
                .expect("two distinct `name` overrides must inline successfully without collision");
        assert_eq!(result.len(), 2, "both references must be inlined");

        let names: Vec<&str> = result
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"CartItemAddItem"),
            "first override name missing; got {:?}",
            names
        );
        assert!(
            names.contains(&"CartItemEditItem"),
            "second override name missing; got {:?}",
            names
        );

        // And both carry the linked_entity override.
        for t in &result {
            if let TraitReference::Inline(def) = t {
                assert_eq!(def.linked_entity.as_deref(), Some("CartItem"));
            }
        }
    }

    #[test]
    fn inline_collision_one_reference_with_name_override_one_without_succeeds() {
        // One reference renames to `CartItemAddItem`; the other has no
        // `name` override and stays as `ModalRecordModal`. Two distinct
        // post-rename names, so no collision.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(Some("CartItemAddItem"), Some("CartItem")),
            modal_ref(None, Some("CartItem")),
        ];

        let (result, _rename_map, _aux, _pulled, _cfg_diags) =
            inline_traits(&traits, &ctx, "<test>", &HashMap::new(), &mut Vec::new())
                .expect("one renamed + one not-renamed must succeed (no name collision)");
        assert_eq!(result.len(), 2);

        let names: Vec<&str> = result
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"CartItemAddItem"));
        assert!(names.contains(&"ModalRecordModal"));
    }

    #[test]
    fn inline_collision_inline_and_reference_same_name_errors() {
        // A literal `TraitReference::Inline` named `ModalRecordModal` plus
        // a Reference that resolves to a trait of the same name. The
        // collision must be detected even though one half is a literal
        // (not a Reference). This protects molecules that hand-author one
        // trait and import a same-named one accidentally.
        let ctx = make_modal_ctx();
        let traits = vec![
            TraitReference::Inline(make_trait("ModalRecordModal")),
            modal_ref(None, None),
        ];

        let result = inline_traits(&traits, &ctx, "<test>", &HashMap::new(), &mut Vec::new());
        match result {
            Err(InlineError::DuplicateTraitName { name, .. }) => {
                assert_eq!(name, "ModalRecordModal");
            }
            other => panic!(
                "expected Err(DuplicateTraitName(\"ModalRecordModal\")), got {:?}",
                other
            ),
        }
    }

    // ------------------------------------------------------------------
    // inline_services + ServiceRef::Object — Phase 3.5.G.A.4
    // ------------------------------------------------------------------

    /// Build a minimal REST `ServiceDefinition` so the service-override
    /// tests don't have to hand-construct every field.
    fn make_rest_service(name: &str, base_url: &str) -> ServiceDefinition {
        use orbital_core::schema::RestServiceDefInner;
        ServiceDefinition::Rest(RestServiceDefInner {
            name: name.to_string(),
            description: Some("upstream description".to_string()),
            base_url: base_url.to_string(),
            headers: None,
            auth: None,
            timeout: None,
        })
    }

    /// Build an `Orbital` whose only service is the supplied REST
    /// `ServiceDefinition`. Mirrors `make_orbital_with_trait` for services.
    fn make_orbital_with_service(name: &str, service_def: ServiceDefinition) -> Orbital {
        use orbital_core::schema::EntityDefinition;
        Orbital {
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: Some(vec![ServiceRef::Inline(service_def)]),
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: format!("{}Entity", name),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    /// Build an `InlineContext` with one alias `Llm` whose only service is
    /// `LlmService` (REST, with a placeholder upstream baseUrl). Mirrors
    /// `make_modal_ctx` for the service path.
    fn make_llm_service_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let llm_orbital = make_orbital_with_service(
            "Llm",
            make_rest_service("LlmService", "https://upstream.example.com"),
        );
        ctx.add_alias("Llm", &llm_orbital);
        ctx
    }

    /// Helper: build a `ServiceRef::Object` pointing at
    /// `Llm.services.LlmService` with optional override fields.
    fn llm_service_object(
        base_url_override: Option<&str>,
        description_override: Option<&str>,
    ) -> ServiceRef {
        ServiceRef::Object(ServiceRefObject {
            reference: "Llm.services.LlmService".to_string(),
            description: description_override.map(|s| s.to_string()),
            base_url: base_url_override.map(|s| s.to_string()),
            headers: None,
            url: None,
            server_path: None,
        })
    }

    #[test]
    fn inline_service_object_resolves_with_no_overrides() {
        // A `ServiceRef::Object` with no override fields must resolve to
        // the same Inline service that a plain `Reference` string would
        // produce. The resolved service keeps every upstream field
        // unchanged (here: baseUrl + description).
        let ctx = make_llm_service_ctx();
        let services = Some(vec![llm_service_object(None, None)]);

        let result = inline_services(&services, &ctx)
            .expect("ServiceRef::Object with no overrides must resolve cleanly");
        let result = result.expect("Some result expected");
        assert_eq!(result.len(), 1, "exactly one service must be inlined");

        match &result[0] {
            ServiceRef::Inline(ServiceDefinition::Rest(rest)) => {
                assert_eq!(rest.name, "LlmService");
                assert_eq!(
                    rest.base_url, "https://upstream.example.com",
                    "no override → upstream baseUrl preserved"
                );
                assert_eq!(
                    rest.description.as_deref(),
                    Some("upstream description"),
                    "no override → upstream description preserved"
                );
            }
            other => panic!("expected ServiceRef::Inline(Rest), got {:?}", other),
        }
    }

    #[test]
    fn inline_service_object_applies_endpoint_override() {
        // Same context as above, but the Object carries a baseUrl
        // override. After inlining, the resulting Inline service must
        // have the new baseUrl. Description is also overridden to prove
        // the override-application loop covers more than one field.
        let ctx = make_llm_service_ctx();
        let services = Some(vec![llm_service_object(
            Some("https://custom.example.com"),
            Some("custom description"),
        )]);

        let result = inline_services(&services, &ctx)
            .expect("ServiceRef::Object with baseUrl override must resolve");
        let result = result.expect("Some result expected");
        assert_eq!(result.len(), 1);

        match &result[0] {
            ServiceRef::Inline(ServiceDefinition::Rest(rest)) => {
                assert_eq!(rest.name, "LlmService", "name is not overrideable");
                assert_eq!(
                    rest.base_url, "https://custom.example.com",
                    "baseUrl override must replace upstream value"
                );
                assert_eq!(
                    rest.description.as_deref(),
                    Some("custom description"),
                    "description override must replace upstream value"
                );
            }
            other => panic!("expected ServiceRef::Inline(Rest), got {:?}", other),
        }
    }

    #[test]
    fn inline_service_object_with_unknown_alias_errors() {
        // An Object whose `reference` points to an alias that the
        // InlineContext doesn't know must error with `UnknownAlias`,
        // surfaced through `ctx.get_service` → `get_orbital`.
        let ctx = make_llm_service_ctx();
        let services = Some(vec![ServiceRef::Object(ServiceRefObject {
            reference: "NoSuchAlias.services.SomeService".to_string(),
            description: None,
            base_url: None,
            headers: None,
            url: None,
            server_path: None,
        })]);

        let result = inline_services(&services, &ctx);
        match result {
            Err(InlineError::UnknownAlias(alias)) => {
                assert_eq!(
                    alias, "NoSuchAlias",
                    "UnknownAlias must carry the offending alias name"
                );
            }
            other => panic!(
                "expected Err(UnknownAlias(\"NoSuchAlias\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn inline_service_object_invalid_reference_format_errors() {
        // An Object whose `reference` is not in the
        // `Alias.services.Name` format must surface as InvalidReference.
        // This guards against typos like `Llm.LlmService` (missing the
        // `services` segment).
        let ctx = make_llm_service_ctx();
        let services = Some(vec![ServiceRef::Object(ServiceRefObject {
            reference: "Llm.LlmService".to_string(),
            description: None,
            base_url: None,
            headers: None,
            url: None,
            server_path: None,
        })]);

        let result = inline_services(&services, &ctx);
        assert!(
            matches!(result, Err(InlineError::InvalidReference(_))),
            "expected InvalidReference, got {:?}",
            result
        );
    }

    // ------------------------------------------------------------------
    // inline_entity + EntityRef::Extends — Phase 3.5.G.A.5
    // ------------------------------------------------------------------

    use orbital_core::schema::{FieldType, Persistence};

    /// Build a minimal `EntityDefinition` mirroring the upstream
    /// `ModalRecord` shape from the embedded std-modal registry: 5 fields
    /// (id, name, description, status, createdAt) with `runtime`
    /// persistence. The Extends tests use this as the base entity that the
    /// molecule inherits from, so the field counts and names match the
    /// real registry shape that the cart molecule consumes.
    fn make_modal_entity_def() -> EntityDefinition {
        let make_field = |n: &str, t: FieldType| FieldDefinition {
            name: Some(n.to_string()),
            field_type: t,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        };
        EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ModalRecord".to_string(),
            collection: None,
            persistence: Some(Persistence::Runtime),
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
            ],
            instances: None,
            id: None,
        }
    }

    /// Build an `Orbital` whose only entity is the supplied
    /// `EntityDefinition`. Mirrors `make_orbital_with_trait` /
    /// `make_orbital_with_service` for entities so the Extends tests can
    /// stash a base entity in the InlineContext under an alias.
    fn make_orbital_with_entity(name: &str, entity_def: EntityDefinition) -> Orbital {
        Orbital {
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity_def),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    /// Build an `InlineContext` with one alias `Modal` whose only entity
    /// is `ModalRecord` (matching the embedded std-modal registry). Mirrors
    /// `make_modal_ctx` and `make_llm_service_ctx` for the entity path.
    fn make_modal_entity_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let modal_orbital = make_orbital_with_entity("Modal", make_modal_entity_def());
        ctx.add_alias("Modal", &modal_orbital);
        ctx
    }

    /// Helper: build a `FieldDefinition` for an extra field, with just a
    /// name and a type. The Extends tests pass these as the molecule's
    /// `fields` array.
    fn make_extra_field(name: &str, ft: FieldType) -> FieldDefinition {
        FieldDefinition {
            name: Some(name.to_string()),
            field_type: ft,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    #[test]
    fn inline_entity_extends_inherits_all_base_fields_when_no_extras() {
        // EntityRef::Extends with no rename and no extra fields must
        // produce an Inline entity equivalent to a bare
        // EntityRef::Reference("Modal.entity") — same name, same fields,
        // same persistence — confirming the no-op path through the
        // override-application loop is byte-faithful.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: None,
            fields: None,
            persistence: None,
            collection: None,
            extends_id: None,
        });

        let result =
            inline_entity(&entity, &ctx).expect("Extends with no overrides must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.name, "ModalRecord",
            "no rename → upstream entity name preserved"
        );
        assert_eq!(
            inlined.persistence,
            Some(Persistence::Runtime),
            "no persistence override → upstream value preserved"
        );
        assert_eq!(
            inlined.fields.len(),
            5,
            "no extras → all 5 inherited fields preserved"
        );
        let field_names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        assert_eq!(
            field_names,
            vec!["id", "name", "description", "status", "createdAt"]
        );
    }

    #[test]
    fn inline_entity_extends_renames_when_name_set() {
        // Extends with `name: Some("CartItem")` and no extras must rename
        // the resulting entity to `CartItem` while still preserving every
        // inherited field. This is what molecules do when they want to
        // inherit a base shape but give it a contextual name.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            fields: None,
            persistence: None,
            collection: None,
            extends_id: None,
        });

        let result = inline_entity(&entity, &ctx).expect("Extends with rename must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.name, "CartItem",
            "name override must rename the entity"
        );
        assert_eq!(
            inlined.fields.len(),
            5,
            "rename without extras → all 5 inherited fields preserved"
        );
    }

    #[test]
    fn inline_entity_extends_appends_extra_fields() {
        // Extends with both a rename AND two extra fields must produce a
        // 7-field entity: the 5 inherited fields followed by the 2
        // molecule fields, in that order. This is the cart molecule case
        // (5 ModalRecord fields + pendingId + lineItemCount).
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            fields: Some(vec![
                make_extra_field("pendingId", FieldType::String),
                make_extra_field("lineItemCount", FieldType::Number),
            ]),
            persistence: None,
            collection: None,
            extends_id: None,
        });

        let result =
            inline_entity(&entity, &ctx).expect("Extends with rename + extras must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(inlined.name, "CartItem");
        assert_eq!(
            inlined.fields.len(),
            7,
            "5 inherited + 2 extras → 7 total fields"
        );

        let field_names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        // Order: inherited first, then extras (in declaration order).
        assert_eq!(
            field_names,
            vec![
                "id",
                "name",
                "description",
                "status",
                "createdAt",
                "pendingId",
                "lineItemCount",
            ]
        );
    }

    #[test]
    fn inline_entity_extends_extra_fields_override_collisions() {
        // Extends with an extra field whose `name` matches an inherited
        // field (`status`) must produce only ONE field with that name —
        // the molecule's version. This is the "user wins on conflict"
        // contract from the design notes.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            // Override `status` (inherited) and add a brand new
            // `pendingId`.
            fields: Some(vec![
                make_extra_field("status", FieldType::Number),
                make_extra_field("pendingId", FieldType::String),
            ]),
            persistence: None,
            collection: None,
            extends_id: None,
        });

        let result = inline_entity(&entity, &ctx)
            .expect("Extends with collision-override must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.fields.len(),
            6,
            "5 inherited - 1 collision + 2 extras (1 collision + 1 new) = 6"
        );

        // The inherited `status` field (FieldType::String) must be gone;
        // the molecule's `status` (FieldType::Number) must be the only one.
        let status_fields: Vec<&FieldDefinition> = inlined
            .fields
            .iter()
            .filter(|f| f.name.as_deref() == Some("status"))
            .collect();
        assert_eq!(
            status_fields.len(),
            1,
            "exactly one `status` field must survive (the molecule's override)"
        );
        assert_eq!(
            status_fields[0].field_type,
            FieldType::Number,
            "molecule's `status` (Number) must replace the inherited `status` (String)"
        );

        // The new `pendingId` is present.
        assert!(
            inlined
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some("pendingId")),
            "molecule's new `pendingId` field must be present"
        );

        // The other 4 inherited fields must still be there in their
        // original positions (relative order preserved).
        let names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec![
                "id",
                "name",
                "description",
                "createdAt",
                "status",
                "pendingId"
            ],
            "non-colliding inherited fields keep their order; the molecule's \
             override `status` lands at the extras position, not the inherited slot"
        );
    }

    #[test]
    fn inline_entity_extends_unknown_alias_errors() {
        // Extends with a `reference` pointing at an alias the InlineContext
        // doesn't know must surface UnknownAlias via ctx.get_entity →
        // get_orbital. The error must carry the offending alias name.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "NoSuchAlias.entity".to_string(),
            name: None,
            fields: None,
            persistence: None,
            collection: None,
            extends_id: None,
        });

        let result = inline_entity(&entity, &ctx);
        match result {
            Err(InlineError::UnknownAlias(alias)) => {
                assert_eq!(
                    alias, "NoSuchAlias",
                    "UnknownAlias must carry the offending alias name"
                );
            }
            other => panic!(
                "expected Err(UnknownAlias(\"NoSuchAlias\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn rewrite_entity_refs_noop_when_no_match() {
        // An expression with no entity references anywhere should be
        // byte-identical after rewrite.
        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert("type".to_string(), s("stack"));
        obj.insert("direction".to_string(), s("vertical"));
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![
                SExpression::Array(vec![s("emit"), s("SAVED")]),
                s("@payload.data"),
            ]),
        );

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);
        let snapshot = expr.clone();
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, snapshot);
    }

    #[test]
    fn test_parse_entity_reference() {
        let result = parse_reference("Health.entity").unwrap();
        assert!(matches!(result, ParsedReference::Entity { alias } if alias == "Health"));
    }

    #[test]
    fn test_parse_trait_reference() {
        let result = parse_reference("Health.traits.HealthTrait").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Trait { alias, name } if alias == "Health" && name == "HealthTrait"
        ));
    }

    #[test]
    fn test_parse_page_reference() {
        let result = parse_reference("Health.pages.HealthStatus").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Page { alias, name } if alias == "Health" && name == "HealthStatus"
        ));
    }

    #[test]
    fn test_parse_theme_reference() {
        let result = parse_reference("Ocean.theme").unwrap();
        assert!(matches!(result, ParsedReference::Theme { alias } if alias == "Ocean"));
    }

    #[test]
    fn test_parse_service_reference() {
        let result = parse_reference("Payment.services.Stripe").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Service { alias, name } if alias == "Payment" && name == "Stripe"
        ));
    }

    #[test]
    fn test_parse_invalid_reference() {
        assert!(parse_reference("Health").is_err());
        assert!(parse_reference("Health.invalid").is_err());
        assert!(parse_reference("Health.traits").is_err());
    }

    #[test]
    fn test_inline_player_schema() {
        use crate::phases::parse::parse_file;

        let player_path = fixtures_dir().join("player.orb");
        let mut schema = parse_file(&player_path).unwrap();

        // Verify entity is a reference before inlining
        assert!(schema.orbitals[0].entity.is_reference());
        assert_eq!(
            schema.orbitals[0].entity.as_reference(),
            Some("Health.entity")
        );

        // Inline the schema
        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: fixtures_dir(),
                allow_outside_base_path: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = inline_schema(&mut schema, &config, Some(&player_path));
        assert!(result.is_ok(), "Inline failed: {:?}", result.err());

        // Verify entity is now inline
        assert!(!schema.orbitals[0].entity.is_reference());
        let entity = schema.orbitals[0].entity.as_inline().unwrap();
        assert_eq!(entity.name, "HealthEntity");

        // Verify traits are inlined
        assert!(!schema.orbitals[0].traits.is_empty());
        if let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] {
            assert_eq!(trait_def.name, "HealthTrait");
        } else {
            panic!("Expected inlined trait");
        }

        // Verify pages are inlined with path override
        let pages = schema.orbitals[0].pages.as_ref().expect("Expected pages");
        assert!(!pages.is_empty());
        if let PageRef::Inline(page_def) = &pages[0] {
            assert_eq!(page_def.name, "HealthStatus");
            // Path should be overridden
            assert_eq!(page_def.path, "/player/:id/health");
        } else {
            panic!("Expected inlined page");
        }
    }

    // ------------------------------------------------------------------
    // Phase 3.5.G.F.3 — Generic identifier-substitution rewriter
    // ------------------------------------------------------------------

    #[test]
    fn rewrite_identifiers_handles_multi_entry_entity_map() {
        // The walker accepts a HashMap with multiple entries; every entry
        // fires independently. `["ref", "A"]` and `["ref", "B"]` in the
        // same tree both get rewritten in a single walk.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("A".to_string(), "X".to_string());
        entity_subs.insert("B".to_string(), "Y".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("A".to_string()),
            ]),
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("B".to_string()),
            ]),
        ]);

        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        if let SExpression::Array(items) = &expr {
            // Both nested ref calls were rewritten in one pass.
            if let SExpression::Array(inner) = &items[0] {
                assert_eq!(inner[1], SExpression::String("X".to_string()));
            }
            if let SExpression::Array(inner) = &items[1] {
                assert_eq!(inner[1], SExpression::String("Y".to_string()));
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_emit_arrays_via_event_subs() {
        // `["emit", "OPEN"]` is the symmetric event-side counterpart of
        // `["ref", "EntityName"]`. With OPEN → ADD_ITEM in event_subs,
        // the second element gets substituted.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("OPEN".to_string()),
        ]);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
        } else {
            panic!("expected array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_emit_with_payload_tail() {
        // The 3-element `(emit EVENT payload)` shape — what every state
        // machine actually emits when the event carries data. Pre-fix, the
        // rewriter only matched `items.len() == 2` and silently passed
        // `["emit", "FILTER", { value: "" }]` through unchanged, leaving
        // a stale event reference that the validator flagged as
        // ORB_T_EVT_UNDECLARED.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("FILTER".to_string(), "NARROW".to_string());

        let mut payload_obj: HashMap<String, SExpression> = HashMap::new();
        payload_obj.insert("value".to_string(), SExpression::String("".to_string()));
        payload_obj.insert("field".to_string(), SExpression::String("".to_string()));
        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("FILTER".to_string()),
            SExpression::Object(payload_obj),
        ]);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items.len(), 3, "payload tail should be preserved");
            assert_eq!(items[1], SExpression::String("NARROW".to_string()));
            // Payload object is left alone — only the event name in
            // position 1 was a rename target.
            assert!(matches!(items[2], SExpression::Object(_)));
        } else {
            panic!("expected array");
        }
    }

    #[test]
    fn rewrite_identifiers_emit_payload_tail_inner_ref_still_walks() {
        // After the emit-event rewrite we DON'T return — the walk
        // continues into the payload tail. A nested `["ref", X]` inside
        // the payload should still get entity-rewritten in the same pass.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("OldEntity".to_string(), "NewEntity".to_string());
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("OPEN".to_string()),
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("OldEntity".to_string()),
            ]),
        ]);

        rewrite_identifiers(&mut expr, &entity_subs, &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
            if let SExpression::Array(inner) = &items[2] {
                assert_eq!(inner[1], SExpression::String("NewEntity".to_string()));
            } else {
                panic!("expected inner ref array");
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_render_ui_event_object_value() {
        // Registry-driven rule: when an SExpression::Object has `type: "button"`
        // and the registry declares `"action"` as button's event-valued prop
        // (see PATTERN_EVENT_PROPS in validation::sync::patterns), a string
        // value under "action" that matches an event_subs key gets rewritten.
        // This handles render-ui button event references that live inside
        // object maps rather than `["emit", X]` arrays.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut object = HashMap::new();
        object.insert(
            "type".to_string(),
            SExpression::String("button".to_string()),
        );
        object.insert(
            "action".to_string(),
            SExpression::String("OPEN".to_string()),
        );
        let mut expr = SExpression::Object(object);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Object(map) = &expr {
            assert_eq!(
                map.get("action"),
                Some(&SExpression::String("ADD_ITEM".to_string())),
                "button action= attribute must be substituted"
            );
            // The unrelated "type" entry is untouched.
            assert_eq!(
                map.get("type"),
                Some(&SExpression::String("button".to_string()))
            );
        } else {
            panic!("expected object");
        }
    }

    #[test]
    fn rewrite_identifiers_does_not_rewrite_event_substring_in_other_strings() {
        // A plain string value whose key is NOT "event" must NOT be
        // rewritten even if it equals an event key. This protects content
        // strings like `{ "type": "typography", "content": "OPEN" }` from
        // accidental substitution.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut object = HashMap::new();
        object.insert(
            "content".to_string(),
            SExpression::String("OPEN".to_string()),
        );
        let mut expr = SExpression::Object(object);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Object(map) = &expr {
            assert_eq!(
                map.get("content"),
                Some(&SExpression::String("OPEN".to_string())),
                "content strings must NOT be substituted by event_subs"
            );
        }
    }

    #[test]
    fn rewrite_trait_event_fields_renames_state_machine_events_and_transitions() {
        use orbital_core::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            name: "T".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "closed".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: Some(vec![
                    EventDefinition {
                        id: None,
                        key: "OPEN".to_string(),
                        name: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        payload_schema: None,
                    },
                    EventDefinition {
                        id: None,
                        key: "CLOSE".to_string(),
                        name: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        payload_schema: None,
                    },
                ]),
                transitions: vec![
                    TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("closed".to_string()),
                        to: "open".to_string(),
                        event: "OPEN".to_string(),
                        guard: None,
                        effects: None,
                    },
                    TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("open".to_string()),
                        to: "closed".to_string(),
                        event: "CLOSE".to_string(),
                        guard: None,
                        effects: None,
                    },
                ],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        };

        let mut event_subs = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());
        event_subs.insert("CLOSE".to_string(), "CANCEL_ADD".to_string());

        rewrite_trait_event_fields(&mut trait_def, &event_subs);

        // StateMachine.events keys are rewritten.
        let event_keys: Vec<&str> = trait_def
            .state_machine
            .events
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        assert!(event_keys.contains(&"ADD_ITEM"));
        assert!(event_keys.contains(&"CANCEL_ADD"));
        assert!(!event_keys.contains(&"OPEN"));
        assert!(!event_keys.contains(&"CLOSE"));

        // Transition events are rewritten.
        assert_eq!(trait_def.state_machine.transitions[0].event, "ADD_ITEM");
        assert_eq!(trait_def.state_machine.transitions[1].event, "CANCEL_ADD");
    }

    #[test]
    fn apply_overrides_to_trait_renames_events_via_events_arg() {
        // End-to-end of the call_args path: build a trait with an OPEN
        // transition + a corresponding event entry, pass an `events` arg
        // renaming OPEN → ADD_ITEM, and confirm both the structured fields
        // and (would-be) SExpression payloads agree.
        use orbital_core::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            name: "ModalRecordModal".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("ModalRecord".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "closed".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: Some(vec![EventDefinition {
                    id: None,
                    key: "OPEN".to_string(),
                    name: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                }]),
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("closed".to_string()),
                    to: "open".to_string(),
                    event: "OPEN".to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("emit".to_string()),
                        SExpression::String("OPEN".to_string()),
                    ])]),
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        };

        let mut events_arg = HashMap::new();
        events_arg.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        apply_overrides_to_trait(
            &mut trait_def,
            None,
            &HashMap::new(),
            None,
            None,
            Some(&events_arg),
            None,
            false,
            &mut Vec::new(),
        );

        // Structured field rewritten.
        assert_eq!(trait_def.state_machine.transitions[0].event, "ADD_ITEM");
        // SExpression `["emit", "OPEN"]` → `["emit", "ADD_ITEM"]`.
        let effects = trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("effects present");
        if let SExpression::Array(items) = &effects[0] {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
        } else {
            panic!("expected effect array");
        }
    }

    // ------------------------------------------------------------------
    // GAP-COMP-11 — linkedEntity rebinding rewrites fetch/persist shapes
    // ------------------------------------------------------------------

    #[test]
    fn rewrite_identifiers_rewrites_fetch_entity_slot() {
        // Atom trait effect: ["fetch", "Source"]. With entity_subs
        // Source → Target, position 1 must be rewritten.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut expr = SExpression::Array(vec![s("fetch"), s("Source")]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(expr, SExpression::Array(vec![s("fetch"), s("Target")]));
    }

    #[test]
    fn rewrite_identifiers_rewrites_fetch_entity_slot_with_criteria_tail() {
        // ["fetch", "Source", {criteria...}] — the entity slot at index 1
        // is rewritten; the trailing object is untouched (criteria objects
        // don't carry entity names by convention).
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut criteria = HashMap::new();
        criteria.insert("limit".to_string(), SExpression::Number(10.0));

        let mut expr = SExpression::Array(vec![
            s("fetch"),
            s("Source"),
            SExpression::Object(criteria.clone()),
        ]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr,
            SExpression::Array(vec![s("fetch"), s("Target"), SExpression::Object(criteria),])
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_persist_create_entity_slot() {
        // ["persist", "create", "Source", {payload}] — index 2 is the entity
        // name and must be rewritten. The payload object is untouched.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut payload = HashMap::new();
        payload.insert("data".to_string(), s("@payload.data"));

        let mut expr = SExpression::Array(vec![
            s("persist"),
            s("create"),
            s("Source"),
            SExpression::Object(payload.clone()),
        ]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr,
            SExpression::Array(vec![
                s("persist"),
                s("create"),
                s("Target"),
                SExpression::Object(payload),
            ])
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_persist_update_and_delete_entity_slot() {
        // update + delete variants share the single-record shape and must
        // rewrite index 2 identically to create.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut update = SExpression::Array(vec![s("persist"), s("update"), s("Source")]);
        rewrite_identifiers(&mut update, &entity_subs, &HashMap::new());
        assert_eq!(
            update,
            SExpression::Array(vec![s("persist"), s("update"), s("Target")])
        );

        let mut delete = SExpression::Array(vec![s("persist"), s("delete"), s("Source")]);
        rewrite_identifiers(&mut delete, &entity_subs, &HashMap::new());
        assert_eq!(
            delete,
            SExpression::Array(vec![s("persist"), s("delete"), s("Target")])
        );
    }

    #[test]
    fn rewrite_identifiers_does_not_mangle_persist_batch_rows() {
        // ["persist", "batch", [row1, row2]] — index 2 is a rows array,
        // NOT an entity name. The walker must NOT rewrite it as a string
        // substitution, and the rows array must stay structurally intact.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut row1 = HashMap::new();
        row1.insert("name".to_string(), s("row-1"));
        let mut row2 = HashMap::new();
        row2.insert("name".to_string(), s("row-2"));

        let rows = SExpression::Array(vec![
            SExpression::Object(row1.clone()),
            SExpression::Object(row2.clone()),
        ]);

        let mut expr = SExpression::Array(vec![s("persist"), s("batch"), rows.clone()]);
        let snapshot = expr.clone();
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        // Batch shape is byte-identical after rewrite — the walker must not
        // touch the rows array.
        assert_eq!(
            expr, snapshot,
            "persist batch rows array must be untouched by entity rewrite"
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_nested_fetch_inside_render_ui() {
        // Recursion test: ["render-ui", "main", {children: [["fetch",
        // "Source"]]}] — the nested fetch effect must be found and rewritten
        // by the recursive walk.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![SExpression::Array(vec![s("fetch"), s("Source")])]),
        );

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        if let SExpression::Array(items) = &expr {
            if let SExpression::Object(map) = &items[2] {
                let children = map.get("children").unwrap();
                assert_eq!(
                    children,
                    &SExpression::Array(vec![SExpression::Array(vec![s("fetch"), s("Target")])]),
                    "nested fetch inside render-ui children must be rewritten"
                );
            } else {
                panic!("expected object in slot 2");
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_leaves_unrelated_fetch_entity_untouched() {
        // Negative: ["fetch", "OtherEntity"] where OtherEntity is NOT the
        // from-entity. The walker must leave it as-is.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut expr = SExpression::Array(vec![s("fetch"), s("OtherEntity")]);
        let snapshot = expr.clone();
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr, snapshot,
            "fetch target that isn't the from-entity must be untouched"
        );
    }

    #[test]
    fn apply_overrides_to_trait_rewrites_fetch_and_persist_via_linked_entity() {
        // End-to-end: build a trait whose transitions carry fetch + persist
        // effects targeting the atom's original entity. Call
        // apply_overrides_to_trait with a linked_entity override and assert
        // both effect shapes are rewritten to the new entity.
        use orbital_core::schema::{StateMachine, StringOrArray, TransitionDefinition};

        let mut trait_def = TraitDefinition {
            name: "SourceSearchTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Source".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![
                    TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("idle".to_string()),
                        to: "loading".to_string(),
                        event: "LOAD".to_string(),
                        guard: None,
                        effects: Some(vec![SExpression::Array(vec![s("fetch"), s("Source")])]),
                    },
                    TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("editing".to_string()),
                        to: "saved".to_string(),
                        event: "SAVE".to_string(),
                        guard: None,
                        effects: Some(vec![SExpression::Array(vec![
                            s("persist"),
                            s("create"),
                            s("Source"),
                            s("@payload.data"),
                        ])]),
                    },
                ],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        };

        let linked = "Target".to_string();
        apply_overrides_to_trait(
            &mut trait_def,
            Some(&linked),
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &mut Vec::new(),
        );

        // linked_entity field flipped.
        assert_eq!(trait_def.linked_entity.as_deref(), Some("Target"));

        // Transition 0: fetch effect rewritten.
        let fetch_effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("fetch effects present")[0];
        assert_eq!(
            fetch_effect,
            &SExpression::Array(vec![s("fetch"), s("Target")]),
            "fetch effect entity slot must be rewritten by linked_entity override"
        );

        // Transition 1: persist effect rewritten (position 2 only).
        let persist_effect = &trait_def.state_machine.transitions[1]
            .effects
            .as_ref()
            .expect("persist effects present")[0];
        assert_eq!(
            persist_effect,
            &SExpression::Array(vec![
                s("persist"),
                s("create"),
                s("Target"),
                s("@payload.data"),
            ]),
            "persist create effect entity slot must be rewritten by linked_entity override"
        );
    }

    // ------------------------------------------------------------------
    // Multi-orbital imported-behavior resolution — trait/page lookups
    // must iterate every orbital in the imported `.orb`, not just
    // orbital 0.
    //
    // Bug fix: previously `inline_orbital` took `loaded.schema.orbitals
    // .first()` when building the context, so any ref like
    // `Alias.traits.X` whose X lived in orbital 1+ of a multi-orbital
    // behavior (e.g. every organism registry) failed to resolve with
    // "Trait 'X' not found in orbital 'Y'". The resolver now stashes
    // every orbital under the alias and `get_trait` / `get_page`
    // iterate them all.
    // ------------------------------------------------------------------

    /// Build a minimal multi-orbital imported behavior with two orbitals,
    /// each carrying exactly one trait. Returns the list of orbitals in
    /// the order (orbital 0, orbital 1). Used to simulate the shape of an
    /// organism registry without touching the loader or disk.
    fn make_multi_orbital_behavior() -> Vec<Orbital> {
        let orb0 = make_orbital_with_trait("ArcadeState", make_trait("ArcadeStateGameflow"));
        let orb1 = make_orbital_with_trait("ArcadeCanvas", make_trait("ArcadeCanvasGameCanvas2d"));
        vec![orb0, orb1]
    }

    #[test]
    fn multi_orbital_trait_ref_resolves_from_orbital_zero() {
        // Trait name `ArcadeStateGameflow` lives in orbital 0 of the
        // imported behavior. The resolver must find it. This case
        // already worked before the fix, but is pinned here so a future
        // regression that "helpfully" skips orbital 0 is caught.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let trait_ref = ctx
            .get_trait("Arcade", "ArcadeStateGameflow")
            .expect("trait in orbital 0 must resolve");

        match trait_ref {
            TraitReference::Inline(def) => {
                assert_eq!(def.name, "ArcadeStateGameflow");
            }
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn multi_orbital_trait_ref_resolves_from_orbital_one() {
        // Trait name `ArcadeCanvasGameCanvas2d` lives in orbital 1 of
        // the imported behavior. Before the fix this failed with
        // "Trait 'ArcadeCanvasGameCanvas2d' not found in orbital
        // 'ArcadeState'" because the resolver only searched orbital 0.
        // This is the regression test for the bug this commit fixes.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let trait_ref = ctx
            .get_trait("Arcade", "ArcadeCanvasGameCanvas2d")
            .expect("trait in orbital 1 must resolve after multi-orbital fix");

        match trait_ref {
            TraitReference::Inline(def) => {
                assert_eq!(def.name, "ArcadeCanvasGameCanvas2d");
            }
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn multi_orbital_trait_ref_missing_errors_with_behavior_label() {
        // No orbital in the imported behavior declares this trait. The
        // error must (a) use the updated `TraitNotFound` variant and
        // (b) cite the behavior's `from` path (passed as source_label
        // to `add_alias_multi`) rather than just orbital 0's name, so
        // the author knows which imported .orb is missing the symbol.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let result = ctx.get_trait("Arcade", "DoesNotExist");
        match result {
            Err(InlineError::TraitNotFound(trait_name, source_label)) => {
                assert_eq!(trait_name, "DoesNotExist");
                assert_eq!(
                    source_label, "std/behaviors/std-arcade-game",
                    "error must cite the imported behavior's `from` path, not orbital 0's name"
                );
            }
            other => panic!(
                "expected Err(TraitNotFound(\"DoesNotExist\", \
                 \"std/behaviors/std-arcade-game\")), got {:?}",
                other
            ),
        }
    }

    // ------------------------------------------------------------------
    // V4 id-keyed cross-file resolution (W3c-Rust). `get_trait_by_ref_id`
    // is tried before the name-based `get_trait`; it must resolve the SAME
    // node the name path would, and must disambiguate same-named traits
    // across orbitals that only an id can distinguish. A ref carrying only
    // a name (no id, the pre-V4 shape) must still fall back to `get_trait`
    // unchanged.
    // ------------------------------------------------------------------

    #[test]
    fn get_trait_by_ref_id_resolves_via_id_when_names_collide() {
        // Two orbitals under the same alias declare a trait with the SAME
        // name ("Dup") but distinct ids. The name-based `get_trait` cannot
        // tell them apart (first match wins); `get_trait_by_ref_id` must
        // return the one whose id actually matches.
        let orb0 = make_orbital_with_trait(
            "OrbZero",
            TraitDefinition {
                id: Some("id-zero".to_string()),
                ..make_trait("Dup")
            },
        );
        let orb1 = make_orbital_with_trait(
            "OrbOne",
            TraitDefinition {
                id: Some("id-one".to_string()),
                ..make_trait("Dup")
            },
        );
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Import",
            vec![orb0, orb1],
            "std/behaviors/dup-name".to_string(),
        );

        let resolved = ctx
            .get_trait_by_ref_id("Import", "id-one")
            .expect("id match must resolve the orbital-1 trait");
        match resolved {
            TraitReference::Inline(def) => {
                assert_eq!(def.id.as_deref(), Some("id-one"));
            }
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn get_trait_by_ref_id_none_when_absent_falls_back_to_name() {
        // A trait carrying only a name (no `id` — the pre-V4 shape) is not
        // found by id. The caller falls back to `get_trait` by name, which
        // still resolves it: the id path is strictly additive.
        let orb = make_orbital_with_trait("Orb", make_trait("Legacy"));
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi("Import", vec![orb], "std/behaviors/legacy".to_string());

        assert!(
            ctx.get_trait_by_ref_id("Import", "anything").is_none(),
            "no orbital carries a matching id, so the id path must return None"
        );
        let by_name = ctx
            .get_trait("Import", "Legacy")
            .expect("name-based fallback must still resolve");
        match by_name {
            TraitReference::Inline(def) => assert_eq!(def.name, "Legacy"),
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn get_entity_for_trait_orbital_prefers_ref_id_over_name() {
        // Mirrors the ambiguous-name scenario above at the entity level:
        // two orbitals both declare a trait named "Dup" (so the name scan
        // alone can't disambiguate), each owning its own entity. Passing
        // the orbital-1 trait's id must return orbital 1's entity, not
        // orbital 0's (which a name-only lookup would return first).
        let orb0 = make_orbital_with_trait(
            "OrbZero",
            TraitDefinition {
                id: Some("id-zero".to_string()),
                ..make_trait("Dup")
            },
        );
        let orb1 = make_orbital_with_trait(
            "OrbOne",
            TraitDefinition {
                id: Some("id-one".to_string()),
                ..make_trait("Dup")
            },
        );
        let orb1_entity_name = match &orb1.entity {
            EntityRef::Inline(e) => e.name.clone(),
            other => panic!("expected Inline entity, got {:?}", other),
        };
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Import",
            vec![orb0, orb1],
            "std/behaviors/dup-name".to_string(),
        );

        let by_id = ctx
            .get_entity_for_trait_orbital("Import", "Dup", Some("id-one"))
            .expect("id match must resolve orbital 1's entity");
        assert_eq!(by_id.name, orb1_entity_name);

        // No id given (pre-V4 shape): falls back to the existing name scan,
        // which returns the FIRST match — orbital 0's entity. Pinned here
        // so the fallback path stays unchanged.
        let by_name = ctx
            .get_entity_for_trait_orbital("Import", "Dup", None)
            .expect("name-based fallback must still resolve");
        assert_eq!(by_name.name, "OrbZeroEntity");
    }

    #[test]
    fn multi_orbital_page_ref_resolves_from_orbital_one() {
        // Same coverage as the trait-in-orbital-1 test, but for pages.
        // Pages follow the same code path in the resolver, so this
        // pins the fix for `Alias.pages.X` as well.
        use orbital_core::schema::{EntityDefinition, PageDefinition};

        let orb0 = Orbital {
            name: "ArcadeState".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "ArcadeState".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        };

        let canvas_page = PageDefinition {
            name: "ArcadeCanvasPage".to_string(),
            path: "/arcade/canvas".to_string(),
            view_type: None,
            primary_entity: None,
            is_initial: None,
            traits: None,
            sections: None,
            icon: None,
            label: None,
            id: None,
        };

        let orb1 = Orbital {
            name: "ArcadeCanvas".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "ArcadeCanvas".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: Some(vec![PageRef::Inline(canvas_page)]),
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        };

        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            vec![orb0, orb1],
            "std/behaviors/std-arcade-game".to_string(),
        );

        let page_ref = ctx
            .get_page("Arcade", "ArcadeCanvasPage")
            .expect("page in orbital 1 must resolve after multi-orbital fix");

        match page_ref {
            PageRef::Inline(def) => {
                assert_eq!(def.name, "ArcadeCanvasPage");
                assert_eq!(def.path, "/arcade/canvas");
            }
            other => panic!("expected Inline page, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // merge_imported_entity_fields — GAP-AGB-MOLECULE-ENTITY-CONTRACT
    // ------------------------------------------------------------------

    /// Helper to build a FieldDefinition with just a name and type.
    fn make_field(name: &str, ft: orbital_core::schema::FieldType) -> FieldDefinition {
        FieldDefinition {
            name: Some(name.to_string()),
            field_type: ft,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    /// Helper to extract field names from an EntityDefinition.
    fn field_names(entity: &EntityDefinition) -> Vec<String> {
        entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect()
    }

    #[test]
    fn merge_entity_fields_basic_collision_and_addition() {
        // Caller has [id, price], imported has [id, name, pendingId].
        // After merge: [id, price, name, pendingId] — id from caller wins.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "MyCartItem".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("price", FieldType::Number),
            ],
            instances: None,
            id: None,
        };
        let imported = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "CartItem".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
            id: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "price", "name", "pendingId"]);
        // Verify caller's original field count + merged fields.
        assert_eq!(caller.fields.len(), 4);
    }

    #[test]
    fn merge_entity_fields_no_collision() {
        // Caller has [id, title], imported has [description, status].
        // After merge: [id, title, description, status].
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("title", FieldType::String),
            ],
            instances: None,
            id: None,
        };
        let imported = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ImportedEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
            ],
            instances: None,
            id: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "title", "description", "status"]);
    }

    #[test]
    fn merge_entity_fields_full_collision_no_change() {
        // Caller already has all fields the imported entity declares.
        // After merge: no change.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
            id: None,
        };
        let imported = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ImportedEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
            id: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(
            names,
            vec![
                "id",
                "name",
                "description",
                "status",
                "createdAt",
                "pendingId"
            ]
        );
        assert_eq!(caller.fields.len(), 6, "no fields should be added");
    }

    #[test]
    fn merge_entity_fields_multiple_imports() {
        // Caller uses std-browse AND std-modal; each contributes different fields.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![make_field("id", FieldType::String)],
            instances: None,
            id: None,
        };
        let import_browse = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "BrowseRecord".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("status", FieldType::String),
            ],
            instances: None,
            id: None,
        };
        let import_modal = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ModalRecord".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("pendingId", FieldType::String),
                make_field("description", FieldType::String),
            ],
            instances: None,
            id: None,
        };

        // First import adds name, status.
        merge_imported_entity_fields(&mut caller, &import_browse, false, &HashSet::new());
        // Second import adds pendingId, description (id already present).
        merge_imported_entity_fields(&mut caller, &import_modal, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(
            names,
            vec!["id", "name", "status", "pendingId", "description"]
        );
    }

    #[test]
    fn merge_entity_fields_no_uses_no_change() {
        // Standalone orbital with no imports — entity unchanged.
        use orbital_core::schema::FieldType;

        let caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("title", FieldType::String),
            ],
            instances: None,
            id: None,
        };
        let original_len = caller.fields.len();

        // No imports to merge from — just confirm nothing changes.
        let names_before = field_names(&caller);
        // (In the real flow, if there are no uses, the merge loop is never entered.)
        assert_eq!(caller.fields.len(), original_len);
        assert_eq!(field_names(&caller), names_before);
    }

    #[test]
    fn merge_entity_fields_self_relation_retargets_id_with_name() {
        // Caller (rebind target) has its own stamped id; imported entity has a
        // self-relational field (e.g. ThreadPost.replies : [ThreadPost]) whose
        // relation must be retargeted to the caller BOTH by name and by id — a
        // dual-carry mismatch (ORB_ID_NAME_MISMATCH) fires if the id sibling is
        // left pointing at the imported entity's own ledger row.
        use orbital_core::schema::{Cardinality, FieldType, RelationDefinition};

        let mut caller = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "Ticket".to_string(),
            collection: None,
            persistence: None,
            fields: vec![make_field("id", FieldType::String)],
            instances: None,
            id: Some("ent_CALLER_TICKET".to_string()),
        };

        let mut replies_field = make_field("replies", FieldType::Relation);
        replies_field.relation = Some(RelationDefinition {
            entity: "ThreadPost".to_string(),
            entity_id: Some("ent_IMPORTED_THREADPOST".to_string()),
            cardinality: Some(Cardinality::Many),
        });
        let imported = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ThreadPost".to_string(),
            collection: None,
            persistence: None,
            fields: vec![make_field("id", FieldType::String), replies_field],
            instances: None,
            id: Some("ent_IMPORTED_THREADPOST".to_string()),
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let merged = caller
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("replies"))
            .expect("replies field must be merged onto the caller entity");
        let rel = merged
            .relation
            .as_ref()
            .expect("merged replies field must keep its relation");

        assert_eq!(rel.entity, "Ticket", "relation name must retarget to the caller entity");
        assert_eq!(
            rel.entity_id.as_deref(),
            Some("ent_CALLER_TICKET"),
            "relation id must move together with the name to the caller entity's own id"
        );
        assert_ne!(
            rel.entity_id.as_deref(),
            Some("ent_IMPORTED_THREADPOST"),
            "the imported source entity's id must not survive the retarget"
        );
    }

    #[test]
    fn get_entity_for_trait_orbital_finds_correct_orbital() {
        // Multi-orbital import: trait lives in orbital 1, its entity should
        // be returned (not orbital 0's entity).
        use orbital_core::schema::FieldType;

        let orb0 = Orbital {
            name: "StateOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "StateEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![make_field("stateField", FieldType::String)],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("StateTrait"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        };

        let orb1 = Orbital {
            name: "ActionOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "ActionEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![
                    make_field("actionField", FieldType::String),
                    make_field("pendingId", FieldType::String),
                ],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("ActionTrait"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        };

        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Import",
            vec![orb0, orb1],
            "std/behaviors/test-import".to_string(),
        );

        // Asking for ActionTrait's orbital should return ActionEntity.
        let entity = ctx
            .get_entity_for_trait_orbital("Import", "ActionTrait", None)
            .expect("should find entity for ActionTrait's orbital");
        assert_eq!(entity.name, "ActionEntity");
        assert_eq!(field_names(&entity), vec!["actionField", "pendingId"]);

        // Asking for StateTrait's orbital should return StateEntity.
        let entity = ctx
            .get_entity_for_trait_orbital("Import", "StateTrait", None)
            .expect("should find entity for StateTrait's orbital");
        assert_eq!(entity.name, "StateEntity");
        assert_eq!(field_names(&entity), vec!["stateField"]);

        // Asking for a non-existent trait should return None.
        assert!(ctx
            .get_entity_for_trait_orbital("Import", "NoSuchTrait", None)
            .is_none());
    }
}
