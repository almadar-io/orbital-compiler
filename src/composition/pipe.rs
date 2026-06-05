//! Pipe Behaviors
//!
//! `behavior/pipe` is a sugar/sequencing operator. It is normally desugared at
//! parser time and never reaches the Rust Compose pass: the .lolo lowering
//! step rewrites `(behavior/pipe seed step1 step2 ...)` into a sequence of
//! direct calls before the JSON .orb is emitted.
//!
//! For the rare cases where a `behavior/pipe` literal does survive into the
//! .orb, this module provides a no-op fallback that returns the seed value
//! unchanged. The compose pass replaces the call site with whatever this
//! function returns, which is exactly the input.
//!
//! TODO: full pipe implementation; pipe is sugar resolved at .lolo lowering time.

use orbital_core::schema::SExpression;

/// Resolve a `behavior/pipe` call by returning the seed value unchanged.
///
/// The arguments after the seed are ignored. The TS implementation does
/// `steps.reduce((acc, step) => step(acc), seed)`, but at the .orb level
/// the steps are not callable closures, just s-expressions, so the only
/// well-defined fallback is to leave the seed alone and trust the .lolo
/// parser to have already desugared the pipe.
pub fn pipe_behaviors(args: &[SExpression]) -> SExpression {
    args.first().cloned().unwrap_or(SExpression::Null)
}
