//! Code Generation Module
//!
//! Handles the full compilation pipeline: Schema → OIR → Shell → Generated Code
//!
//! This module provides the high-level API for generating code from Orbital schemas.

use crate::phases::{compile_file, parse_file, resolve_schema_with_path, CompilerOptions};
use orbital_core::ir::OirModule;
use orbital_core::shell::{Shell, ShellError};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

// Re-export core generation types so existing consumers of
// `orbital_compiler::generate` continue to compile without changes.
pub use orbital_core::generate::{Backend, GeneratedFile, GeneratedOutput, ShellMode};

/// Generation error
#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("Compilation error: {0}")]
    Compile(#[from] crate::phases::CompilerError),

    #[error("Shell error: {0}")]
    Shell(#[from] ShellError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Backend error: {0}")]
    Backend(String),
}

/// Result type for generation operations
pub type GenerateResult<T> = Result<T, GenerateError>;

/// Generation options
#[derive(Debug, Clone, Default)]
pub struct GenerateOptions {
    /// Compiler options
    pub compiler: CompilerOptions,
    /// Whether to copy the shell template
    pub copy_template: bool,
    /// Whether to clean existing generated files first
    pub clean: bool,
    /// Verbose output
    pub verbose: bool,
    /// Shell mode (full, client, server)
    pub mode: ShellMode,
}

/// Generate code from a schema file
///
/// This is the main entry point for code generation. It:
/// 1. Compiles the schema to OIR
/// 2. Optionally copies the shell template
/// 3. Generates code using the backend
/// 4. Writes files to the output directory
/// 5. Copies the source schema to schema.orb in the output directory
pub fn generate_from_file<S, B>(
    schema_path: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
    shell: &S,
    backend: &B,
    options: &GenerateOptions,
) -> GenerateResult<GeneratedOutput>
where
    S: Shell,
    B: Backend,
{
    let schema_path = schema_path.as_ref();
    let output_dir = output_dir.as_ref();

    if options.verbose {
        eprintln!("Compiling: {}", schema_path.display());
    }

    // Step 1: Compile schema to OIR
    let oir = compile_file(schema_path, &options.compiler)?;

    // Generate code from OIR
    let result = generate_from_oir(&oir, output_dir, shell, backend, options)?;

    // Step 6: Copy source schema to output directory for reference
    let dest_schema = output_dir.join("schema.orb");
    std::fs::copy(schema_path, &dest_schema)?;
    if options.verbose {
        eprintln!("Copied schema to: {}", dest_schema.display());
    }

    // Step 7: Emit `resolved.orb` alongside the copied source schema.
    //
    // The resolved artifact is the bundled view of the schema: post Inline +
    // Compose + Enrich, with every `uses[]` import resolved, every
    // `TraitReference::Reference` collapsed to `TraitReference::Inline`,
    // every Phase 0 composer (`behavior/compose`, `wire`, `pipe`,
    // `detect-layout`) expanded, and every `events: {}` rename applied
    // through transitions AND render-ui props.
    //
    // Structurally identical to the input `.orb` format so downstream TS
    // tools (`orbital-verify-unified/src/analyze.ts`, future `--parity`
    // mode, verifiers) can `JSON.parse` the file and consume it as an
    // `OrbitalSchema` without new parsing code.
    //
    // Skipped silently on parse / resolve errors — emission is best-effort,
    // not a blocker for codegen. `orbital resolve` exposes the same shape
    // as a dedicated CLI subcommand for tooling that wants resolved
    // visibility without generating TSX.
    if let Ok(parsed) = parse_file(schema_path) {
        if let Ok(resolved) = resolve_schema_with_path(parsed, &options.compiler, Some(schema_path))
        {
            let resolved_path = output_dir.join("resolved.orb");
            if let Ok(json) = serde_json::to_string_pretty(&resolved) {
                if let Err(err) = std::fs::write(&resolved_path, json) {
                    eprintln!(
                        "[generate_from_file] Failed to write resolved.orb at {}: {}",
                        resolved_path.display(),
                        err
                    );
                } else if options.verbose {
                    eprintln!("Wrote resolved.orb to: {}", resolved_path.display());
                }
            }
        }
    }

    Ok(result)
}

/// Generate code from an OIR module
pub fn generate_from_oir<S, B>(
    oir: &OirModule,
    output_dir: impl AsRef<Path>,
    shell: &S,
    backend: &B,
    options: &GenerateOptions,
) -> GenerateResult<GeneratedOutput>
where
    S: Shell,
    B: Backend,
{
    let output_dir = output_dir.as_ref();

    if options.verbose {
        eprintln!("Output: {}", output_dir.display());
        eprintln!("Shell: {}", shell.name());
        eprintln!("Backend: {}", backend.name());
    }

    // Step 2: Copy shell template if requested
    if options.copy_template {
        if options.verbose {
            eprintln!("Copying shell template...");
        }
        shell.init_project(output_dir, &oir.name)?;
    }

    // Step 3: Clean existing generated files if requested
    if options.clean {
        if options.verbose {
            eprintln!("Cleaning generated directory...");
        }
        shell.clean_generated(output_dir)?;
    }

    // Step 4: Generate code
    if options.verbose {
        eprintln!("Generating code (mode: {:?})...", options.mode);
    }
    let output = backend.generate_with_mode(oir, options.mode);

    // Step 5: Write files
    let generated_dir = output_dir.join(shell.generated_dir());
    std::fs::create_dir_all(&generated_dir)?;

    for file in &output.files {
        let file_path = generated_dir.join(&file.path);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file_path, &file.content)?;

        if options.verbose {
            eprintln!("  Written: {}", file.path);
        }
    }

    if options.verbose {
        eprintln!(
            "Generated {} files to {}",
            output.files.len(),
            generated_dir.display()
        );
    }

    Ok(output)
}

/// Registry of backends
pub struct BackendRegistry {
    backends: HashMap<String, Box<dyn Backend>>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    pub fn register(&mut self, backend: Box<dyn Backend>) {
        self.backends.insert(backend.name().to_string(), backend);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Backend> {
        self.backends.get(name).map(|b| b.as_ref())
    }

    pub fn list(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generated_file() {
        let file = GeneratedFile::new("entities/task.ts", "export interface Task {}");
        assert_eq!(file.path, "entities/task.ts");
        assert_eq!(file.content, "export interface Task {}");
    }
}
