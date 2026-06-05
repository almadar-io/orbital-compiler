//! Composition Module
//!
//! Native Rust port of the TypeScript behavior composition logic from
//! `@almadar/core/src/builders/`. Used by the `compose` phase to evaluate
//! `behavior/compose`, `behavior/wire`, `behavior/detect-layout`, and
//! `behavior/pipe` calls embedded in `.orb` programs.
//!
//! The TypeScript implementation in `packages/almadar-core/src/builders/` is
//! the canonical reference. This Rust port must produce byte-identical output
//! for the same input. Algorithmic drift between Rust and TS is a blocker.

pub mod compose_behaviors;
pub mod event_wiring;
pub mod layout_strategy;
pub mod pipe;

pub use compose_behaviors::{
    compose_behaviors, ComposeBehaviorsInput, ComposeBehaviorsResult, ComposeError, LayoutMetadata,
    WiringMetadata,
};
pub use event_wiring::{apply_event_wiring, EventWiringEntry};
pub use layout_strategy::{detect_layout_strategy, LayoutStrategy};
pub use pipe::pipe_behaviors;
