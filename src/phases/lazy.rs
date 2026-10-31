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
#[derive(Debug, Clone)]
pub struct ResolvedLazy {
    pub orbital: String,
    pub orb_ref: String,
    pub schema: OrbitalSchema,
    pub lazy: Vec<ResolvedLazy>,
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
/// app-level theme, config, egress and ledger, and only the uses it can resolve: the
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
                    themes: None,
                    site: schema.site.clone(),
                    public_dir: None,
                    name: name.clone(),
                    version: schema.version.clone(),
                    description: None,
                    theme: schema.theme.clone(),
                    config: schema.config.clone(),
                    orbitals: vec![orbital],
                    locales: schema.locales.clone(),
                    lazy_pages: None,
                    metadata: None,
                    schema_version: schema.schema_version,
                    ledger,
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
                background_ticks: def.ticks.iter().flatten().any(|tick| tick.background),
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
    let mut resolved = Vec::new();
    for artifact in split_lazy(schema) {
        let mut child = artifact.schema.clone();
        let nested = split_lazy_and_resolve_in(&mut child, options, inline, source_path)?;
        let child = resolve_schema_with_inline(child, options, inline, source_path)?;
        record_lazy_pages(schema, &artifact, &child)?;
        resolved.push(ResolvedLazy {
            orbital: artifact.orbital,
            orb_ref: artifact.orb_ref,
            schema: child,
            lazy: nested,
        });
    }
    Ok(resolved)
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
    program_dir: &Path,
    serialize: &dyn Fn(&OrbitalSchema) -> std::io::Result<String>,
) -> std::io::Result<()> {
    for item in lazy {
        let out = dir.join(&item.orb_ref);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serialize(&item.schema)?)?;
        super::messages::write_catalogs(&out, &item.schema, program_dir)?;
        write_lazy(
            out.parent().unwrap_or(dir),
            &item.lazy,
            program_dir,
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
    serde_json::to_string_pretty(&resolved_tree(resolved)?).map_err(|e| format!("Failed to serialize resolved schema: {e}"))
}

/// The resolved program as a key-sorted core value, built straight from the schema with no JSON tree between.
pub fn resolved_value(resolved: &OrbitalSchema) -> Result<orbital_core_base::value::Value, String> {
    let mut value = orbital_core_base::value_serialize::to_canonical_value(resolved).map_err(|e| format!("Failed to serialize resolved schema: {e}"))?;
    super::await_render::attach_await_renders(resolved, &mut value);
    Ok(value)
}

/// [`write_lazy`] with each child printed by [`resolved_json`].
pub fn write_resolved_lazy(dir: &Path, lazy: &[ResolvedLazy], program_dir: &Path) -> std::io::Result<()> {
    write_lazy(dir, lazy, program_dir, &|schema| resolved_json(schema).map_err(std::io::Error::other))
}
