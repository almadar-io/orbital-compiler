//! Page Validation
//!
//! Validates page structure, paths, and trait references.
//! Ported from `builder/packages/shared/src/orbitals/validation/page-validator.ts`.

use super::{
    is_std_trait, ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_VIEW_TYPES,
};
use orbital_core::schema::{OrbitalSchema, PageDefinition, TraitReference};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Regex for valid page paths (must start with /)
static PATH_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/[a-zA-Z0-9\-_/:]*$").unwrap());

/// Regex for valid page names (PascalCase)
static NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Validate all pages in a schema
pub fn validate_all_pages(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut all_paths: HashSet<&str> = HashSet::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        // Check if orbital has pages defined
        let has_pages = orbital
            .pages
            .as_ref()
            .map(|p| !p.is_empty())
            .unwrap_or(false);

        if !has_pages {
            ctx.add_error(
                ValidationError::warning(
                    ErrorCode::OrbitalMissingPages,
                    format!("orbitals[{}]", i),
                    format!(
                        "Orbital '{}' has no pages defined - traits won't be accessible",
                        orbital.name
                    ),
                )
                .with_suggestion(
                    "Add a pages array with at least one page to expose the orbital's traits",
                ),
            );
        }

        if let Some(pages) = &orbital.pages {
            for (j, page_ref) in pages.iter().enumerate() {
                let path = format!("orbitals[{}].pages[{}]", i, j);
                // Only validate inline pages, not references (references are validated during resolution)
                if let Some(page) = page_ref.as_inline() {
                    let check_naming = ctx.schema_language.should_check_ascii_naming();
                    let result = validate_page(
                        page,
                        &path,
                        &ctx.trait_names,
                        &ctx.trait_ids,
                        &mut all_paths,
                        check_naming,
                    );
                    ctx.merge(result);
                }
            }
        }
    }
}

/// Validate a single page definition
pub fn validate_page(
    page: &PageDefinition,
    path: &str,
    known_traits: &HashSet<String>,
    known_trait_ids: &HashSet<String>,
    _seen_paths: &mut HashSet<&str>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Page must have a name
    if page.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageMissingName,
                format!("{}.name", path),
                "Page name is required",
            )
            .with_suggestion(
                "Add a PascalCase name for the page (e.g., 'TaskList', 'UserProfile')",
            ),
        );
    } else if check_naming && !NAME_REGEX.is_match(&page.name) {
        // GAP-V001: Skip ASCII naming check for non-Latin schemas
        result.add_error(
            ValidationError::warning(
                ErrorCode::PageMissingName,
                format!("{}.name", path),
                format!("Page name '{}' should be PascalCase", page.name),
            )
            .with_suggestion("Use PascalCase for page names (e.g., 'TaskList', 'Dashboard')"),
        );
    }

    // Page must have a path
    if page.path.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::PageMissingPath,
                format!("{}.path", path),
                "Page path is required",
            )
            .with_suggestion("Add a path starting with / (e.g., '/tasks', '/users/:id')"),
        );
    } else {
        // Path must start with /
        if !page.path.starts_with('/') {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Page path must start with '/': '{}'", page.path),
                )
                .with_suggestion("Prefix the path with / (e.g., '/tasks' instead of 'tasks')"),
            );
        }

        // Path format validation (skip for non-Latin schemas — Arabic/etc. paths are valid)
        if check_naming && !PATH_REGEX.is_match(&page.path) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Invalid page path format: '{}'", page.path),
                )
                .with_suggestion("Use valid path characters: letters, numbers, hyphens, underscores, and colons for params"),
            );
        }

        // Reject paths starting with /api (collides with Vite dev server proxy)
        if page.path.starts_with("/api") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    format!("{}.path", path),
                    format!("Page path '{}' starts with '/api' which collides with the API server proxy", page.path),
                )
                .with_suggestion("Choose a different path that doesn't start with '/api' (e.g., '/requests' instead of '/apirequests')"),
            );
        }

        // Check for duplicate paths
        // Note: We can't use seen_paths.insert() with &page.path due to lifetime issues,
        // so we'll check containment and handle duplicates at a higher level
    }

    // Page must have traits (trait-driven UI)
    if let Some(traits) = &page.traits {
        if traits.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageEmptyTraits,
                    format!("{}.traits", path),
                    "Page traits array cannot be empty",
                )
                .with_suggestion("Add at least one trait to render UI on the page"),
            );
        }

        // Validate each trait reference
        for (i, trait_ref) in traits.iter().enumerate() {
            let trait_name = trait_ref.trait_name().unwrap_or("<anonymous>");
            let trait_path = format!("{}.traits[{} ({})]", path, i, trait_name);
            let trait_result =
                validate_page_trait_ref(trait_ref, &trait_path, known_traits, known_trait_ids);
            result.merge(trait_result);
        }
    } else {
        // Traits array is optional for pages that use primaryEntity + viewType
        // Only warn if neither traits nor primaryEntity is specified
        if page.primary_entity.is_none() {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::PageMissingTraits,
                    format!("{}.traits", path),
                    "Page should have traits array for trait-driven UI",
                )
                .with_suggestion("Add traits: ['TraitName'] to define page behavior"),
            );
        }
    }

    // Validate view type if specified
    if let Some(view_type) = &page.view_type {
        if !VALID_VIEW_TYPES.contains(&view_type.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidViewType,
                    format!("{}.viewType", path),
                    format!("Invalid view type: '{}'", view_type),
                )
                .with_valid_values(VALID_VIEW_TYPES.iter().map(|s| s.to_string()).collect()),
            );
        }
    }

    // Check for deprecated static sections
    // This is checked via the PageDefinition type - if it has sections, warn
    // Note: The Rust schema type may not have sections field, but we check just in case

    result
}

/// Validate a page trait reference
fn validate_page_trait_ref(
    trait_ref: &TraitReference,
    path: &str,
    known_traits: &HashSet<String>,
    known_trait_ids: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match trait_ref {
        TraitReference::Inline(trait_def) => {
            // Inline traits are validated separately
            // Here we just ensure it has a name
            if trait_def.name.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageInvalidTraitRef,
                        path,
                        "Inline trait must have a name",
                    )
                    .with_suggestion("Add a name property to the inline trait"),
                );
            }
        }
        TraitReference::Reference {
            reference,
            name,
            ref_id,
            ..
        } => {
            // V4 id-first: when the ref_id resolves to a known inline trait, the
            // ref is valid regardless of its `reference` name — which may be
            // stale after a cross-orbital dedup rename (the id is stable). This
            // is what lets `rewrite_page_trait_refs` be deleted.
            let id_resolves = ref_id
                .as_ref()
                .is_some_and(|id| known_trait_ids.contains(id));
            let trait_name = reference.as_ref().or(name.as_ref());

            if id_resolves {
                // ok — id carries the edge
            } else if let Some(name) = trait_name {
                if !known_traits.contains(name) && !is_std_trait(name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::PageInvalidTraitRef,
                            path,
                            format!("Trait '{}' referenced on page does not exist", name),
                        )
                        .with_suggestion("Define traits inline within orbitals or use a std trait"),
                    );
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PageInvalidTraitRef,
                        path,
                        "Page trait reference must have 'ref' or 'name' property",
                    )
                    .with_suggestion("Add ref: 'TraitName' to reference a trait"),
                );
            }
        }
    }

    result
}

/// Check for duplicate page paths across all orbitals
pub fn validate_page_path_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut path_to_pages: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(pages) = &orbital.pages {
            for (j, page_ref) in pages.iter().enumerate() {
                let location = format!("orbitals[{}].pages[{}]", i, j);
                // Only check inline pages for path uniqueness
                if let Some(page) = page_ref.as_inline() {
                    path_to_pages
                        .entry(page.path.clone())
                        .or_default()
                        .push(location);
                }
            }
        }
    }

    for (path, locations) in path_to_pages {
        if locations.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageDuplicatePath,
                    locations[0].clone(),
                    format!("Duplicate page path '{}' found at: {:?}", path, locations),
                )
                .with_suggestion("Each page must have a unique path"),
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_page() -> PageDefinition {
        PageDefinition {
            name: "TaskList".to_string(),
            path: "/tasks".to_string(),
            traits: Some(vec![TraitReference::Reference {
                type_args: None,
                id: None,
                reference: Some("std/List".to_string()),
                name: None,
                linked_entity: None,
                config: None,
                events: None,
                fields: None,
                listens: None,
                emits_scope: None,
                event_ids: None,
                linked_entity_id: None,
                ref_id: None,
                }]),
            primary_entity: None,
            view_type: None,
            is_initial: None,
            sections: None,
            icon: None,
            label: None,
            id: None,
        }
    }

    /// A page whose single trait ref carries `reference` (name) + optional
    /// `ref_id`. Used to prove the V4 id-first page-ref check.
    fn page_with_ref(reference: &str, ref_id: Option<&str>) -> PageDefinition {
        let mut p = create_valid_page();
        p.traits = Some(vec![TraitReference::Reference {
            type_args: None,
            id: None,
            reference: Some(reference.to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: ref_id.map(str::to_string),
        }]);
        p
    }

    #[test]
    fn test_valid_page() {
        let page = create_valid_page();
        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn page_trait_ref_valid_via_ref_id_when_name_stale() {
        // V4: after a molecule/dedup rename, a page ref keeps the OLD name but a
        // STABLE ref_id. With the name absent from `known_traits` but the id in
        // `known_trait_ids`, the ref is valid — the id carries the edge. This is
        // what replaced the deleted `rewrite_page_trait_refs` name rewrite.
        let page = page_with_ref("StaleOldName", Some("trt_current0000000000000000"));
        let known_traits = HashSet::new(); // stale name is NOT here
        let known_trait_ids =
            HashSet::from(["trt_current0000000000000000".to_string()]);
        let mut seen_paths = HashSet::new();
        let result = validate_page(
            &page,
            "test",
            &known_traits,
            &known_trait_ids,
            &mut seen_paths,
            true,
        );
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::PageInvalidTraitRef),
            "id-resolving page ref must not fire PageInvalidTraitRef: {:?}",
            result.errors
        );
    }

    #[test]
    fn page_trait_ref_invalid_when_neither_name_nor_id_resolves() {
        // Neither the name nor the ref_id resolves — a genuine dangling ref.
        let page = page_with_ref("Ghost", Some("trt_ghost000000000000000000"));
        let mut seen_paths = HashSet::new();
        let result = validate_page(
            &page,
            "test",
            &HashSet::new(),
            &HashSet::new(),
            &mut seen_paths,
            true,
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::PageInvalidTraitRef),
            "unresolvable page ref must fire PageInvalidTraitRef"
        );
    }

    #[test]
    fn test_page_missing_name() {
        let mut page = create_valid_page();
        page.name = String::new();

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageMissingName));
    }

    #[test]
    fn test_page_missing_path() {
        let mut page = create_valid_page();
        page.path = String::new();

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageMissingPath));
    }

    #[test]
    fn test_page_invalid_path_no_slash() {
        let mut page = create_valid_page();
        page.path = "tasks".to_string(); // Missing leading /

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidPath));
    }

    #[test]
    fn test_page_empty_traits() {
        let mut page = create_valid_page();
        page.traits = Some(vec![]);

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageEmptyTraits));
    }

    #[test]
    fn test_page_invalid_trait_ref() {
        let mut page = create_valid_page();
        page.traits = Some(vec![TraitReference::Reference {
            type_args: None,
            id: None,
            reference: Some("NonExistentTrait".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        }]);

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidTraitRef));
    }

    #[test]
    fn test_page_std_trait_ref() {
        let page = create_valid_page(); // Uses std/List

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(result.is_valid()); // std traits are always valid
    }

    #[test]
    fn test_page_invalid_view_type() {
        let mut page = create_valid_page();
        page.view_type = Some("invalid".to_string());

        let mut seen_paths = HashSet::new();
        let result = validate_page(&page, "test", &HashSet::new(), &HashSet::new(), &mut seen_paths, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::PageInvalidViewType));
    }
}
