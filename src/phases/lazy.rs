//! `uses lazy`: a behavior imported lazily is compiled to its own `.orb` and
//! loaded when one of its pages is opened. Each `orbital X = Alias.orbitals.Y`
//! reference through a lazy alias is split out of the importer into its own
//! schema, which then goes through the same pipeline as any schema. The
//! importer keeps only `lazyPages`, recorded from the split schema's resolved
//! pages.

use super::inline::InlineError;
use super::{resolve_schema_with_path, CompileResult, CompilerOptions};
use orbital_core::schema::{LazyPage, Orbital, OrbitalSchema, PageRef};
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
/// app-level theme, config and ledger, and only the uses it can resolve: the
/// eager ones plus its own alias, no longer lazy.
pub fn split_lazy(schema: &mut OrbitalSchema) -> Vec<LazyArtifact> {
    let (lazy, eager): (Vec<Orbital>, Vec<Orbital>) = std::mem::take(&mut schema.orbitals)
        .into_iter()
        .partition(is_lazy_reference);
    schema.orbitals = eager;
    lazy.into_iter()
        .map(|mut orbital| {
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
                orb_ref: lazy_orb_ref(&name),
                schema: OrbitalSchema {
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
    ledger: &mut Option<orbital_core::identity::IdentityLedger>,
    orbital_id: Option<&str>,
) -> Option<orbital_core::identity::IdentityLedger> {
    let (ledger, orbital_id) = (ledger.as_mut()?, orbital_id?);
    let ids: Vec<String> = ledger
        .entries
        .values()
        .filter(|e| e.id == orbital_id || e.parent.as_deref() == Some(orbital_id))
        .map(|e| e.id.clone())
        .collect();
    Some(orbital_core::identity::IdentityLedger {
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
    let pages: Vec<LazyPage> = resolved
        .orbitals
        .iter()
        .flat_map(|o| o.pages.iter().flatten())
        .filter_map(|p| match p {
            PageRef::Inline(def) => Some(LazyPage {
                path: def.path.clone(),
                orbital: artifact.orbital.clone(),
                orb_ref: artifact.orb_ref.clone(),
            }),
            _ => None,
        })
        .collect();
    if pages.is_empty() {
        return Err(InlineError::LazyNoPages {
            orbital: artifact.orbital.clone(),
        });
    }
    importer.lazy_pages.get_or_insert_with(Vec::new).extend(pages);
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
    let mut resolved = Vec::new();
    for artifact in split_lazy(schema) {
        let mut child = artifact.schema.clone();
        let nested = split_lazy_and_resolve(&mut child, options, source_path)?;
        let child = resolve_schema_with_path(child, options, source_path)?;
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
    let lazy = split_lazy_and_resolve(&mut schema, options, source_path)?;
    let schema = resolve_schema_with_path(schema, options, source_path)?;
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
        write_lazy(out.parent().unwrap_or(dir), &item.lazy, program_dir, serialize)?;
    }
    Ok(())
}
