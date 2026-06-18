//! Known operator registries and operator-existence check.

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
    "fetch-stream",
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

/// Check if an operator is in any known operator list.
///
/// This consults the embedded `operators.json` registry first (Phase 3 of Lolo v3),
/// then falls back to the legacy hardcoded constant lists for operators that haven't
/// been migrated yet (e.g. `nn/*`, `tensor/*`, `train/*`, ML effects). The fallback
/// will eventually disappear as those operators move into `operators.json`.
pub(crate) fn is_known_operator(operator: &str) -> bool {
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
