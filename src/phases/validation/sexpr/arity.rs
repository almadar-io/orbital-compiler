//! Operator arity rules.

use super::operators::EFFECT_OPERATORS;

/// Operator arity rules
pub struct ArityRule {
    pub min: usize,
    pub max: Option<usize>, // None means unlimited
}

/// Get arity rules for an operator.
///
/// Consults the embedded `operators.json` registry first (Phase 3 of Lolo v3 plan).
/// Falls back to the hardcoded match for ML operators not yet migrated to the registry.
pub(crate) fn get_arity_rule(operator: &str) -> Option<ArityRule> {
    // Replaced primary lookup with embedded::get_operator() in Phase 3 of Lolo v3 plan.
    if let Some(meta) = crate::embedded::get_operator(operator) {
        return Some(ArityRule {
            min: meta.min_arity as usize,
            max: meta.max_arity.map(|m| m as usize),
        });
    }
    match operator {
        // Unary
        "not" | "abs" | "floor" | "ceil" | "round" | "upper" | "lower" | "trim" | "length"
        | "first" | "last" | "rest" | "count" | "keys" | "values" | "entries" | "type"
        | "is-null" | "is-empty" | "is-string" | "is-number" | "is-boolean" | "is-array"
        | "is-object" | "reverse" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // Binary
        "=" | "!=" | "<" | ">" | "<=" | ">=" | "eq" | "neq" | "lt" | "gt" | "lte" | "gte" | "-"
        | "/" | "%" | "sub" | "div" | "mod" | "get" | "nth" | "date-diff" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Binary or more
        "and" | "or" | "+" | "*" | "add" | "mul" | "concat" | "merge" | "min" | "max" => {
            Some(ArityRule { min: 2, max: None })
        }

        // Ternary
        "if" | "substring" | "replace" | "slice" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // Variable arity
        "do" | "let" | "cond" => Some(ArityRule { min: 1, max: None }),

        // Path building (variadic, 1+ segments)
        "path" => Some(ArityRule { min: 1, max: None }),

        // Effects have their own validation
        _ if EFFECT_OPERATORS.contains(&operator) => None,

        // ============================================
        // Neural Network Operators (nn module)
        // ============================================

        // nn/sequential - variadic (1+ layers)
        "nn/sequential" => Some(ArityRule { min: 1, max: None }),

        // nn/linear - binary (in_features, out_features)
        "nn/linear" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Activation functions - unary (optional dim for softmax)
        "nn/relu" | "nn/tanh" | "nn/sigmoid" => Some(ArityRule {
            min: 0,
            max: Some(0),
        }),
        "nn/softmax" => Some(ArityRule {
            min: 0,
            max: Some(1),
        }),

        // Regularization - unary (rate/features)
        "nn/dropout" => Some(ArityRule {
            min: 0,
            max: Some(1),
        }),
        "nn/batchnorm" | "nn/layernorm" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // nn/forward - binary (module, input)
        "nn/forward" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Weight operations - unary/binary
        "nn/getWeights" | "nn/paramCount" | "nn/clone" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),
        "nn/setWeights" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // ============================================
        // Tensor Operators (tensor module)
        // ============================================

        // Creation - unary (data/shape)
        "tensor/from" | "tensor/zeros" | "tensor/ones" | "tensor/rand" | "tensor/randn" => {
            Some(ArityRule {
                min: 1,
                max: Some(1),
            })
        }

        // Shape/Indexing - unary/binary
        "tensor/shape" | "tensor/flatten" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),
        "tensor/get" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),
        "tensor/slice" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),
        "tensor/reshape" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Math - binary
        "tensor/add" | "tensor/sub" | "tensor/mul" | "tensor/div" | "tensor/matmul"
        | "tensor/dot" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Reductions - unary
        "tensor/sum" | "tensor/mean" | "tensor/max" | "tensor/min" | "tensor/argmax"
        | "tensor/norm" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // Range validation - binary
        "tensor/allInRange" | "tensor/clampPerDim" | "tensor/outOfRangeDims" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),
        "tensor/outOfRangeIndices" | "tensor/clamp" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // Conversion - unary
        "tensor/toArray" | "tensor/toList" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // ============================================
        // Training Operators (train module)
        // ============================================

        // train/loop - ternary (module, data, config)
        "train/loop" => Some(ArityRule {
            min: 3,
            max: Some(3),
        }),

        // train/step - quaternary (module, input, target, config)
        "train/step" => Some(ArityRule {
            min: 4,
            max: Some(4),
        }),

        // Validation - binary/ternary
        "train/validate" => Some(ArityRule {
            min: 2,
            max: Some(3),
        }),
        "train/checkRegression" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Constraints - binary
        "train/checkConstraints" | "train/checkWeightMagnitude" | "train/checkForbiddenOutputs" => {
            Some(ArityRule {
                min: 2,
                max: Some(2),
            })
        }

        // Gradients/Weights - binary
        "train/clipGradients" | "train/clipWeights" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),
        "train/getGradientNorm" | "train/getMaxWeightMagnitude" => Some(ArityRule {
            min: 1,
            max: Some(1),
        }),

        // Loss functions - binary (prediction, target)
        "train/mse" | "train/crossEntropy" | "train/huber" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // Optimizers - binary (params, config)
        "train/sgd" | "train/adam" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),

        // RL - binary/ternary
        "train/sampleBatch" => Some(ArityRule {
            min: 2,
            max: Some(2),
        }),
        "train/computeReturns" | "train/computeAdvantages" => Some(ArityRule {
            min: 2,
            max: Some(3),
        }),

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
