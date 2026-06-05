// Build script for orbital-compiler.
//
// Tells Cargo to rebuild the crate whenever the embedded source-of-truth
// JSON registries change. The actual embedding happens in `src/embedded.rs`
// via `include_str!`. See `docs/Lolo_Language_Spec_v3_PLAN.md` Section 8 (Phase 3).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packages/almadar-std/canonical-operators.json");
    println!(
        "cargo:rerun-if-changed=../../packages/almadar-patterns/src/patterns-registry.json"
    );
    println!(
        "cargo:rerun-if-changed=../../packages/almadar-std/behaviors/behaviors-registry.json"
    );
    // Sub-unit 3.5.C: the full std behavior registry (atoms + molecules +
    // organisms) is embedded via `include_str!` in `src/embedded.rs`. Rerun
    // whenever any `.orb` file under `behaviors/registry/` changes. The single
    // directive covers all three subdirectories.
    println!("cargo:rerun-if-changed=../../packages/almadar-std/behaviors/registry");
    println!("cargo:rerun-if-changed=../../packages/almadar-core/src/types/bindings.ts");
}
