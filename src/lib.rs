//! Orbital Compiler
//!
//! Compiles Orbital schemas (.orb files) to generated code via the
//! Orbital Intermediate Representation (OIR).

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
pub mod pattern_registry;
pub mod phases;
pub mod shell;
pub mod test_generator;
pub mod type_infer;

pub use composition::ComposeError;
pub use exhaustive_tests::{
    resolve_transition, ExhaustiveTestGenerator, JourneyStep, TestCase, TestCategory,
    TraitTestResult,
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
pub use shell::{BaseShell, Shell, ShellRegistry};
