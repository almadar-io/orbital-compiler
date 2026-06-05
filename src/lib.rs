//! Orbital Compiler
//!
//! Compiles Orbital schemas (.orb files) to generated code via the
//! Orbital Intermediate Representation (OIR).

#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    clippy::too_many_arguments,
    clippy::redundant_closure,
    clippy::derivable_impls,
    clippy::should_implement_trait,
    clippy::unwrap_or_default,
    clippy::unnecessary_map_or,
    clippy::collapsible_if,
    clippy::needless_borrow,
    clippy::manual_strip,
    clippy::manual_pattern_char_comparison,
    clippy::redundant_pattern_matching,
    clippy::explicit_auto_deref,
    clippy::map_entry,
    clippy::map_clone,
    clippy::to_string_in_format_args,
    clippy::manual_is_multiple_of,
    clippy::iter_on_single_items,
    clippy::needless_borrows_for_generic_args,
    clippy::collapsible_match,
    clippy::for_kv_map,
    unexpected_cfgs
)]
//!
//! # Compilation Pipeline
//!
//! ```text
//! .orb file → Parse → Validate → Enrich → Resolve → OIR → Backend → Generated Code
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use orbital_compiler::{compile_file, CompilerOptions};
//! use orbital_compiler::generate::{generate_from_file, GenerateOptions};
//!
//! // Just get OIR
//! let options = CompilerOptions::default();
//! let oir = compile_file("app.orb", &options)?;
//!
//! // Or generate full code
//! let gen_options = GenerateOptions::default();
//! let output = generate_from_file("app.orb", "./output", &shell, &backend, &gen_options)?;
//! ```

pub mod composition;
pub mod embedded;
pub mod exhaustive_tests;
pub mod generate;
pub mod ir_helpers;
pub mod loader;
pub mod phases;
pub mod shell;
pub mod test_generator;

pub use composition::ComposeError;
pub use exhaustive_tests::{
    ExhaustiveTestGenerator, JourneyStep, TestCase, TestCategory, TraitTestResult,
};
pub use generate::{
    generate_from_file, generate_from_oir, Backend, BackendRegistry, GenerateError,
    GenerateOptions, GenerateResult, GeneratedFile, GeneratedOutput, ShellMode,
};
pub use phases::{
    compile_file, compile_json, compile_schema, compose_schema, parse_file,
    resolve_schema_with_path, validate_comprehensive, validate_comprehensive_full,
    validate_comprehensive_with_options, validate_comprehensive_with_patterns, CompileResult,
    CompilerError, CompilerOptions, ComposeConfig, ComprehensiveValidationResult, ExtraPatterns,
    SchemaLanguage,
};
