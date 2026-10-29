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

/// Whether `unknown` exactly matches some real operator's basename under a
/// different (or missing) namespace — see `namespace_hits`.
pub(crate) fn has_namespace_hit(unknown: &str) -> bool {
    orbital_core_base::language::registry::names_operator_basename(unknown.rsplit('/').next().unwrap_or(unknown))
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
    fn the_retired_async_family_is_no_longer_a_known_operator() {
        for name in ["async/delay", "async/race", "async/all", "async/timeout"] {
            assert!(!is_known_operator(name), "{name} should be retired");
        }
        assert!(is_known_operator("array/map"));
    }
}
