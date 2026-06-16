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
pub mod enrich;
pub mod inline;
pub mod parse;
pub mod resolve;
pub mod sexpr_json;
pub mod validate;
pub mod validation;

pub use compose::{compose_schema, ComposeConfig};
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
}
