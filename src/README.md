# orbital-compiler layout

Directories here are **organizational only**. Every module is declared in its parent (`lib.rs`,
`phases/mod.rs`, `phases/validation/mod.rs`) with a `#[path]` attribute, so public paths stay flat
(`orbital_compiler::type_infer`, not `orbital_compiler::inference::type_infer`) and downstream crates never change.

- The parent's `#[path]` list is the map of where each module lives; its group comments name each directory's role.
- Do **not** add a second `mod` declaration pointing at these files: that creates a duplicate owner.
- When moving a file, update its `#[path]` and any relative `include_str!`/`include_bytes!` or `build.rs` paths.
