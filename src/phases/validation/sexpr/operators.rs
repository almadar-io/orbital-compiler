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
    // String — bare "concat" is deliberately absent: ambiguous between
    // str/concat and array/concat (R-EVAL-CONCAT-PATH-DIVERGENCE), rejected
    // explicitly in validate.rs rather than accepted as an alias.
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
    "str/slice",
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
    "array/count",
    "array/len",
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
    "math/ln",
    "math/log10",
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
    "print",
    "debug",
];

/// Effect operators (only allowed in effect context, not guards)
pub const EFFECT_OPERATORS: &[&str] = &[
    "set",
    "emit",
    "fetch",
    "fetch-stream",
    "send-server",
    "persist",
    "delete",
    "navigate",
    "navigate-back",
    "render-ui",
    "render_ui",
    "call-service",
    "call_service",
    "cancel-call",
    "http",
    "random-bytes",
    "credential-status",
    "credential-forget",
    "oauth-authorize",
    "oauth-connection-status",
    "oauth-connection-forget",
    "notify",
    "log",
    "spawn",
    "destroy", // Legacy alias — canonical name is "despawn"
    "despawn", // Canonical (GAP-E001): used by TS evaluator + runtime
    "update",
];

pub const OS_OPERATORS: &[&str] = &[
    "os/watch-files",
    "os/watch-process",
    "os/watch-port",
    "os/watch-http",
    "os/watch-cron",
    "os/watch-signal",
    "os/watch-env",
];

/// Check if an operator is known.
///
/// The embedded canonical `operators.json` registry is the single source of
/// truth for the ML operator families (`nn/`, `tensor/`, `train/`, `graph/`,
/// `contract/`, `data/`, and the bare ML-effect aliases): those legacy
/// hardcoded lists were retired once canonical became a verified superset. A
/// few non-ML legacy lists remain (`core`/`effect`/`os`), plus the `agent/`
/// namespace prefix.
pub(crate) fn is_known_operator(operator: &str) -> bool {
    if crate::embedded::is_known_operator(operator) {
        return true;
    }
    CORE_OPERATORS.contains(&operator)
        || EFFECT_OPERATORS.contains(&operator)
        || OS_OPERATORS.contains(&operator)
        || operator.starts_with("agent/")
}

fn known_operator_names() -> impl Iterator<Item = &'static str> {
    crate::embedded::operators()
        .keys()
        .map(String::as_str)
        .chain(CORE_OPERATORS.iter().copied())
        .chain(EFFECT_OPERATORS.iter().copied())
        .chain(OS_OPERATORS.iter().copied())
}

pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Exact-basename cross-namespace hits for an unknown name (`join` →
/// `["str/join"]`, `array/floor` → `["math/floor"]`). Unlike edit-distance
/// fuzzing, this is a precise signal: a bare name that exactly matches a REAL
/// operator's basename under some namespace is not coincidental — genuine
/// literal data essentially never collides with a real operator's exact
/// spelling. Shared by `suggest_operator` (the did-you-mean message) and
/// `validate.rs`'s call-vs-data classification (a bare unnamespaced head with
/// a namespace hit is treated as an attempted call, not literal data).
pub(crate) fn namespace_hits(unknown: &str) -> Vec<&'static str> {
    let basename = unknown.rsplit('/').next().unwrap_or(unknown);
    let mut hits: Vec<&str> = known_operator_names()
        .filter(|k| k.contains('/') && k.rsplit('/').next() == Some(basename))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    hits
}

/// Whether `unknown` exactly matches some real operator's basename under a
/// different (or missing) namespace — see `namespace_hits`.
pub(crate) fn has_namespace_hit(unknown: &str) -> bool {
    !namespace_hits(unknown).is_empty()
}

/// Did-you-mean for an unknown operator: exact basename in another namespace
/// first (`array/floor` → `math/floor`), then closest names by edit distance.
pub(crate) fn suggest_operator(unknown: &str) -> Option<String> {
    let namespace_hits = namespace_hits(unknown);
    if !namespace_hits.is_empty() {
        let list = namespace_hits
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!("did you mean {list}?"));
    }
    let mut best: Option<(usize, &str)> = None;
    for k in known_operator_names() {
        if k.len().abs_diff(unknown.len()) > 2 {
            continue;
        }
        let d = edit_distance(unknown, k);
        if d <= 2 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, k));
        }
    }
    best.map(|(_, k)| format!("did you mean '{k}'?"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_namespace_hit_matches_exact_basename_across_namespaces() {
        assert!(
            has_namespace_hit("join"),
            "expected 'join' to hit 'str/join'"
        );
        assert!(
            has_namespace_hit("camelCase"),
            "expected 'camelCase' to hit 'str/camelCase'"
        );
        assert!(!has_namespace_hit("totally-not-an-operator"));
    }

    #[test]
    fn suggestions_never_offer_a_name_the_registry_rejects() {
        for name in known_operator_names() {
            if name.contains('/') {
                assert!(
                    crate::embedded::is_known_operator(name),
                    "'{name}' is offered as a suggestion but the cross-registry check rejects it"
                );
            }
        }
        let hint = suggest_operator("array/length").unwrap_or_default();
        assert!(!hint.contains("'array/length'"), "{hint}");
    }

    #[test]
    fn the_retired_async_family_is_no_longer_a_known_operator() {
        for name in ["async/delay", "async/race", "async/all", "async/timeout"] {
            assert!(!is_known_operator(name), "{name} should be retired");
        }
        assert!(is_known_operator("array/map"));
    }
}
