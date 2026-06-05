//! Lucide Icon Definitions
//!
//! Auto-synced from lucide-react package (node_modules/lucide-react/dist/esm/icons/).
//! To regenerate: `ls node_modules/lucide-react/dist/esm/icons/*.js | sed 's|.*/||; s|\.js$||' | sort`
//!
//! Complete Lucide icon set for validation and suggestions.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Complete Lucide icon set (1517 icons, synced from lucide-react).
/// Used for validation warnings and suggestions.
pub static COMMON_LUCIDE_ICONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("lucide-icons.txt")
        .lines()
        .filter(|l| !l.is_empty())
        .collect()
});

/// Check if an icon name is a common Lucide icon
pub fn is_common_icon(name: &str) -> bool {
    let normalized = normalize_icon_name(name);
    COMMON_LUCIDE_ICONS.contains(normalized.as_str())
}

/// Normalize icon name to kebab-case for comparison
pub fn normalize_icon_name(name: &str) -> String {
    if is_pascal_case(name) {
        pascal_to_kebab(name)
    } else {
        name.to_lowercase()
    }
}

/// Check if name is PascalCase
fn is_pascal_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.chars().next().unwrap();
    first.is_uppercase() && name.chars().all(|c| c.is_alphanumeric())
}

/// Convert PascalCase to kebab-case
fn pascal_to_kebab(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('-');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Convert kebab-case to PascalCase
pub fn kebab_to_pascal(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

/// Find similar icon names for suggestions
pub fn find_similar_icons(name: &str, max_suggestions: usize) -> Vec<&'static str> {
    let normalized = normalize_icon_name(name);
    let mut scored: Vec<(&'static str, f64)> = COMMON_LUCIDE_ICONS
        .iter()
        .map(|&icon| (icon, similarity(&normalized, icon)))
        .filter(|(_, score)| *score > 0.3)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_suggestions);
    scored.into_iter().map(|(icon, _)| icon).collect()
}

/// Simple string similarity score (0-1) based on common characters
fn similarity(a: &str, b: &str) -> f64 {
    let a_chars: HashSet<char> = a.to_lowercase().chars().collect();
    let b_chars: HashSet<char> = b.to_lowercase().chars().collect();

    let common = a_chars.intersection(&b_chars).count();
    let max_len = a_chars.len().max(b_chars.len());

    if max_len == 0 {
        0.0
    } else {
        common as f64 / max_len as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_common_icon() {
        assert!(is_common_icon("plus"));
        assert!(is_common_icon("trash-2"));
        assert!(is_common_icon("Plus")); // PascalCase version
        assert!(!is_common_icon("nonexistent-icon"));
    }

    #[test]
    fn test_normalize_icon_name() {
        assert_eq!(normalize_icon_name("ChevronRight"), "chevron-right");
        assert_eq!(normalize_icon_name("plus"), "plus");
        assert_eq!(normalize_icon_name("Trash2"), "trash2");
    }

    #[test]
    fn test_kebab_to_pascal() {
        assert_eq!(kebab_to_pascal("chevron-right"), "ChevronRight");
        assert_eq!(kebab_to_pascal("plus"), "Plus");
        assert_eq!(kebab_to_pascal("trash-2"), "Trash2");
    }

    #[test]
    fn test_find_similar_icons() {
        let similar = find_similar_icons("trsh", 3);
        assert!(similar.contains(&"trash") || similar.contains(&"trash-2"));
    }
}
