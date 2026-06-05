//! S-Expression Semantic Validation
//!
//! Validates S-expression syntax, operators, arity, and binding references.
//! Ported from `builder/packages/shared/src/orbitals/validation/sexpr-validator.ts`.

mod arity;
mod binding;
mod operators;
mod validate;

pub use arity::ArityRule;
pub use operators::{
    CORE_OPERATORS, CONTRACT_OPERATORS, DATA_OPERATORS, EFFECT_OPERATORS, GRAPH_OPERATORS,
    ML_EFFECT_OPERATORS, NN_OPERATORS, OS_OPERATORS, TENSOR_OPERATORS, TRAIN_OPERATORS,
};
pub use validate::{
    validate_all_sexprs, validate_sexpr, validate_trait_sexprs, SExprContext,
};
