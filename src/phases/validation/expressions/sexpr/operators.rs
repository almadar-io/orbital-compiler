//! Known operator registries and operator-existence check.

/// The operators the table declares in its `effect` category (only allowed in effect context, not guards).
pub fn effect_category_operators() -> impl Iterator<Item = &'static str> {
    crate::embedded::operators()
        .iter()
        .filter(|(_, operator)| operator.category == "effect")
        .map(|(name, _)| name.as_str())
}

/// Whether the table declares `operator` in its `effect` category.
pub fn is_effect_category_operator(operator: &str) -> bool {
    crate::embedded::get_operator(operator).is_some_and(|declared| declared.category == "effect")
}

/// Check if an operator is known.
///
/// The embedded canonical operator table is the one source; only table names
/// are operators (bare aliases are retired, P-017 K6).
pub(crate) fn is_known_operator(operator: &str) -> bool {
    crate::embedded::is_known_operator(operator)
}

fn known_operator_names() -> impl Iterator<Item = &'static str> {
    crate::embedded::operators().keys().map(String::as_str)
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
    orbital_core_base::language::registry::names_operator_basename(unknown.rsplit('/').next().unwrap_or(unknown))
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
