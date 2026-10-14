//! Compiler Phases
//!
//! The Orbital compiler is organized into phases that transform the input schema
//! into the final Orbital IR (OIR) suitable for code generation.
//!
//! # Phase Pipeline
//!
//! ```text
//! .orb file → Parse → Validate → Enrich → Inline → Resolve → OIR
//! ```
//!
//! Each phase is responsible for a specific transformation:
//!
//! 1. **Parse**: Read JSON/YAML and deserialize into schema types
//! 2. **Validate**: Check structural validity and type correctness
//! 3. **Enrich**: Add derived information (defaults, cross-references)
//! 4. **Inline**: Resolve `uses` declarations and inline external references
//! 5. **Resolve**: Resolve all references and produce final OIR

pub mod await_render;
pub mod compose;
pub mod cross_file_ids;
pub(crate) mod embed_graph;
pub mod enrich;
pub mod inline;
pub mod lazy;
pub mod messages;
pub mod nominalize;
pub mod page_content_owner;
pub mod parse;
pub mod public;
pub mod reflect;
pub mod resolve;
pub mod sexpr_json;
pub mod validate;
pub mod validation;

pub use compose::{compose_schema, ComposeConfig};
pub use cross_file_ids::{resolve_cross_file_id_edges, CrossFileIdError};
pub use enrich::{enrich_schema, EnrichmentConfig};
pub use inline::{inline_schema, InlineConfig, InlineError};
pub use parse::{parse_file, parse_json, ParseError};
pub use resolve::{resolve_to_oir, ResolveError};
pub use validate::{validate_schema, ValidationError as SchemaValidationError};
pub use validation::sync::ExtraPatterns;
pub use validation::{
    validate_comprehensive, validate_comprehensive_full, validate_comprehensive_with_options,
    validate_comprehensive_with_patterns, SchemaLanguage,
    ValidationResult as ComprehensiveValidationResult,
};

use crate::composition::ComposeError;
use orbital_core::ir::OirModule;
use orbital_core::schema::OrbitalSchema;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Compiler error
#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("Validation error: {0}")]
    Validation(#[from] SchemaValidationError),

    #[error("Inline error: {0}")]
    Inline(#[from] InlineError),

    #[error("Compose error: {0}")]
    Compose(#[from] ComposeError),

    #[error("Resolution error: {0}")]
    Resolution(#[from] ResolveError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Reflection error: {0}")]
    Reflection(String),
}

/// Compilation result
pub type CompileResult<T> = Result<T, CompilerError>;

/// Compiler options
// `Clone` dropped: `discovery_error` carries `orbital_core::behavior_packages::
// DiscoverError`, which is not `Clone` (its `Io` variant wraps `std::io::Error`)
// — verified zero call sites clone a `CompilerOptions` today.
#[derive(Debug, Default)]
pub struct CompilerOptions {
    /// Skip validation phase
    pub skip_validation: bool,
    /// Run comprehensive validation (includes all semantic checks)
    pub comprehensive_validation: bool,
    /// Enrichment configuration
    pub enrichment: EnrichmentConfig,
    /// Inline configuration
    pub inline: InlineConfig,
    /// Compose configuration
    pub compose: ComposeConfig,
    /// Run the Phase-3D nominalization pass post-Resolve: replace every
    /// anonymous `OirType::Struct`/`Union` with a `TypeRef` registered in
    /// `OirModule::named_types`. Default OFF — every shell's `type_ref`
    /// readiness landed in the same change set, but the owner flips the
    /// default only after reviewing the TS before/after diff.
    pub nominalize: bool,
    /// Verbose output
    pub verbose: bool,
    /// Set when behavior-package discovery (`orbital_core::behavior_packages::
    /// discover`, run from the process cwd) failed — a `PrefixConflict` or a
    /// malformed installed package. `packages` is then empty, which callers
    /// must not silently treat as "no packages installed": `orb validate` /
    /// `compile` report this before parsing, as validator infrastructure, not
    /// a schema authoring error.
    pub discovery_error: Option<orbital_core::behavior_packages::DiscoverError>,
    /// Service name → declared runtimes, from the services registry the caller
    /// loaded; `None` reads the baked registry. Decides which calls run in the
    /// browser (`OirModule::browser_services`).
    pub service_runtimes: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl CompilerOptions {
    /// Discover installed behavior packages node-style from the process
    /// working directory (`orbital_core::behavior_packages::discover`) and
    /// wire them into the loader config both phases share. No environment
    /// variables — the prior dev-mode toggles are retired
    /// (Almadar_Behavior_Packages.md Phase 1) — a workspace-linked
    /// `node_modules` (or `~/.orb` with none above cwd) already puts the
    /// live package sources on the walk.
    pub fn from_env() -> Self {
        // `ORBITAL_NOMINALIZE=1` opts into the Phase-3D nominalization pass —
        // it changes codegen output for every shell, so it stays a dedicated
        // toggle rather than folding into package discovery.
        let nominalize = std::env::var("ORBITAL_NOMINALIZE")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let mut opts = Self {
            nominalize,
            ..Self::default()
        };

        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return opts,
        };
        let user_store =
            orbital_core::behavior_packages::user_store_dir().unwrap_or_else(|| cwd.clone());
        match orbital_core::behavior_packages::discover(&cwd, &user_store) {
            Ok(packages) => opts.inline.loader_config.packages = packages,
            Err(e) => opts.discovery_error = Some(e),
        }
        if let Some(root) = app_root(std::env::var("ORB_APP_ROOT").ok().as_deref(), &cwd) {
            opts.inline
                .loader_config
                .scoped_paths
                .insert(APP_SCOPE.to_string(), root);
        }
        opts
    }
}

/// The import scope of the app being compiled: `uses X from "@app/blocks/<Name>"`
/// resolves against the app's workspace root from any file in it.
pub const APP_SCOPE: &str = "@app";

/// The app's workspace root: `ORB_APP_ROOT` when a caller names it (a server
/// validating a temp copy), else the nearest ancestor of `cwd` holding the
/// workspace marker `.almadar/app-marker.json`.
pub fn app_root(explicit: Option<&str>, cwd: &Path) -> Option<PathBuf> {
    if let Some(root) = explicit.filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(root));
    }
    cwd.ancestors()
        .find(|dir| dir.join(".almadar").join("app-marker.json").is_file())
        .map(Path::to_path_buf)
}

/// Compile a schema file to OIR
pub fn compile_file(path: impl AsRef<Path>, options: &CompilerOptions) -> CompileResult<OirModule> {
    let path = path.as_ref();

    if options.verbose {
        eprintln!("Compiling: {}", path.display());
    }

    // Phase 1: Parse
    if options.verbose {
        eprintln!("  Phase 1: Parsing...");
    }
    let schema = parse_file(path)?;

    // Continue with common compilation, passing source path for inline phase
    compile_schema_with_path(schema, options, Some(path))
}

/// [`compile_file`] for a schema written somewhere other than its source (a
/// lowered or translated copy): imports resolve against `source_path`.
pub fn compile_file_with_source(
    path: impl AsRef<Path>,
    source_path: &Path,
    options: &CompilerOptions,
) -> CompileResult<OirModule> {
    let schema = parse_file(path.as_ref())?;
    compile_schema_with_path(schema, options, Some(source_path))
}

/// Resolve a parsed schema: run Inline → Compose → Enrich and return the
/// fully-inlined `OrbitalSchema`. This is the pre-OIR "resolved" form — the
/// same JSON shape authors write, but with every import expanded, every
/// trait ref flattened to `TraitReference::Inline`, every `behavior/*`
/// composer collapsed, every `events: { OLD: NEW }` rename applied through
/// transitions AND render-ui props.
///
/// Consumers (the `orbital resolve` CLI, `generate_from_file`'s resolved.orb
/// emission, future `--parity` mode) use this shape as the canonical
/// "bundled" view of the schema. Structurally identical to the input so
/// every downstream TS tool that already understands `OrbitalSchema` can
/// consume it without new parsing code.
///
/// Skips Validate and Resolve-to-OIR on purpose — those happen inside
/// `compile_schema_with_path` which calls this helper first, then finishes.
pub fn resolve_schema_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    resolve_schema_with_inline(schema, options, &options.inline, source_path)
}

/// [`resolve_schema_with_path`] inlining with `inline` instead of
/// `options.inline` — how a program shares one inline session across itself
/// and its lazy behaviors.
pub(crate) fn resolve_schema_with_inline(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    inline: &inline::InlineConfig,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    // Intrinsic identity: a raw/name-only `.orb` reaching the compiler (e.g.
    // `orb emit ts <name-only.orb>`, which parses through the raw core
    // deserializer) is stamped here — the single pre-inline chokepoint — so
    // inline/compose/resolve/codegen never see a ledger-less schema and the
    // name-fallback rewriters are dead. `.lolo`-emitted and factory-stamped
    // schemas already carry a ledger and pass through untouched (zero churn).
    orbital_core::stamp::ensure_stamped(&mut schema);

    // Phase 2: Inline (resolve uses declarations — must happen before validation
    //           so that trait/entity/page references are present when checked).
    //           `inline_schema` runs the V4-W4 identity normalization internally
    //           (right after its own `ensure_stamped`), so `orb validate` — which
    //           calls the inline entry directly — is covered by the same pass.
    if options.verbose {
        eprintln!("  Phase 2: Inlining...");
    }
    inline_schema(&mut schema, inline, source_path)?;

    // Phase 2.5: Compose (depends on inline having resolved aliases)
    if options.verbose {
        eprintln!("  Phase 2.5: Composing behaviors...");
    }
    compose_schema(&mut schema, &options.compose)?;

    // Phase 3: Enrich (add defaults like collection names — runs on resolved schema
    //           so inlined entities are enriched too)
    if options.verbose {
        eprintln!("  Phase 3: Enriching...");
    }
    enrich_schema(&mut schema, &options.enrichment);

    // The theme presets the program references travel with it, so a runtime
    // with no installed `@almadar/core` (wasm, mobile) resolves its theme.
    orbital_core::runtime::theme::carry_theme_definitions(
        &mut schema,
        orbital_core::runtime::theme::ThemePresets::installed(),
    );

    Ok(schema)
}

/// The program exactly as the compiled path compiles it: lazy behaviors compiled in
/// as route chunks, then Inline, Compose, Enrich. What a compiled client's own
/// runtime (the browser store for `[persistent, local]` entities) runs.
pub fn resolve_compiled_program_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    let mut program = resolve_compiled_program_as_written(schema, options, source_path)?;
    reflect::fold_literal_reflection(&mut program, source_path)?;
    Ok(program)
}

/// [`resolve_compiled_program_with_path`] before reflection is answered: what validation judges.
fn resolve_compiled_program_as_written(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    lazy::compile_lazy_as_routes(&mut schema);
    // Phases 2–3: Inline, Compose, Enrich.
    let mut schema = resolve_schema_with_path(schema, options, source_path)?;

    // Canonicalize flat `fn` lambdas post-inline so raw `.orb` input (which skips
    // the lolo lowerer) is normalized before validation + codegen, and inlined
    // reference traits are caught too.
    orbital_core::schema::normalize_fn_shapes(&mut schema);
    Ok(schema)
}

/// Compile a schema file to OIR with source path for resolving imports
fn compile_schema_with_path(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OirModule> {
    let mut schema = resolve_compiled_program_as_written(schema, options, source_path)?;

    // Phase 4: Validate (runs on the fully inlined + enriched schema so reference
    //           checks and collection-presence checks are accurate)
    if !options.skip_validation {
        if options.verbose {
            eprintln!("  Phase 4: Validating...");
        }
        validate_schema(&schema)?;

        // Run comprehensive validation if enabled
        if options.comprehensive_validation {
            if options.verbose {
                eprintln!("  Phase 4b: Comprehensive validation...");
            }
            let result = validate_comprehensive(&schema);
            if !result.is_valid() {
                // Log errors but don't fail compilation yet (comprehensive validation is advisory)
                for error in &result.errors {
                    eprintln!("    [{}] {}: {}", error.code, error.path, error.message);
                }
            }
            if result.has_warnings() && options.verbose {
                for warning in &result.warnings {
                    eprintln!("    [WARN] {}: {}", warning.path, warning.message);
                }
            }
        }
    }

    // Phase 4c: answer the reflection the build already knows, on the validated program.
    reflect::fold_literal_reflection(&mut schema, source_path)?;

    // Phase 5: Resolve to OIR
    if options.verbose {
        eprintln!("  Phase 5: Resolving...");
    }
    let mut oir = resolve_to_oir(&schema)?;
    let runtimes = options
        .service_runtimes
        .as_ref()
        .unwrap_or(&crate::embedded::baked_service_registry().runtimes);
    oir.browser_services = oir
        .invoked_services()
        .into_iter()
        .filter(|s| {
            runtimes
                .get(s)
                .is_some_and(|r| orbital_core::effects::runs_on::is_browser_only(r))
        })
        .collect();

    // Phase 5b (opt-in): nominalize anonymous struct/union shapes into
    // `named_types` + `TypeRef`s — post-Resolve, pre-codegen, so every
    // backend sees the same table.
    if options.nominalize {
        if options.verbose {
            eprintln!("  Phase 5b: Nominalizing...");
        }
        let stats = nominalize::nominalize_module(&mut oir);
        if options.verbose {
            eprintln!(
                "    registered={}, reused_existing={}, reused_dedup={}, skipped_collisions={}",
                stats.registered,
                stats.reused_existing,
                stats.reused_dedup,
                stats.skipped_collisions
            );
        }
    }

    if options.verbose {
        eprintln!("  Compilation complete!");
    }

    Ok(oir)
}

/// Compile a JSON string to OIR
pub fn compile_json(json: &str, options: &CompilerOptions) -> CompileResult<OirModule> {
    // Phase 1: Parse
    if options.verbose {
        eprintln!("  Phase 1: Parsing JSON...");
    }
    let schema = parse_json(json)?;

    // Continue with common compilation
    compile_schema(schema, options)
}

/// Compile a parsed schema to OIR
pub fn compile_schema(
    schema: OrbitalSchema,
    options: &CompilerOptions,
) -> CompileResult<OirModule> {
    // Delegate to the path-aware version with no source path
    compile_schema_with_path(schema, options, None)
}

#[cfg(test)]
mod app_root_tests {
    use super::*;

    #[test]
    fn an_explicit_root_wins_over_the_marker() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        std::fs::write(dir.path().join(".almadar/app-marker.json"), "{}").expect("marker");
        assert_eq!(
            app_root(Some("/elsewhere"), dir.path()),
            Some(PathBuf::from("/elsewhere"))
        );
    }

    #[test]
    fn the_nearest_ancestor_with_the_workspace_marker_is_the_root() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        std::fs::write(dir.path().join(".almadar/app-marker.json"), "{}").expect("marker");
        let inner = dir.path().join("orbitals/deep");
        std::fs::create_dir_all(&inner).expect("mkdir");
        assert_eq!(app_root(None, &inner), Some(dir.path().to_path_buf()));
        assert_eq!(
            app_root(Some(""), &inner),
            Some(dir.path().to_path_buf()),
            "an empty value is unset"
        );
    }

    #[test]
    fn control_no_marker_and_no_explicit_root_is_no_app_scope() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        assert_eq!(
            app_root(None, dir.path()),
            None,
            "a .almadar without the marker is not a workspace"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple_schema() {
        let json = r#"{
            "name": "TestApp",
            "version": "1.0.0",
            "orbitals": []
        }"#;

        let options = CompilerOptions::default();
        let result = compile_json(json, &options);
        assert!(result.is_ok());

        let oir = result.unwrap();
        assert_eq!(oir.name, "TestApp");
        assert_eq!(oir.version, "1.0.0");
    }

    // Wave-8: the wasm mount resolves its schema through `resolve_schema_with_path(.., None)`
    // and relies on std peers resolving from the installed `@almadar/std` package (no
    // filesystem source path). std-export `uses` a handful of atoms as
    // `TraitReference::Reference`; after resolve with `source_path=None` every one must
    // become `TraitReference::Inline`.
    #[test]
    fn resolves_installed_std_uses_with_no_source_path() {
        use orbital_core::schema::TraitReference;
        use std::path::Path;

        let schema = orbital_test_support::installed_schema(
            "almadar-std",
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "std-export",
        );

        // Pre-condition: the peers are unresolved References before inline.
        let orbital = &schema.orbitals[0];
        let refs_before = orbital
            .traits
            .iter()
            .filter(|t| matches!(t, TraitReference::Reference { .. }))
            .count();
        // The installed std-export's peer count drifts across registry
        // publishes; one unresolved Reference is enough to exercise
        // no-source-path resolution.
        assert!(
            refs_before >= 1,
            "expected >=1 unresolved ref, got {refs_before}"
        );

        // Discover installed behavior packages from this crate's manifest
        // dir (the monorepo workspace link), not the process cwd — mirrors
        // `CompilerOptions::from_env` rooted at a stable path for tests.
        let start = Path::new(env!("CARGO_MANIFEST_DIR"));
        let user_store = orbital_core::behavior_packages::user_store_dir()
            .unwrap_or_else(|| start.to_path_buf());
        let packages = orbital_core::behavior_packages::discover(start, &user_store)
            .expect("behavior package discovery failed");
        let mut options = CompilerOptions::default();
        options.inline.loader_config.packages = packages;

        let resolved = resolve_schema_with_path(schema, &options, None)
            .expect("resolve std-export with no source path");

        for orbital in &resolved.orbitals {
            for t in &orbital.traits {
                assert!(
                    matches!(t, TraitReference::Inline(_)),
                    "every trait must inline after resolve; found unresolved Reference"
                );
            }
        }
    }
}
