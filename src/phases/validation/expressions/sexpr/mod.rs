//! S-Expression Semantic Validation
//!
//! Validates S-expression syntax, operators, arity, and binding references.
//! Ported from `builder/packages/shared/src/orbitals/validation/sexpr-validator.ts`.

mod arity;
mod binding;
pub(crate) mod operators;
mod validate;

pub use arity::ArityRule;
pub use operators::EFFECT_OPERATORS;
pub(crate) use validate::render_sexpr_brief;
pub use validate::{validate_all_sexprs, validate_sexpr, validate_trait_sexprs, SExprContext};
