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

//! Directories under `src/` are organizational only: every module keeps its flat public path via `#[path]`.

// phases — the L2 pipeline (inline · compose · enrich · validate · resolve) and the loader that resolves `uses`
pub mod composition;
pub mod loader;
pub mod phases;

// inference — expression type inference
#[path = "inference/ir_helpers.rs"]
pub mod ir_helpers;
#[path = "inference/type_infer.rs"]
pub mod type_infer;

// codegen — IR handed to a shell backend
#[path = "codegen/generate.rs"]
pub mod generate;
#[path = "codegen/mixed.rs"]
pub mod mixed;
#[path = "codegen/shell.rs"]
pub mod shell;

// testing — the exhaustive-test generator
#[path = "testing/exhaustive_tests.rs"]
pub mod exhaustive_tests;
#[path = "testing/test_generator.rs"]
pub mod test_generator;

pub use orbital_core_base::language::pattern_registry;
pub use orbital_core_base::language::registry as embedded;

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
    resolve_cross_file_id_edges, resolve_schema_with_path,
    validate_comprehensive_full, validate_comprehensive_without_services, CompileResult, CompilerError, CompilerOptions,
    ComposeConfig, ComprehensiveValidationResult, CrossFileIdError, ExtraPatterns, SchemaLanguage,
};
