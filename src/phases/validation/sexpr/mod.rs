//! S-Expression Semantic Validation
//!
//! Validates S-expression syntax, operators, arity, and binding references.
//! Ported from `builder/packages/shared/src/orbitals/validation/sexpr-validator.ts`.

mod arity;
mod binding;
pub(crate) mod operators;
mod validate;

pub use arity::ArityRule;
pub use operators::{CORE_OPERATORS, EFFECT_OPERATORS, OS_OPERATORS};
pub use validate::{validate_all_sexprs, validate_sexpr, validate_trait_sexprs, SExprContext};
pub(crate) use validate::render_sexpr_brief;
