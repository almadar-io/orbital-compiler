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

pub mod compose;
pub mod cross_file_ids;
pub mod enrich;
pub mod inline;
pub mod parse;
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
use std::path::Path;
use std::path::PathBuf;
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
}

/// Compilation result
pub type CompileResult<T> = Result<T, CompilerError>;

/// Compiler options
#[derive(Debug, Clone, Default)]
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
    /// Verbose output
    pub verbose: bool,
}

impl CompilerOptions {
    /// Build options with dev-workspace loader settings from environment
    /// variables. When `ALMADAR_DEV` is set (any non-empty value other than
    /// `0`), `std/behaviors/<name>` imports resolve against the local
    /// workspace registry BEFORE the embedded baked registry, so behavior
    /// edits take effect without rebuilding + rebaking the `orb` binary.
    ///
    /// Registry roots are populated from:
    ///   1. `ALMADAR_WORKSPACE` (explicit `<repo>/packages` parent) if set, or
    ///   2. auto-discovery from each schema's source path at inline time
    ///      (the existing `discover_registry_roots` path, unchanged).
    ///
    /// Explicit per-package env overrides take precedence when present:
    ///   - `ALMADAR_STD_REGISTRY`        → packages/almadar-std/behaviors/registry
    ///   - `ALMADAR_BEHAVIORS_REGISTRY`  → packages/almadar-behaviors/behaviors/registry
    ///
    /// Production binaries ignore all of these (env unset → embedded baked
    /// registry is authoritative, as before). Rebaking (`make sync && make
    /// release install`) only needs to run for `cascade --publish`.
    pub fn from_env() -> Self {
        let mut opts = Self::default();
        // Dev mode is ON when:
        //   - ALMADAR_DEV is set to a truthy value, OR
        //   - ALMADAR_ROOT is set AND the workspace registry dirs exist under
        //     it (auto-on whenever you're inside the Almadar workspace, so
        //     `orb` reads live behavior sources without a rebuild + rebake).
        //   - ALMADAR_DEV=0 forces it OFF (to exercise the published-binary
        //     baked-registry path).
        let dev_on = match std::env::var("ALMADAR_DEV") {
            Ok(v) => !v.is_empty() && v != "0",
            Err(_) => {
                // Auto-enable when ALMADAR_ROOT points at a workspace that
                // has the behavior registry directories on disk.
                std::env::var("ALMADAR_ROOT")
                    .ok()
                    .map(|root| {
                        PathBuf::from(&root)
                            .join("packages/almadar-std/behaviors/registry")
                            .is_dir()
                            || PathBuf::from(&root)
                                .join("packages/almadar-behaviors/behaviors/registry")
                                .is_dir()
                    })
                    .unwrap_or(false)
            }
        };
        if !dev_on {
            return opts;
        }
        opts.inline.loader_config.dev_workspace = true;

        // Per-package explicit overrides win.
        if let Ok(p) = std::env::var("ALMADAR_STD_REGISTRY") {
            let root = PathBuf::from(p);
            if root.is_dir() {
                opts.inline
                    .loader_config
                    .behaviors_registry_roots
                    .insert("almadar-std".to_string(), root);
            }
        }
        if let Ok(p) = std::env::var("ALMADAR_BEHAVIORS_REGISTRY") {
            let root = PathBuf::from(p);
            if root.is_dir() {
                opts.inline
                    .loader_config
                    .behaviors_registry_roots
                    .insert("almadar-behaviors".to_string(), root);
            }
        }
        // If no explicit roots yet, seed from ALMADAR_WORKSPACE or the
        // already-set ALMADAR_ROOT (the repo root). Otherwise leave roots
        // empty so `inline_schema`'s source-path auto-discovery fills them
        // per file.
        if opts
            .inline
            .loader_config
            .behaviors_registry_roots
            .is_empty()
        {
            let ws = std::env::var("ALMADAR_WORKSPACE")
                .or_else(|_| std::env::var("ALMADAR_ROOT"))
                .ok();
            if let Some(ws) = ws {
                let ws_path = PathBuf::from(ws);
                for (scope, rel) in [
                    ("almadar-std", "packages/almadar-std/behaviors/registry"),
                    (
                        "almadar-behaviors",
                        "packages/almadar-behaviors/behaviors/registry",
                    ),
                ] {
                    let root = ws_path.join(rel);
                    if root.is_dir() {
                        opts.inline
                            .loader_config
                            .behaviors_registry_roots
                            .insert(scope.to_string(), root);
                    }
                }
            }
        }
        opts
    }
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
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
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
    //           so that trait/entity/page references are present when checked)
    if options.verbose {
        eprintln!("  Phase 2: Inlining...");
    }
    inline_schema(&mut schema, &options.inline, source_path)?;

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

    Ok(schema)
}

/// Compile a schema file to OIR with source path for resolving imports
fn compile_schema_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OirModule> {
    // Phases 2–3: Inline, Compose, Enrich.
    let mut schema = resolve_schema_with_path(schema, options, source_path)?;

    // Canonicalize flat `fn` lambdas post-inline so raw `.orb` input (which skips
    // the lolo lowerer) is normalized before validation + codegen, and inlined
    // reference traits are caught too.
    orbital_core::schema::normalize_fn_shapes(&mut schema);

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

    // Phase 5: Resolve to OIR
    if options.verbose {
        eprintln!("  Phase 5: Resolving...");
    }
    let oir = resolve_to_oir(&schema)?;

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
    // and relies on std peers resolving from the embedded `phf` (no filesystem source path).
    // std-export `uses` Button1/DataList1/Popover1 as `TraitReference::Reference`; after resolve
    // with `source_path=None` every one must become `TraitReference::Inline`.
    #[test]
    fn resolves_embedded_std_uses_with_no_source_path() {
        use orbital_core::schema::TraitReference;

        let json = crate::embedded::get_std_behavior_content("std/behaviors/std-export")
            .expect("std-export must be embedded");
        let schema = parse_json(json).expect("parse std-export");

        // Pre-condition: the three peers are unresolved References before inline.
        let orbital = &schema.orbitals[0];
        let refs_before = orbital
            .traits
            .iter()
            .filter(|t| matches!(t, TraitReference::Reference { .. }))
            .count();
        // The baked std-export's peer count drifts across registry bakes
        // (was 3, now 2); one unresolved Reference is enough to exercise
        // no-source-path resolution.
        assert!(
            refs_before >= 1,
            "expected >=1 unresolved ref, got {refs_before}"
        );

        let options = CompilerOptions::default();
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
