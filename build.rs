// Build script for orbital-compiler.
//
// Rebuild whenever the committed in-crate baked registries change. The bake is
// produced by `@almadar-io/almadar-pattern-sync rust` (`make sync`) from the
// pinned npm packages; the actual embedding happens in `src/embedded.rs` via
// `include_str!("baked/…")`. No `packages/*` submodules are involved.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/baked/canonical-operators.json");
    println!("cargo:rerun-if-changed=src/baked/patterns-registry.json");
    println!("cargo:rerun-if-changed=src/baked/behaviors-registry-std.json");
    println!("cargo:rerun-if-changed=src/baked/behaviors-registry-io.json");
    // The full std+io behavior corpus is embedded via `include_str!` from
    // `src/baked/{std,io}/…`. These directives cover every `.orb`.
    println!("cargo:rerun-if-changed=src/baked/std");
    println!("cargo:rerun-if-changed=src/baked/io");
    println!("cargo:rerun-if-changed=src/baked/bindings.ts");
}
