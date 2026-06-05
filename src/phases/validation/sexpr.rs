//! S-Expression Semantic Validation
//!
//! Validates S-expression syntax, operators, arity, and binding references.
//! Ported from `builder/packages/shared/src/orbitals/validation/sexpr-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};
use std::collections::HashSet;

/// Core operators (always available)
pub const CORE_OPERATORS: &[&str] = &[
    // Comparison
    "=",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "eq",
    "neq",
    "lt",
    "gt",
    "lte",
    "gte",
    // Logical
    "and",
    "or",
    "not",
    "if",
    "cond",
    // Arithmetic
    "+",
    "-",
    "*",
    "/",
    "%",
    "add",
    "sub",
    "mul",
    "div",
    "mod",
    "abs",
    "min",
    "max",
    "floor",
    "ceil",
    "round",
    // String
    "concat",
    "substring",
    "length",
    "upper",
    "lower",
    "trim",
    "split",
    "join",
    "replace",
    "includes",
    "starts-with",
    "ends-with",
    "path", // Build dot-separated path strings for dynamic field access
    // String namespaced (std library)
    "str/concat",
    "str/length",
    "str/upper",
    "str/lower",
    "str/trim",
    "str/split",
    "str/join",
    "str/replace",
    "str/replaceAll",
    "str/includes",
    "str/startsWith",
    "str/endsWith",
    "str/substring",
    "str/slice",
    "str/indexOf",
    "str/padStart",
    "str/padEnd",
    "str/capitalize",
    "str/truncate",
    "str/template",
    "str/len",
    // GAP-V004: Missing string operators from operators.json
    "str/default",
    "str/repeat",
    "str/reverse",
    // Lambda
    "fn",
    "lambda",
    // Array
    "first",
    "last",
    "nth",
    "rest",
    "count",
    "map",
    "filter",
    "reduce",
    "find",
    "some",
    "every",
    "sort",
    "reverse",
    "append",
    "prepend",
    "slice",
    // Array namespaced (std library)
    "array/first",
    "array/last",
    "array/nth",
    "array/rest",
    "array/count",
    "array/length",
    "array/len", // GAP-V003: alias for array/length (used in i18n)
    "array/map",
    "array/filter",
    "array/reduce",
    "array/find",
    "array/some",
    "array/every",
    "array/sort",
    "array/reverse",
    "array/append",
    "array/prepend",
    "array/slice",
    "array/concat",
    "array/flatten",
    "array/unique",
    "array/indexOf",
    "array/includes",
    "array/sum",
    "array/avg",
    "array/min",
    "array/max",
    // GAP-V004: Missing array operators from operators.json
    "array/drop",
    "array/dropLast",
    "array/findIndex",
    "array/groupBy",
    "array/insert",
    "array/partition",
    "array/range",
    "array/reject",
    "array/remove",
    "array/repeat",
    "array/shuffle",
    "array/take",
    "array/takeLast",
    "array/zip",
    // Object
    "get",
    "get-in",
    "assoc",
    "dissoc",
    "merge",
    "keys",
    "values",
    "entries",
    // Object namespaced (std library)
    "object/get",
    "object/set",
    "object/has",
    "object/keys",
    "object/values",
    "object/entries",
    "object/merge",
    "object/deepMerge",
    "object/pick",
    "object/omit",
    "object/mapValues",
    "object/filter",
    "object/fromEntries",
    "object/empty?",
    // GAP-V004: Missing object operators from operators.json
    "object/clone",
    "object/equals",
    // Type checking
    "type",
    "is-null",
    "is-empty",
    "is-string",
    "is-number",
    "is-boolean",
    "is-array",
    "is-object",
    // Date/Time
    "now",
    "date",
    "date-add",
    "date-diff",
    "format-date",
    "parse-date",
    // Time namespaced (std library)
    "time/now",
    "time/today",
    "time/add",
    "time/subtract",
    "time/diff",
    "time/format",
    "time/parse",
    "time/year",
    "time/month",
    "time/day",
    "time/weekday",
    "time/hour",
    "time/minute",
    "time/second",
    "time/isBefore",
    "time/isAfter",
    // GAP-V004: Missing time operators from operators.json
    "time/isFuture",
    "time/isPast",
    // GAP-V004: Math operators from operators.json
    "math/abs",
    "math/ceil",
    "math/clamp",
    "math/default",
    "math/floor",
    "math/lerp",
    "math/map",
    "math/max",
    "math/min",
    "math/mod",
    "math/pow",
    "math/random",
    "math/randomInt",
    "math/round",
    "math/sign",
    "math/sqrt",
    // GAP-V004: Validate operators from operators.json
    "validate/check",
    "validate/email",
    "validate/equals",
    "validate/length",
    "validate/max",
    "validate/maxLength",
    "validate/min",
    "validate/minLength",
    "validate/oneOf",
    "validate/pattern",
    "validate/phone",
    "validate/range",
    "validate/required",
    "validate/url",
    "validate/uuid",
    // GAP-V004: Format operators from operators.json
    "format/bytes",
    "format/currency",
    "format/list",
    "format/number",
    "format/ordinal",
    "format/percent",
    "format/phone",
    "format/plural",
    // GAP-V004: Async operators from operators.json
    "async/all",
    "async/debounce",
    "async/delay",
    "async/interval",
    "async/race",
    "async/retry",
    "async/sequence",
    "async/throttle",
    "async/timeout",
    // GAP-V004: Missing core operators from operators.json
    "clamp",
    "when",
    "average",
    "contains",
    "empty",
    "sum",
    // Misc
    "let",
    "do",
    "quote",
    "eval",
    "print",
    "debug",
];

/// Effect operators (only allowed in effect context, not guards)
pub const EFFECT_OPERATORS: &[&str] = &[
    "set",
    "emit",
    "fetch",
    "persist",
    "delete",
    "navigate",
    "render-ui",
    "render_ui",
    "call-service",
    "call_service",
    "notify",
    "log",
    "spawn",
    "destroy", // Legacy alias — canonical name is "despawn"
    "despawn", // Canonical (GAP-E001): used by TS evaluator + runtime
    "update",
];

/// Neural network operators (nn module)
/// See: orbital-shared/std/modules/nn.ts
pub const NN_OPERATORS: &[&str] = &[
    // Architecture
    "nn/sequential",
    "nn/linear",
    "nn/relu",
    "nn/tanh",
    "nn/sigmoid",
    "nn/softmax",
    "nn/dropout",
    "nn/batchnorm",
    "nn/layernorm",
    // Execution
    "nn/forward",
    "nn/getWeights",
    "nn/setWeights",
    "nn/paramCount",
    "nn/clone",
];

/// Tensor operators (tensor module)
/// See: orbital-shared/std/modules/tensor.ts
pub const TENSOR_OPERATORS: &[&str] = &[
    // Creation
    "tensor/from",
    "tensor/zeros",
    "tensor/ones",
    "tensor/rand",
    "tensor/randn",
    // Shape/Indexing
    "tensor/shape",
    "tensor/get",
    "tensor/slice",
    "tensor/reshape",
    "tensor/flatten",
    // Math
    "tensor/add",
    "tensor/sub",
    "tensor/mul",
    "tensor/div",
    "tensor/matmul",
    "tensor/dot",
    // Reductions
    "tensor/sum",
    "tensor/mean",
    "tensor/max",
    "tensor/min",
    "tensor/argmax",
    "tensor/norm",
    // Range validation
    "tensor/allInRange",
    "tensor/outOfRangeIndices",
    "tensor/clamp",
    "tensor/clampPerDim",
    "tensor/outOfRangeDims",
    // Conversion
    "tensor/toArray",
    "tensor/toList",
    // Concatenation/Stacking
    "tensor/cat",
    "tensor/stack",
    // Shape manipulation
    "tensor/transpose",
    "tensor/unsqueeze",
    "tensor/squeeze",
];

/// Training operators (train module)
/// See: orbital-shared/std/modules/train.ts
pub const TRAIN_OPERATORS: &[&str] = &[
    // Training
    "train/loop",
    "train/step",
    // Validation
    "train/validate",
    "train/checkRegression",
    // Constraints
    "train/checkConstraints",
    "train/checkWeightMagnitude",
    "train/checkForbiddenOutputs",
    // Gradients
    "train/clipGradients",
    "train/getGradientNorm",
    // Weights
    "train/clipWeights",
    "train/getMaxWeightMagnitude",
    // Loss functions
    "train/mse",
    "train/crossEntropy",
    "train/huber",
    // Optimizers
    "train/sgd",
    "train/adam",
    // RL
    "train/sampleBatch",
    "train/computeReturns",
    "train/computeAdvantages",
];

pub const OS_OPERATORS: &[&str] = &[
    "os/watch-files",
    "os/watch-process",
    "os/watch-port",
    "os/watch-http",
    "os/watch-cron",
    "os/watch-signal",
    "os/watch-env",
    "os/debounce",
];

/// Graph operators (graph module)
pub const GRAPH_OPERATORS: &[&str] = &[
    "graph/from-entities",
    "graph/from-adjacency",
    "graph/from-edge-list",
    "graph/add-self-loops",
    "graph/to-undirected",
    "graph/subgraph",
    "graph/k-hop",
    "graph/node-features",
    "graph/edge-index",
    "graph/edge-features",
    "graph/num-nodes",
    "graph/num-edges",
    "graph/degree",
    "graph/batch",
];

/// Contract operators (contract module)
pub const CONTRACT_OPERATORS: &[&str] = &[
    "contract/validate-input",
    "contract/validate-output",
    "contract/clamp-output",
    "contract/violations",
    "contract/entity-to-tensor",
    "contract/tensor-to-payload",
];

/// Data preprocessing operators (data module)
pub const DATA_OPERATORS: &[&str] = &[
    "data/dataset",
    "data/dataloader",
    "data/split",
    "data/normalize",
    "data/augment",
    "data/tokenize",
    "data/pad",
];

/// ML effect operators (forward, train, evaluate, checkpoint)
pub const ML_EFFECT_OPERATORS: &[&str] = &[
    "forward",
    "train",
    "evaluate",
    "checkpoint/save",
    "checkpoint/load",
];

/// Operator arity rules
pub struct ArityRule {
    pub min: usize,
    pub max: Option<usize>, // None means unlimited
}

/// Check if an operator is in any known operator list.
///
/// This consults the embedded `operators.json` registry first (Phase 3 of Lolo v3),
/// then falls back to the legacy hardcoded constant lists for operators that haven't
/// been migrated yet (e.g. `nn/*`, `tensor/*`, `train/*`, ML effects). The fallback
/// will eventually disappear as those operators move into `operators.json`.
fn is_known_operator(operator: &str) -> bool {
    // Replaced primary lookup with embedded::is_known_operator() in Phase 3 of Lolo v3 plan.
    if crate::embedded::is_known_operator(operator) {
        return true;
    }
    CORE_OPERATORS.contains(&operator)
        || EFFECT_OPERATORS.contains(&operator)
        || NN_OPERATORS.contains(&operator)
        || TENSOR_OPERATORS.contains(&operator)
        || TRAIN_OPERATORS.contains(&operator)
        || OS_OPERATORS.contains(&operator)
        || GRAPH_OPERATORS.contains(&operator)
        || CONTRACT_OPERATORS.contains(&operator)
        || DATA_OPERATORS.contains(&operator)
        || ML_EFFECT_OPERATORS.contains(&operator)
        || operator.starts_with("agent/")
        || operator.starts_with("behavior/")
}

/// Get arity rules for an operator.
///
/// Consults the embedded `operators.json` registry first (Phase 3 of Lolo v3 plan).
/// Falls back to the hardcoded match for ML operators not yet migrated to the registry.
fn get_arity_rule(operator: &str) -> Option<ArityRule> {
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

/// Validation context for S-expressions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SExprContext {
    Guard,
    Effect,
    TickEffect,
}

/// Validate all S-expressions in a schema
pub fn validate_all_sexprs(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_trait_sexprs(trait_def, &path, &ctx.entity_names);
                ctx.merge(result);
            }
        }
    }
}

/// Validate S-expressions in a trait
pub fn validate_trait_sexprs(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Validate guards and effects in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        // Validate guard
        if let Some(guard) = &transition.guard {
            let guard_result = validate_sexpr(
                guard,
                &format!("{}.guard", trans_path),
                SExprContext::Guard,
                known_entities,
            );
            result.merge(guard_result);
        }

        // Validate effects
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_result = validate_sexpr(
                    effect,
                    &format!("{}.effects[{}]", trans_path, j),
                    SExprContext::Effect,
                    known_entities,
                );
                result.merge(effect_result);
            }
        }
    }

    // Validate tick effects
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_result = validate_sexpr(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, j),
                    SExprContext::TickEffect,
                    known_entities,
                );
                result.merge(effect_result);
            }
        }
    }

    result
}

/// Validate a single S-expression
pub fn validate_sexpr(
    expr: &SExpression,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    validate_sexpr_with_locals(expr, path, context, known_entities, &HashSet::new())
}

/// Validate a single S-expression with lambda-local variable tracking.
fn validate_sexpr_with_locals(
    expr: &SExpression,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            // Check if this looks like an S-expression call (first element is a known operator)
            // vs a data array (first element is a field name or literal value)
            let is_sexpr_call = if let SExpression::String(first) = &arr[0] {
                // It's an S-expression call if the first element is a known operator
                // or looks like an operator (contains '/' for namespaced ops)
                is_known_operator(first) || first.contains('/')
            } else {
                // Non-string first element (array, object, etc) - treat as data
                false
            };

            if is_sexpr_call {
                if let SExpression::String(operator) = &arr[0] {
                    // Detect lambda expressions: ["fn", "param", body] or ["lambda", ["p1", "p2"], body]
                    if (operator == "fn" || operator == "lambda") && arr.len() == 3 {
                        let mut extended_locals = lambda_locals.clone();
                        if let SExpression::Array(params) = &arr[1] {
                            for param in params {
                                if let SExpression::String(name) = param {
                                    extended_locals.insert(name.clone());
                                }
                            }
                        } else if let SExpression::String(param_name) = &arr[1] {
                            extended_locals.insert(param_name.clone());
                        }
                        // Validate the body with extended locals
                        result.merge(validate_sexpr_with_locals(
                            &arr[2],
                            &format!("{}[2]", path),
                            context,
                            known_entities,
                            &extended_locals,
                        ));
                        return result;
                    }
                    // Check if effect operator used in guard context
                    if context == SExprContext::Guard
                        && EFFECT_OPERATORS.contains(&operator.as_str())
                    {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::SExprEffectInGuard,
                                path,
                                format!(
                                    "Effect operator '{}' cannot be used in guard context",
                                    operator
                                ),
                            )
                            .with_suggestion(
                                "Guards should only use pure operators for conditions",
                            ),
                        );
                    }

                    // Check operator validity (only warn for namespaced operators that aren't known)
                    if !is_known_operator(operator) && operator.contains('/') {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::SExprInvalidOperator,
                                path,
                                format!("Unknown operator: '{}'", operator),
                            )
                            .with_suggestion(
                                "Use a known operator or ensure custom operator is registered",
                            ),
                        );
                    }

                    // Check arity
                    if let Some(arity) = get_arity_rule(operator) {
                        let arg_count = arr.len() - 1; // Exclude operator
                        if arg_count < arity.min {
                            result.add_error(ValidationError::error(
                                ErrorCode::SExprArityMismatch,
                                path,
                                format!(
                                    "Operator '{}' requires at least {} argument(s), got {}",
                                    operator, arity.min, arg_count
                                ),
                            ));
                        }
                        if let Some(max) = arity.max {
                            if arg_count > max {
                                result.add_error(ValidationError::error(
                                    ErrorCode::SExprArityMismatch,
                                    path,
                                    format!(
                                        "Operator '{}' takes at most {} argument(s), got {}",
                                        operator, max, arg_count
                                    ),
                                ));
                            }
                        }
                    }

                    // Recursively validate arguments
                    for (i, arg) in arr.iter().skip(1).enumerate() {
                        let arg_result = validate_sexpr_with_locals(
                            arg,
                            &format!("{}[{}]", path, i + 1),
                            context,
                            known_entities,
                            lambda_locals,
                        );
                        result.merge(arg_result);
                    }
                }
            } else {
                // Data array - just validate bindings within elements, don't check operators
                for (i, item) in arr.iter().enumerate() {
                    // Only validate bindings (@...) within data arrays
                    if let SExpression::String(s) = item {
                        if s.starts_with('@') {
                            let binding_result = validate_binding(
                                s,
                                &format!("{}[{}]", path, i),
                                context,
                                known_entities,
                                lambda_locals,
                            );
                            result.merge(binding_result);
                        }
                    } else if matches!(item, SExpression::Array(_) | SExpression::Object(_)) {
                        // Recursively check nested structures for bindings
                        let item_result = validate_sexpr_with_locals(
                            item,
                            &format!("{}[{}]", path, i),
                            context,
                            known_entities,
                            lambda_locals,
                        );
                        result.merge(item_result);
                    }
                }
            }
        }
        SExpression::String(s) if s.starts_with('@') => {
            // Validate binding
            let binding_result = validate_binding(s, path, context, known_entities, lambda_locals);
            result.merge(binding_result);
        }
        SExpression::Object(obj) => {
            // Recursively validate object values
            for (key, value) in obj {
                let value_result = validate_sexpr_with_locals(
                    value,
                    &format!("{}.{}", path, key),
                    context,
                    known_entities,
                    lambda_locals,
                );
                result.merge(value_result);
            }
        }
        SExpression::Array(arr) => {
            // Empty array or array literal
            for (i, item) in arr.iter().enumerate() {
                let item_result = validate_sexpr_with_locals(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    known_entities,
                    lambda_locals,
                );
                result.merge(item_result);
            }
        }
        _ => {
            // Null, Bool, Number, plain String - all valid
        }
    }

    result
}

/// Validate a binding reference
fn validate_binding(
    binding: &str,
    path: &str,
    context: SExprContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Parse binding: @root.field.nested or @root
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() || parts[0].is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SExprInvalidBinding,
                path,
                format!("Invalid binding: '{}'", binding),
            )
            .with_suggestion("Bindings should be @root.field format"),
        );
        return result;
    }

    let root = parts[0];

    // Validate binding root
    match root {
        "entity" | "payload" | "state" | "now" | "config" | "trait" | "user" => {
            // Valid core binding root
        }
        // Check if this is a lambda-local variable from an enclosing fn/lambda
        _ if lambda_locals.contains(root) => {
            // Valid lambda-bound variable
        }
        // Lambda/iterator variable bindings (used in map, filter, reduce, etc.)
        // Common patterns: @item, @index, @x, @y, @f, @acc, @elem, @val, @key, @value
        "item" | "index" | "x" | "y" | "f" | "acc" | "elem" | "val" | "key" | "value" | "i"
        | "j" | "k" | "n" | "a" | "b" | "c" | "o" | "result" => {
            // Valid lambda/iterator variable binding
        }
        _ if root
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false) =>
        {
            // Entity binding: @EntityName
            if !known_entities.is_empty() && !known_entities.contains(root) {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::SExprUnknownEntity,
                        path,
                        format!("Entity '{}' in binding may not exist", root),
                    )
                    .with_suggestion("Ensure the entity is defined in the schema"),
                );
            }
        }
        _ => {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::BindingUnknownRoot,
                    path,
                    format!("Unknown binding root: '@{}'", root),
                )
                .with_suggestion("Use @entity, @payload, @state, @now, @config, @EntityName, or lambda variables (@item, @index, @x, etc.)"),
            );
        }
    }

    // Context-specific validation
    if context == SExprContext::TickEffect && root == "payload" {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingPayloadInTick,
                path,
                "@payload is not available in tick context",
            )
            .with_suggestion("Tick effects don't have event payloads - use @entity instead"),
        );
    }

    // @state and @now should not have paths
    if (root == "state" || root == "now") && parts.len() > 1 {
        result.add_error(
            ValidationError::warning(
                ErrorCode::BindingStateNoPath,
                path,
                format!("@{} should not have a path", root),
            )
            .with_suggestion(format!("Use @{} without path", root)),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String("=".to_string()),
            SExpression::String("@entity.status".to_string()),
            SExpression::String("active".to_string()),
        ]);

        let result = validate_sexpr(&guard, "test", SExprContext::Guard, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_effect_in_guard() {
        let guard = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.status".to_string()),
            SExpression::String("active".to_string()),
        ]);

        let result = validate_sexpr(&guard, "test", SExprContext::Guard, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprEffectInGuard));
    }

    #[test]
    fn test_arity_mismatch() {
        let expr = SExpression::Array(vec![
            SExpression::String("not".to_string()),
            // not takes exactly 1 argument, but we give 2
            SExpression::Bool(true),
            SExpression::Bool(false),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Guard, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprArityMismatch));
    }

    #[test]
    fn test_payload_in_tick() {
        let effect = SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.value".to_string()),
            SExpression::String("@payload.data".to_string()),
        ]);

        let result = validate_sexpr(&effect, "test", SExprContext::TickEffect, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn test_valid_entity_binding() {
        let expr = SExpression::String("@Task.title".to_string());

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &known_entities);
        assert!(result.is_valid());
    }

    #[test]
    fn test_unknown_entity_binding() {
        let expr = SExpression::String("@NonExistent.title".to_string());

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &known_entities);
        // This is a warning, not an error
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprUnknownEntity));
    }

    #[test]
    fn test_nn_forward_valid() {
        // ["nn/forward", "@entity.architecture", "@entity.sensors"]
        let expr = SExpression::Array(vec![
            SExpression::String("nn/forward".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            SExpression::String("@entity.sensors".to_string()),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_nn_sequential_valid() {
        // ["nn/sequential", ["nn/linear", 16, 64], ["nn/relu"], ["nn/linear", 64, 4]]
        let expr = SExpression::Array(vec![
            SExpression::String("nn/sequential".to_string()),
            SExpression::Array(vec![
                SExpression::String("nn/linear".to_string()),
                SExpression::Number(16.0),
                SExpression::Number(64.0),
            ]),
            SExpression::Array(vec![SExpression::String("nn/relu".to_string())]),
            SExpression::Array(vec![
                SExpression::String("nn/linear".to_string()),
                SExpression::Number(64.0),
                SExpression::Number(4.0),
            ]),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_tensor_clamp_valid() {
        // ["tensor/clamp", "@entity.output", 0.0, 1.0]
        let expr = SExpression::Array(vec![
            SExpression::String("tensor/clamp".to_string()),
            SExpression::String("@entity.output".to_string()),
            SExpression::Number(0.0),
            SExpression::Number(1.0),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_train_loop_valid() {
        // ["train/loop", "@entity.architecture", "@entity.buffer", "@entity.config"]
        let expr = SExpression::Array(vec![
            SExpression::String("train/loop".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            SExpression::String("@entity.buffer".to_string()),
            SExpression::String("@entity.config".to_string()),
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_nn_forward_arity_error() {
        // nn/forward requires exactly 2 arguments
        let expr = SExpression::Array(vec![
            SExpression::String("nn/forward".to_string()),
            SExpression::String("@entity.architecture".to_string()),
            // Missing second argument
        ]);

        let result = validate_sexpr(&expr, "test", SExprContext::Effect, &HashSet::new());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SExprArityMismatch));
    }

    #[test]
    fn test_is_known_operator() {
        // Core operators
        assert!(is_known_operator("="));
        assert!(is_known_operator("and"));

        // Effect operators
        assert!(is_known_operator("set"));
        assert!(is_known_operator("fetch"));

        // NN operators
        assert!(is_known_operator("nn/forward"));
        assert!(is_known_operator("nn/sequential"));
        assert!(is_known_operator("nn/linear"));

        // Tensor operators
        assert!(is_known_operator("tensor/from"));
        assert!(is_known_operator("tensor/clamp"));

        // Train operators
        assert!(is_known_operator("train/loop"));
        assert!(is_known_operator("train/mse"));

        // Unknown
        assert!(!is_known_operator("unknown/op"));
        assert!(!is_known_operator("foo"));
    }
}
