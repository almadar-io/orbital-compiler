//! The registry-backed value-source walk: which leaf values an expression can
//! yield, read from the operators' declared `returnSemantics`.
//!
//! `orbital-core` owns the one interpreter of the provenance kinds
//! (`SExpression::value_leaves`); the operator registry lives here, so this
//! module only supplies the lookup. No operator is named anywhere on this path.

use super::canonical_operators::get_canonical_operator;
use orbital_core_base::schema::{OperatorReturn, ValueWalk};

fn operator_return(name: &str) -> Option<OperatorReturn> {
    get_canonical_operator(name).map(|meta| OperatorReturn {
        semantics: meta.return_semantics,
        returns_function: meta.return_type == "function",
    })
}

/// Every branch of every declared value source contributes.
pub(crate) fn all_sources() -> ValueWalk<'static> {
    ValueWalk {
        lookup: &operator_return,
        skip_dead_branches: false,
    }
}

/// As [`all_sources`], except a branch whose condition provably never fires
/// contributes nothing (the emitter side: a dead control is not an emitter).
pub(crate) fn live_sources() -> ValueWalk<'static> {
    ValueWalk {
        lookup: &operator_return,
        skip_dead_branches: true,
    }
}
