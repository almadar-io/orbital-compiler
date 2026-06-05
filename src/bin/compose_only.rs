//! compose-only binary
//!
//! Tiny CLI bridge used by `orbital-rust/scripts/parity-compose.ts` to
//! exercise the Rust compose pipeline on a composition fixture.
//!
//! Usage:
//! ```text
//! compose_only <path-to-fixture>
//! ```
//!
//! The fixture must be a JSON file shaped like:
//! ```json
//! { "input": { /* ComposeBehaviorsInput */ } }
//! ```
//!
//! The binary parses the `input` block, runs `compose_behaviors` from
//! `orbital_compiler::composition`, and prints the resulting schema as
//! `JSON.stringify(..., null, 2)`-equivalent JSON to stdout.

use orbital_compiler::composition::{compose_behaviors, ComposeBehaviorsInput};
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: compose_only <fixture.orb>");
            return ExitCode::from(2);
        }
    };

    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {}: {}", path, e);
            return ExitCode::from(2);
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to parse {}: {}", path, e);
            return ExitCode::from(2);
        }
    };

    let input_value = match parsed.get("input").cloned() {
        Some(v) => v,
        None => {
            eprintln!("fixture {} is missing top-level `input` key", path);
            return ExitCode::from(2);
        }
    };

    let input: ComposeBehaviorsInput = match serde_json::from_value(input_value) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("invalid ComposeBehaviorsInput in {}: {}", path, e);
            return ExitCode::from(2);
        }
    };

    let result = match compose_behaviors(input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("compose_behaviors failed: {}", e);
            return ExitCode::from(1);
        }
    };

    // Print the schema portion only — matches what `parity-compose.ts`
    // compares against `composeBehaviors(input).schema` from the TS side.
    match serde_json::to_string_pretty(&result.schema) {
        Ok(s) => {
            println!("{}", s);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to serialize result: {}", e);
            ExitCode::from(1)
        }
    }
}
