//! `uses lazy`: a behavior imported lazily is compiled to its own `.orb` and
//! loaded when one of its pages is opened. Each `orbital X = Alias.orbitals.Y`
//! reference through a lazy alias is split out of the importer into its own
//! schema, which then goes through the same pipeline as any schema. The
//! importer keeps only `lazyPages`, recorded from the split schema's resolved
//! pages.

use super::inline::InlineConfig;
use super::inline::InlineError;
use super::{resolve_schema_with_inline, CompileResult, CompilerOptions};
use orbital_core_base::schema::{LazyPage, LazyTrait, Orbital, OrbitalSchema, PageRef, TraitReference};
use std::path::Path;

/// One lazily-loaded behavior, split out of its importer.
#[derive(Debug, Clone)]
pub struct LazyArtifact {
    /// The importer's orbital name for it — also the split schema's name.
    pub orbital: String,
    /// Where the split schema's `.orb` lives, relative to the importer's `.orb`.
    pub orb_ref: String,
    /// The split schema, unresolved.
    pub schema: OrbitalSchema,
    /// The eager orbital that preceded it in the importer.
    pub after: Option<String>,
}

/// A resolved lazily-loaded behavior, ready to write at `orb_ref` (relative to
/// its importer's `.orb`). Its own lazy imports nest under it the same way.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedLazy {
    pub orbital: String,
    pub orb_ref: String,
    pub schema: OrbitalSchema,
    pub lazy: Vec<ResolvedLazy>,
    /// The key this behavior is stored under in the options' `LazyResolveCache`, when there is one; never written.
    #[serde(skip)]
    pub cache_key: Option<String>,
}

/// One lazily-used behavior resolved, with every file its resolution read: reused while each is as it was read.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedLazy {
    pub resolved: ResolvedLazy,
    pub reads: orbital_core_base::read_set::ReadSet,
}

/// Where resolved lazy behaviors are kept between resolves, by the key [`split_lazy_and_resolve`] computes. Storage only:
/// the resolver decides whether an entry is still current from its reads.
pub trait LazyResolveCache: Send + Sync {
    fn get(&self, key: &str) -> Option<CachedLazy>;
    /// Store `entry` under `key`; `slot` names the lazy behavior (its importer and file), whose older entries it replaces.
    fn put(&self, slot: &str, key: &str, entry: CachedLazy);
}

/// The resolve options' lazy-resolve cache; none by default.
#[derive(Clone, Default)]
pub struct LazyCache(pub Option<std::sync::Arc<dyn LazyResolveCache>>);

impl std::fmt::Debug for LazyCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "LazyCache(set)" } else { "LazyCache(none)" })
    }
}

/// The lazy behavior an entry is for, whatever its sources say: its importer and its file.
fn lazy_cache_slot(artifact: &LazyArtifact, source_path: Option<&Path>) -> String {
    let text = format!("{}\n{}", source_path.map(|p| p.display().to_string()).unwrap_or_default(), artifact.orb_ref);
    orbital_core_base::codec::sha256(text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// What a lazy behavior's resolution depends on besides the files it reads: its own split program, where it was
/// imported from, and the resolve options that change the result.
fn lazy_cache_key(artifact: &LazyArtifact, options: &CompilerOptions, source_path: Option<&Path>) -> Option<String> {
    let mut text = String::from("lazy-resolve 3\n");
    text.push_str(&serde_json::to_string(&artifact.schema).ok()?);
    text.push('\n');
    text.push_str(&artifact.orb_ref);
    text.push('\n');
    text.push_str(&source_path.map(|p| p.display().to_string()).unwrap_or_default());
    text.push_str(if options.nominalize { "\nnominalize" } else { "\nplain" });
    for package in &options.inline.loader_config.packages {
        text.push_str(&format!("\n{}@{}:{}:{}", package.name, package.version, package.prefix, package.registry.root().display()));
        if let Some(source) = &package.lolo_source {
            text.push_str(&format!(":{}", source.display()));
        }
    }
    let mut scoped: Vec<String> = options.inline.loader_config.scoped_paths.iter().map(|(scope, path)| format!("{scope}={}", path.display())).collect();
    scoped.sort();
    for scope in scoped {
        text.push('\n');
        text.push_str(&scope);
    }
    Some(orbital_core_base::codec::sha256(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect())
}

pub fn lazy_orb_ref(orbital: &str) -> String {
    format!("lazy/{orbital}.orb")
}

fn reference_alias(orbital: &Orbital) -> Option<&str> {
    let reference = orbital.reference.as_ref()?;
    reference.reference.split('.').next()
}

pub(crate) fn is_lazy_reference(orbital: &Orbital) -> bool {
    let Some(alias) = reference_alias(orbital) else {
        return false;
    };
    orbital
        .uses
        .iter()
        .flatten()
        .any(|u| u.alias == alias && u.lazy == Some(true))
}

/// Removes every orbital that references a lazy alias from `schema` and returns
/// each as its own unresolved schema. The split schema keeps the importer's
/// app-level theme, declared themes, config, egress and ledger, and only the uses it can resolve: the
/// eager ones plus its own alias, no longer lazy.
pub fn split_lazy(schema: &mut OrbitalSchema) -> Vec<LazyArtifact> {
    let mut lazy: Vec<(Option<String>, Orbital)> = Vec::new();
    let mut eager: Vec<Orbital> = Vec::new();
    for orbital in std::mem::take(&mut schema.orbitals) {
        if is_lazy_reference(&orbital) {
            lazy.push((eager.last().map(|o| o.name.clone()), orbital));
        } else {
            eager.push(orbital);
        }
    }
    schema.orbitals = eager;
    lazy.into_iter()
        .map(|(after, mut orbital)| {
            let own_alias = reference_alias(&orbital).unwrap_or_default().to_string();
            orbital.uses = orbital.uses.map(|uses| {
                uses.into_iter()
                    .filter(|u| u.lazy != Some(true) || u.alias == own_alias)
                    .map(|mut u| {
                        u.lazy = None;
                        u
                    })
                    .collect()
            });
            let name = orbital.name.clone();
            let ledger = take_ledger_rows(&mut schema.ledger, orbital.id.as_deref());
            LazyArtifact {
                after,
                orb_ref: lazy_orb_ref(&name),
                schema: OrbitalSchema {
                    type_contract_version: None,
                    resolved: None,                    types: None,
                    store: schema.store.clone(),
                    theme_definitions: None,
                    themes: schema.themes.clone(),
                    site: schema.site.clone(),
                    public_dir: None,
                    name: name.clone(),
                    version: schema.version.clone(),
                    description: None,
                    theme: schema.theme.clone(),
                    config: schema.config.clone(),
                    orbitals: vec![orbital],
                    locales: schema.locales.clone(),
                    messages: None,
                    lazy_pages: None,
                    metadata: None,
                    schema_version: schema.schema_version,
                    ledger,
                    dispatch_kernel: None,
                    egress: schema.egress.clone(),
                    inbound: schema.inbound.clone(),
                    os_grants: schema.os_grants.clone(),
                    deployment: None,
                },
                orbital: name,
            }
        })
        .collect()
}

/// A reference-form orbital carries only itself before resolution, so its own
/// ledger row (and any row parented to it) moves with it; the importer keeps
/// the rest.
fn take_ledger_rows(
    ledger: &mut Option<orbital_core_base::identity::IdentityLedger>,
    orbital_id: Option<&str>,
) -> Option<orbital_core_base::identity::IdentityLedger> {
    let (ledger, orbital_id) = (ledger.as_mut()?, orbital_id?);
    let ids: Vec<String> = ledger
        .entries
        .values()
        .filter(|e| e.id == orbital_id || e.parent.as_deref() == Some(orbital_id))
        .map(|e| e.id.clone())
        .collect();
    Some(orbital_core_base::identity::IdentityLedger {
        schema_version: ledger.schema_version,
        entries: ids
            .into_iter()
            .filter_map(|id| ledger.entries.remove_entry(&id))
            .collect(),
    })
}

/// A lazy import that keeps its upstream orbital's name is reproduced unprefixed (`import_prefix`), so its trait names
/// are qualified here once it is resolved: every lazy behavior's names stay unique in the program it joins.
pub fn qualify_kept_name_import(artifact: &LazyArtifact, resolved: &mut OrbitalSchema) {
    let kept_name = artifact.schema.orbitals.first().is_some_and(|o| {
        o.reference.as_ref().and_then(|r| r.reference.rsplit('.').next()) == Some(o.name.as_str())
    });
    if kept_name {
        crate::phases::inline::qualify_lazy_orbital_traits(resolved, &artifact.orbital);
    }
}

/// Records the pages of a resolved lazy behavior on its importer.
pub fn record_lazy_pages(
    importer: &mut OrbitalSchema,
    artifact: &LazyArtifact,
    resolved: &OrbitalSchema,
) -> Result<(), InlineError> {
    let links = crate::phases::validation::page::reachability_targets(resolved);
    let traits: Vec<LazyTrait> = resolved
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter_map(|t| match t {
            TraitReference::Inline(def) => Some(LazyTrait {
                name: def.name.clone(),
                dispatch_mode: def.dispatch_mode(None, &orbital_core_base::runs_on::Residence::none()),
                listens: def.listens.iter().flat_map(|l| l.iter()).cloned().collect(),
                background_ticks: def.ticks_in_background_at_rest(),
            }),
            _ => None,
        })
        .collect();
    let pages: Vec<LazyPage> = resolved
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .filter_map(|p| match p {
            PageRef::Inline(def) => Some(LazyPage {
                path: def.path.clone(),
                orbital: artifact.orbital.clone(),
                orb_ref: artifact.orb_ref.clone(),
                links: links.clone(),
                protocol: def.protocol.clone(),
                after: artifact.after.clone(),
                places: def
                    .traits
                    .iter()
                    .flatten()
                    .filter_map(|t| t.trait_name().map(str::to_string))
                    .collect(),
                traits: traits.clone(),
            }),
            _ => None,
        })
        .collect();
    if pages.is_empty() {
        return Err(InlineError::LazyNoPages {
            orbital: artifact.orbital.clone(),
        });
    }
    importer
        .lazy_pages
        .get_or_insert_with(Vec::new)
        .extend(pages);
    Ok(())
}

/// Splits every lazy reference out of `schema`, resolves each split schema
/// through the same pipeline (resolving its own lazy imports first), and
/// records its pages on `schema`. Run before any pipeline that inlines
/// `schema`; inlining refuses a schema that still holds a lazy reference.
pub fn split_lazy_and_resolve(
    schema: &mut OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<Vec<ResolvedLazy>> {
    split_lazy_and_resolve_in(schema, options, &program_inline(options), source_path)
}

/// One inline session for a whole program: every lazy behavior (and the
/// importer) reuses the files loaded and the behaviors inlined before it.
fn program_inline(options: &CompilerOptions) -> InlineConfig {
    let mut inline = options.inline.clone();
    inline.session.get_or_insert_with(Default::default);
    inline
}

fn split_lazy_and_resolve_in(
    schema: &mut OrbitalSchema,
    options: &CompilerOptions,
    inline: &InlineConfig,
    source_path: Option<&Path>,
) -> CompileResult<Vec<ResolvedLazy>> {
    if super::already_resolved(schema, source_path)? {
        return Ok(Vec::new());
    }
    let artifacts = split_lazy(schema);
    let cache = options.lazy_cache.0.as_ref();
    let keys: Vec<Option<String>> = artifacts.iter().map(|a| cache.and_then(|_| lazy_cache_key(a, options, source_path))).collect();
    let mut done: Vec<Option<ResolvedLazy>> = Vec::with_capacity(artifacts.len());
    let mut misses = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let held = match (cache, key) {
            (Some(cache), Some(key)) => cache.get(key).filter(|entry| entry.reads.is_current()),
            _ => None,
        };
        match held {
            Some(entry) => {
                entry.reads.replay();
                let mut resolved = entry.resolved;
                resolved.cache_key = key.clone();
                done.push(Some(resolved));
            }
            None => {
                done.push(None);
                misses.push(i);
            }
        }
    }
    for (i, built, reads) in resolve_lazy_misses(&artifacts, &misses, options, inline, source_path) {
        reads.replay();
        let mut built = built?;
        if let (Some(cache), Some(key)) = (cache, &keys[i]) {
            cache.put(&lazy_cache_slot(&artifacts[i], source_path), key, CachedLazy { resolved: built.clone(), reads });
            built.cache_key = Some(key.clone());
        }
        done[i] = Some(built);
    }
    let mut resolved = Vec::with_capacity(artifacts.len());
    for (artifact, child) in artifacts.iter().zip(done) {
        let child = child.expect("every lazy behavior was read from the cache or resolved");
        record_lazy_pages(schema, artifact, &child.schema)?;
        resolved.push(child);
    }
    Ok(resolved)
}

/// One lazy behavior resolved with `inline`, recording every file it read.
fn resolve_lazy_artifact(
    artifact: &LazyArtifact,
    options: &CompilerOptions,
    inline: &InlineConfig,
    source_path: Option<&Path>,
) -> (CompileResult<ResolvedLazy>, orbital_core_base::read_set::ReadSet) {
    orbital_core_base::read_set::begin();
    let built = (|| {
        let mut child = artifact.schema.clone();
        let mut nested = split_lazy_and_resolve_in(&mut child, options, inline, source_path)?;
        let mut child = resolve_schema_with_inline(child, options, inline, source_path)?;
        qualify_kept_name_import(artifact, &mut child);
        super::reflect::fold_program(&mut child, &mut nested, source_path)?;
        Ok(ResolvedLazy { orbital: artifact.orbital.clone(), orb_ref: artifact.orb_ref.clone(), schema: child, lazy: nested, cache_key: None })
    })();
    (built, orbital_core_base::read_set::end())
}

/// Lazy behaviors resolved at most at once: each worker loads its own files.
const LAZY_WORKERS: usize = 4;
/// Inlining recurses deeply; a worker gets the stack the CLI's own workers get.
const LAZY_WORKER_STACK: usize = 8 * 1024 * 1024;

/// The lazy behaviors at `misses` resolved, each with its index and the files it read, in index order. Several run at once,
/// in contiguous chunks that each share one inline session of their own, unless `lazy_workers` is 1 or the options
/// cannot leave this thread; then each shares `inline`, the program's own session.
fn resolve_lazy_misses(
    artifacts: &[LazyArtifact],
    misses: &[usize],
    options: &CompilerOptions,
    inline: &InlineConfig,
    source_path: Option<&Path>,
) -> Vec<(usize, CompileResult<ResolvedLazy>, orbital_core_base::read_set::ReadSet)> {
    let limit = match options.lazy_workers {
        0 => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(LAZY_WORKERS),
        n => n,
    };
    let workers = limit.min(misses.len());
    let worker_options = WorkerOptions::of(options);
    let Some(worker_options) = worker_options.filter(|_| workers > 1) else {
        return misses
            .iter()
            .map(|&i| {
                let (built, reads) = resolve_lazy_artifact(&artifacts[i], options, inline, source_path);
                (i, built, reads)
            })
            .collect();
    };
    let chunk = misses.len().div_ceil(workers);
    let worker_options = &worker_options;
    let mut results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = misses
            .chunks(chunk)
            .map(|indices| {
                std::thread::Builder::new()
                    .stack_size(LAZY_WORKER_STACK)
                    .spawn_scoped(scope, move || {
                        let options = worker_options.options();
                        let inline = program_inline(&options);
                        indices
                            .iter()
                            .map(|&i| {
                                let (built, reads) = resolve_lazy_artifact(&artifacts[i], &options, &inline, source_path);
                                (i, built, reads)
                            })
                            .collect::<Vec<_>>()
                    })
                    .expect("a lazy-resolve worker starts")
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    });
    results.sort_by_key(|(i, _, _)| *i);
    results
}

/// The settings a worker thread resolves under: `options` without its inline session, which stays on its thread.
struct WorkerOptions {
    skip_validation: bool,
    comprehensive_validation: bool,
    enrichment: super::enrich::EnrichmentConfig,
    loader_config: crate::loader::LoaderConfig,
    skip_inline: bool,
    compose: super::compose::ComposeConfig,
    nominalize: bool,
    service_runtimes: Option<std::collections::HashMap<String, Vec<String>>>,
    lazy_cache: LazyCache,
}

impl WorkerOptions {
    /// `None` when package discovery failed: that error is the caller's to report, so its resolve stays on its thread.
    fn of(options: &CompilerOptions) -> Option<Self> {
        if options.discovery_error.is_some() {
            return None;
        }
        Some(Self {
            skip_validation: options.skip_validation,
            comprehensive_validation: options.comprehensive_validation,
            enrichment: options.enrichment.clone(),
            loader_config: options.inline.loader_config.clone(),
            skip_inline: options.inline.skip_inline,
            compose: options.compose.clone(),
            nominalize: options.nominalize,
            service_runtimes: options.service_runtimes.clone(),
            lazy_cache: options.lazy_cache.clone(),
        })
    }

    fn options(&self) -> CompilerOptions {
        CompilerOptions {
            skip_validation: self.skip_validation,
            comprehensive_validation: self.comprehensive_validation,
            enrichment: self.enrichment.clone(),
            inline: InlineConfig { loader_config: self.loader_config.clone(), skip_inline: self.skip_inline, session: None },
            compose: self.compose.clone(),
            nominalize: self.nominalize,
            discovery_error: None,
            service_runtimes: self.service_runtimes.clone(),
            lazy_cache: self.lazy_cache.clone(),
            lazy_workers: 1,
        }
    }
}

/// [`resolve_schema_with_path`] for a schema that may `uses lazy`: the
/// importer resolved, plus every lazily-used behavior resolved on its own.
pub fn resolve_program_with_path(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<(OrbitalSchema, Vec<ResolvedLazy>)> {
    if let Some(path) = source_path {
        crate::loader::anchor_relative_uses(&mut schema, path);
    }
    if super::already_resolved(&schema, source_path)? {
        return Ok((schema, Vec::new()));
    }
    let inline = program_inline(options);
    let mut lazy = split_lazy_and_resolve_in(&mut schema, options, &inline, source_path)?;
    let mut schema = resolve_schema_with_inline(schema, options, &inline, source_path)?;
    super::reflect::fold_program(&mut schema, &mut lazy, source_path)?;
    Ok((schema, lazy))
}

/// The compiled path loads a lazily-used behavior through its route: every
/// page is already its own dynamically imported chunk, so the program is
/// compiled whole and each lazy behavior's code is fetched when its page opens.
pub fn compile_lazy_as_routes(schema: &mut OrbitalSchema) {
    for orbital in &mut schema.orbitals {
        for u in orbital.uses.iter_mut().flatten() {
            u.lazy = None;
        }
    }
}

/// Writes each lazily-used behavior at its `orb_ref`, relative to `dir` (the
/// folder of the `.orb` that imports it); nested lazy imports land relative
/// to their own importer.
pub fn write_lazy(
    dir: &Path,
    lazy: &[ResolvedLazy],
    serialize: &dyn Fn(&OrbitalSchema) -> std::io::Result<String>,
) -> std::io::Result<()> {
    for item in lazy {
        let out = dir.join(&item.orb_ref);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serialize(&item.schema)?)?;
        write_lazy(
            out.parent().unwrap_or(dir),
            &item.lazy,
            serialize,
        )?;
    }
    Ok(())
}

/// The resolved program as a key-sorted JSON tree with the awaiting-server skeleton predictions attached (resolve output
/// only, never persisted): `HashMap` iteration varies run to run, so the keys are sorted before the tree is printed.
pub fn resolved_tree(resolved: &OrbitalSchema) -> Result<serde_json::Value, String> {
    let mut tree = serde_json::to_value(resolved).map_err(|e| format!("Failed to serialize resolved schema: {e}"))?;
    super::await_render::attach_await_renders(resolved, &mut tree);
    Ok(orbital_core_eval::util::into_canonical_json(tree))
}

/// [`resolved_tree`] pretty-printed: the text `orb resolve` prints and writes.
pub fn resolved_json(resolved: &OrbitalSchema) -> Result<String, String> {
    serde_json::to_string(&resolved_tree(resolved)?).map_err(|e| format!("Failed to serialize resolved schema: {e}"))
}

/// The resolved program as a key-sorted core value, built straight from the schema with no JSON tree between.
pub fn resolved_value(resolved: &OrbitalSchema) -> Result<orbital_core_base::value::Value, String> {
    let mut value = orbital_core_base::value_serialize::to_canonical_value(resolved).map_err(|e| format!("Failed to serialize resolved schema: {e}"))?;
    super::await_render::attach_await_renders(resolved, &mut value);
    Ok(value)
}

/// [`write_lazy`] with each child printed by [`resolved_json`].
pub fn write_resolved_lazy(dir: &Path, lazy: &[ResolvedLazy]) -> std::io::Result<()> {
    write_lazy(dir, lazy, &|schema| resolved_json(schema).map_err(std::io::Error::other))
}
