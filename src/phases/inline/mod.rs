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
mod effect_row;
mod entity;
mod event_id_hygiene;
mod host_entity;
mod identity_normalize;
mod orbital;
mod page;
pub(crate) mod rewrite;
mod service;
mod sigil_types;
mod splice;
#[path = "trait.rs"]
mod r#trait;
mod trait_value_refs;

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
use entity::{
    collect_trait_referenced_entity_fields, inline_entity, merge_imported_entity_fields,
    merge_into_named_entity, trait_referenced_fields_of_entity_recursive,
};
// Re-exported (not just used here) so `phases::validation::entity_mutation`'s
// post-inline ORB_E_MERGED_FIELD_UNREFERENCED rule shares the SAME
// own-entity-vs-cross-entity field-usage primitive the §52 travel-copy
// filter gates a traveled auxiliary's merged fields on — one definition,
// not two.
pub(crate) use entity::trait_referenced_fields_of_entity;
use page::inline_pages;
use r#trait::inline_traits;
// Re-exported at the `inline` namespace so validation passes (e.g.
// `validation::entity_mutation`) can reach it as
// `crate::phases::inline::collect_sibling_trait_refs`. The fn lives in the
// private `r#trait` submodule as `pub(crate)`; without this re-export the path
// is E0425 (latent — surfaces whenever orbital-compiler recompiles).
pub(crate) use r#trait::{collect_sibling_trait_refs, sibling_trait_refs_of};
// Re-exported so `phases::validation::trait_ref_config` (and
// `inline_schema_with_config_diagnostics`'s public signature above) can name
// the type as `crate::phases::inline::TraitRefConfigDiagnostic` without
// reaching into the private `r#trait` submodule directly.
pub use r#trait::TraitRefConfigDiagnostic;
// Diagnostics collected while resolving `orbital X = Alias.orbitals.Y { … }`
// references — re-exported so `phases::validation` (W4-R) can name them.
use orbital::{
    derive_materialized_id, effective_page_remap, fold_declared_config_override,
    materialized_primary_entity, materialized_trait_subs, only_closure, resolve_orbital_reference,
    SiblingImportInfo,
};
pub use orbital::{OrbitalRefDiagnostic, UsesConfigDiagnostic};
use rewrite::{apply_inline_config_defaults, trait_ref_name};
use service::inline_services;
use splice::splice_lambda_trait_refs;

// Test-only helpers exercised directly by the unit tests below; gated so a
// non-test build doesn't flag them as unused.
#[cfg(test)]
use r#trait::apply_overrides_to_trait;
#[cfg(test)]
use rewrite::{rewrite_entity_refs, rewrite_identifiers, rewrite_trait_event_fields};

use crate::loader::{ExternalOrbitalLoader, ImportChain, LoaderConfig, LoaderError};
use orbital_core_base::schema::{
    ConfigField, ConfigProvenanceRecord, EntityDefinition, EntityRef, ExpectDeclaration,
    ListenSource, Orbital, OrbitalSchema, PayloadField, SchemaMetadata, ThemeRef, TraitDefinition,
    TraitReference,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

// orbital_core schema types referenced only by the `tests` module via
// `use super::*`. Gated so a non-test build doesn't flag them as unused.
#[cfg(test)]
use orbital_core_base::schema::{
    EntityRefExtends, FieldDefinition, PageRef, SExpression, ServiceDefinition, ServiceRef,
    ServiceRefObject,
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

    #[error("Orbital '{0}' not found in imported behavior '{1}'")]
    OrbitalNotFound(String, String),

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

    #[error(
        "Struct-alias '{name}' at {path} is declared BOTH locally and by an imported trait's \
         `::` argument / declaration default with a DIFFERENT shape — ambiguity is never \
         resolved silently. Rename one of the two declarations."
    )]
    ConflictingTypeAlias {
        /// The colliding struct-alias name.
        name: String,
        /// Source-tagged path to the consuming orbital.
        path: String,
    },

    #[error(
        "Orbital import `listens` at {path} routes into '{target}', which is not a trait of the imported set (after omit/only)"
    )]
    OrbitalRefListenTargetUnknown {
        /// The upstream trait name the route names as its target.
        target: String,
        /// Source-tagged path to the importing orbital.
        path: String,
    },

    #[error("Duplicate import alias '{alias}' at {path} — each `uses` alias names one import")]
    DuplicateUseAlias {
        /// The alias declared more than once in one orbital's `uses`.
        alias: String,
        /// Source-tagged path to the repeated declaration.
        path: String,
    },

    #[error(
        "'{alias}' is imported with `uses lazy` at {path}, so only a whole page-bearing \
         `orbital X = {alias}.orbitals.Y` can use it — its traits, entity and pages are not \
         available to an eager orbital"
    )]
    LazyEagerRef { alias: String, path: String },

    #[error(
        "Orbital '{orbital}' is imported lazily but declares no pages, so nothing can ever load it"
    )]
    LazyNoPages { orbital: String },

    #[error(
        "Orbital '{orbital}' references a `uses lazy` import; split it out with \
         `phases::lazy::split_lazy_and_resolve` before inlining"
    )]
    LazyNotSplit { orbital: String },

    #[error(
        "Page reference '{reference}' at {path} sets `access: public` on an upstream page declared \
         `access: authenticated` — an import cannot weaken inherited protection"
    )]
    PageAccessWeakened { reference: String, path: String },

    #[error("Behavior value '{literal}' at {path} does not resolve: {reason}")]
    BehaviorRefUnresolved {
        literal: String,
        path: String,
        reason: String,
    },

    #[error(
        "`behavior/ref` at {path} takes a computed argument; a behavior that comes from data \
         must arrive as a `behavior` value (the `value` of a `program/eval` result or a catalog \
         entry), not as text re-parsed at run time"
    )]
    BehaviorRefNotLiteral { path: String },

    #[error("`unquote` at {path} appears outside a quasiquote body")]
    UnquoteOutsideQuasiquote { path: String },
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
            InlineError::ConflictingTypeAlias { path, .. } => path,
            InlineError::DuplicateUseAlias { path, .. } => path,
            InlineError::OrbitalRefListenTargetUnknown { path, .. } => path,
            InlineError::LazyEagerRef { path, .. } => path,
            InlineError::PageAccessWeakened { path, .. } => path,
            InlineError::BehaviorRefUnresolved { path, .. } => path,
            InlineError::BehaviorRefNotLiteral { path } => path,
            InlineError::UnquoteOutsideQuasiquote { path } => path,
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
            InlineError::ConflictingTypeAlias { .. } => "ORB_T_CONFLICTING_TYPE_ALIAS",
            InlineError::DuplicateUseAlias { .. } => "ORB_O_DUPLICATE_USE_ALIAS",
            InlineError::OrbitalRefListenTargetUnknown { .. } => "ORB_O_LISTEN_TARGET_UNKNOWN",
            InlineError::LazyEagerRef { .. } => "ORB_LAZY_EAGER_REF",
            InlineError::LazyNoPages { .. } => "ORB_LAZY_NO_PAGES",
            InlineError::LazyNotSplit { .. } => "ORB_LAZY_NOT_SPLIT",
            InlineError::PageAccessWeakened { .. } => "ORB_O_PAGE_ACCESS_WEAKENED",
            InlineError::BehaviorRefUnresolved { .. } => "ORB_T_BEHAVIOR_REF_UNRESOLVED",
            InlineError::BehaviorRefNotLiteral { .. } => "ORB_T_BEHAVIOR_REF_NOT_LITERAL",
            InlineError::UnquoteOutsideQuasiquote { .. } => "ORB_T_UNQUOTE_OUTSIDE_QUASIQUOTE",
            InlineError::Loader(LoaderError::PackageNotInstalled { .. })
            | InlineError::Loader(LoaderError::BehaviorPackageNotInstalled { .. }) => {
                "ORB_BEHAVIOR_PACKAGE_NOT_INSTALLED"
            }
            InlineError::Loader(LoaderError::BehaviorNotFound { .. }) => "ORB_BEHAVIOR_NOT_FOUND",
            InlineError::Loader(LoaderError::BehaviorAmbiguous { .. }) => "ORB_BEHAVIOR_AMBIGUOUS",
            InlineError::Loader(LoaderError::BehaviorPackage(
                orbital_core_base::behavior_packages::DiscoverError::DuplicateBehaviorName { .. },
            )) => "ORB_BEHAVIOR_NAME_DUPLICATE",
            InlineError::Loader(LoaderError::BehaviorPackage(_)) => "ORB_BEHAVIOR_PACKAGE_INVALID",
            InlineError::Loader(LoaderError::PrefixConflict { .. }) => {
                "ORB_BEHAVIOR_PREFIX_CONFLICT"
            }
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
    /// Shared across every schema of one program (the program and its lazy
    /// behaviors): loaded files and each imported behavior's inlined result.
    /// `None` gives each inline call its own.
    pub session: Option<InlineSession>,
}

/// One program's inline state, shared by clones: the loader (parsed files)
/// and each imported behavior's inlined result, so a behavior imported in many
/// places is loaded and inlined once.
#[derive(Clone, Default)]
pub struct InlineSession(std::rc::Rc<std::cell::RefCell<InlineSessionState>>);

impl InlineSession {
    /// Every behavior file the last inline through this session loaded, as
    /// parsed (before `@config.<knob>` reads were substituted).
    pub fn each_loaded_schema(&self, visit: &mut dyn FnMut(&crate::loader::LoadedSchema)) {
        if let Some(loader) = &self.0.borrow().loader {
            loader.each_touched_schema(visit);
        }
    }
}

impl std::fmt::Debug for InlineSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InlineSession")
    }
}

#[derive(Default)]
struct InlineSessionState {
    loader: Option<ExternalOrbitalLoader>,
    imports: ImportMemo,
}

/// An imported behavior inlined once (keyed by its canonical file): its
/// orbitals as `inline_orbital` left them, plus everything that work reported
/// and loaded, replayed on reuse so each use reports exactly what it did before.
#[derive(Clone)]
struct InlinedImport {
    orbitals: std::rc::Rc<Vec<Orbital>>,
    provenance: Vec<ConfigProvenanceRecord>,
    config_diagnostics: Vec<r#trait::TraitRefConfigDiagnostic>,
    orbital_ref_diagnostics: Vec<orbital::OrbitalRefDiagnostic>,
    uses_config_diagnostics: Vec<orbital::UsesConfigDiagnostic>,
    deferred_merges: Vec<DeferredMerge>,
    loaded_files: HashSet<PathBuf>,
}

type ImportMemo = HashMap<PathBuf, InlinedImport>;

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
    let mut orbital_ref_diagnostics = Vec::new();
    let mut uses_config_diagnostics = Vec::new();
    inline_schema_with_orbital_diagnostics(
        schema,
        config,
        source_path,
        diagnostics,
        &mut orbital_ref_diagnostics,
        &mut uses_config_diagnostics,
    )
}

/// Same as [`inline_schema_with_config_diagnostics`], additionally collecting
/// the two orbital-import diagnostic classes (Almadar_Orbital_Import.md §4.5,
/// §8): a call-site `config {}` override on an `orbital X = Alias.orbitals.Y
/// { … }` reference (or on the `uses … { config }` that feeds it) naming a
/// knob the upstream doesn't declare, and an `omit {}` that drops a trait a
/// surviving trait still embeds via `@trait.<X>`. W4-R's validators turn
/// these into `ORB_O_CONFIG_UNKNOWN_KEY` / `ORB_O_OMIT_EMBEDDED_TRAIT` /
/// `ORB_U_CONFIG_UNKNOWN_KEY`.
///
/// A separate entry point — rather than adding params directly to
/// [`inline_schema_with_config_diagnostics`] — because that function's
/// production callers in `orbital-cli` are outside this crate (W3-R's scope
/// is `orbital-compiler` only); this keeps their call sites untouched while
/// still surfacing the new diagnostics to whichever caller wants them.
pub fn inline_schema_with_orbital_diagnostics(
    schema: &mut OrbitalSchema,
    config: &InlineConfig,
    source_path: Option<&Path>,
    diagnostics: &mut Vec<r#trait::TraitRefConfigDiagnostic>,
    orbital_ref_diagnostics: &mut Vec<orbital::OrbitalRefDiagnostic>,
    uses_config_diagnostics: &mut Vec<orbital::UsesConfigDiagnostic>,
) -> Result<(), InlineError> {
    if config.skip_inline {
        return Ok(());
    }

    if let Some(orbital) = schema
        .orbitals
        .iter()
        .find(|o| super::lazy::is_lazy_reference(o))
    {
        return Err(InlineError::LazyNotSplit {
            orbital: orbital.name.clone(),
        });
    }

    // Intrinsic identity, belt-and-suspenders: `resolve_schema_with_path`
    // already stamps before calling inline, but direct `inline_schema` callers
    // (unit tests, tooling) may hand in a ledger-less schema. Stamp here too so
    // the id side-maps (`trait_embed_ids`/`entity_ref_ids`) always cover their
    // tokens by the time the cross-orbital dedup rename runs — the invariant
    // that lets the name-fallback rewriters be deleted. No-op when stamped.
    orbital_core_base::stamp::ensure_stamped(schema);

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

    let session = config.session.clone().unwrap_or_default();
    let mut session_state = session.0.borrow_mut();
    let InlineSessionState { loader, imports } = &mut *session_state;
    let loader =
        loader.get_or_insert_with(|| ExternalOrbitalLoader::new(config.loader_config.clone()));
    loader.begin_schema();
    let mut chain = ImportChain::new();

    // If we have a source path, add it to the chain
    if let Some(path) = source_path {
        chain.push(path);
    }

    trait_value_refs::resolve_trait_value_refs(schema, loader, &chain, source_path)?;

    // Computed once: threaded into every orbital as the outermost §4.5
    // config-forward rung. Cloned (not borrowed) so the per-orbital loop
    // below can hold `&mut schema.orbitals` without fighting the borrow
    // checker over a second field on the same `schema`.
    let schema_config = schema.config.clone();

    // Stage B: every CONSUMER orbital's own primary + auxiliary entity name
    // → id, built ONCE here (before any orbital-reference resolves) so an
    // `entities {}` remap can repoint a relation at an already-declared
    // consumer entity and know its id. Only `EntityRef::Inline` entities
    // count — a reference-form orbital's own (not-yet-resolved) placeholder
    // entity is excluded by construction, which is exactly right: its real
    // entity doesn't exist yet at this point, so it can't be a valid
    // `entities {}` target anyway.
    let mut consumer_entity_ids: HashMap<String, String> = inline_entity_ids(&schema.orbitals);

    // (B) fix: the build above excludes reference-form orbitals BY
    // CONSTRUCTION (their real primary entity doesn't exist until THEY
    // resolve), which made the approved two-import Project Friday shape
    // order-dependent — orbital N's `entities {}` naming orbital M's own
    // materialized primary only worked if M happened to already be inline.
    // Add every reference-form orbital's OWN materialized primary name →
    // derived id too, so `entities {}` targeting works ORDER-FREE across
    // sibling imports in the same file. `extend` never overwrites an
    // already-inline orbital's OWN entry (this schema's orbitals are either
    // inline XOR reference-form, never both, so no name can collide with
    // itself) — the shallow pre-pass below only ever adds NEW keys.
    consumer_entity_ids.extend(precompute_reference_form_primary_ids(
        &schema.orbitals,
        loader,
        &mut chain,
        source_path,
    ));

    // G1 (`docs/Almadar_Compiler_Gaps.md` §82): same order-free precompute,
    // for listen-source rewriting instead of entity ids — built once, before
    // any orbital in `schema` resolves, so a listen source naming ANY
    // reference-form orbital of this schema resolves regardless of
    // declaration order.
    let (listen_source_targets, imported_trait_names) =
        precompute_reference_form_listen_targets(&schema.orbitals, loader, &mut chain, source_path);

    // Process each orbital, recording the names the auto-pull loop added and
    // the config→knob provenance for substituted render-ui props.
    let mut pulled_by_orbital: Vec<(usize, Vec<String>)> = Vec::new();
    let mut all_provenance: HashMap<String, Vec<ConfigProvenanceRecord>> = std::collections::HashMap::new();
    // One orbital's inline failure must not leave its SIBLINGS un-inlined: a
    // `?` here abandoned every later orbital, and validators (which index
    // Inline traits only) then cascaded spurious ORB_T_UNDEFINED_TRAIT /
    // ORB_X_LISTEN_SOURCE_UNRESOLVED findings across the rest of the file.
    // Inline everything that resolves, then report the first real failure.
    let mut first_error: Option<InlineError> = None;
    // §52: rebind-merge targets no orbital could resolve locally, retried
    // schema-wide once every orbital below has finished inlining — see
    // `DeferredMerge`.
    let mut deferred_merges: Vec<DeferredMerge> = Vec::new();
    for (idx, orbital) in schema.orbitals.iter_mut().enumerate() {
        let mut pulled: Vec<String> = Vec::new();
        let mut provenance: Vec<ConfigProvenanceRecord> = Vec::new();
        let lazy_aliases: HashSet<String> = orbital
            .uses
            .iter()
            .flatten()
            .filter(|u| u.lazy == Some(true))
            .map(|u| u.alias.clone())
            .collect();
        let orbital_name = orbital.name.clone();
        if let Err(e) = inline_orbital(
            orbital,
            loader,
            &mut chain,
            source_path,
            true,
            Some(&mut pulled),
            &mut provenance,
            diagnostics,
            schema_config.as_ref(),
            &consumer_entity_ids,
            &listen_source_targets,
            orbital_ref_diagnostics,
            uses_config_diagnostics,
            Some(idx),
            &mut deferred_merges,
            imports,
        ) {
            let e = match e {
                InlineError::UnknownAlias(alias) if lazy_aliases.contains(&alias) => {
                    InlineError::LazyEagerRef {
                        path: format!("orbitals[name={orbital_name}]"),
                        alias,
                    }
                }
                e => e,
            };
            if first_error.is_none() {
                first_error = Some(e);
            }
            continue;
        }
        // Feed this orbital's now-final entity set — including any
        // NO-REBIND sibling pull `inline_orbital` just merged into
        // `orbital.auxiliary_entities` (a trait composed with no `-> X`
        // override keeps the SOURCE atom's own bound entity, carried in
        // verbatim with the atom's OWN id, e.g. `Audit.traits.
        // AuditCaptureListener` pulling `AuditEntry`) — FORWARD into
        // `consumer_entity_ids` for every orbital still to come. Without
        // this, a LATER orbital composing the SAME entity NAME through a
        // DIFFERENT atom with an EXPLICIT rebind (`Browse.traits.
        // BrowseItemBrowse -> AuditEntry`, std-api-gateway's
        // `GatewayUserOrbital.AuditBrowseList`) never finds "AuditEntry" in
        // its own `entity_ids` snapshot (built once, before ANY orbital
        // inlines) and keeps the rebind atom's foreign id
        // (`ORB_ID_UNKNOWN_REF`) even though an EARLIER orbital in this same
        // file already established the canonical id for that name. This
        // orbital's own names still win within itself via
        // `collect_orbital_entity_ids` layered on top inside
        // `inline_orbital` — this only widens what LATER orbitals see.
        // First id wins: an entity name keeps the id the schema established first (an owner's
        // primary, or an import's precomputed one) — a later orbital's pulled same-named copy
        // never re-points it to a foreign id.
        for (name, id) in collect_orbital_entity_ids(orbital) {
            consumer_entity_ids.entry(name).or_insert(id);
        }
        // Increment 3-I (`docs/Almadar_LOLO.md` ~line 315/1387): now that this
        // orbital's traits are fully inlined — every `linkedEntity` rebind,
        // sibling-pull, and auxiliary-entity import resolved — monomorphize
        // any `$E`/`[$E]` type-param sentinel an event alias's payload
        // carries against the trait's FINAL bound entity. Must run before
        // `uniquify_cross_orbital_pulled_siblings` reads `orbital.traits`
        // only for renaming, so ordering here doesn't matter relative to
        // that call; it must simply happen once per orbital, post-inline.
        rewrite::resolve_orbital_type_param_sentinels(orbital);

        // The trait bodies are final here — call-site config merged and
        // substituted, every ref materialized — so the effect row L1 stamped
        // from single-file knowledge is now re-derivable with real resources.
        effect_row::recompute_effect_rows(orbital);

        if !pulled.is_empty() {
            pulled_by_orbital.push((idx, pulled));
        }
        if !provenance.is_empty() {
            // Deterministic order: `record_config_provenance` walks each
            // trait's render-ui pattern nodes (`HashMap<String,
            // SExpression>` props) and config-default trees, so the order
            // records land in `provenance` for one trait is a hash-iteration
            // artifact — it can differ between runs even though the SET of
            // `{trait, patternPath, prop, knob}` records is identical.
            // Canonicalize once here, at the point `_metadata.configProvenance`
            // is finalized for this orbital, rather than reordering the
            // render-ui walker itself. (Found pre-existing — this recorder
            // and its walk predate the orbital-import program.)
            provenance.sort_by(|a, b| {
                (&a.trait_name, &a.pattern_path, &a.prop, &a.knob).cmp(&(
                    &b.trait_name,
                    &b.pattern_path,
                    &b.prop,
                    &b.knob,
                ))
            });
            all_provenance.insert(orbital.name.clone(), provenance);
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    address_imported_traits(&mut schema.orbitals, &imported_trait_names);

    // An entity another orbital owns as its primary is that entity: a copy pulled in as an
    // auxiliary (a sibling trait still bound to its source behavior's same-named entity) would
    // shadow it by name and drop the fields only the owner carries. Before the retry, so merges
    // land on the owner.
    // §52 schema-wide retry: every orbital has now inlined (every Gap #22
    // no-rebind auxiliary registered, every primary entity declared), so a
    // merge target unresolved LOCALLY may resolve here — a later no-rebind
    // import in the same orbital, or a sibling orbital's own entity. A
    // target still unresolved after this is left unmerged; there is no
    // further fallback, and `ORB_T_REBIND_TARGET_UNRESOLVED` reports it.
    settle_shadowed_auxiliaries(&mut schema.orbitals, &deferred_merges);

    uniquify_cross_orbital_pulled_siblings(schema, &pulled_by_orbital);

    // Preserve cross-file identity: copy the ledger rows of any source id that a
    // cloned node still references but the consumer arena/ledger doesn't carry.
    // Runs after all orbitals inline (so every reference position is final) and
    // uses the loader's caches as the source-ledger source of truth.
    merge_source_ledgers(schema, &loader.source_ledger_union());

    // Expansion copies an atom's baked event ids into every `use` instance, so
    // a multi-bound atom leaves one id declared by N traits — ambiguous for
    // every id→name reader (the identity-normalize clobber, id-primary
    // dispatch). Re-derive per instance (deterministic, `derive_id`) and
    // reconcile the ledger; runs after the source-ledger merge so retired rows'
    // metadata is copyable onto the derived rows. No shared ids → no-op.
    event_id_hygiene::uniquify_instance_event_ids(schema);

    // G5 (`docs/Almadar_Compiler_Gaps.md` §82): an orbital reference's
    // `events { OLD: NEW }` rename changes every event KEY inside
    // `resolve_orbital_reference`, which has no `schema.ledger` access (it
    // operates on one `Orbital`) — so a renamed declaration's id keeps the
    // ledger's PRE-rename `curName`, tripping `ORB_ID_NAME_MISMATCH`. Runs
    // right after the sibling-instance id-hygiene pass above so both see a
    // ledger-present, fully-inlined schema; a schema with no `events {}}`
    // orbital reference is a no-op.
    reconcile_orbital_ref_event_id_renames(schema, orbital_ref_diagnostics.as_slice());

    // An imported auxiliary entity never displaces a consumer's own entity.
    drop_unreferenced_colliding_auxiliaries(schema);

    orbital::bind_inherited_entity_expectations(schema, orbital_ref_diagnostics.as_mut_slice());

    // Before sigil types, so `@entity <knob>` sees the binding.
    host_entity::bind_unlinked_traits_to_host_entity(schema);

    // Sigil type constructors (`@payload <knob>`, `@entity <knob>`, `@user`)
    // resolve here: after every orbital has inlined, so a knob's value is the
    // call site's folded override and an event declared on a SIBLING orbital is
    // visible. Unresolved sentinels are surfaced by the caller as blocking
    // errors — never left to ship as a literal string.
    // Anything it cannot resolve is left as a sentinel and reported by the
    // Phase-5f sweep — resolution mutates, reporting validates, same split the
    // type-parameter sentinels already use.
    sigil_types::resolve_sigil_types(schema);

    // §61 OPEN 1, FIX 2: once every trait has inlined (so FIX 1's own
    // narrowing above has already run per-trait), rewrite the ui-input
    // atom's OWN internal display-buffer `set` to stringify the now-numeric
    // wire value back into its string-typed entity field. Whole-schema for
    // the same reason `resolve_sigil_types` is: a rebind target entity can
    // live on a sibling orbital.
    r#trait::rewrite_narrowed_number_display_writes(schema);

    // Emit the config→knob provenance into `_metadata.configProvenance` so a
    // consumer (Studio contextual edit) can trace a substituted render-ui
    // literal back to the config knob it came from.
    if !all_provenance.is_empty() {
        match schema.metadata.as_mut() {
            Some(meta) => meta.config_provenance = Some(all_provenance),
            None => {
                schema.metadata = Some(SchemaMetadata {
                    message_sources: None,
                    version: None,
                    created_at: None,
                    source: None,
                    updated_at: None,
                    config_provenance: Some(all_provenance),
                });
            }
        }
    }

    if schema.locales.is_some() {
        if let Some(root) = source_path {
            let sources = loader.message_sources(root, &schema.name);
            schema
                .metadata
                .get_or_insert_with(SchemaMetadata::default)
                .message_sources = Some(sources);
        }
    }

    // The wire contract (Almadar_Orbital_Import.md §6): `reference` is
    // transient, set only by lowering an unresolved `orbital X =
    // Alias.orbitals.Y { … }` and must never survive inline into validation,
    // codegen, or a committed `.orb`. Every orbital (including ones loaded
    // purely as imports, never spliced into `schema.orbitals` — those never
    // carried a reference at this level to begin with) must show `None` here.
    debug_assert!(
        schema.orbitals.iter().all(|o| o.reference.is_none()),
        "orbital.reference must be cleared by inline_orbital's resolve_orbital_reference call"
    );

    Ok(())
}

/// Drop an imported auxiliary entity when its name collides with a PRIMARY
/// entity the consumer declares AND nothing in the importing orbital references
/// it.
///
/// `uses` imports individual TRAITS, but auxiliary entities travel per ORBITAL:
/// the inliner carries the source orbital's whole closure, because a trait body
/// can fetch or persist a sibling entity by name and stranding that effect fails
/// at runtime. The granularity mismatch means asking for one browse trait can
/// hand you an unrelated subsystem — importing `std-hr-portal`'s
/// `TimeOffBrowseList` carries seven auxiliaries, among them `CalendarEvent` and
/// `CalendarSyncRun` from the `std-calendar-sync` that orbital also composes.
///
/// When the consumer already declares its own `CalendarEvent`, that is a hard
/// `DuplicateDefinition` — and unlike a trait's bound entity there is no call
/// site to rebind, because no imported trait named it. This is the same shape as
/// the `[identity]` roster rule (a primary roster shadows an imported copy),
/// minus the tag.
///
/// The guard is what makes it safe: the name must appear NOWHERE in the
/// importing orbital's resolved traits. Then dropping it strands nothing — there
/// is no fetch, persist, `linkedEntity` or binding left to resolve. If an
/// imported trait DOES use the name, the collision stays a hard error, which is
/// correct: two different shapes under one name, both live, is a real conflict
/// only the consumer can resolve.
fn drop_unreferenced_colliding_auxiliaries(schema: &mut OrbitalSchema) {
    let primary_names: HashSet<String> = schema
        .orbitals
        .iter()
        .filter_map(|o| match &o.entity {
            EntityRef::Inline(def) => Some(def.name.clone()),
            _ => None,
        })
        .collect();
    if primary_names.is_empty() {
        return;
    }

    for orbital in schema.orbitals.iter_mut() {
        let own_primary = match &orbital.entity {
            EntityRef::Inline(def) => Some(def.name.clone()),
            _ => None,
        };
        let Some(aux) = orbital.auxiliary_entities.as_mut() else {
            continue;
        };
        // Conservative reference test over the RESOLVED traits: if the name
        // appears anywhere in them we keep the entity, so this can only ever
        // narrow where the analysis is provably complete.
        let traits_blob = serde_json::to_string(&orbital.traits).unwrap_or_default();
        aux.retain(|e| {
            let EntityRef::Inline(def) = e else {
                return true;
            };
            // Only a collision with ANOTHER orbital's primary is at issue; an
            // aux matching this orbital's own primary is already filtered when
            // the list is built.
            if own_primary.as_deref() == Some(def.name.as_str()) {
                return true;
            }
            if !primary_names.contains(&def.name) {
                return true;
            }
            // `retain` KEEPS on true: hold the entity only while something
            // still names it. Unreferenced + colliding is the drop case.
            trait_blob_mentions(&traits_blob, &def.name)
        });
    }
}

/// Whether a resolved-trait blob names this entity. Matched on the quoted JSON
/// token so `Task` does not match `TaskDraft`.
fn trait_blob_mentions(blob: &str, entity: &str) -> bool {
    blob.contains(&format!("\"{}\"", entity))
        || blob.contains(&format!("\"{}.", entity))
        || blob.contains(&format!("@{}.", entity))
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
                                    orbital_core_base::identity::derive_id(&source_id, &orbital_name);
                                td.id = Some(copy_id.clone());
                                ledger_ops.push((source_id, new_name.clone(), Some(copy_id)));
                            }
                        }
                    }
                    // A sibling-listening parent in the same orbital points its
                    // `listens` source at the old sibling name; track the rename.
                    if let Some(listens) = td.listens.as_mut() {
                        for listen in listens.iter_mut() {
                            if let Some(orbital_core_base::schema::ListenSource::Trait {
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
                let at = orbital_core_base::time_util::now_rfc3339();
                for (source_id, new_name, copy_id) in &ledger_ops {
                    match copy_id {
                        None => ledger.rename(source_id, new_name, &at),
                        Some(copy_id) => {
                            if let Some(src) = ledger.entries.get(source_id).cloned() {
                                let mut rename_hist = src.renames;
                                rename_hist.push(orbital_core_base::identity::RenameRecord {
                                    from: src.cur_name,
                                    to: new_name.clone(),
                                    at: at.clone(),
                                });
                                ledger.entries.insert(
                                    copy_id.clone(),
                                    orbital_core_base::identity::LedgerEntry {
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

/// (B) fix: the materialized primary name → derived id of every
/// REFERENCE-FORM orbital this schema declares (`orbital X = Alias.orbitals.Y
/// { … }`), computed order-free — WITHOUT requiring any other orbital in
/// `schema` to have resolved first, and without recursively inlining the
/// upstream. Each reference's alias is loaded SHALLOWLY (`loader.load`,
/// which caches by path — the REAL load `inline_orbital`'s own `uses` loop
/// performs moments later is then a cache hit, no duplicate work) and its
/// named orbital looked up directly in the loaded (un-inlined) schema: an
/// upstream's OWN primary entity id/name never change through inline
/// (zero-churn stamping — only a CONSUMER-side CLONE gets a fresh derived
/// id), so the shallow load's `entity.id` is exactly what the real resolve
/// pass would see too. Uses the SAME `materialized_primary_entity` pair
/// `resolve_orbital_reference` computes for itself — one owner. Best-effort:
/// a reference this pre-pass cannot resolve (unknown alias, malformed
/// reference string, upstream orbital not found) is silently skipped here —
/// the REAL `resolve_orbital_reference` call moments later still surfaces
/// the actual `InlineError` for it.
fn precompute_reference_form_primary_ids(
    orbitals: &[Orbital],
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for orbital in orbitals {
        let Some(reference) = orbital.reference.as_ref() else {
            continue;
        };
        let Ok(ParsedReference::Orbital { alias, name }) = parse_reference(&reference.reference)
        else {
            continue;
        };
        let Some(use_decl) = orbital.uses.iter().flatten().find(|u| u.alias == alias) else {
            continue;
        };
        let Ok(loaded) = loader.load(&use_decl.from, source_path, chain) else {
            continue;
        };
        let Some(upstream) = loaded
            .schema
            .orbitals
            .iter()
            .find(|o| o.name == name)
            .cloned()
        else {
            continue;
        };
        let upstream_path = loaded.source_path.clone();
        if let Some((primary_name, primary_id)) =
            materialized_primary_entity(&upstream, reference, &orbital.name)
        {
            out.insert(primary_name, primary_id);
        } else if let Some((up_name, up_id, upstream_id)) =
            flattened_primary_entity(&upstream, loader, chain, Some(&upstream_path), 0)
        {
            let primary_name = reference
                .entity
                .clone()
                .unwrap_or_else(|| format!("{}{up_name}", orbital.name));
            let primary_id = derive_materialized_id(
                up_id.as_deref(),
                upstream_id.as_deref(),
                &up_name,
                &orbital.name,
                orbital_core_base::identity::IdKind::Entity,
            );
            out.insert(primary_name, primary_id);
        }
    }
    out
}

/// An orbital's primary entity once its own import (if any) flattens: (name,
/// id, the orbital's effective id). Recursive through an import of an import;
/// mirrors `resolve_orbital_reference`'s primary naming and id derivation.
fn flattened_primary_entity(
    orbital: &Orbital,
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
    depth: usize,
) -> Option<(String, Option<String>, Option<String>)> {
    const MAX_IMPORT_DEPTH: usize = 16;
    if let EntityRef::Inline(def) = &orbital.entity {
        return Some((def.name.clone(), def.id.clone(), orbital.id.clone()));
    }
    let reference = orbital.reference.as_ref()?;
    if depth >= MAX_IMPORT_DEPTH {
        return None;
    }
    let Ok(ParsedReference::Orbital { alias, name }) = parse_reference(&reference.reference) else {
        return None;
    };
    let use_decl = orbital.uses.iter().flatten().find(|u| u.alias == alias)?;
    let loaded = loader.load(&use_decl.from, source_path, chain).ok()?;
    let upstream = loaded
        .schema
        .orbitals
        .iter()
        .find(|o| o.name == name)
        .cloned()?;
    let upstream_path = loaded.source_path.clone();
    let (up_name, up_id, upstream_id) =
        flattened_primary_entity(&upstream, loader, chain, Some(&upstream_path), depth + 1)?;
    let final_name = reference
        .entity
        .clone()
        .unwrap_or_else(|| format!("{}{up_name}", orbital.name));
    let final_id = derive_materialized_id(
        up_id.as_deref(),
        upstream_id.as_deref(),
        &up_name,
        &orbital.name,
        orbital_core_base::identity::IdKind::Entity,
    );
    let effective_id = orbital.id.clone().or_else(|| {
        upstream_id
            .as_deref()
            .map(|uid| orbital_core_base::identity::derive_id(uid, &orbital.name))
    });
    Some((final_name, Some(final_id), effective_id))
}

/// G1 (`docs/Almadar_Compiler_Gaps.md` §82): every reference-form orbital
/// among `orbitals`, keyed by (upstream alias, upstream orbital name),
/// mapped to that import's local name + every kept trait's final
/// name/derived id (`materialized_trait_subs`) — built ONCE, order-free,
/// straight off each reference's UPSTREAM schema, the same shallow-load
/// pattern `precompute_reference_form_primary_ids` already uses (same
/// caching, same best-effort skip on an unresolvable reference — the REAL
/// `resolve_orbital_reference` call moments later still surfaces the actual
/// error for it). Consulted by `rewrite_listen_sources` so an `Orbital`-kind
/// listen source naming ANY reference-form import of the SAME alias
/// re-points at that import's local name/trait, whichever import — this one
/// or a sibling — was declared first. Takes `&[Orbital]` (not `&OrbitalSchema`)
/// so it can scope to either the TOP-LEVEL schema's own orbitals or one
/// nested alias's own `inlined_orbitals` batch — each is its own "schema" of
/// sibling imports for this purpose.
fn precompute_reference_form_listen_targets(
    orbitals: &[Orbital],
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
) -> (
    HashMap<(String, String), SiblingImportInfo>,
    ImportedTraitNames,
) {
    let mut out: HashMap<(String, String), SiblingImportInfo> = HashMap::new();
    let mut by_local: ImportedTraitNames = HashMap::new();
    for orbital in orbitals {
        let Some(reference) = orbital.reference.as_ref() else {
            continue;
        };
        let Ok(ParsedReference::Orbital { alias, name }) = parse_reference(&reference.reference)
        else {
            continue;
        };
        let Some(use_decl) = orbital.uses.iter().flatten().find(|u| u.alias == alias) else {
            continue;
        };
        let Ok(loaded) = loader.load(&use_decl.from, source_path, chain) else {
            continue;
        };
        let Some(upstream) = loaded
            .schema
            .orbitals
            .iter()
            .find(|o| o.name == name)
            .cloned()
        else {
            continue;
        };
        let upstream_path = loaded.source_path.clone();
        let mut trait_subs = materialized_trait_subs(&upstream, reference, &orbital.name);
        // An upstream that is itself an import carries only its body traits until
        // it flattens; address what it will carry, with the ids inlining gives it.
        if upstream.reference.is_some() {
            let only_kept = reference.only.as_ref().map(|v| only_closure(&upstream, v));
            let (flattened, upstream_id) =
                flattened_traits(&upstream, loader, chain, Some(&upstream_path), 0);
            trait_subs = flattened
                .into_iter()
                .filter(|(n, _)| match &only_kept {
                    Some(only) => only.contains(n),
                    None => !reference.omit.iter().flatten().any(|o| o == n),
                })
                .map(|(n, id)| {
                    let final_id = derive_materialized_id(
                        id.as_deref(),
                        upstream_id.as_deref(),
                        &n,
                        &orbital.name,
                        orbital_core_base::identity::IdKind::Trait,
                    );
                    (n.clone(), (format!("{}{n}", orbital.name), final_id))
                })
                .collect();
        }
        by_local.insert(orbital.name.clone(), trait_subs.clone());
        let key = (alias, name);
        let (mut page_remaps, mut imports) = out
            .remove(&key)
            .map(|prev: SiblingImportInfo| (prev.page_remaps, prev.imports))
            .unwrap_or_default();
        imports.push((orbital.name.clone(), trait_subs.clone()));
        let remap = effective_page_remap(&upstream, reference);
        if !remap.is_empty() {
            page_remaps.push((orbital.name.clone(), remap));
        }
        out.insert(
            key,
            SiblingImportInfo {
                local_name: orbital.name.clone(),
                trait_subs,
                page_remaps,
                imports,
            },
        );
    }
    (out, by_local)
}

/// The traits an orbital carries once its own import (if any) flattens — its
/// body traits, plus every surviving upstream trait as `<orbital><upstream>` —
/// each with the id inlining gives it, and the orbital's own effective id.
/// Recursive through an import of an import; mirrors `resolve_orbital_reference`.
fn flattened_traits(
    orbital: &Orbital,
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
    depth: usize,
) -> (Vec<(String, Option<String>)>, Option<String>) {
    const MAX_IMPORT_DEPTH: usize = 16;
    let mut traits: Vec<(String, Option<String>)> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some((td.name.clone(), td.id.clone())),
            TraitReference::Reference {
                name: Some(n), id, ..
            } => Some((n.clone(), id.clone())),
            TraitReference::Reference { .. } => None,
        })
        .collect();
    let own_id = orbital.id.clone();
    let Some(reference) = orbital.reference.as_ref() else {
        return (traits, own_id);
    };
    if depth >= MAX_IMPORT_DEPTH {
        return (traits, own_id);
    }
    let Ok(ParsedReference::Orbital { alias, name }) = parse_reference(&reference.reference) else {
        return (traits, own_id);
    };
    let Some(use_decl) = orbital.uses.iter().flatten().find(|u| u.alias == alias) else {
        return (traits, own_id);
    };
    let Ok(loaded) = loader.load(&use_decl.from, source_path, chain) else {
        return (traits, own_id);
    };
    let Some(upstream) = loaded
        .schema
        .orbitals
        .iter()
        .find(|o| o.name == name)
        .cloned()
    else {
        return (traits, own_id);
    };
    let upstream_path = loaded.source_path.clone();
    let only_kept = reference.only.as_ref().map(|v| only_closure(&upstream, v));
    let (upstream_traits, upstream_id) =
        flattened_traits(&upstream, loader, chain, Some(&upstream_path), depth + 1);
    for (n, id) in upstream_traits {
        let kept = match &only_kept {
            Some(only) => only.contains(&n),
            None => !reference.omit.iter().flatten().any(|o| o == &n),
        };
        if kept {
            let final_id = derive_materialized_id(
                id.as_deref(),
                upstream_id.as_deref(),
                &n,
                &orbital.name,
                orbital_core_base::identity::IdKind::Trait,
            );
            traits.push((format!("{}{n}", orbital.name), Some(final_id)));
        }
    }
    let effective_id = own_id.or_else(|| {
        upstream_id
            .as_deref()
            .map(|uid| orbital_core_base::identity::derive_id(uid, &orbital.name))
    });
    (traits, effective_id)
}

/// Entity name → id over a batch of orbitals' inline entities (primary and
/// auxiliary); a reference-form orbital's unresolved placeholder doesn't count.
fn inline_entity_ids(orbitals: &[Orbital]) -> HashMap<String, String> {
    orbitals
        .iter()
        .flat_map(|o| {
            let primary = match &o.entity {
                EntityRef::Inline(def) => Some((def.name.clone(), def.id.clone())),
                _ => None,
            };
            let aux: Vec<(String, Option<String>)> = o
                .auxiliary_entities
                .iter()
                .flatten()
                .filter_map(|e| match e {
                    EntityRef::Inline(def) => Some((def.name.clone(), def.id.clone())),
                    _ => None,
                })
                .collect();
            primary.into_iter().chain(aux)
        })
        .filter_map(|(name, id)| id.map(|id| (name, id)))
        .collect()
}

/// Each reference-form orbital by its LOCAL name → upstream trait name →
/// (materialized name, derived id).
type ImportedTraitNames = HashMap<String, HashMap<String, (String, String)>>;

/// A consumer addresses an imported orbital's trait by the import's local name
/// and the trait's upstream name (`Things.ThingPersistor.SAVED`, or
/// `ThingPersistor.SAVED` from a sibling authored in the import body); land
/// every such `listens` source and event-typed config value on the
/// materialized `<Local><Upstream>` trait. Runs once the orbitals are inlined.
fn address_imported_traits(orbitals: &mut [Orbital], imported: &ImportedTraitNames) {
    if imported.is_empty() {
        return;
    }
    // A trait present under the exact name (one the consumer authored in the
    // import body) is what that name means; only absent names map.
    let present: HashSet<(String, String)> = orbitals
        .iter()
        .flat_map(|o| {
            o.traits.iter().filter_map(|t| match t {
                TraitReference::Inline(td) => Some((o.name.clone(), td.name.clone())),
                _ => None,
            })
        })
        .collect();
    for orbital in orbitals.iter_mut() {
        let own = orbital.name.clone();
        let resolve = |source: &mut ListenSource| {
            let (scope, trait_name, trait_id) = match source {
                ListenSource::Trait {
                    trait_name,
                    trait_id,
                } => (own.as_str(), trait_name, trait_id),
                ListenSource::Orbital {
                    orbital,
                    trait_name,
                    trait_id,
                    ..
                } => (orbital.as_str(), trait_name, trait_id),
                ListenSource::Any => return,
            };
            if present.contains(&(scope.to_string(), trait_name.clone())) {
                return;
            }
            if let Some((final_name, final_id)) = imported
                .get(scope)
                .and_then(|subs| subs.get(trait_name.as_str()))
            {
                *trait_name = final_name.clone();
                *trait_id = Some(final_id.clone());
            }
        };
        for tr in orbital.traits.iter_mut() {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            if let Some(listens) = td.listens.as_mut() {
                for listen in listens.iter_mut() {
                    if let Some(source) = listen.source.as_mut() {
                        resolve(source);
                    }
                }
            }
            rewrite::map_config_event_addresses(td, &mut |event, source| {
                let mut moved = source.clone();
                resolve(&mut moved);
                (moved != *source).then(|| (event.to_string(), moved))
            });
        }
    }
}

/// A §52 rebind-merge whose target didn't resolve within its own orbital —
/// no local auxiliary by that name yet, and not the orbital's own primary
/// (`merge_into_named_entity` tried the single-orbital slice and came back
/// empty). Held until `inline_schema_with_orbital_diagnostics` has finished
/// inlining EVERY orbital (so every Gap #22 no-rebind auxiliary is
/// registered and every sibling orbital's primary entity exists), then
/// retried once, schema-wide, via the SAME `merge_into_named_entity`. Covers
/// both known misplacement shapes with one mechanism: a target registered
/// only by a LATER no-rebind import in this same orbital (ordering), and a
/// target that is a SIBLING orbital's primary or auxiliary entity
/// (cross-orbital). A target that still resolves nowhere after the
/// schema-wide retry is simply never merged — `ORB_T_REBIND_TARGET_UNRESOLVED`
/// (`validation::trait_validator::validate_entity_rebind`) reports it; there
/// is no fallback landing site left.
#[derive(Clone)]
struct DeferredMerge {
    target: String,
    imported: EntityDefinition,
    intrinsic_only: bool,
    required_data: HashSet<String>,
    referenced_fields: HashSet<String>,
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
#[allow(clippy::too_many_arguments)]
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
    // Schema-level (app) declared `config {}` — the outermost §4.5 rung, for
    // both `resolve_orbital_reference`'s config fold and (further down) the
    // sibling-pull chain's fallback.
    schema_config: Option<&HashMap<String, ConfigField>>,
    // Stage B: every consumer orbital's own primary + aux entity name → id,
    // built once in `inline_schema_with_orbital_diagnostics` — threaded
    // through recursive `inline_orbital` calls unchanged (an import's OWN
    // `entities {}` remap still resolves against the TOP-LEVEL consumer's
    // roster, since that is what "declares an entity for this name" means).
    consumer_entity_ids: &HashMap<String, String>,
    // G1 (`docs/Almadar_Compiler_Gaps.md` §82): every reference-form orbital
    // AMONG THIS CALL'S OWN SIBLINGS (the top-level `schema.orbitals`, or —
    // for a recursive call loading an alias — that alias's own orbitals),
    // keyed by (upstream alias, upstream orbital name). Recomputed fresh at
    // each recursion depth (below) since a nested schema's own `uses`
    // aliases are a different namespace than the caller's.
    listen_source_targets: &HashMap<(String, String), SiblingImportInfo>,
    orbital_ref_diagnostics: &mut Vec<orbital::OrbitalRefDiagnostic>,
    uses_config_diagnostics: &mut Vec<orbital::UsesConfigDiagnostic>,
    // `.lolo` spans (Stage C): this orbital's index into the TOP-LEVEL
    // `schema.orbitals` array (`Some` only when `is_top_level` — a
    // recursively-inlined alias orbital lives in a different file's index
    // space, so `None` there; `diag_path` falls back to the old
    // `orbitals[name=X]` shape, unresolvable but not a regression since it
    // never resolved before either). See `resolve_orbital_reference`'s own
    // `orbital_index` doc.
    orbital_index: Option<usize>,
    // §52 rebind-merge landing sites this call couldn't resolve locally —
    // shared across every `inline_orbital` call (top-level AND recursive
    // alias loads) in this `inline_schema` run, so the schema-level retry
    // after every orbital has inlined sees ALL of them, not just the
    // top-level orbital's own. See `DeferredMerge`.
    deferred_merges: &mut Vec<DeferredMerge>,
    imports: &mut ImportMemo,
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
    let mut aliases = HashSet::new();
    for (i, decl) in orbital.uses.iter().flatten().enumerate() {
        if !aliases.insert(decl.alias.as_str()) {
            return Err(InlineError::DuplicateUseAlias {
                alias: decl.alias.clone(),
                path: format!("orbitals[name={}].uses[{}]", orbital.name, i),
            });
        }
    }
    let uses = match &orbital.uses {
        Some(uses) if !uses.is_empty() => uses.clone(),
        _ => {
            if is_top_level {
                for trait_ref in orbital.traits.iter_mut() {
                    if let TraitReference::Inline(trait_def) = trait_ref {
                        // §157 `config-knobs`, §12 three-level chain: an
                        // uncomposed orbital (no `uses[]`) still has traits
                        // whose OWN declared config defaults may forward to
                        // this orbital's (or the app's) `config {}`
                        // (`label: string = @config.title`) — resolve those
                        // BEFORE folding the trait's effective config into its
                        // transitions/ticks below, the same fix the `uses[]`
                        // path applies via `inline_traits`'s top-level `Inline`
                        // arm (`phases/inline/trait.rs`).
                        if let Some(resolved) = r#trait::forwarded_sibling_config_from(
                            trait_def.config.as_ref(),
                            &[],
                            orbital.config.as_ref(),
                            None,
                            None,
                            schema_config,
                        ) {
                            let cfg = trait_def.config.get_or_insert_with(HashMap::new);
                            for (key, field) in resolved {
                                cfg.insert(key, field);
                            }
                        }
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

    // (A) fix: per-alias schema (app-level) config rung for `orbital X =
    // Alias.orbitals.Y { … }` — the ALIAS's own loaded `config {}`, folded
    // with THIS `uses Alias { config {…} }` call-site override. Built here,
    // beside `ctx.add_alias_multi` below, so `resolve_orbital_reference`
    // (called after the whole `uses` loop) can look it up by alias instead
    // of receiving the CONSUMER's own top-level `schema_config` — a
    // different file's app knobs entirely, which is what it was
    // (incorrectly) receiving before.
    let mut alias_schema_configs: HashMap<String, HashMap<String, ConfigField>> = HashMap::new();

    for use_decl in uses.iter().filter(|u| u.lazy != Some(true)) {
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
        let (loaded_source, loaded_schema_config) = {
            let loaded = loader.load(&use_decl.from, source_path, chain)?;
            if loaded.schema.orbitals.is_empty() {
                // Defensive: a valid imported `.orb` always has at least one
                // orbital (the parser rejects empty ones), so this branch
                // should be unreachable. Skip the alias rather than panic.
                continue;
            }
            (loaded.source_path.clone(), loaded.schema.config.clone())
        };

        // `uses A from "…" { config { … } }` — a call-site override of a
        // knob the imported behavior's app-level `config {}` never declared
        // is the JS twin's ORB_U_CONFIG_UNKNOWN_KEY (Almadar_Orbital_Import.md
        // §4.5). Collected as a diagnostic (not a hard error) so the rest of
        // this orbital still inlines; W4-R turns it into the validator code.
        if let Some(overrides) = use_decl.config.as_ref() {
            let unknown_keys: Vec<String> = overrides
                .keys()
                .filter(|k| {
                    loaded_schema_config
                        .as_ref()
                        .is_none_or(|declared| !declared.contains_key(*k))
                })
                .cloned()
                .collect();
            if !unknown_keys.is_empty() {
                // `.lolo` spans (Stage C): the digit-bracket indexed form
                // `LoloSourceMap::normalize` accepts — see
                // `resolve_orbital_reference`'s matching `diag_path` fix.
                let path = match orbital_index {
                    Some(i) => format!("orbitals[{i}]"),
                    None => format!("orbitals[name={}]", orbital.name),
                };
                uses_config_diagnostics.push(orbital::UsesConfigDiagnostic {
                    path,
                    alias: use_decl.alias.clone(),
                    unknown_keys,
                });
            }
        }

        // Fold this alias's OWN loaded app-config with its call-site
        // override — the SAME fold `resolve_orbital_reference`'s own
        // orbital-rung uses (`fold_declared_config_override`), one owner.
        // `None` when the alias declares no app-level `config {}` at all
        // (nothing to key), matching the prior `Option`-typed passthrough.
        if let Some(mut folded) =
            fold_declared_config_override(loaded_schema_config.as_ref(), use_decl.config.as_ref())
        {
            // A `uses Alias { config { k: @config.j } }` override FORWARDS the
            // consumer's own app knob into the alias's app knob: take that
            // last hop here (whole-string token only, provenance stamped) so
            // the imported shells never see the literal token.
            orbital::forward_consumer_app_knobs(&mut folded, schema_config);
            alias_schema_configs.insert(use_decl.alias.clone(), folded);
        }

        // An imported behavior inlines the same way wherever it is used with the
        // same `uses { config }` override, so it is inlined once per (file,
        // override) and reused. Its own reference-form orbitals resolve
        // `@config.X` against the alias config folded with that override.
        let alias_config: Option<HashMap<String, ConfigField>> = alias_schema_configs
            .get(&use_decl.alias)
            .cloned()
            .or_else(|| loaded_schema_config.clone());
        let import_key = match use_decl.config.as_ref() {
            Some(overrides) if !overrides.is_empty() => {
                let mut keyed: Vec<(&String, String)> = overrides
                    .iter()
                    .map(|(k, v)| (k, serde_json::to_string(v).unwrap_or_default()))
                    .collect();
                keyed.sort();
                let canonical = loader.canonical(&loaded_source);
                std::path::PathBuf::from(format!("{}#uses-config={keyed:?}", canonical.display()))
            }
            _ => loader.canonical(&loaded_source),
        };
        let inlined_orbitals: std::rc::Rc<Vec<Orbital>> =
            if let Some(done) = imports.get(&import_key) {
                recorder.extend(done.provenance.iter().cloned());
                config_diagnostics.extend(done.config_diagnostics.iter().cloned());
                orbital_ref_diagnostics.extend(done.orbital_ref_diagnostics.iter().cloned());
                uses_config_diagnostics.extend(done.uses_config_diagnostics.iter().cloned());
                deferred_merges.extend(done.deferred_merges.iter().cloned());
                loader.mark_loaded(&done.loaded_files);
                std::rc::Rc::clone(&done.orbitals)
            } else {
                let mut inlined_orbitals = loader
                    .load(&use_decl.from, source_path, chain)?
                    .schema
                    .orbitals
                    .clone();
                let marks = (
                    recorder.len(),
                    config_diagnostics.len(),
                    orbital_ref_diagnostics.len(),
                    uses_config_diagnostics.len(),
                    deferred_merges.len(),
                );
                loader.begin_recording();
                let inlined: Result<(), InlineError> = (|| {
                    // G1 (`docs/Almadar_Compiler_Gaps.md` §82): this loaded alias's OWN
                    // orbitals are a DIFFERENT `uses`-namespace than the caller's — a
                    // sibling-import listen source inside one of THEM resolves against
                    // THEIR OWN reference-form orbitals, never the caller's. Recomputed
                    // per alias load (cheap: `loader.load` inside it is a cache hit).
                    let (nested_listen_source_targets, nested_imported_trait_names) =
                        precompute_reference_form_listen_targets(
                            &inlined_orbitals,
                            loader,
                            chain,
                            Some(&loaded_source),
                        );
                    // The alias's own `entities {}` targets name ITS entities, never the
                    // importing program's.
                    let mut nested_entity_ids = inline_entity_ids(&inlined_orbitals);
                    nested_entity_ids.extend(precompute_reference_form_primary_ids(
                        &inlined_orbitals,
                        loader,
                        chain,
                        Some(&loaded_source),
                    ));
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
                            alias_config.as_ref(),
                            &nested_entity_ids,
                            &nested_listen_source_targets,
                            orbital_ref_diagnostics,
                            uses_config_diagnostics,
                            None,
                            deferred_merges,
                            imports,
                        )?;
                    }
                    address_imported_traits(&mut inlined_orbitals, &nested_imported_trait_names);
                    Ok(())
                })();
                let loaded_files = loader.end_recording();
                inlined?;
                // Land the alias's own cross-orbital merges on the alias's orbitals, so a
                // definition copied out of it (an import's clone or retarget) carries the merged
                // fields. Only a merge whose target the alias does not declare travels on to the
                // importing program; a landed one reaches it through the imports that use it.
                let unlanded: Vec<DeferredMerge> = deferred_merges
                    .split_off(marks.4)
                    .into_iter()
                    .filter(|dm| {
                        !merge_into_named_entity(
                            &mut inlined_orbitals,
                            &dm.target,
                            &dm.imported,
                            dm.intrinsic_only,
                            &dm.required_data,
                            &dm.referenced_fields,
                        )
                    })
                    .collect();
                deferred_merges.extend(unlanded);
                move_copy_merges_to_owners(&mut inlined_orbitals);
                drop_shadowed_auxiliaries(&mut inlined_orbitals);
                let inlined_orbitals = std::rc::Rc::new(inlined_orbitals);
                imports.insert(
                    import_key,
                    InlinedImport {
                        orbitals: std::rc::Rc::clone(&inlined_orbitals),
                        provenance: recorder[marks.0..].to_vec(),
                        config_diagnostics: config_diagnostics[marks.1..].to_vec(),
                        orbital_ref_diagnostics: orbital_ref_diagnostics[marks.2..].to_vec(),
                        uses_config_diagnostics: uses_config_diagnostics[marks.3..].to_vec(),
                        deferred_merges: deferred_merges[marks.4..].to_vec(),
                        loaded_files,
                    },
                );
                inlined_orbitals
            };

        // The source label used in error messages is the `from` string from
        // the `uses` declaration (e.g. `"std/behaviors/std-arcade-game"`).
        // That is what the author wrote, so it is the actionable label when
        // a trait/page lookup fails.
        ctx.add_alias_multi(&use_decl.alias, inlined_orbitals, use_decl.from.clone());
    }

    // `orbital X = Alias.orbitals.Y { … }` — resolve AFTER every alias in
    // this orbital's `uses` (including file-scope ones merged in above) is
    // loaded, so `Alias` resolves through the SAME per-orbital `ctx` loop
    // every other reference form uses, and BEFORE the entity-field auto-merge
    // below (which reads `orbital.entity`/`orbital.traits` and must see the
    // materialized ones, not the reference-form placeholder).
    if let Some(reference) = orbital.reference.take() {
        resolve_orbital_reference(
            orbital,
            &reference,
            &ctx,
            &alias_schema_configs,
            consumer_entity_ids,
            listen_source_targets,
            schema_config,
            orbital_ref_diagnostics,
            orbital_index,
            deferred_merges,
        )?;
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
    let caller_entity_name = match &orbital.entity {
        EntityRef::Inline(e) => Some(e.name.clone()),
        _ => None,
    };
    if let Some(caller_entity_name) = caller_entity_name {
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
        //
        // §52: the rebind target is honored, not assumed — each merge lands on
        // the entity the ref's `linkedEntity` names (primary or auxiliary),
        // falling back to the primary when the name matches neither (today's
        // behavior; the existing ORB_T rebind validators then report). Two
        // passes for borrow safety: collect over `&orbital.traits`, then apply
        // mutably. Known limitation: a rebind target that is an IMPORTED aux
        // entity (surfaced by the Gap-#22 pass below) is not yet present here
        // and falls back to the primary — author-declared extras always are.
        struct PendingMerge {
            target: Option<String>,
            imported: EntityDefinition,
            intrinsic_only: bool,
            required_data: HashSet<String>,
            referenced_fields: HashSet<String>,
        }
        // Keyed on (alias, name, rebind target) — NOT (alias, name) alone.
        // The same imported trait rebound to two DIFFERENT targets in one
        // orbital (e.g. `Modal.traits.ModalRecordModal` composed once
        // `-> Listing` and again `-> ModQueueItem`) is two distinct merges,
        // each with its own destination; collapsing them by (alias, name)
        // silently dropped every rebind after the first one encountered.
        let mut merged_aliases: HashSet<(String, String, Option<String>)> = HashSet::new();
        let mut pending: Vec<PendingMerge> = Vec::new();

        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity,
                ref_id,
                fields: field_renames,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    if let Some(imported_entity) =
                        ctx.get_bound_entity_for_trait_orbital(&alias, &name, ref_id.as_deref())
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
                        if carries_fields
                            && merged_aliases.insert((
                                alias.clone(),
                                name.clone(),
                                linked_entity.clone(),
                            ))
                        {
                            // A required DATA field (in the trait's
                            // `entity_contract.requires`) must be satisfied by
                            // the target, NOT auto-merged from the source — that
                            // would mask a genuinely-missing field. Skip those so
                            // ORB_T_REBIND_MISSING_FIELDS can fire. Intrinsic +
                            // provided fields still carry.
                            let resolved_trait_def: Option<TraitDefinition> = ref_id
                                .as_deref()
                                .and_then(|rid| ctx.get_trait_by_ref_id(&alias, rid))
                                .or_else(|| ctx.get_trait(&alias, &name).ok())
                                .and_then(|tr| match tr {
                                    TraitReference::Inline(td) => Some(td),
                                    _ => None,
                                });
                            let required_data = resolved_trait_def
                                .as_ref()
                                .and_then(|td| td.entity_contract.as_ref())
                                .map(|c| c.requires.iter().cloned().collect::<HashSet<String>>())
                                .unwrap_or_default();
                            // R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY: on
                            // an explicit rebind, only the fields the composed
                            // trait (or its hoisted children) actually uses
                            // merge onto the differently-named host entity.
                            // The no-rebind intrinsic_only path is untouched —
                            // it never consults this set.
                            let mut referenced_fields = if !intrinsic_only {
                                resolved_trait_def
                                    .as_ref()
                                    .map(|td| {
                                        collect_trait_referenced_entity_fields(
                                            td,
                                            &imported_entity,
                                            &alias,
                                            &ctx,
                                            &mut HashSet::new(),
                                        )
                                    })
                                    .unwrap_or_default()
                            } else {
                                HashSet::new()
                            };
                            // A call-site `fields { <old>: <new> }` rename
                            // rewrites every `@entity.<old>` read in the
                            // resolved trait to `@entity.<new>` before this
                            // trait's body is inlined (`apply_overrides_to_trait`
                            // / `rewrite_entity_field_paths`, `trait.rs`) — so
                            // `<old>` never survives into the final trait
                            // body. `collect_trait_referenced_entity_fields`
                            // above walked the PRE-rename body and still
                            // reports `<old>` as referenced; without this,
                            // `<old>` merges onto the target as a live field
                            // that nothing will ever read post-rename
                            // (`<new>` is expected to already exist on the
                            // rebind target — that's the point of renaming),
                            // tripping ORB_E_MERGED_FIELD_UNREFERENCED on a
                            // field the merge itself made dead.
                            if let Some(renames) = field_renames {
                                for old_name in renames.keys() {
                                    referenced_fields.remove(old_name);
                                }
                            }
                            pending.push(PendingMerge {
                                target: linked_entity.clone(),
                                imported: imported_entity,
                                intrinsic_only,
                                required_data,
                                referenced_fields,
                            });
                        }
                    }
                }
            }
        }

        for pm in pending {
            match pm.target {
                // No explicit rebind: case (b) always lands on the caller's
                // own primary entity — there is no name to resolve.
                None => {
                    if let EntityRef::Inline(ref mut caller_entity) = orbital.entity {
                        merge_imported_entity_fields(
                            caller_entity,
                            &pm.imported,
                            pm.intrinsic_only,
                            &pm.required_data,
                            &pm.referenced_fields,
                        );
                    }
                }
                // Explicit rebind: try this orbital alone first (covers the
                // caller's own primary and any auxiliary ALREADY registered
                // — e.g. by an earlier no-rebind import in this same
                // orbital). If that fails, defer for the schema-wide retry
                // — a LATER no-rebind import in this orbital, or a sibling
                // orbital's entity, may still resolve it once every orbital
                // has inlined.
                Some(target) => {
                    let resolved = merge_into_named_entity(
                        std::slice::from_mut(orbital),
                        &target,
                        &pm.imported,
                        pm.intrinsic_only,
                        &pm.required_data,
                        &pm.referenced_fields,
                    );
                    if !resolved {
                        deferred_merges.push(DeferredMerge {
                            target,
                            imported: pm.imported,
                            intrinsic_only: pm.intrinsic_only,
                            required_data: pm.required_data,
                            referenced_fields: pm.referenced_fields,
                        });
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
        // Author-declared auxiliary entities (lowered by L1) are already
        // present — never register an imported same-named entity beside one.
        for aux in orbital.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(e) = aux {
                seen_aux_names.insert(e.name.clone());
            }
        }
        // The imported orbital's OWN auxiliary entities travel ONLY when
        // the composed trait — or one of its hoisted inline children —
        // still references that entity after THIS call site's rebind is
        // applied: as the trait's effective bound entity (the rebind target
        // when there is one, else its natural binding — the no-rebind case
        // below), as a fetch/persist target name anywhere in the trait or a
        // hoisted child's body (e.g. std-calendar-sync's pull-upsert into
        // CalendarEvent, still covered), or as a hoisted child's OWN
        // `linkedEntity` binding. Retired 2026-09-19: this used to travel
        // the WHOLE source aux list unconditionally, which is how an atom's
        // pre-rebind entity (std-wiki-page's `WikiPageRevision`,
        // `WikiHistory`'s OWN binding before std-wiki rebinds it to
        // `WikiRevision`) rode along as a phantom, unreferenced copy no
        // trait of the consumer ever touches.
        //
        // A traveled copy keeps its declared (non-`mergedFrom`) fields plus
        // every `mergedFrom` field ANY importing trait ref in THIS orbital
        // references on it — `trait_referenced_fields_of_entity_recursive`,
        // the SAME primitive `ORB_E_MERGED_FIELD_UNREFERENCED` checks the
        // final schema with. `seen_aux_names` gives the copy one shared
        // entry per name, so the referenced set is UNIONED across every
        // trait ref that travels it (pass 1) before it is filtered and
        // pushed once (pass 2) — never per trait ref, or the first one
        // processed would decide the copy's fate for every other importer.
        struct TravelCandidate {
            entity: EntityDefinition,
            referenced: HashSet<String>,
        }
        let mut travel_by_name: HashMap<String, TravelCandidate> = HashMap::new();
        let mut pulled_types: std::collections::BTreeMap<String, Vec<orbital_core_base::schema::PayloadField>> =
            Default::default();
        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity,
                ref_id,
                fields: field_renames,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    // The trait body reads the aliases its own file declares (a `json/parse x Name` shape).
                    for (type_name, fields) in ctx.get_types_for_trait_orbital(&alias, &name, ref_id.as_deref()) {
                        pulled_types.entry(type_name).or_insert(fields);
                    }
                    let natural_bound_entity =
                        ctx.get_bound_entity_for_trait_orbital(&alias, &name, ref_id.as_deref());
                    let effective_bound_name: Option<String> = linked_entity
                        .clone()
                        .or_else(|| natural_bound_entity.as_ref().map(|e| e.name.clone()));
                    let resolved_trait_def: Option<TraitDefinition> = ref_id
                        .as_deref()
                        .and_then(|rid| ctx.get_trait_by_ref_id(&alias, rid))
                        .or_else(|| ctx.get_trait(&alias, &name).ok())
                        .and_then(|tr| match tr {
                            TraitReference::Inline(td) => Some(td),
                            _ => None,
                        });

                    let mut union_reference =
                        |entity: EntityDefinition, mut referenced: HashSet<String>| {
                            if let Some(renames) = field_renames {
                                for old_name in renames.keys() {
                                    referenced.remove(old_name);
                                }
                            }
                            travel_by_name
                                .entry(entity.name.clone())
                                .or_insert_with(|| TravelCandidate {
                                    entity,
                                    referenced: HashSet::new(),
                                })
                                .referenced
                                .extend(referenced);
                        };

                    if linked_entity.is_none() {
                        if let Some(imported_entity) = natural_bound_entity.clone() {
                            let referenced = resolved_trait_def
                                .as_ref()
                                .map(|td| {
                                    trait_referenced_fields_of_entity_recursive(
                                        td,
                                        &imported_entity,
                                        true,
                                        &alias,
                                        &ctx,
                                        &mut HashSet::new(),
                                    )
                                    .1
                                })
                                .unwrap_or_default();
                            union_reference(imported_entity, referenced);
                        }
                    }
                    for aux_entity in
                        ctx.get_aux_entities_for_trait_orbital(&alias, &name, ref_id.as_deref())
                    {
                        // An explicit rebind names the entity this trait binds — the consumer's
                        // own or a sibling's, resolved by the rebind merge above. A same-named
                        // source copy must not travel: it would shadow that entity by name.
                        if linked_entity.as_deref() == Some(aux_entity.name.as_str()) {
                            continue;
                        }
                        let is_own_entity =
                            effective_bound_name.as_deref() == Some(aux_entity.name.as_str());
                        let travel_result = resolved_trait_def.as_ref().map(|td| {
                            trait_referenced_fields_of_entity_recursive(
                                td,
                                &aux_entity,
                                is_own_entity,
                                &alias,
                                &ctx,
                                &mut HashSet::new(),
                            )
                        });
                        let travels = travel_result.as_ref().is_some_and(|(refd, _)| *refd)
                            // Couldn't resolve the trait body to check —
                            // conservative: keep the pre-existing "always
                            // travels" safety property rather than risk
                            // stranding an effect this can't analyze.
                            || resolved_trait_def.is_none();
                        if travels {
                            let referenced = travel_result.map(|(_, f)| f).unwrap_or_default();
                            union_reference(aux_entity, referenced);
                        }
                    }
                }
            }
        }

        if !pulled_types.is_empty() {
            let own = orbital.types.get_or_insert_with(Default::default);
            for (type_name, fields) in pulled_types {
                own.entry(type_name).or_insert(fields);
            }
        }
        let mut travel_names: Vec<String> = travel_by_name.keys().cloned().collect();
        travel_names.sort_unstable();
        let mut aux_collected: Vec<EntityDefinition> = Vec::new();
        for name in travel_names {
            if !seen_aux_names.insert(name.clone()) {
                continue;
            }
            let candidate = travel_by_name
                .remove(&name)
                .expect("just collected its key");
            let mut entity = candidate.entity;
            entity.fields.retain(|f| {
                f.merged_from.is_none()
                    || f.name
                        .as_deref()
                        .is_some_and(|n| candidate.referenced.contains(n))
            });
            aux_collected.push(entity);
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

    // ----------------------------------------------------------------
    // Imported expectations travel with the trait, exactly like the
    // auxiliary entities above.
    //
    // `uses` inlines individual TRAITS, never orbitals — so an organism's
    // `expects entity [identity] { … }` is invisible to the app composing it. Left
    // un-propagated the contract is only ever checked when the organism is
    // validated STANDALONE, where its own roster satisfies it trivially, and a
    // composing app whose roster lacks a field the organism reads off `@user`
    // validates clean. Merging the declarations onto the CONSUMING orbital is
    // what makes `validate_expectation_links` check the host's roster.
    //
    // Merge, never append: the parser rejects duplicate expectations (two
    // `expects entity [identity]`, or two `expects entity X`), so shapes are unioned
    // per target. A field required by ANY contributor stays required — the
    // strictest wins, since every contributor's need must be met.
    {
        let mut imported: Vec<ExpectDeclaration> = Vec::new();
        // The entities this orbital holds once the auxiliary ones travelled with their traits (above).
        let held: HashSet<String> = std::iter::once(&orbital.entity)
            .chain(orbital.auxiliary_entities.iter().flatten())
            .filter_map(|e| match e {
                EntityRef::Inline(def) => Some(def.name.clone()),
                _ => None,
            })
            .collect();
        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                ref_id,
                name: local_name,
                linked_entity,
                events,
                fields,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    let Some(source) = ctx.get_trait_orbital(&alias, &name, ref_id.as_deref()) else { continue };
                    let renames = ObligationRenames {
                        source,
                        trait_name: &name,
                        local_name: local_name.as_deref().unwrap_or(&name),
                        entity_to: linked_entity.as_deref(),
                        events: events.as_ref(),
                        fields: fields.as_ref(),
                        held: &held,
                    };
                    imported.extend(
                        ctx.get_expects_for_trait_orbital(&alias, &name, ref_id.as_deref())
                            .into_iter()
                            .filter_map(|decl| carry_obligation(decl, &renames)),
                    );
                }
            }
        }
        if !imported.is_empty() {
            let mut merged = orbital.expects.take().unwrap_or_default();
            for decl in imported {
                merge_expectation(&mut merged, decl);
            }
            orbital.expects = Some(merged);
        }
    }

    // ----------------------------------------------------------------
    // A `uses` of a spec file (an orbital of only strict `expects`) binds its obligations to THIS
    // orbital: the import is the explicit link, so nothing else names the spec.
    {
        let mut obligations: Vec<ExpectDeclaration> = Vec::new();
        for use_decl in orbital.uses.iter().flatten() {
            if let Some(entry) = ctx.aliases.get(&use_decl.alias) {
                for spec in entry.orbitals.iter().filter(|o| o.is_spec()) {
                    obligations.extend(spec.expects.iter().flatten().cloned());
                }
            }
        }
        if !obligations.is_empty() {
            let mut merged = orbital.expects.take().unwrap_or_default();
            for decl in obligations {
                merge_expectation(&mut merged, decl);
            }
            orbital.expects = Some(merged);
        }
    }

    // ----------------------------------------------------------------
    // Type-parameter struct-alias resolution (Gap A/C follow-up,
    // `docs/Almadar_LOLO.md`): a `::` struct-alias arg or a declared param
    // default resolves LEXICALLY — against the `types` map of the orbital
    // whose FILE the clause was AUTHORED in, never the consumer's. For a
    // trait imported wholesale via `uses`, that authoring scope is the
    // SOURCE behavior's orbital, not this one. The per-orbital sentinel
    // pass (`rewrite::resolve_orbital_type_param_sentinels`, run once this
    // orbital is fully inlined) only ever consults the CURRENT orbital's
    // `types` map — so the specific alias shapes an imported trait's
    // call-site `::` args or its own declared param defaults reference are
    // merged onto THIS orbital's `types` here, before that pass runs.
    // A name already present locally with a DIFFERENT shape is a blocking
    // conflict — ambiguity is never resolved silently; an identical shape
    // merges fine (harmless re-declaration).
    //
    // TRANSITIVE imports (std-inventory shape: A imports B's trait, which is
    // itself B's `::`-bound reference onto C's atom) need one more step.
    // `ctx.get_trait` returns B's OWN already-spliced clone of the trait,
    // whose `type_args` STILL carries B's `::` binding — `inline_traits`
    // (below, later) hasn't run for THIS orbital's references yet, and the
    // per-orbital sentinel pass never runs on an imported orbital, only on
    // the top-level one — so it survives unconsumed. But when THIS orbital's
    // own reference to B's trait supplies no `::` of its own, `inline_traits`
    // unconditionally OVERWRITES `type_args` with this reference's (here:
    // none), discarding B's binding and stranding the sentinel. So: for any
    // param this reference did NOT itself supply an arg for, the source's
    // still-unconsumed arg is carried FORWARD onto this reference's own
    // `type_args` — exactly as if this orbital had authored the same `::`
    // clause itself (the value string is copied verbatim, never
    // reinterpreted) — so the later splice sees something to preserve
    // instead of a clobbering `None`.
    {
        let mut merged_types: BTreeMap<String, Vec<PayloadField>> =
            orbital.types.clone().unwrap_or_default();
        let orbital_path = format!("orbitals[name={}]", orbital.name);
        for trait_ref in orbital.traits.iter_mut() {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                ref_id,
                type_args,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    let source_types =
                        ctx.get_types_for_trait_orbital(&alias, &name, ref_id.as_deref());
                    let source_trait = ref_id
                        .as_deref()
                        .and_then(|rid| ctx.get_trait_by_ref_id(&alias, rid))
                        .or_else(|| ctx.get_trait(&alias, &name).ok());
                    let source_def = match &source_trait {
                        Some(TraitReference::Inline(def)) => Some(def),
                        _ => None,
                    };

                    // Inherit forward: fill only the params THIS reference
                    // left unset, from the source's own still-unconsumed
                    // `type_args`.
                    let mut effective_args = type_args.clone().unwrap_or_default();
                    if let Some(source_def) = source_def {
                        if let Some(inherited) = &source_def.type_args {
                            for (k, v) in inherited {
                                effective_args.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                    }
                    if !effective_args.is_empty() {
                        *type_args = Some(effective_args.clone());
                    }

                    if source_types.is_empty() {
                        continue;
                    }
                    // Candidate names: every effective `::` arg VALUE
                    // (call-site + inherited), plus the source trait's OWN
                    // declared param DEFAULT values — either may name a
                    // struct alias (a primitive/entity name simply isn't
                    // found in `source_types` and is skipped below).
                    let mut candidates: HashSet<String> =
                        effective_args.values().cloned().collect();
                    if let Some(source_def) = source_def {
                        if let Some(params) = &source_def.type_params {
                            candidates.extend(params.iter().filter_map(|p| p.default.clone()));
                        }
                    }
                    for candidate in candidates {
                        let Some(shape) = source_types.get(&candidate) else {
                            continue;
                        };
                        match merged_types.get(&candidate) {
                            Some(existing) if existing != shape => {
                                return Err(InlineError::ConflictingTypeAlias {
                                    name: candidate,
                                    path: orbital_path,
                                });
                            }
                            _ => {
                                merged_types.insert(candidate, shape.clone());
                            }
                        }
                    }
                }
            }
        }
        if !merged_types.is_empty() {
            orbital.types = Some(merged_types);
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
    // by L1 before the compiler runs). Widened with `consumer_entity_ids` (every
    // SIBLING orbital's own primary+aux roster too, spanning the whole composed
    // `.orb` file) — a trait's SECONDARY (non-`linked_entity`) positional
    // reference can name an entity that lives in a DIFFERENT orbital of the
    // same multi-orbital file (std-wiki: `RevisionOrbital`'s primary
    // `WikiRevision`, not `DocumentOrbital`'s own aux — `WikiRevisionCapture`'s
    // positional `persist create` target needs it to resolve via
    // `consumer_known_entity_rebind_subs`). This orbital's OWN names win on
    // collision — the more specific, authoritative view for entities actually
    // declared here.
    let mut entity_ids = consumer_entity_ids.clone();
    entity_ids.extend(collect_orbital_entity_ids(orbital));
    let (
        inlined_traits,
        // V4: the returned name→name map was consumed only by the deleted
        // `rewrite_page_trait_refs`; page refs now resolve by `ref_id`. The
        // trait-declaration renames still happen INSIDE `inline_traits`.
        _trait_rename_map,
        pulled_aux_entities,
        pulled_sibling_names,
        trait_config_diagnostics,
        receiver_mounts,
    ) = inline_traits(
        &orbital.traits,
        &ctx,
        &orbital_path_prefix,
        &entity_ids,
        recorder,
        orbital.config.as_ref(),
        schema_config,
        &alias_schema_configs,
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
    mount_pulled_receivers(orbital, &receiver_mounts);

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

/// A pulled event receiver joins every inline page that mounts the owner trait
/// it serves (by name or by the owner's id), so its handler is bound wherever
/// the emitter is.
fn mount_pulled_receivers(orbital: &mut Orbital, receiver_mounts: &[(String, String)]) {
    if receiver_mounts.is_empty() {
        return;
    }
    let ids: HashMap<String, String> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(def) => def.id.clone().map(|id| (def.name.clone(), id)),
            _ => None,
        })
        .collect();
    for page in orbital.pages.iter_mut().flatten() {
        let orbital_core_base::schema::types::PageRef::Inline(page) = page else {
            continue;
        };
        let Some(mounted) = page.traits.as_mut() else {
            continue;
        };
        for (owner, receiver) in receiver_mounts {
            let owner_id = ids.get(owner);
            let mounts_owner = mounted.iter().any(|t| {
                t.trait_name() == Some(owner.as_str())
                    || owner_id.is_some_and(|id| t.trait_ref_id() == Some(id.as_str()))
            });
            let receiver_id = ids.get(receiver);
            let mounts_receiver = mounted.iter().any(|t| {
                t.trait_name() == Some(receiver.as_str())
                    || receiver_id.is_some_and(|id| t.trait_ref_id() == Some(id.as_str()))
            });
            if mounts_owner && !mounts_receiver {
                mounted.push(TraitReference::Reference {
                    local: false,
                    id: None,
                    reference: Some(receiver.clone()),
                    name: None,
                    linked_entity: None,
                    config: None,
                    events: None,
                    fields: None,
                    listens: None,
                    emits_scope: None,
                    event_ids: None,
                    linked_entity_id: None,
                    ref_id: receiver_id.cloned(),
                    type_args: None,
                });
            }
        }
    }
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
// Orbital-reference `events {}` id reconciliation (G5)
// ============================================================================
//
// `resolve_orbital_reference` (`inline/orbital.rs`) renames every event KEY
// an `events { OLD: NEW }` override touches, but it operates on one
// `Orbital`, not the whole `OrbitalSchema` — it has no `schema.ledger` to
// update. A renamed declaration's id is therefore left naming its PRE-rename
// key in the ledger (copied in wholesale from the upstream behavior file at
// load time), and the next dual-carry read trips `ORB_ID_NAME_MISMATCH`.

/// One event declaration this pass found still naming a PRE-rename id —
/// `orbital_name`/`trait_name` pin exactly which materialized trait it
/// belongs to (so pass 2 below rewrites only that trait, never the whole
/// schema), `old_id` is the upstream id, `new_key` is the event's current
/// (already-renamed) name.
struct EventIdRenameSite {
    orbital_name: String,
    trait_name: String,
    old_id: String,
    new_key: String,
}

/// G5 (`docs/Almadar_Compiler_Gaps.md` §82): re-derive the id of every event
/// declaration an orbital reference's `events {}` rename actually renamed.
///
/// Whether that means minting a FRESH id or renaming the existing one IN
/// PLACE depends on a whole-schema census, mirroring the sibling-pull
/// rename above (`id_claimed`: first claim renames in place, every
/// subsequent claim mints a copy) — inverted here into "how many DIFFERENT
/// (orbital, trait) declarations still carry this id": exactly one (the
/// trait being renamed, and nothing else in this schema) means the id is
/// this schema's alone to rename, so the ledger entry is updated in place
/// (no orphaned row, no `ORB_ID_LEDGER_ORPHAN`); more than one means some
/// OTHER, untouched declaration still legitimately holds the OLD id under
/// the OLD name, so only the renamed trait's occurrence is repointed at a
/// freshly derived id, leaving the original entry intact for that other
/// owner. A schema with no `events {}` orbital reference is a no-op.
fn reconcile_orbital_ref_event_id_renames(
    schema: &mut OrbitalSchema,
    diagnostics: &[orbital::OrbitalRefDiagnostic],
) {
    let renamed: Vec<&orbital::OrbitalRefDiagnostic> = diagnostics
        .iter()
        .filter(|d| !d.event_subs.is_empty())
        .collect();
    if renamed.is_empty() {
        return;
    }
    if schema.ledger.is_none() {
        return;
    }

    // Pass 1 (read-only): every declaration site whose id still disagrees
    // with the ledger's recorded name for one of THIS rename's new keys.
    let mut sites: Vec<EventIdRenameSite> = Vec::new();
    {
        let ledger = schema.ledger.as_ref().expect("checked Some above");
        for diag in &renamed {
            let new_keys: HashSet<&str> = diag.event_subs.values().map(String::as_str).collect();
            let Some(orbital) = schema.orbitals.iter().find(|o| o.name == diag.local_name) else {
                continue;
            };
            for tr in &orbital.traits {
                let TraitReference::Inline(td) = tr else {
                    continue;
                };
                for ev in td.state_machine.events.iter().flatten() {
                    let (Some(id), true) = (ev.id.as_ref(), new_keys.contains(ev.key.as_str()))
                    else {
                        continue;
                    };
                    let Some(entry) = ledger.entries.get(id) else {
                        continue;
                    };
                    if entry.cur_name == ev.key {
                        continue; // ledger already agrees — nothing to reconcile
                    }
                    sites.push(EventIdRenameSite {
                        orbital_name: orbital.name.clone(),
                        trait_name: td.name.clone(),
                        old_id: id.clone(),
                        new_key: ev.key.clone(),
                    });
                }
            }
        }
    }
    if sites.is_empty() {
        return;
    }

    // Whole-schema owner census for just the candidate ids — how many
    // DIFFERENT (orbital, trait) declarations carry each one, regardless of
    // whether they were touched by a rename.
    let candidate_ids: HashSet<&str> = sites.iter().map(|s| s.old_id.as_str()).collect();
    let mut owners: HashMap<String, HashSet<(String, String)>> = HashMap::new();
    for orbital in &schema.orbitals {
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            for ev in td.state_machine.events.iter().flatten() {
                if let Some(id) = ev.id.as_ref() {
                    if candidate_ids.contains(id.as_str()) {
                        owners
                            .entry(id.clone())
                            .or_default()
                            .insert((orbital.name.clone(), td.name.clone()));
                    }
                }
            }
        }
    }

    // Sole-owner ids rename their ledger entry in place; shared ids mint a
    // fresh derived id, scoped to exactly the (orbital, trait) that needs it.
    let mut rename_in_place: HashMap<String, String> = HashMap::new(); // old_id -> new_key
    let mut derive_sites: Vec<(&EventIdRenameSite, String)> = Vec::new(); // (site, new_id)
    let mut seen: HashSet<&str> = HashSet::new();
    for site in &sites {
        if owners.get(&site.old_id).map(|o| o.len()).unwrap_or(1) <= 1 {
            rename_in_place.insert(site.old_id.clone(), site.new_key.clone());
        } else if seen.insert(site.old_id.as_str()) {
            derive_sites.push((
                site,
                orbital_core_base::identity::derive_id(&site.old_id, &site.new_key),
            ));
        }
    }

    // Pass 2 (write): swap the shared-id sites' declarations to their fresh
    // id — the same field set `event_id_hygiene::uniquify_orbital` swaps for
    // its own per-instance derivation (`stateMachine.events[].id`,
    // `transitions[].eventId`, `emits[].eventId`,
    // `listens[].eventId`/`triggersId`) — scoped to the ONE named trait so a
    // same-id, still-unrenamed declaration elsewhere is never touched.
    for (site, new_id) in &derive_sites {
        let Some(orbital) = schema
            .orbitals
            .iter_mut()
            .find(|o| o.name == site.orbital_name)
        else {
            continue;
        };
        for tr in orbital.traits.iter_mut() {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            if td.name != site.trait_name {
                continue;
            }
            let subs: HashMap<&str, &str> = [(site.old_id.as_str(), new_id.as_str())].into();
            for ev in td.state_machine.events.iter_mut().flatten() {
                if let Some(id) = ev.id.as_mut() {
                    if let Some(to) = subs.get(id.as_str()) {
                        *id = to.to_string();
                    }
                }
            }
            for t in td.state_machine.transitions.iter_mut() {
                if let Some(id) = t.event_id.as_mut() {
                    if let Some(to) = subs.get(id.as_str()) {
                        *id = to.to_string();
                    }
                }
            }
            for e in td.emits.iter_mut().flatten() {
                if let Some(id) = e.event_id.as_mut() {
                    if let Some(to) = subs.get(id.as_str()) {
                        *id = to.to_string();
                    }
                }
            }
            for l in td.listens.iter_mut().flatten() {
                if let Some(id) = l.event_id.as_mut() {
                    if let Some(to) = subs.get(id.as_str()) {
                        *id = to.to_string();
                    }
                }
                if let Some(id) = l.triggers_id.as_mut() {
                    if let Some(to) = subs.get(id.as_str()) {
                        *id = to.to_string();
                    }
                }
            }
        }
    }

    // Pass 3: apply the ledger side of both branches. Each derive site's
    // owning trait id is looked up FIRST (an immutable `schema.orbitals`
    // borrow) so the mutable `schema.ledger` borrow right after never
    // overlaps it.
    let derive_parents: Vec<Option<String>> = derive_sites
        .iter()
        .map(|(site, _)| {
            schema
                .orbitals
                .iter()
                .find(|o| o.name == site.orbital_name)
                .and_then(|o| {
                    o.traits.iter().find_map(|tr| match tr {
                        TraitReference::Inline(td) if td.name == site.trait_name => td.id.clone(),
                        _ => None,
                    })
                })
        })
        .collect();

    let ledger = schema.ledger.as_mut().expect("checked Some above");
    let at = orbital_core_base::time_util::now_rfc3339();
    for (old_id, new_key) in rename_in_place {
        ledger.rename(&old_id, &new_key, &at);
    }
    for ((site, new_id), parent) in derive_sites.into_iter().zip(derive_parents) {
        let Some(src) = ledger.entries.get(&site.old_id) else {
            continue;
        };
        let mut renames = src.renames.clone();
        renames.push(orbital_core_base::identity::RenameRecord {
            from: src.cur_name.clone(),
            to: site.new_key.clone(),
            at: at.clone(),
        });
        ledger.entries.insert(
            new_id.clone(),
            orbital_core_base::identity::LedgerEntry {
                id: new_id,
                kind: src.kind,
                baked_name: src.baked_name.clone(),
                cur_name: site.new_key.clone(),
                renames,
                owner: src.owner,
                parent,
            },
        );
    }
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
    source_union: &HashMap<String, orbital_core_base::identity::LedgerEntry>,
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

    let mut to_add: Vec<orbital_core_base::identity::LedgerEntry> = Vec::new();
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
        .get_or_insert_with(|| orbital_core_base::identity::IdentityLedger {
            schema_version: 1,
            entries: std::collections::BTreeMap::new(),
        });
    for row in to_add {
        ledger.entries.entry(row.id.clone()).or_insert(row);
    }
}

/// Ids carried BY a declaration node (orbital/entity/trait/event/page/field) —
/// these are arena-known without a ledger row. Mirrors the declaration sites
/// that `validation::id_integrity::IdIndex::build` treats as arena nodes.
fn collect_declared_ids(schema: &OrbitalSchema) -> HashSet<String> {
    fn decl_entity(e: Option<&orbital_core_base::schema::EntityDefinition>, out: &mut HashSet<String>) {
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
            if let orbital_core_base::schema::PageRef::Inline(p) = page {
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
    use orbital_core_base::schema::{EntityRef, ListenSource, PageRef, TraitReference};
    let mut out = HashSet::new();
    let add = |id: &Option<String>, out: &mut HashSet<String>| {
        if let Some(id) = id {
            out.insert(id.clone());
        }
    };

    fn field_relations(field: &orbital_core_base::schema::FieldDefinition, out: &mut HashSet<String>) {
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

/// Union one imported expectation into a consuming orbital's `expects` list.
///
/// The `.lolo` parser rejects duplicates (`expects_duplicate`): two
/// `expects entity [identity]` — bare included — or two `expects entity X` are a parse
/// error. So propagated declarations must MERGE with any same-target entry
/// rather than append, or the composed `.orb` would carry a shape the language
/// itself forbids.
///
/// Field union rules: a field contributed by any party is required if ANY
/// contributor marks it required (the strictest need must be satisfiable), and
/// an existing field's declaration wins on type so a consumer's own hand-written
/// expectation is never silently retyped by an import.
/// How one trait reference renames what it imports: the trait, its events, its linked entity and that entity's fields.
struct ObligationRenames<'a> {
    source: &'a Orbital,
    trait_name: &'a str,
    local_name: &'a str,
    entity_to: Option<&'a str>,
    events: Option<&'a HashMap<String, String>>,
    fields: Option<&'a HashMap<String, String>>,
    /// The entities the importing orbital holds after its imports' auxiliary entities travelled.
    held: &'a HashSet<String>,
}

/// An imported trait's obligations travel with it, under the reference's renames (G-ORB-378). An obligation about
/// another trait of the same orbital travels only with a reference to that trait; one about an entity that orbital
/// owns travels with the entity (when the import brought it along); a strict page obligation stays with the orbital
/// that declares the page: a trait reference imports none of these.
fn carry_obligation(decl: ExpectDeclaration, r: &ObligationRenames<'_>) -> Option<ExpectDeclaration> {
    let own_trait = |t: &str| r.source.traits.iter().any(|tr| tr.trait_name() == Some(t));
    let linked = r.source.traits.iter().find_map(|tr| match tr {
        TraitReference::Inline(td) if td.name == r.trait_name => td.linked_entity.clone(),
        _ => None,
    });
    let event = |e: String| r.events.and_then(|m| m.get(&e).cloned()).unwrap_or(e);
    let fields = |e: &mut orbital_core_base::schema::SExpression| {
        if let Some(subs) = r.fields {
            rewrite::rewrite_entity_field_paths(e, subs);
        }
    };
    let rename_trait = |t: String| -> Option<String> {
        if t == r.trait_name {
            Some(r.local_name.to_string())
        } else if own_trait(&t) {
            None
        } else {
            Some(t)
        }
    };
    Some(match decl {
        ExpectDeclaration::State { trait_name, state, mut holds, strict } => {
            let trait_name = rename_trait(trait_name)?;
            holds.iter_mut().for_each(fields);
            ExpectDeclaration::State { trait_name, state, holds, strict }
        }
        ExpectDeclaration::Transition { trait_name, from, event: ev, to, mut ensures, strict } => {
            let trait_name = rename_trait(trait_name)?;
            ensures.iter_mut().flatten().for_each(fields);
            ExpectDeclaration::Transition { trait_name, from, event: event(ev), to, ensures, strict }
        }
        ExpectDeclaration::Event { trait_name, event: ev, payload, strict } => {
            let trait_name = rename_trait(trait_name)?;
            ExpectDeclaration::Event { trait_name, event: event(ev), payload, strict }
        }
        ExpectDeclaration::Page { strict: true, .. } => return None,
        // A knob obligation binds the atom that declares the knob; the composer's call-site config is its own to set.
        ExpectDeclaration::Config { .. } => return None,
        ExpectDeclaration::Entity {
            name,
            mut shape,
            persistence,
            shared,
            local,
            mut read_policy,
            mut create_policy,
            mut update_policy,
            mut delete_policy,
            access_waivers,
            mut invariants,
            strict,
        } if linked.as_deref() == Some(name.as_str()) => {
            for policy in [&mut read_policy, &mut create_policy, &mut update_policy, &mut delete_policy] {
                policy.iter_mut().for_each(fields);
            }
            if let Some(subs) = r.fields {
                for f in shape.iter_mut().flatten() {
                    if let Some(to) = f.name.as_ref().and_then(|n| subs.get(n)) {
                        f.name = Some(to.clone());
                    }
                }
            }
            invariants.iter_mut().flatten().for_each(fields);
            ExpectDeclaration::Entity {
                name: r.entity_to.map(str::to_string).unwrap_or(name),
                shape,
                persistence,
                shared,
                local,
                read_policy,
                create_policy,
                update_policy,
                delete_policy,
                access_waivers,
                invariants,
                strict,
            }
        }
        ExpectDeclaration::Entity { ref name, strict: true, .. }
            if owned_entity(r.source, name) && !r.held.contains(name) =>
        {
            return None
        }
        other => other,
    })
}

fn owned_entity(orbital: &Orbital, name: &str) -> bool {
    std::iter::once(&orbital.entity)
        .chain(orbital.auxiliary_entities.iter().flatten())
        .any(|e| matches!(e, EntityRef::Inline(def) if def.name == name))
}

pub(super) fn merge_expectation(into: &mut Vec<ExpectDeclaration>, incoming: ExpectDeclaration) {
    fn union_shape(
        existing: &mut Option<Vec<orbital_core_base::schema::FieldDefinition>>,
        incoming: Option<Vec<orbital_core_base::schema::FieldDefinition>>,
    ) {
        let Some(add) = incoming else { return };
        let target = existing.get_or_insert_with(Vec::new);
        for field in add {
            let Some(name) = field.name.as_deref() else {
                continue;
            };
            match target.iter_mut().find(|f| f.name.as_deref() == Some(name)) {
                Some(existing_field) => {
                    // Strictest wins: any contributor needing it required makes
                    // it required for the provider.
                    if field.required == Some(true) {
                        existing_field.required = Some(true);
                    }
                }
                None => target.push(field),
            }
        }
    }

    match incoming {
        ExpectDeclaration::Identity { name, shape, strict } => {
            // At most ONE identity expectation per orbital — `@user` is a single
            // viewer, and the parser enforces it. Merge into whichever exists.
            if let Some(ExpectDeclaration::Identity {
                name: existing_name,
                shape: existing_shape,
                strict: existing_strict,
            }) = into
                .iter_mut()
                .find(|d| matches!(d, ExpectDeclaration::Identity { .. }))
            {
                if existing_name.is_none() {
                    // A named expectation is more specific than a bare one, but
                    // an unnamed one resolves to whatever the host tagged
                    // [identity] — keep the bare form, which is what lets a
                    // composed app satisfy imports from several organisms whose
                    // rosters have different names.
                    let _ = &name;
                }
                union_shape(existing_shape, shape);
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Identity { name, shape, strict });
            }
        }
        ExpectDeclaration::Entity {
            name,
            shape,
            persistence,
            shared,
            local,
            read_policy,
            create_policy,
            update_policy,
            delete_policy,
            access_waivers,
            invariants,
            strict,
        } => {
            if let Some(ExpectDeclaration::Entity {
                shape: existing_shape,
                persistence: existing_persistence,
                shared: existing_shared,
                local: existing_local,
                read_policy: existing_read,
                create_policy: existing_create,
                update_policy: existing_update,
                delete_policy: existing_delete,
                access_waivers: existing_waivers,
                invariants: existing_invariants,
                strict: existing_strict,
                ..
            }) = into
                .iter_mut()
                .find(|d| matches!(d, ExpectDeclaration::Entity { name: n, .. } if *n == name))
            {
                union_shape(existing_shape, shape);
                // What an expectation already states wins; a contributor fills only what is unstated.
                if existing_persistence.is_none() {
                    *existing_persistence = persistence;
                }
                *existing_shared |= shared;
                *existing_local |= local;
                for (slot, incoming) in [
                    (existing_read, read_policy),
                    (existing_create, create_policy),
                    (existing_update, update_policy),
                    (existing_delete, delete_policy),
                ] {
                    if slot.is_none() {
                        *slot = incoming;
                    }
                }
                if let Some(incoming) = access_waivers {
                    let waivers = existing_waivers.get_or_insert_with(Default::default);
                    waivers.read = waivers.read.take().or(incoming.read);
                    waivers.create = waivers.create.take().or(incoming.create);
                    waivers.update = waivers.update.take().or(incoming.update);
                    waivers.delete = waivers.delete.take().or(incoming.delete);
                }
                // Every contributor's invariants are owed.
                for invariant in invariants.into_iter().flatten() {
                    let owed = existing_invariants.get_or_insert_with(Vec::new);
                    if !owed.contains(&invariant) {
                        owed.push(invariant);
                    }
                }
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Entity {
                    name,
                    shape,
                    persistence,
                    shared,
                    local,
                    read_policy,
                    create_policy,
                    update_policy,
                    delete_policy,
                    access_waivers,
                    invariants,
                    strict,
                });
            }
        }
        ExpectDeclaration::Event {
            trait_name,
            event,
            payload,
            strict,
        } => {
            if let Some(ExpectDeclaration::Event {
                payload: existing_payload,
                strict: existing_strict,
                ..
            }) = into.iter_mut().find(|d| {
                matches!(d, ExpectDeclaration::Event { trait_name: t, event: e, .. }
                     if *t == trait_name && *e == event)
            }) {
                if existing_payload.is_none() {
                    *existing_payload = payload;
                }
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Event {
                    trait_name,
                    event,
                    payload,
                    strict,
                });
            }
        }
        // A route has no shape to union — the path IS the declaration, so
        // dedup by path and keep the first.
        ExpectDeclaration::Page { path, traits, strict } => {
            if let Some(ExpectDeclaration::Page {
                traits: existing_traits,
                strict: existing_strict,
                ..
            }) = into
                .iter_mut()
                .find(|d| matches!(d, ExpectDeclaration::Page { path: p, .. } if *p == path))
            {
                if let Some(incoming) = traits {
                    let mounted = existing_traits.get_or_insert_with(Vec::new);
                    for name in incoming {
                        if !mounted.contains(&name) {
                            mounted.push(name);
                        }
                    }
                }
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Page { path, traits, strict });
            }
        }
        ExpectDeclaration::State { trait_name, state, holds, strict } => {
            if let Some(ExpectDeclaration::State { holds: existing_holds, strict: existing_strict, .. }) = into.iter_mut().find(|d| {
                matches!(d, ExpectDeclaration::State { trait_name: t, state: s, .. } if *t == trait_name && *s == state)
            }) {
                // Both contributors' propositions are owed.
                *existing_holds = match (existing_holds.take(), holds) {
                    (Some(a), Some(b)) if a != b => Some(orbital_core_base::schema::SExpression::Array(vec![orbital_core_base::schema::SExpression::String("and".to_string()), a, b])),
                    (a, b) => a.or(b),
                };
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::State { trait_name, state, holds, strict });
            }
        }
        ExpectDeclaration::Transition {
            trait_name,
            from,
            event,
            to,
            ensures,
            strict,
        } => {
            if let Some(ExpectDeclaration::Transition { ensures: existing_ensures, strict: existing_strict, .. }) =
                into.iter_mut().find(|d| {
                    matches!(d, ExpectDeclaration::Transition { trait_name: t, from: f, event: e, to: o, .. }
                        if *t == trait_name && *f == from && *e == event && *o == to)
                })
            {
                // Both contributors' propositions are owed; a structurally equal one is stored once.
                if let Some(incoming) = ensures {
                    let owed = existing_ensures.get_or_insert_with(Vec::new);
                    for p in incoming {
                        if !owed.contains(&p) {
                            owed.push(p);
                        }
                    }
                }
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Transition { trait_name, from, event, to, ensures, strict });
            }
        }
        ExpectDeclaration::Config { name, knob, strict } => {
            if let Some(ExpectDeclaration::Config { strict: existing_strict, .. }) =
                into.iter_mut().find(|d| matches!(d, ExpectDeclaration::Config { name: n, .. } if *n == name))
            {
                *existing_strict |= strict;
            } else {
                into.push(ExpectDeclaration::Config { name, knob, strict });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_transition_expectations_conjoin_their_ensures_and_store_an_equal_one_once() {
        let edge = |ensures: serde_json::Value, strict: bool| -> ExpectDeclaration {
            serde_json::from_value(serde_json::json!({
                "kind": "transition", "traitName": "Wallet", "from": "active", "event": "SPEND", "to": "active",
                "ensures": ensures, "strict": strict
            }))
            .unwrap()
        };
        let a = serde_json::json!(["=", "@entity.balance", ["-", "@fromEntity.balance", "@payload.amount"]]);
        let b = serde_json::json!([">=", "@entity.balance", 0.0]);
        let mut into = Vec::new();
        merge_expectation(&mut into, edge(serde_json::json!([a.clone()]), false));
        merge_expectation(&mut into, edge(serde_json::json!([a.clone(), b.clone()]), true));
        assert_eq!(into.len(), 1);
        let merged = serde_json::to_value(&into[0]).unwrap();
        assert_eq!(merged["ensures"], serde_json::json!([a, b]), "conjunction, the equal one stored once");
        assert_eq!(merged["strict"], serde_json::json!(true), "a strict contributor keeps it strict");
        let mut other = Vec::new();
        merge_expectation(&mut other, edge(serde_json::json!([b]), false));
        let control: ExpectDeclaration = serde_json::from_value(serde_json::json!({
            "kind": "transition", "traitName": "Wallet", "from": "idle", "event": "SPEND", "to": "active" })).unwrap();
        merge_expectation(&mut other, control);
        assert_eq!(other.len(), 2, "control: a different edge is its own expectation");
    }
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures")
    }

    #[test]
    fn structured_code_maps_the_five_behavior_package_loader_errors() {
        let cases = [
            (
                LoaderError::PackageNotInstalled {
                    prefix: "almadar-behaviors".to_string(),
                },
                "ORB_BEHAVIOR_PACKAGE_NOT_INSTALLED",
            ),
            (
                LoaderError::BehaviorPackageNotInstalled {
                    package: "@almadar/std".to_string(),
                },
                "ORB_BEHAVIOR_PACKAGE_NOT_INSTALLED",
            ),
            (
                LoaderError::BehaviorNotFound {
                    name: "rpg-hero".to_string(),
                    searched: vec!["@almadar/std@16.223.0".to_string()],
                },
                "ORB_BEHAVIOR_NOT_FOUND",
            ),
            (
                LoaderError::BehaviorAmbiguous {
                    name: "rpg-hero".to_string(),
                    package_a: "pkg-a".to_string(),
                    package_b: "pkg-b".to_string(),
                },
                "ORB_BEHAVIOR_AMBIGUOUS",
            ),
            (
                LoaderError::PrefixConflict {
                    prefix: "shared".to_string(),
                    package_a: "pkg-a".to_string(),
                    path_a: PathBuf::from("/a"),
                    package_b: "pkg-b".to_string(),
                    path_b: PathBuf::from("/b"),
                },
                "ORB_BEHAVIOR_PREFIX_CONFLICT",
            ),
        ];
        for (loader_err, expected) in cases {
            let err = InlineError::Loader(loader_err);
            assert_eq!(err.structured_code(), expected);
        }
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
            &[
                (0, vec!["DataGrid1".to_string()]),
                (1, vec!["DataGrid1".to_string()]),
            ],
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
                orb.traits
                    .iter()
                    .any(|t| matches!(t, TraitReference::Inline(td) if td.name == renamed)),
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
            let effects_json =
                serde_json::to_string(&host.state_machine.transitions[0].effects).unwrap();
            assert!(
                effects_json.contains(&format!("@trait.{renamed}")),
                "{orb_name}: effect token rewritten"
            );
            let config_json = serde_json::to_string(&host.config).unwrap();
            assert!(
                config_json.contains(&format!("@trait.{renamed}")),
                "{orb_name}: config default rewritten"
            );
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
        let mut obj: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        use orbital_core_base::schema::StateMachine;
        TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
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
        use orbital_core_base::schema::{StringOrArray, TransitionDefinition};
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
                ensures: None,
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
        use orbital_core_base::identity::{IdKind, LedgerEntry, LedgerOwner};

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
        use orbital_core_base::schema::EntityDefinition;
        Orbital {
            spec: false,
            types: None,
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(trait_def)],
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
            local: false,
            type_args: None,
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

    /// Call-site `[local]` (2026-09-24): the resolved trait dispatches
    /// client-only whatever the atom declared; without it the atom's own
    /// `local` stands.
    #[test]
    fn a_call_site_local_rebind_resolves_local() {
        let ctx = make_modal_ctx();
        let mut local_ref = modal_ref(Some("LocalModal"), Some("CartItem"));
        if let TraitReference::Reference { local, .. } = &mut local_ref {
            *local = true;
        }
        let traits = vec![local_ref, modal_ref(Some("PlainModal"), Some("CartItem"))];
        let result = inline_traits(
            &traits,
            &ctx,
            "<test>",
            &HashMap::new(),
            &mut Vec::new(),
            None,
            None,
            &HashMap::new(),
        )
        .expect("inlines");
        let local_of = |name: &str| {
            result.0.iter().find_map(|t| match t {
                TraitReference::Inline(td) if td.name == name => Some(td.local),
                _ => None,
            })
        };
        assert_eq!(local_of("LocalModal"), Some(true));
        assert_eq!(local_of("PlainModal"), Some(false));
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

        let result = inline_traits(
            &traits,
            &ctx,
            "<test>",
            &HashMap::new(),
            &mut Vec::new(),
            None,
            None,
            &HashMap::new(),
        );
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

        let (result, _rename_map, _aux, _pulled, _cfg_diags, _receivers) = inline_traits(
            &traits,
            &ctx,
            "<test>",
            &HashMap::new(),
            &mut Vec::new(),
            None,
            None,
            &HashMap::new(),
        )
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

        let (result, _rename_map, _aux, _pulled, _cfg_diags, _receivers) = inline_traits(
            &traits,
            &ctx,
            "<test>",
            &HashMap::new(),
            &mut Vec::new(),
            None,
            None,
            &HashMap::new(),
        )
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

        let result = inline_traits(
            &traits,
            &ctx,
            "<test>",
            &HashMap::new(),
            &mut Vec::new(),
            None,
            None,
            &HashMap::new(),
        );
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
        use orbital_core_base::schema::RestServiceDefInner;
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
        use orbital_core_base::schema::EntityDefinition;
        Orbital {
            spec: false,
            types: None,
            name: name.to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: Some(vec![ServiceRef::Inline(service_def)]),
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
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

    use orbital_core_base::schema::{FieldType, Persistence};

    /// Build a minimal `EntityDefinition` mirroring the upstream
    /// `ModalRecord` shape from the embedded std-modal registry: 5 fields
    /// (id, name, description, status, createdAt) with `runtime`
    /// persistence. The Extends tests use this as the base entity that the
    /// molecule inherits from, so the field counts and names match the
    /// real registry shape that the cart molecule consumes.
    fn make_modal_entity_def() -> EntityDefinition {
        let make_field = |n: &str, t: FieldType| FieldDefinition {
            mock: None,
            merged_from: None,
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
            projected_from: None,
        };
        EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        }
    }

    /// Build an `Orbital` whose only entity is the supplied
    /// `EntityDefinition`. Mirrors `make_orbital_with_trait` /
    /// `make_orbital_with_service` for entities so the Extends tests can
    /// stash a base entity in the InlineContext under an alias.
    fn make_orbital_with_entity(name: &str, entity_def: EntityDefinition) -> Orbital {
        Orbital {
            spec: false,
            types: None,
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
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
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
            mock: None,
            merged_from: None,
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
            projected_from: None,
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
        let mut obj: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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

        let mut payload_obj: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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

        let mut object = orbital_core_base::schema::SObject::new();
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

        let mut object = orbital_core_base::schema::SObject::new();
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
        use orbital_core_base::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
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
                    holds: None,
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
                        payload_entity: None,
                        external: None,
                        listened: None,
                        inbound: None,
                    },
                    EventDefinition {
                        id: None,
                        key: "CLOSE".to_string(),
                        name: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        payload_schema: None,
                        payload_entity: None,
                        external: None,
                        listened: None,
                        inbound: None,
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
                        ensures: None,
                    },
                    TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("open".to_string()),
                        to: "closed".to_string(),
                        event: "CLOSE".to_string(),
                        guard: None,
                        effects: None,
                        ensures: None,
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
        use orbital_core_base::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
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
                    holds: None,
                }],
                events: Some(vec![EventDefinition {
                    id: None,
                    key: "OPEN".to_string(),
                    name: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    external: None,
                    listened: None,
                    inbound: None,
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
                    ensures: None,
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

        let mut criteria = orbital_core_base::schema::SObject::new();
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

        let mut payload = orbital_core_base::schema::SObject::new();
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

        let mut row1 = orbital_core_base::schema::SObject::new();
        row1.insert("name".to_string(), s("row-1"));
        let mut row2 = orbital_core_base::schema::SObject::new();
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

        let mut obj: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        use orbital_core_base::schema::{StateMachine, StringOrArray, TransitionDefinition};

        let mut trait_def = TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
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
                        ensures: None,
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
                        ensures: None,
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
        use orbital_core_base::schema::{EntityDefinition, PageDefinition};

        let orb0 = Orbital {
            spec: false,
            types: None,
            name: "ArcadeState".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
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
        };

        let canvas_page = PageDefinition {
            translation_of: None,
            source_page: None,
            access: None,
            indexing: None,
            title: None,
            description: None,
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
            roles: None,
        };

        let orb1 = Orbital {
            spec: false,
            types: None,
            name: "ArcadeCanvas".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: Some(vec![PageRef::Inline(canvas_page)]),
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
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
    fn make_field(name: &str, ft: orbital_core_base::schema::FieldType) -> FieldDefinition {
        FieldDefinition {
            mock: None,
            merged_from: None,
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
            projected_from: None,
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
        use orbital_core_base::schema::FieldType;

        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };
        let imported = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        // Not testing the field-usage narrowing here — assert this test's
        // own referenced set so the collision/addition mechanics still get
        // exercised the same way they did before the gate existed.
        let referenced: HashSet<String> = ["id", "name", "pendingId"]
            .into_iter()
            .map(String::from)
            .collect();
        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new(), &referenced);

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "price", "name", "pendingId"]);
        // Verify caller's original field count + merged fields.
        assert_eq!(caller.fields.len(), 4);
    }

    #[test]
    fn merge_entity_fields_no_collision() {
        // Caller has [id, title], imported has [description, status].
        // After merge: [id, title, description, status].
        use orbital_core_base::schema::FieldType;

        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };
        let imported = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        let referenced: HashSet<String> = ["description", "status"]
            .into_iter()
            .map(String::from)
            .collect();
        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new(), &referenced);

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "title", "description", "status"]);
    }

    #[test]
    fn merge_entity_fields_full_collision_no_change() {
        // Caller already has all fields the imported entity declares.
        // After merge: no change.
        use orbital_core_base::schema::FieldType;

        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };
        let imported = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        // Every imported field is treated as referenced — this test is about
        // the name-collision short-circuit, not the usage gate.
        let referenced: HashSet<String> = [
            "id",
            "name",
            "description",
            "status",
            "createdAt",
            "pendingId",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new(), &referenced);

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
        use orbital_core_base::schema::FieldType;

        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };
        let import_browse = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };
        let import_modal = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        // First import adds name, status.
        let browse_referenced: HashSet<String> =
            ["name", "status"].into_iter().map(String::from).collect();
        merge_imported_entity_fields(
            &mut caller,
            &import_browse,
            false,
            &HashSet::new(),
            &browse_referenced,
        );
        // Second import adds pendingId, description (id already present).
        let modal_referenced: HashSet<String> = ["pendingId", "description"]
            .into_iter()
            .map(String::from)
            .collect();
        merge_imported_entity_fields(
            &mut caller,
            &import_modal,
            false,
            &HashSet::new(),
            &modal_referenced,
        );

        let names = field_names(&caller);
        assert_eq!(
            names,
            vec!["id", "name", "status", "pendingId", "description"]
        );
    }

    #[test]
    fn merge_entity_fields_no_uses_no_change() {
        // Standalone orbital with no imports — entity unchanged.
        use orbital_core_base::schema::FieldType;

        let caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
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
        use orbital_core_base::schema::{Cardinality, FieldType, RelationDefinition};

        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        let mut replies_field = make_field("replies", FieldType::Relation);
        replies_field.relation = Some(RelationDefinition {
            entity: "ThreadPost".to_string(),
            entity_id: Some("ent_IMPORTED_THREADPOST".to_string()),
            cardinality: Some(Cardinality::Many),
            on_delete: None,
        });
        let imported = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
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
            invariants: None,
        };

        let referenced: HashSet<String> = ["replies"].into_iter().map(String::from).collect();
        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new(), &referenced);

        let merged = caller
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("replies"))
            .expect("replies field must be merged onto the caller entity");
        let rel = merged
            .relation
            .as_ref()
            .expect("merged replies field must keep its relation");

        assert_eq!(
            rel.entity, "Ticket",
            "relation name must retarget to the caller entity"
        );
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
    fn merge_entity_fields_stamps_merged_from_on_added_fields_only() {
        // Host `ChatMessage { content! }` rebinds std-browse's `BrowseItem
        // { name! }` onto it (explicit rebind: intrinsic_only = false, the
        // long-standing full-field-set merge). The merged `name` field must
        // carry `merged_from == Some("BrowseItem")` — its `required: true`
        // is BrowseItem's own write contract, never ChatMessage's — while
        // `content`, authored by the host itself, must carry `merged_from
        // == None` even though it was already present before the merge ran.
        use orbital_core_base::schema::FieldType;

        let mut content_field = make_field("content", FieldType::String);
        content_field.required = Some(true);
        let mut caller = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "ChatMessage".to_string(),
            collection: None,
            persistence: None,
            fields: vec![content_field],
            instances: None,
            id: None,
            invariants: None,
        };

        let mut name_field = make_field("name", FieldType::String);
        name_field.required = Some(true);
        let imported = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "BrowseItem".to_string(),
            collection: None,
            persistence: None,
            fields: vec![name_field],
            instances: None,
            id: None,
            invariants: None,
        };

        let referenced: HashSet<String> = ["name"].into_iter().map(String::from).collect();
        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new(), &referenced);

        let content = caller
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("content"))
            .expect("host-authored content field must survive the merge");
        assert_eq!(
            content.merged_from, None,
            "a host-declared field must never carry merged_from"
        );

        let name = caller
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("name"))
            .expect("imported name field must be merged onto the host");
        assert_eq!(
            name.merged_from,
            Some("BrowseItem".to_string()),
            "a field ADDED by the rebind merge must carry the imported entity's name"
        );
    }

    #[test]
    fn get_entity_for_trait_orbital_finds_correct_orbital() {
        // Multi-orbital import: trait lives in orbital 1, its entity should
        // be returned (not orbital 0's entity).
        use orbital_core_base::schema::FieldType;

        let orb0 = Orbital {
            spec: false,
            types: None,
            name: "StateOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("StateTrait"))],
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
        };

        let orb1 = Orbital {
            spec: false,
            types: None,
            name: "ActionOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                store: None,
                shared: false,
                identity: false,
                local: false,
                seed_mock: false,
                locale_instances: None,
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
                invariants: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("ActionTrait"))],
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

    // ------------------------------------------------------------------
    // PendingMerge — call-site `fields { <old>: <new> }` rename must not
    // resurrect the renamed-away OLD field name (riya-level-sine.lolo's
    // `SinePickup = Pickup.traits.Pickup -> RiyaSineState { fields { pickups:
    // stars } }` corpus case, ORB_E_MERGED_FIELD_UNREFERENCED).
    // ------------------------------------------------------------------

    /// Writes a minimal imported atom — one entity `PickupData` with fields
    /// `id`/`pickups`, one trait `Pickup` bound to it whose only transition
    /// writes `@entity.pickups` — to `<dir>/pickup.orb` so `inline_schema`'s
    /// real `ExternalOrbitalLoader` can load it via a genuine `uses`. This
    /// exercises the actual `PendingMerge` code path (not just
    /// `merge_imported_entity_fields` in isolation), since the bug lived in
    /// what `referenced_fields` the pending-merge loop hands that fn.
    fn write_pickup_atom(dir: &std::path::Path) {
        let pickup_orb = r#"{
            "name": "PickupModule",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "PickupOrbital",
                    "entity": {
                        "name": "PickupData",
                        "fields": [
                            { "name": "id", "type": "string" },
                            { "name": "pickups", "type": "string" }
                        ]
                    },
                    "traits": [
                        {
                            "name": "Pickup",
                            "linkedEntity": "PickupData",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [ ["set", "@entity.pickups", "seed"] ]
                                    }
                                ]
                            }
                        }
                    ]
                }
            ],
            "ledger": { "schemaVersion": 1, "entries": {} }
        }"#;
        std::fs::write(dir.join("pickup.orb"), pickup_orb).expect("write pickup.orb");
    }

    /// Builds the consumer schema: one orbital with primary entity
    /// `HostEntity` (`id`, `stars`), composing `Pickup.traits.Pickup ->
    /// HostEntity` with an optional call-site `fields` rename.
    fn pickup_consumer_schema(field_rename: Option<HashMap<String, String>>) -> OrbitalSchema {
        let host_entity = EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "HostEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("stars", FieldType::String),
            ],
            instances: None,
            id: None,
            invariants: None,
        };
        let trait_ref = TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some("Pickup.traits.Pickup".to_string()),
            ref_id: None,
            name: None,
            linked_entity: Some("HostEntity".to_string()),
            linked_entity_id: None,
            config: None,
            events: None,
            event_ids: None,
            fields: field_rename,
            listens: None,
            emits_scope: None,
        };
        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![orbital_core_base::schema::UseDeclaration {
                lazy: None,
                from: "./pickup.orb".to_string(),
                alias: "Pickup".to_string(),
                config: None,
            }]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(host_entity),
            auxiliary_entities: None,
            traits: vec![trait_ref],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        }
    }

    fn pickup_schema_with_uses(uses: &[(&str, &str)]) -> OrbitalSchema {
        let mut schema = pickup_consumer_schema(None);
        schema.orbitals[0].uses = Some(
            uses.iter()
                .map(|(alias, from)| orbital_core_base::schema::UseDeclaration {
                    lazy: None,
                    from: from.to_string(),
                    alias: alias.to_string(),
                    config: None,
                })
                .collect(),
        );
        schema
    }

    fn inline_pickup(schema: &mut OrbitalSchema, dir: &std::path::Path) -> Result<(), InlineError> {
        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(schema, &config, None)
    }

    /// An organism whose orbitals share an entity: `OwnerOrbital` owns `Note` (aux,
    /// `id` only); `UserOrbital`'s trait writes `Note.body` through an atom.
    fn write_shared_entity_organism(dir: &std::path::Path) {
        let atom = serde_json::json!({"name": "NoteAtom", "orbitals": [{"name": "NoteOrbital",
            "entity": {"name": "Note", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true}, {"name": "body", "type": "string"}]},
            "traits": [{"name": "NoteKeeper", "linkedEntity": "Note", "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}], "events": [{"key": "WRITE"}],
                "transitions": [{"from": "idle", "to": "idle", "event": "WRITE", "effects": [["set", "@entity.body", "x"]]}]}}],
            "pages": []}]});
        let org = serde_json::json!({"name": "Org", "orbitals": [
            {"name": "OwnerOrbital",
             "entity": {"name": "Owner", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
             "auxiliaryEntities": [{"name": "Note", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]}],
             "traits": [], "pages": []},
            {"name": "UserOrbital", "uses": [{"from": "./atom.orb", "as": "Atom"}],
             "entity": {"name": "UserView", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
             "traits": [{"ref": "Atom.traits.NoteKeeper", "linkedEntity": "Note"}], "pages": []}]});
        std::fs::write(dir.join("atom.orb"), atom.to_string()).expect("atom.orb");
        std::fs::write(dir.join("org.orb"), org.to_string()).expect("org.orb");
    }

    fn shared_entity_consumer(user_entities: serde_json::Value) -> OrbitalSchema {
        let import = |name: &str, upstream: &str, entity: &str, entities: serde_json::Value| {
            let mut reference =
                serde_json::json!({"ref": format!("Org.orbitals.{upstream}"), "entity": entity});
            if !entities.is_null() {
                reference["entities"] = entities;
            }
            serde_json::json!({"name": name, "uses": [{"from": "./org.orb", "as": "Org"}],
                "entity": format!("Org.orbitals.{upstream}.entity"), "traits": [], "pages": [], "reference": reference})
        };
        serde_json::from_value(serde_json::json!({"name": "Consumer", "orbitals": [
            import("Owner", "OwnerOrbital", "Owner", serde_json::Value::Null),
            import("User", "UserOrbital", "UserView", user_entities)]}))
        .expect("consumer schema")
    }

    fn aux_fields(schema: &OrbitalSchema, orbital: &str, entity: &str) -> Vec<String> {
        let o = schema
            .orbitals
            .iter()
            .find(|o| o.name == orbital)
            .expect("orbital");
        o.auxiliary_entities
            .iter()
            .flatten()
            .find_map(|e| match e {
                EntityRef::Inline(def) if def.name == entity => {
                    Some(def.fields.iter().filter_map(|f| f.name.clone()).collect())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    fn linked_entities(schema: &OrbitalSchema, orbital: &str) -> Vec<String> {
        let o = schema
            .orbitals
            .iter()
            .find(|o| o.name == orbital)
            .expect("orbital");
        o.traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => td.linked_entity.clone(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_retargeted_shared_entity_carries_the_fields_the_importing_traits_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_shared_entity_organism(dir.path());
        let mut schema = shared_entity_consumer(serde_json::json!({"Note": "OwnerNote"}));
        inline_pickup(&mut schema, dir.path()).expect("inlines");
        assert!(
            linked_entities(&schema, "User").contains(&"OwnerNote".to_string()),
            "{:?}",
            linked_entities(&schema, "User")
        );
        assert!(
            aux_fields(&schema, "Owner", "OwnerNote").contains(&"body".to_string()),
            "{:?}",
            aux_fields(&schema, "Owner", "OwnerNote")
        );
    }

    #[test]
    fn control_without_a_retarget_the_import_keeps_its_own_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_shared_entity_organism(dir.path());
        let mut schema = shared_entity_consumer(serde_json::Value::Null);
        inline_pickup(&mut schema, dir.path()).expect("inlines");
        assert!(
            linked_entities(&schema, "User").contains(&"UserNote".to_string()),
            "{:?}",
            linked_entities(&schema, "User")
        );
    }

    fn diagnose(
        schema: &mut OrbitalSchema,
        dir: &std::path::Path,
    ) -> Vec<orbital::OrbitalRefDiagnostic> {
        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        let mut orbital_ref = Vec::new();
        inline_schema_with_orbital_diagnostics(
            schema,
            &config,
            None,
            &mut Vec::new(),
            &mut orbital_ref,
            &mut Vec::new(),
        )
        .expect("inlines");
        orbital_ref
    }

    fn import_of(
        name: &str,
        alias: &str,
        from: &str,
        upstream: &str,
        entity: &str,
        entities: serde_json::Value,
    ) -> serde_json::Value {
        let mut reference =
            serde_json::json!({"ref": format!("{alias}.orbitals.{upstream}"), "entity": entity});
        if !entities.is_null() {
            reference["entities"] = entities;
        }
        serde_json::json!({"name": name, "uses": [{"from": from, "as": alias}],
            "entity": format!("{alias}.orbitals.{upstream}.entity"), "traits": [], "pages": [], "reference": reference})
    }

    /// `base.orb`: `PersonOrbital` owns `Person`; `ChatOrbital`'s trait links it.
    /// `solution.orb` imports both, naming `Person` `Customer` and retargeting the chat onto it.
    fn write_nested_import_solution(dir: &std::path::Path, chat_entities: serde_json::Value) {
        let base = serde_json::json!({"name": "Base", "orbitals": [
            {"name": "PersonOrbital",
             "entity": {"name": "Person", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
             "traits": [], "pages": []},
            {"name": "ChatOrbital",
             "entity": {"name": "Chat", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
             "auxiliaryEntities": [{"name": "Person", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]}],
             "traits": [{"name": "ChatWho", "linkedEntity": "Person", "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}], "events": [{"key": "INIT"}],
                "transitions": [{"from": "idle", "to": "idle", "event": "INIT", "effects": []}]}}],
             "pages": []}]});
        let solution = serde_json::json!({"name": "Solution", "orbitals": [
            import_of("Account", "Base", "./base.orb", "PersonOrbital", "Customer", serde_json::Value::Null),
            import_of("Chat", "Base", "./base.orb", "ChatOrbital", "Chat", chat_entities)]});
        std::fs::write(dir.join("base.orb"), base.to_string()).expect("base.orb");
        std::fs::write(dir.join("solution.orb"), solution.to_string()).expect("solution.orb");
    }

    fn nested_consumer() -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({"name": "Consumer", "orbitals": [
            import_of("Account", "Sol", "./solution.orb", "Account", "Customer", serde_json::Value::Null),
            import_of("Chat", "Sol", "./solution.orb", "Chat", "Chat", serde_json::json!({"Customer": "Customer"}))]}))
        .expect("consumer schema")
    }

    #[test]
    fn an_imported_programs_own_retarget_resolves_against_its_own_sibling_imports() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_nested_import_solution(dir.path(), serde_json::json!({"Person": "Customer"}));
        let mut schema = nested_consumer();
        let diagnostics = diagnose(&mut schema, dir.path());
        let unknown: Vec<_> = diagnostics
            .iter()
            .flat_map(|d| d.entity_target_unknown.clone())
            .collect();
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(
            linked_entities(&schema, "Chat").contains(&"Customer".to_string()),
            "{:?}",
            linked_entities(&schema, "Chat")
        );
    }

    #[test]
    fn control_an_imported_programs_retarget_naming_nothing_is_still_diagnosed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_nested_import_solution(dir.path(), serde_json::json!({"Person": "Nobody"}));
        let mut schema = nested_consumer();
        let diagnostics = diagnose(&mut schema, dir.path());
        let unknown: Vec<_> = diagnostics
            .iter()
            .flat_map(|d| d.entity_target_unknown.clone())
            .collect();
        assert!(
            unknown.contains(&("Person".to_string(), "Nobody".to_string())),
            "{unknown:?}"
        );
    }

    /// One file, two orbitals sharing `Product`: `ProductOrbital` owns it, `ShelfOrbital`
    /// carries an auxiliary copy (pulled through an atom) its trait writes.
    fn write_same_file_shared_entity(dir: &std::path::Path) {
        let org = serde_json::json!({"name": "Shop", "orbitals": [
            {"name": "ProductOrbital",
             "entity": {"name": "Product", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true}, {"name": "title", "type": "string"}]},
             "traits": [], "pages": []},
            {"name": "ShelfOrbital",
             "entity": {"name": "Shelf", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
             "auxiliaryEntities": [{"name": "Product", "persistence": "runtime", "fields": [
                {"name": "id", "type": "string", "required": true}, {"name": "title", "type": "string"}]}],
             "traits": [{"name": "ShelfRows", "linkedEntity": "Product", "stateMachine": {
                "states": [{"name": "idle", "isInitial": true}], "events": [{"key": "INIT"}],
                "transitions": [{"from": "idle", "to": "idle", "event": "INIT", "effects": [["set", "@entity.title", "x"]]}]}}],
             "pages": []}]});
        std::fs::write(dir.join("shop.orb"), org.to_string()).expect("shop.orb");
    }

    fn same_file_consumer(shelf_entities: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({"name": "Consumer", "orbitals": [
            import_of("ProductOrbital", "Shop", "./shop.orb", "ProductOrbital", "Product", serde_json::Value::Null),
            import_of("ShelfOrbital", "Shop", "./shop.orb", "ShelfOrbital", "Shelf", shelf_entities)]}))
        .expect("consumer schema")
    }

    #[test]
    fn a_same_file_siblings_entity_is_shared_through_the_import_not_copied() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_same_file_shared_entity(dir.path());
        let mut schema = same_file_consumer(serde_json::json!({"Product": "Product"}));
        let diagnostics = diagnose(&mut schema, dir.path());
        let unknown_keys: Vec<_> = diagnostics
            .iter()
            .flat_map(|d| d.entity_unknown_key.clone())
            .collect();
        assert!(unknown_keys.is_empty(), "{unknown_keys:?}");
        assert!(
            linked_entities(&schema, "ShelfOrbital").contains(&"Product".to_string()),
            "{:?}",
            linked_entities(&schema, "ShelfOrbital")
        );
        assert!(
            aux_fields(&schema, "ShelfOrbital", "ShelfOrbitalProduct").is_empty(),
            "the shelf import must not keep its own copy"
        );
    }

    #[test]
    fn control_an_unmapped_same_file_siblings_entity_is_diagnosed_unmapped() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_same_file_shared_entity(dir.path());
        let mut schema = same_file_consumer(serde_json::Value::Null);
        let diagnostics = diagnose(&mut schema, dir.path());
        let unmapped: Vec<_> = diagnostics
            .iter()
            .flat_map(|d| {
                d.entity_unmapped
                    .iter()
                    .map(|(_, e)| e.clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(unmapped.contains(&"Product".to_string()), "{unmapped:?}");
    }

    /// `exp.orb` declares the app knob `welcomeTitle`; its `Welcome` trait forwards `headline`
    /// to it. `sol.orb` composes `Exp.traits.Welcome` into its own `View` orbital, optionally
    /// re-exposing the knob through its own app config.
    fn write_two_level_forward(
        dir: &std::path::Path,
        sol_uses_config: serde_json::Value,
        sol_config: serde_json::Value,
    ) {
        let exp = serde_json::json!({"name": "Exp", "config": {"welcomeTitle": {"type": "string", "default": "Your ledger"}},
            "orbitals": [{"name": "HomeOrbital",
                "entity": {"name": "Visit", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
                "traits": [{"name": "Welcome", "linkedEntity": "Visit",
                    "config": {"headline": {"type": "string", "default": "@config.welcomeTitle"}},
                    "stateMachine": {"states": [{"name": "idle", "isInitial": true}], "events": [{"key": "INIT"}],
                        "transitions": [{"from": "idle", "to": "idle", "event": "INIT",
                            "effects": [["render-ui", "main", {"type": "typography", "content": "@config.headline"}]]}]}}],
                "pages": [{"name": "Home", "path": "/", "traits": [{"ref": "Welcome"}]}]}]});
        let mut uses = serde_json::json!({"from": "./exp.orb", "as": "Exp"});
        if !sol_uses_config.is_null() {
            uses["config"] = sol_uses_config;
        }
        let mut sol = serde_json::json!({"name": "Sol", "orbitals": [{"name": "View", "uses": [uses],
            "entity": {"name": "ViewVisit", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}]},
            "traits": [{"ref": "Exp.traits.Welcome"}],
            "pages": [{"name": "ViewPage", "path": "/view", "traits": [{"ref": "Welcome"}]}]}]});
        if !sol_config.is_null() {
            sol["config"] = sol_config;
        }
        std::fs::write(dir.join("exp.orb"), exp.to_string()).expect("exp.orb");
        std::fs::write(dir.join("sol.orb"), sol.to_string()).expect("sol.orb");
    }

    fn two_level_consumer(uses_config: serde_json::Value) -> OrbitalSchema {
        let mut import = import_of(
            "View",
            "Sol",
            "./sol.orb",
            "View",
            "ViewVisit",
            serde_json::Value::Null,
        );
        if !uses_config.is_null() {
            import["uses"][0]["config"] = uses_config;
        }
        serde_json::from_value(serde_json::json!({"name": "Consumer", "orbitals": [import]}))
            .expect("consumer schema")
    }

    fn headline_default(schema: &OrbitalSchema) -> Option<serde_json::Value> {
        schema.orbitals[0].traits.iter().find_map(|t| match t {
            TraitReference::Inline(td) if td.name.ends_with("Welcome") => td
                .config
                .as_ref()
                .and_then(|c| c.get("headline"))
                .and_then(|f| f.default.as_ref())
                .and_then(|d| serde_json::to_value(d).ok()),
            _ => None,
        })
    }

    #[test]
    fn a_forward_to_an_app_knob_two_imports_deep_resolves_in_its_own_home() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_two_level_forward(dir.path(), serde_json::Value::Null, serde_json::Value::Null);
        let mut schema = two_level_consumer(serde_json::Value::Null);
        diagnose(&mut schema, dir.path());
        assert_eq!(
            headline_default(&schema),
            Some(serde_json::json!("Your ledger"))
        );
    }

    #[test]
    fn control_a_knob_the_middle_program_re_exposes_stays_overridable_by_its_importer() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_two_level_forward(
            dir.path(),
            serde_json::json!({"welcomeTitle": {"type": "unknown", "default": "@config.solTitle"}}),
            serde_json::json!({"solTitle": {"type": "string", "default": "Sol default"}}),
        );
        let mut schema = two_level_consumer(
            serde_json::json!({"solTitle": {"type": "unknown", "default": "Candles"}}),
        );
        diagnose(&mut schema, dir.path());
        assert_eq!(
            headline_default(&schema),
            Some(serde_json::json!("Candles"))
        );
    }

    /// `atom.orb`: `List` embeds its sibling `Filter`. `hr.orb` composes the atom twice:
    /// `HomeOrbital` (declared first) rebinds it to `Offboarding`, `EmployeeOrbital` to `Employee`.
    fn write_two_orbitals_composing_one_atom(dir: &std::path::Path) {
        let sm = |effects: serde_json::Value| {
            serde_json::json!({"states": [{"name": "idle", "isInitial": true}], "events": [{"key": "INIT"}],
            "transitions": [{"from": "idle", "to": "idle", "event": "INIT", "effects": effects}]})
        };
        let atom = serde_json::json!({"name": "BrowseAtom", "orbitals": [{"name": "ItemOrbital",
            "entity": {"name": "Item", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}, {"name": "title", "type": "string"}]},
            "traits": [
                {"name": "List", "linkedEntity": "Item", "stateMachine": sm(serde_json::json!([["render-ui", "main", {"type": "stack", "children": ["@trait.Filter"]}]]))},
                {"name": "Filter", "linkedEntity": "Item", "stateMachine": sm(serde_json::json!([["set", "@entity.title", "x"]]))}],
            "pages": []}]});
        let entity = |name: &str| serde_json::json!({"name": name, "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}, {"name": "title", "type": "string"}]});
        let hr = serde_json::json!({"name": "Hr", "orbitals": [
            {"name": "HomeOrbital", "uses": [{"from": "./atom.orb", "as": "HomeAtom"}], "entity": entity("HomeVisit"),
             "auxiliaryEntities": [entity("Offboarding")],
             "traits": [{"ref": "HomeAtom.traits.List", "name": "HomeList", "linkedEntity": "Offboarding"}], "pages": []},
            {"name": "EmployeeOrbital", "uses": [{"from": "./atom.orb", "as": "Atom"}], "entity": entity("Employee"),
             "traits": [{"ref": "Atom.traits.List", "name": "EmployeeList", "linkedEntity": "Employee"}], "pages": []}]});
        std::fs::write(dir.join("atom.orb"), atom.to_string()).expect("atom.orb");
        std::fs::write(dir.join("hr.orb"), hr.to_string()).expect("hr.orb");
    }

    fn consumer_composing(trait_name: &str) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({"name": "Consumer", "orbitals": [{"name": "PersonOrbital",
            "uses": [{"from": "./hr.orb", "as": "Hr"}],
            "entity": {"name": "Person", "persistence": "runtime", "fields": [{"name": "id", "type": "string", "required": true}, {"name": "title", "type": "string"}]},
            "traits": [{"ref": format!("Hr.traits.{trait_name}"), "linkedEntity": "Person"}], "pages": []}]}))
        .expect("consumer schema")
    }

    #[test]
    fn a_pulled_sibling_comes_from_the_orbital_that_declares_its_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_two_orbitals_composing_one_atom(dir.path());
        let mut schema = consumer_composing("EmployeeList");
        inline_pickup(&mut schema, dir.path()).expect("inlines");
        let linked = linked_entities(&schema, "PersonOrbital");
        assert!(!linked.contains(&"Offboarding".to_string()), "{linked:?}");
        let aux: Vec<String> = schema.orbitals[0]
            .auxiliary_entities
            .iter()
            .flatten()
            .filter_map(|e| match e {
                EntityRef::Inline(d) => Some(d.name.clone()),
                _ => None,
            })
            .collect();
        assert!(!aux.contains(&"Offboarding".to_string()), "{aux:?}");
    }

    #[test]
    fn control_composing_the_first_orbitals_trait_still_pulls_its_own_sibling() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_two_orbitals_composing_one_atom(dir.path());
        let mut schema = consumer_composing("HomeList");
        inline_pickup(&mut schema, dir.path()).expect("inlines");
        let names: Vec<String> = schema.orbitals[0]
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some(td.name.clone()),
                _ => None,
            })
            .collect();
        assert!(names.iter().any(|n| n.ends_with("Filter")), "{names:?}");
    }

    #[test]
    fn duplicate_use_alias_is_rejected() {
        // std-iram declared `uses UiCard from "std/behaviors/ui-card"` twice;
        // the JS resolver rejects it ("Duplicate import alias") while inline
        // silently kept the last — the two paths must agree.
        let dir = tempfile::tempdir().expect("tempdir");
        write_pickup_atom(dir.path());
        for uses in [
            vec![("Pickup", "./pickup.orb"), ("Pickup", "./pickup.orb")],
            vec![("Pickup", "./pickup.orb"), ("Pickup", "./other.orb")],
        ] {
            let mut schema = pickup_schema_with_uses(&uses);
            match inline_pickup(&mut schema, dir.path()) {
                Err(e @ InlineError::DuplicateUseAlias { .. }) => {
                    assert_eq!(e.structured_code(), "ORB_O_DUPLICATE_USE_ALIAS");
                    assert_eq!(
                        e.structured_path(),
                        "orbitals[name=ConsumerOrbital].uses[1]"
                    );
                }
                other => panic!("expected DuplicateUseAlias for {:?}, got {:?}", uses, other),
            }
        }
    }

    #[test]
    fn one_file_under_two_aliases_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pickup_atom(dir.path());
        let mut schema =
            pickup_schema_with_uses(&[("Pickup", "./pickup.orb"), ("PickupToo", "./pickup.orb")]);
        inline_pickup(&mut schema, dir.path()).expect("distinct aliases must inline");
    }

    #[test]
    fn pending_merge_drops_field_renamed_away_by_call_site_override() {
        // PASS: `fields { pickups: stars }` rewrites every `@entity.pickups`
        // read in Pickup's body to `@entity.stars` BEFORE the trait is
        // inlined (`apply_overrides_to_trait` / `rewrite_entity_field_paths`,
        // trait.rs). `stars` already exists on HostEntity, so nothing should
        // merge — merging `pickups` anyway would add a field the (renamed)
        // final trait body can never reference, tripping
        // ORB_E_MERGED_FIELD_UNREFERENCED on a field the merge itself made
        // dead.
        let dir = tempfile::tempdir().expect("tempdir");
        write_pickup_atom(dir.path());

        let mut rename = HashMap::new();
        rename.insert("pickups".to_string(), "stars".to_string());
        let mut schema = pickup_consumer_schema(Some(rename));

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
            panic!("expected inline entity post-merge");
        };
        assert!(
            !field_names(entity).contains(&"pickups".to_string()),
            "renamed-away field must not merge back in under its OLD name; fields: {:?}",
            field_names(entity)
        );
    }

    #[test]
    fn pending_merge_still_merges_referenced_field_without_rename_override() {
        // Contrast/guard-rail: with NO `fields` override, `pickups` is
        // genuinely referenced by Pickup's body and HostEntity has no field
        // of that name — the field must still merge (the exclusion above is
        // scoped to renamed keys only, not a blanket suppression).
        let dir = tempfile::tempdir().expect("tempdir");
        write_pickup_atom(dir.path());

        let mut schema = pickup_consumer_schema(None);

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
            panic!("expected inline entity post-merge");
        };
        let merged = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("pickups"))
            .expect("pickups must merge when there is no rename override");
        assert_eq!(merged.merged_from.as_deref(), Some("PickupData"));
    }

    // ------------------------------------------------------------------
    // §52 rebind-merge landing site — retiring the primary-entity fallback.
    // ------------------------------------------------------------------

    /// Writes an imported package with ONE orbital declaring a primary
    /// entity `ExtraFields` (`id`, `note`) and an author-declared auxiliary
    /// entity `SideEntity` (`id`) — to `<dir>/toolkit.orb`. Two traits:
    /// `ExtraTrait` (bound to `ExtraFields`, its only transition writes
    /// `@entity.note`) and `SideThing` (bound to `SideEntity`, a no-op
    /// transition). A consumer composing `ExtraTrait -> SideEntity` (explicit
    /// rebind) alongside `SideThing` (no rebind) exercises the real
    /// `PendingMerge` + Gap #22 code paths together, not a hand-built
    /// `EntityDefinition`.
    fn write_toolkit_atom(dir: &std::path::Path) {
        let toolkit_orb = r#"{
            "name": "ToolkitModule",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "ToolkitOrbital",
                    "entity": {
                        "name": "ExtraFields",
                        "fields": [
                            { "name": "id", "type": "string" },
                            { "name": "note", "type": "string" }
                        ]
                    },
                    "auxiliaryEntities": [
                        {
                            "name": "SideEntity",
                            "fields": [ { "name": "id", "type": "string" } ]
                        }
                    ],
                    "traits": [
                        {
                            "name": "ExtraTrait",
                            "linkedEntity": "ExtraFields",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [ ["set", "@entity.note", "hi"] ]
                                    }
                                ]
                            }
                        },
                        {
                            "name": "SideThing",
                            "linkedEntity": "SideEntity",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": []
                                    }
                                ]
                            }
                        }
                    ]
                }
            ],
            "ledger": { "schemaVersion": 1, "entries": {} }
        }"#;
        std::fs::write(dir.join("toolkit.orb"), toolkit_orb).expect("write toolkit.orb");
    }

    /// One reference-form trait ref: `"<alias>.traits.<name>"`, rebound to
    /// `linked_entity` when given (`None` composes without a rebind).
    fn toolkit_trait_ref(name: &str, linked_entity: Option<&str>) -> TraitReference {
        TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some(format!("Toolkit.traits.{name}")),
            ref_id: None,
            name: None,
            linked_entity: linked_entity.map(|s| s.to_string()),
            linked_entity_id: None,
            config: None,
            events: None,
            event_ids: None,
            fields: None,
            listens: None,
            emits_scope: None,
        }
    }

    fn toolkit_uses_decl() -> orbital_core_base::schema::UseDeclaration {
        orbital_core_base::schema::UseDeclaration {
            lazy: None,
            from: "./toolkit.orb".to_string(),
            alias: "Toolkit".to_string(),
            config: None,
        }
    }

    fn make_entity(name: &str, field_names: &[&str]) -> EntityDefinition {
        EntityDefinition {
            store: None,
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: name.to_string(),
            collection: None,
            persistence: None,
            fields: field_names
                .iter()
                .map(|f| make_field(f, FieldType::String))
                .collect(),
            instances: None,
            id: None,
            invariants: None,
        }
    }

    #[test]
    fn rebind_lands_on_aux_registered_by_a_later_no_rebind_import() {
        // Ordering case: `ExtraTrait -> SideEntity` is declared BEFORE
        // `SideThing` (whose no-rebind import is what registers `SideEntity`
        // as an auxiliary via Gap #22). The rebind target isn't visible
        // locally until the WHOLE orbital's traits have inlined — this is
        // exactly the std-construction-pm shape (§52 case (a)).
        let dir = tempfile::tempdir().expect("tempdir");
        write_toolkit_atom(dir.path());

        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![toolkit_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("HostEntity", &["id"])),
            auxiliary_entities: None,
            traits: vec![
                toolkit_trait_ref("ExtraTrait", Some("SideEntity")),
                toolkit_trait_ref("SideThing", None),
            ],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let mut schema = OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let EntityRef::Inline(host) = &schema.orbitals[0].entity else {
            panic!("expected inline primary entity");
        };
        assert!(
            !field_names(host).contains(&"note".to_string()),
            "the retired fallback used to land 'note' on the primary; fields: {:?}",
            field_names(host)
        );

        let side = schema.orbitals[0]
            .auxiliary_entities
            .iter()
            .flatten()
            .find_map(|e| match e {
                EntityRef::Inline(def) if def.name == "SideEntity" => Some(def),
                _ => None,
            })
            .expect("SideEntity must be registered as an auxiliary");
        let merged = side
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("note"))
            .expect("'note' must land on SideEntity, the rebind's actual target");
        assert_eq!(merged.merged_from.as_deref(), Some("ExtraFields"));
    }

    #[test]
    fn rebind_lands_on_a_sibling_orbitals_primary_entity() {
        // Cross-orbital case: `ExtraTrait -> SideOrbitalEntity` names another
        // TOP-LEVEL orbital's own primary entity, not anything local to the
        // composing orbital — the std-wiki `WikiHistory -> WikiRevision`
        // shape (§52 case (b)). Resolvable only once every orbital in the
        // schema has inlined (the schema-wide retry).
        let dir = tempfile::tempdir().expect("tempdir");
        write_toolkit_atom(dir.path());

        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![toolkit_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("HostEntity", &["id"])),
            auxiliary_entities: None,
            traits: vec![toolkit_trait_ref("ExtraTrait", Some("SideOrbitalEntity"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let sibling_orbital = Orbital {
            spec: false,
            types: None,
            name: "SiblingOrbital".to_string(),
            id: None,
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("SideOrbitalEntity", &["id"])),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let mut schema = OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital, sibling_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let EntityRef::Inline(host) = &schema.orbitals[0].entity else {
            panic!("expected inline primary entity");
        };
        assert!(
            !field_names(host).contains(&"note".to_string()),
            "the retired fallback used to land 'note' on the CALLER's primary; fields: {:?}",
            field_names(host)
        );

        let EntityRef::Inline(sibling) = &schema.orbitals[1].entity else {
            panic!("expected inline sibling entity");
        };
        let merged = sibling
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("note"))
            .expect("'note' must land on the sibling orbital's own primary entity");
        assert_eq!(merged.merged_from.as_deref(), Some("ExtraFields"));
    }

    #[test]
    fn rebind_to_a_name_absent_from_the_whole_schema_merges_nowhere() {
        // §52 hard stop: `ExtraTrait -> NoSuchEntity` names nothing anywhere
        // in the schema — neither the caller's own primary/auxiliaries nor
        // any sibling orbital's. Inline itself no longer has a fallback
        // landing site, so the field must not appear ANYWHERE; the schema
        // still inlines successfully (the hard error is
        // `ORB_T_REBIND_TARGET_UNRESOLVED`, raised by
        // `validation::trait_validator::validate_entity_rebind` on the
        // (unresolved) result, not by inline).
        let dir = tempfile::tempdir().expect("tempdir");
        write_toolkit_atom(dir.path());

        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![toolkit_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("HostEntity", &["id"])),
            auxiliary_entities: None,
            traits: vec![toolkit_trait_ref("ExtraTrait", Some("NoSuchEntity"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let mut schema = OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let EntityRef::Inline(host) = &schema.orbitals[0].entity else {
            panic!("expected inline primary entity");
        };
        assert!(
            !field_names(host).contains(&"note".to_string()),
            "an unresolvable target must never fall back onto the primary; fields: {:?}",
            field_names(host)
        );
        // `note` must not merge onto ANY entity anywhere — not the caller's
        // primary, and not onto any auxiliary either. `ToolkitOrbital`
        // declares `SideEntity` as its own auxiliary, but nothing in
        // `ExtraTrait`'s body references it by name (no fetch/persist, no
        // hoisted child bound to it), so the §52-follow-up aux-travel filter
        // (`trait_references_entity_by_name`) correctly leaves it behind too
        // — this loop is normally empty for this fixture.
        for aux in schema.orbitals[0].auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = aux {
                assert!(
                    !field_names(def).contains(&"note".to_string()),
                    "an unresolvable target must not land on an unrelated auxiliary either; entity {:?} fields: {:?}",
                    def.name,
                    field_names(def)
                );
                assert_ne!(
                    def.name, "NoSuchEntity",
                    "an unresolvable target name must never itself materialize as an entity"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // §52 follow-up — aux-entities-travel is reference-gated, not
    // unconditional (owner ruling 2026-09-19): an imported orbital's
    // auxiliary entity now travels into the consumer only when the composed
    // trait — or a hoisted inline child of it — still references that
    // entity: as a fetch/persist target name, as a hoisted child's OWN
    // `linkedEntity` binding, or as the trait's effective (post-rebind)
    // bound entity. An entity nothing references, including a trait's own
    // PRE-rebind binding once the consumer rebinds it elsewhere (the
    // std-wiki `WikiHistory -> WikiRevision` shape), does not travel.
    // ------------------------------------------------------------------

    /// Writes a source package with one orbital (`SourceOrbital`, primary
    /// `MainWidget`) declaring FOUR auxiliary entities and three traits:
    /// - `MainTrait` (bound to `MainWidget`) `persist create`s `TouchedAux`
    ///   (a real fetch/persist reference) and embeds `@trait.ChildWidget`.
    /// - `ChildWidget`, MainTrait's hoisted child, bound to `ChildBoundAux`
    ///   — referenced only via ITS OWN `linkedEntity`, never fetch/persist.
    /// - `RebindableTrait`, bound to `NestedBoundAux` — analogous to
    ///   std-wiki-page's `WikiHistory -> WikiPageRevision`: nothing in its
    ///   body references `NestedBoundAux` by name, it's simply what the
    ///   trait happens to be bound to until a consumer rebinds it away.
    /// `UntouchedAux` is referenced by nothing anywhere — the baseline
    /// "never travels" case.
    fn write_refs_atom(dir: &std::path::Path) {
        let refs_orb = r#"{
            "name": "RefsModule",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "SourceOrbital",
                    "entity": {
                        "name": "MainWidget",
                        "fields": [ { "name": "id", "type": "string" } ]
                    },
                    "auxiliaryEntities": [
                        {
                            "name": "TouchedAux",
                            "fields": [
                                { "name": "id", "type": "string" },
                                { "name": "flag", "type": "boolean" }
                            ]
                        },
                        {
                            "name": "ChildBoundAux",
                            "fields": [ { "name": "id", "type": "string" } ]
                        },
                        {
                            "name": "UntouchedAux",
                            "fields": [ { "name": "id", "type": "string" } ]
                        },
                        {
                            "name": "NestedBoundAux",
                            "fields": [
                                { "name": "id", "type": "string" },
                                { "name": "secret", "type": "string" }
                            ]
                        },
                        {
                            "name": "RevisionLikeAux",
                            "fields": [
                                { "name": "id", "type": "string" },
                                { "name": "mergedFieldA", "type": "string", "mergedFrom": "SourceX" },
                                { "name": "mergedFieldB", "type": "string", "mergedFrom": "SourceX" }
                            ]
                        },
                        {
                            "name": "UnionAux",
                            "fields": [
                                { "name": "id", "type": "string" },
                                { "name": "fieldA", "type": "string", "mergedFrom": "SourceY" },
                                { "name": "fieldB", "type": "string", "mergedFrom": "SourceY" },
                                { "name": "fieldC", "type": "string", "mergedFrom": "SourceY" }
                            ]
                        }
                    ],
                    "traits": [
                        {
                            "name": "MainTrait",
                            "linkedEntity": "MainWidget",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [
                                            ["persist", "create", "TouchedAux", { "flag": true }],
                                            ["embed", "@trait.ChildWidget"]
                                        ]
                                    }
                                ]
                            }
                        },
                        {
                            "name": "ChildWidget",
                            "linkedEntity": "ChildBoundAux",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": []
                                    }
                                ]
                            }
                        },
                        {
                            "name": "RebindableTrait",
                            "linkedEntity": "NestedBoundAux",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": []
                                    }
                                ]
                            }
                        },
                        {
                            "name": "RebindableMergedTrait",
                            "linkedEntity": "RevisionLikeAux",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [ ["set", "@entity.mergedFieldA", "x"] ]
                                    }
                                ]
                            }
                        },
                        {
                            "name": "CrossPersistTrait",
                            "linkedEntity": "MainWidget",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [
                                            ["persist", "create", "RevisionLikeAux", { "mergedFieldB": "y" }]
                                        ]
                                    }
                                ]
                            }
                        },
                        {
                            "name": "ReadsFieldA",
                            "linkedEntity": "UnionAux",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [ ["set", "@entity.fieldA", "x"] ]
                                    }
                                ]
                            }
                        },
                        {
                            "name": "ReadsFieldB",
                            "linkedEntity": "UnionAux",
                            "stateMachine": {
                                "states": [ { "name": "active", "isInitial": true } ],
                                "events": [],
                                "transitions": [
                                    {
                                        "from": "active",
                                        "to": "active",
                                        "event": "INIT",
                                        "effects": [ ["set", "@entity.fieldB", "x"] ]
                                    }
                                ]
                            }
                        }
                    ]
                }
            ],
            "ledger": { "schemaVersion": 1, "entries": {} }
        }"#;
        std::fs::write(dir.join("refs.orb"), refs_orb).expect("write refs.orb");
    }

    fn refs_trait_ref(name: &str, linked_entity: Option<&str>) -> TraitReference {
        TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some(format!("Refs.traits.{name}")),
            ref_id: None,
            name: None,
            linked_entity: linked_entity.map(|s| s.to_string()),
            linked_entity_id: None,
            config: None,
            events: None,
            event_ids: None,
            fields: None,
            listens: None,
            emits_scope: None,
        }
    }

    fn refs_uses_decl() -> orbital_core_base::schema::UseDeclaration {
        orbital_core_base::schema::UseDeclaration {
            lazy: None,
            from: "./refs.orb".to_string(),
            alias: "Refs".to_string(),
            config: None,
        }
    }

    fn aux_names(orbital: &Orbital) -> Vec<String> {
        orbital
            .auxiliary_entities
            .iter()
            .flatten()
            .filter_map(|e| match e {
                EntityRef::Inline(def) => Some(def.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn aux_travels_when_named_as_fetch_persist_target_or_hoisted_childs_binding() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_refs_atom(dir.path());

        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![refs_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("HostEntity", &["id"])),
            auxiliary_entities: None,
            traits: vec![refs_trait_ref("MainTrait", Some("HostEntity"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let mut schema = OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let names = aux_names(&schema.orbitals[0]);
        assert!(
            names.contains(&"TouchedAux".to_string()),
            "an auxiliary the trait `persist`s into by name must travel; aux: {names:?}"
        );
        assert!(
            names.contains(&"ChildBoundAux".to_string()),
            "an auxiliary that is a hoisted child's OWN linkedEntity binding must travel; aux: {names:?}"
        );
        assert!(
            !names.contains(&"UntouchedAux".to_string()),
            "an auxiliary nothing references must NOT travel; aux: {names:?}"
        );
    }

    #[test]
    fn aux_does_not_travel_once_rebound_away_from_its_pre_rebind_binding() {
        // The std-wiki shape: `RebindableTrait`'s OWN (pre-rebind) binding is
        // `NestedBoundAux`, exactly like `WikiHistory`'s own binding to
        // `std-wiki-page`'s `WikiPageRevision` before std-wiki rebinds it to
        // `WikiRevision`. Nothing in `RebindableTrait`'s body references
        // `NestedBoundAux` by name, so once the consumer rebinds the trait
        // AWAY from it, `NestedBoundAux` must not ride along as a phantom
        // auxiliary copy.
        let dir = tempfile::tempdir().expect("tempdir");
        write_refs_atom(dir.path());

        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![refs_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("OtherHost", &["id"])),
            auxiliary_entities: None,
            traits: vec![refs_trait_ref("RebindableTrait", Some("OtherHost"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        let mut schema = OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };

        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.path().to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(&mut schema, &config, None).expect("inline must succeed");

        let names = aux_names(&schema.orbitals[0]);
        assert!(
            !names.contains(&"NestedBoundAux".to_string()),
            "a trait's PRE-rebind own binding must not travel once the consumer rebinds it elsewhere; aux: {names:?}"
        );
    }

    // ------------------------------------------------------------------
    // Owner ruling 2026-09-19: a traveled auxiliary keeps its declared
    // (non-`mergedFrom`) fields plus every `mergedFrom` field the
    // consumer's importing trait refs actually reference on it — the union
    // over every trait ref that travels it, since the copy is one shared
    // entry per name (`seen_aux_names`).
    // ------------------------------------------------------------------

    fn aux_field_names(orbital: &Orbital, entity_name: &str) -> Vec<String> {
        orbital
            .auxiliary_entities
            .iter()
            .flatten()
            .filter_map(|e| match e {
                EntityRef::Inline(def) if def.name == entity_name => Some(def),
                _ => None,
            })
            .flat_map(|def| def.fields.iter().filter_map(|f| f.name.clone()))
            .collect()
    }

    fn refs_consumer_schema(traits: Vec<TraitReference>) -> OrbitalSchema {
        let consumer_orbital = Orbital {
            spec: false,
            types: None,
            name: "ConsumerOrbital".to_string(),
            id: None,
            uses: Some(vec![refs_uses_decl()]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(make_entity("HostEntity", &["id"])),
            auxiliary_entities: None,
            traits,
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            reference: None,
            config: None,
        };
        OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "ConsumerModule".to_string(),
            version: None,
            description: None,
            orbitals: vec![consumer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        }
    }

    fn inline_refs_schema(dir: &std::path::Path, schema: &mut OrbitalSchema) {
        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: dir.to_path_buf(),
                ..Default::default()
            },
            skip_inline: false,
            session: None,
        };
        inline_schema(schema, &config, None).expect("inline must succeed");
    }

    #[test]
    fn no_rebind_import_reading_a_merged_field_keeps_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_refs_atom(dir.path());
        let mut schema = refs_consumer_schema(vec![refs_trait_ref("ReadsFieldA", None)]);
        inline_refs_schema(dir.path(), &mut schema);

        let fields = aux_field_names(&schema.orbitals[0], "UnionAux");
        assert!(
            fields.contains(&"id".to_string()),
            "declared field must survive; fields: {fields:?}"
        );
        assert!(
            fields.contains(&"fieldA".to_string()),
            "referenced merged field must survive; fields: {fields:?}"
        );
        assert!(
            !fields.contains(&"fieldB".to_string()),
            "unreferenced merged field must not survive; fields: {fields:?}"
        );
        assert!(
            !fields.contains(&"fieldC".to_string()),
            "unreferenced merged field must not survive; fields: {fields:?}"
        );
    }

    #[test]
    fn two_imports_of_the_same_entity_keep_the_union_of_what_each_references() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_refs_atom(dir.path());
        let mut schema = refs_consumer_schema(vec![
            refs_trait_ref("ReadsFieldA", None),
            refs_trait_ref("ReadsFieldB", None),
        ]);
        inline_refs_schema(dir.path(), &mut schema);

        let fields = aux_field_names(&schema.orbitals[0], "UnionAux");
        assert!(fields.contains(&"fieldA".to_string()), "fields: {fields:?}");
        assert!(
            fields.contains(&"fieldB".to_string()),
            "the union of both importers must survive; fields: {fields:?}"
        );
        assert!(
            !fields.contains(&"fieldC".to_string()),
            "still unreferenced by either importer; fields: {fields:?}"
        );
    }

    #[test]
    fn rebound_away_drops_unreferenced_merged_field_keeps_declared_and_cross_referenced() {
        // The std-wiki shape: `RebindableMergedTrait`'s NATURAL binding is
        // `RevisionLikeAux` (reads `@entity.mergedFieldA` there), but the
        // consumer rebinds it to `HostEntity` instead — so `mergedFieldA`
        // is referenced by nobody once travel is decided. `RevisionLikeAux`
        // still travels because `CrossPersistTrait` (bound to `MainWidget`)
        // persists into it, referencing `mergedFieldB` specifically.
        let dir = tempfile::tempdir().expect("tempdir");
        write_refs_atom(dir.path());
        let mut schema = refs_consumer_schema(vec![
            refs_trait_ref("RebindableMergedTrait", Some("HostEntity")),
            refs_trait_ref("CrossPersistTrait", None),
        ]);
        inline_refs_schema(dir.path(), &mut schema);

        let fields = aux_field_names(&schema.orbitals[0], "RevisionLikeAux");
        assert!(
            fields.contains(&"id".to_string()),
            "declared field always survives; fields: {fields:?}"
        );
        assert!(
            fields.contains(&"mergedFieldB".to_string()),
            "CrossPersistTrait's persist target field must survive; fields: {fields:?}"
        );
        assert!(
            !fields.contains(&"mergedFieldA".to_string()),
            "nothing references mergedFieldA once RebindableMergedTrait rebinds away; fields: {fields:?}"
        );
    }

    #[test]
    fn shared_primitive_own_vs_cross_entity() {
        // (iv): `trait_referenced_fields_of_entity` is THE ONE field-usage
        // definition both `validate_all_merged_field_usage` and the Gap #22
        // travel filter call (grep-provable: one definition in entity.rs,
        // two call sites — `validation/entity_mutation.rs` and
        // `inline/mod.rs`, both via `crate::phases::inline::…`). This
        // exercises its own-entity vs cross-entity branch directly.
        use orbital_core_base::schema::{StringOrArray, TransitionDefinition};
        let bound = make_entity("Bound", &["id", "note"]);
        let other = make_entity("Other", &["id", "flag"]);
        let mut trait_def = make_trait("T");
        trait_def.linked_entity = Some("Bound".to_string());
        trait_def
            .state_machine
            .transitions
            .push(TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("active".to_string()),
                to: "active".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(vec![
                    SExpression::Array(vec![s("set"), s("@entity.note"), s("x")]),
                    SExpression::Array(vec![
                        s("persist"),
                        s("create"),
                        s("Other"),
                        SExpression::Object(
                            [("flag".to_string(), s("true"))].into_iter().collect(),
                        ),
                    ]),
                ]),
                ensures: None,
            });

        let own = trait_referenced_fields_of_entity(&trait_def, &bound, true);
        assert!(
            own.contains("note"),
            "own-entity must pick up @entity.note; got {own:?}"
        );

        let cross = trait_referenced_fields_of_entity(&trait_def, &other, false);
        assert!(
            cross.contains("flag"),
            "cross-entity must pick up the persist target field; got {cross:?}"
        );
        assert!(
            !cross.contains("note"),
            "cross-entity must NOT pick up @entity.* reads; got {cross:?}"
        );
    }
}

/// Drop every auxiliary copy that is a narrower view of an entity some orbital owns as its primary.
/// Drop shadowed auxiliary copies, run the deferred merges onto the owners, then drop again: a
/// copy that only lacked a field a deferred merge brings to the owner is a narrower view once the
/// merge lands, while a copy with a shape no merge supplies stays a reported conflict.
fn settle_shadowed_auxiliaries(orbitals: &mut [Orbital], deferred_merges: &[DeferredMerge]) {
    move_copy_merges_to_owners(orbitals);
    drop_shadowed_auxiliaries(orbitals);
    for dm in deferred_merges {
        merge_into_named_entity(
            orbitals,
            &dm.target,
            &dm.imported,
            dm.intrinsic_only,
            &dm.required_data,
            &dm.referenced_fields,
        );
    }
    drop_shadowed_auxiliaries(orbitals);
}

/// A merge that landed on an auxiliary copy of an entity another orbital owns (the copy
/// existed before the owner inlined) belongs to the owner's record: its merged fields move
/// there, so which orbital inlined first never decides where a field lives.
fn move_copy_merges_to_owners(orbitals: &mut [Orbital]) {
    let owners: HashMap<String, usize> = orbitals
        .iter()
        .enumerate()
        .filter_map(|(i, o)| match &o.entity {
            EntityRef::Inline(e) => Some((e.name.clone(), i)),
            _ => None,
        })
        .collect();
    let mut moves: Vec<(usize, orbital_core_base::schema::FieldDefinition)> = Vec::new();
    for (i, orbital) in orbitals.iter().enumerate() {
        for aux in orbital.auxiliary_entities.iter().flatten() {
            let EntityRef::Inline(copy) = aux else {
                continue;
            };
            let Some(&owner) = owners.get(&copy.name).filter(|&&o| o != i) else {
                continue;
            };
            moves.extend(
                copy.fields
                    .iter()
                    .filter(|f| f.merged_from.is_some())
                    .map(|f| (owner, f.clone())),
            );
        }
    }
    for (owner, field) in moves {
        if let EntityRef::Inline(entity) = &mut orbitals[owner].entity {
            if !entity.fields.iter().any(|f| f.name == field.name) {
                entity.fields.push(field);
            }
        }
    }
}

pub(crate) fn drop_shadowed_auxiliaries(orbitals: &mut [Orbital]) {
    let owned: HashMap<String, EntityDefinition> = orbitals
        .iter()
        .filter_map(|o| match &o.entity {
            EntityRef::Inline(e) => Some((e.name.clone(), e.clone())),
            _ => None,
        })
        .collect();
    // A narrower view of the owner's entity: every field it declares is the owner's, same type,
    // same collection. A copy with a shape the owner lacks is a real conflict and stays reported.
    let narrower_view = |copy: &EntityDefinition, owner: &EntityDefinition| -> bool {
        copy.collection.is_none() == owner.collection.is_none()
            && copy.collection == owner.collection
            && copy.fields.iter().all(|f| {
                owner
                    .fields
                    .iter()
                    .any(|o| o.name == f.name && o.field_type == f.field_type)
            })
    };
    for orbital in orbitals.iter_mut() {
        let Some(aux) = orbital.auxiliary_entities.as_mut() else {
            continue;
        };
        aux.retain(|a| {
            !matches!(a, EntityRef::Inline(e) if owned.get(&e.name).is_some_and(|owner| narrower_view(e, owner)))
        });
        if aux.is_empty() {
            orbital.auxiliary_entities = None;
        }
    }
}

#[cfg(test)]
mod shadowed_auxiliary_tests {
    use super::*;

    fn orbitals() -> Vec<Orbital> {
        serde_json::from_value(serde_json::json!([
            { "name": "OrderDesk", "entity": { "name": "OrderRecord", "fields": [
                { "name": "id", "type": "string" }, { "name": "title", "type": "string" }, { "name": "stage", "type": "string" } ] },
              "traits": [] },
            { "name": "MyOrdersPage", "entity": { "name": "Visit", "fields": [ { "name": "id", "type": "string" } ] },
              "auxiliaryEntities": [
                { "name": "OrderRecord", "fields": [ { "name": "id", "type": "string" }, { "name": "title", "type": "string" } ] },
                { "name": "ExportPanel", "fields": [ { "name": "id", "type": "string" } ] }
              ],
              "traits": [] }
        ]))
        .expect("orbitals")
    }

    fn aux_names(o: &Orbital) -> Vec<String> {
        o.auxiliary_entities
            .iter()
            .flatten()
            .filter_map(|e| match e {
                EntityRef::Inline(d) => Some(d.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_auxiliary_copy_of_an_entity_another_orbital_owns_is_dropped() {
        let mut o = orbitals();
        drop_shadowed_auxiliaries(&mut o);
        assert_eq!(aux_names(&o[1]), vec!["ExportPanel".to_string()]);
    }

    #[test]
    fn control_a_copy_with_a_shape_the_owner_lacks_is_a_real_conflict_and_kept() {
        let mut o = orbitals();
        if let Some(EntityRef::Inline(copy)) =
            o[1].auxiliary_entities.as_mut().and_then(|a| a.first_mut())
        {
            copy.fields.push(
                serde_json::from_value(
                    serde_json::json!({ "name": "importedOnly", "type": "string" }),
                )
                .expect("field"),
            );
        }
        drop_shadowed_auxiliaries(&mut o);
        assert!(aux_names(&o[1]).contains(&"OrderRecord".to_string()));
    }

    fn copy_with(o: &mut [Orbital], field: &str) {
        if let Some(EntityRef::Inline(copy)) =
            o[1].auxiliary_entities.as_mut().and_then(|a| a.first_mut())
        {
            copy.fields.push(
                serde_json::from_value(serde_json::json!({ "name": field, "type": "string" }))
                    .expect("field"),
            );
        }
    }

    fn merge_of(field: &str) -> DeferredMerge {
        DeferredMerge {
            target: "OrderRecord".to_string(),
            imported: serde_json::from_value(serde_json::json!({
                "name": "OrderRecord",
                "fields": [ { "name": "id", "type": "string" }, { "name": field, "type": "string", "mergedFrom": "Stats" } ]
            }))
            .expect("entity"),
            intrinsic_only: false,
            required_data: HashSet::new(),
            referenced_fields: [field.to_string()].into_iter().collect(),
        }
    }

    #[test]
    fn a_copy_that_only_lacked_a_field_a_deferred_merge_brings_is_dropped_after_the_merge() {
        let mut o = orbitals();
        copy_with(&mut o, "cards");
        settle_shadowed_auxiliaries(&mut o, &[merge_of("cards")]);
        assert_eq!(aux_names(&o[1]), vec!["ExportPanel".to_string()]);
        assert!(
            matches!(&o[0].entity, EntityRef::Inline(d) if d.fields.iter().any(|f| f.name.as_deref() == Some("cards")))
        );
    }

    fn copy_with_merged(o: &mut [Orbital], field: &str) {
        if let Some(EntityRef::Inline(copy)) =
            o[1].auxiliary_entities.as_mut().and_then(|a| a.first_mut())
        {
            copy.fields.push(
                serde_json::from_value(
                    serde_json::json!({ "name": field, "type": "string", "mergedFrom": "Stats" }),
                )
                .expect("field"),
            );
        }
    }

    #[test]
    fn a_merge_that_landed_on_a_shadowing_copy_moves_to_the_owner() {
        let mut o = orbitals();
        copy_with_merged(&mut o, "cards");
        settle_shadowed_auxiliaries(&mut o, &[]);
        assert_eq!(aux_names(&o[1]), vec!["ExportPanel".to_string()]);
        assert!(matches!(&o[0].entity, EntityRef::Inline(d)
            if d.fields.iter().any(|f| f.name.as_deref() == Some("cards") && f.merged_from.as_deref() == Some("Stats"))));
    }

    #[test]
    fn control_an_authored_field_on_a_shadowing_copy_does_not_move() {
        let mut o = orbitals();
        copy_with(&mut o, "importedOnly");
        settle_shadowed_auxiliaries(&mut o, &[]);
        assert!(aux_names(&o[1]).contains(&"OrderRecord".to_string()));
        assert!(matches!(&o[0].entity, EntityRef::Inline(d)
            if !d.fields.iter().any(|f| f.name.as_deref() == Some("importedOnly"))));
    }

    #[test]
    fn control_a_copy_whose_extra_field_no_merge_brings_stays_a_conflict() {
        let mut o = orbitals();
        copy_with(&mut o, "importedOnly");
        settle_shadowed_auxiliaries(&mut o, &[merge_of("cards")]);
        assert!(aux_names(&o[1]).contains(&"OrderRecord".to_string()));
    }

    #[test]
    fn control_owners_and_unowned_auxiliaries_are_untouched() {
        let mut o = orbitals();
        drop_shadowed_auxiliaries(&mut o);
        assert!(
            matches!(&o[0].entity, EntityRef::Inline(d) if d.name == "OrderRecord" && d.fields.len() == 3)
        );
        assert!(aux_names(&o[1]).contains(&"ExportPanel".to_string()));
    }
}
