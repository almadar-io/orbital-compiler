//! Operator arity rules.

use super::operators::EFFECT_OPERATORS;

/// Operator arity rules
pub struct ArityRule {
    pub min: usize,
    pub max: Option<usize>, // None means unlimited
}

/// Get arity rules for an operator.
///
/// The canonical operator registry (`canonical-operators.json`, surfaced via
/// `embedded::get_operator`) is the source of truth for arity wherever the
/// operator is declared there. The match below is a fallback for operators not
/// yet carried in the registry (legacy aliases, ML/tensor/graph/contract/data
/// modules); every arm here is for an operator the registry does NOT define, so
/// the registry lookup always wins when both have it.
pub(crate) fn get_arity_rule(operator: &str) -> Option<ArityRule> {
    if let Some(meta) = crate::embedded::get_operator(operator) {
        return Some(ArityRule {
            min: meta.min_arity as usize,
            max: meta.max_arity.map(|m| m as usize),
        });
    }
    match operator {
        // Unary
        "abs" | "floor" | "ceil" | "round" | "upper" | "lower" | "trim" | "length" | "first"
        | "last" | "rest" | "count" | "keys" | "values" | "entries" | "type" | "is-null"
        | "is-empty" | "is-string" | "is-number" | "is-boolean" | "is-array" | "is-object"
        | "reverse" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // Binary
        "eq" | "neq" | "lt" | "gt" | "lte" | "gte" | "sub" | "div" | "mod" | "get" | "nth"
        | "date-diff" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Binary or more
        "add" | "mul" | "concat" | "merge" | "min" | "max" => {
            Some(ArityRule { min: 2, max: None })
        }

        // Ternary
        "substring" | "replace" | "slice" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // Variable arity
        "cond" => Some(ArityRule { min: 1, max: None }),

        // Effects have their own validation
        _ if EFFECT_OPERATORS.contains(&operator) => None,

        // ============================================
        // New Tensor Operators
        // ============================================

        // tensor/cat, tensor/stack - binary (two tensors)
        "tensor/cat" | "tensor/stack" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // tensor/transpose - 1 arg (t()) or 3 args (dim0, dim1)
        "tensor/transpose" => Some(ArityRule {
            min: 1,
            max: Some(3),
        }),

        // tensor/unsqueeze - binary (tensor, dim)
        "tensor/unsqueeze" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // tensor/squeeze - unary (tensor)
        "tensor/squeeze" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // ============================================
        // Graph Operators (graph module)
        // ============================================

        // graph/from-entities - unary (entity list)
        "graph/from-entities" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // graph/from-adjacency, graph/from-edge-list - binary (features, adjacency/edges)
        "graph/from-adjacency" | "graph/from-edge-list" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Unary graph accessors
        "graph/add-self-loops"
        | "graph/to-undirected"
        | "graph/node-features"
        | "graph/edge-index"
        | "graph/edge-features"
        | "graph/num-nodes"
        | "graph/num-edges"
        | "graph/degree"
        | "graph/batch" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // graph/subgraph - binary (graph, mask)
        "graph/subgraph" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // graph/k-hop - ternary (graph, node_idx, k)
        "graph/k-hop" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // ============================================
        // Contract Operators (contract module)
        // ============================================

        // All contract ops are binary (tensor/entity, contract/mapping)
        "contract/validate-input"
        | "contract/validate-output"
        | "contract/clamp-output"
        | "contract/violations"
        | "contract/entity-to-tensor"
        | "contract/tensor-to-payload" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // ============================================
        // Data Operators (data module)
        // ============================================

        // All data ops are binary (data, config)
        "data/dataset" | "data/dataloader" | "data/split" | "data/normalize" | "data/augment"
        | "data/tokenize" | "data/pad" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // ============================================
        // ML Effect Operators
        // ============================================

        // forward - 1-2 args (slot, optional config)
        "forward" => Some(ArityRule {
            min: 1,
            max: Some(2),
        }),

        // train, evaluate - unary (config dict)
        "train" | "evaluate" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // checkpoint/save - binary (model/slot, path)
        "checkpoint/save" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // checkpoint/load - unary (path)
        "checkpoint/load" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        _ => None,
    }
}
