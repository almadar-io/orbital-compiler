//! navigate effect validation + path matching helpers.

use super::super::{ErrorCode, PageInitPayload, ValidationError, ValidationResult};
use super::extract_payload_fields;
use orbital_core::schema::SExpression;
use std::collections::HashMap;

fn is_external_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

/// Validate navigate effect
pub(crate) fn validate_navigate(
    arr: &[SExpression],
    path: &str,
    page_init_payloads: &HashMap<String, PageInitPayload>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["navigate", "/path"] or ["navigate", "/path", { payload }]
    // or ["navigate", ["str/concat", ...], { payload }]
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectUnknownType,
                path,
                "navigate requires a path",
            )
            .with_suggestion("Use: [\"navigate\", \"/path\"]"),
        );
        return result;
    }

    // Extract the navigate path (may be static string or dynamic expression)
    let dynamic_prefix = matches!(&arr[1], SExpression::Array(_));
    let nav_path_str: Option<String> = match &arr[1] {
        SExpression::String(s) => Some(s.clone()),
        SExpression::Array(_) => {
            // Dynamic path like ["str/concat", "/inspection/", "@entity.id"]
            // We can't statically validate the exact path, but we can try to
            // extract a pattern if the first segment is a literal
            extract_static_path_prefix(&arr[1])
        }
        _ => None,
    };

    // Check path format
    if let Some(ref nav_path) = nav_path_str {
        if !is_external_url(nav_path) && !nav_path.starts_with('/') && !nav_path.starts_with('@') {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PageInvalidPath,
                    path,
                    format!("Navigate path should start with '/': '{}'", nav_path),
                )
                .with_suggestion("Use absolute paths starting with /"),
            );
        }
    }

    // External URLs are opened by the client runtime/shell; no page match required.
    if let Some(ref nav_path) = nav_path_str {
        if is_external_url(nav_path) {
            return result;
        }
    }

    // The host's own sign-in route has no page: every host provides it itself.
    if let Some(ref nav_path) = nav_path_str {
        let route = nav_path.split_once('?').map_or(nav_path.as_str(), |(route, _)| route);
        if orbital_core::schema::is_host_sign_in_route(route) {
            return result;
        }
    }

    // Check payload requirements for the target page
    if let Some(ref nav_path) = nav_path_str {
        // Try to find a matching page path pattern
        let matching_page = if let Some((route, _query)) = nav_path.split_once('?') {
            find_matching_page_path(route, page_init_payloads)
        } else if dynamic_prefix {
            find_page_beneath_prefix(nav_path, page_init_payloads)
        } else {
            find_matching_page_path(nav_path, page_init_payloads)
        };

        // L2.4 backstop (gap #19A author-side detection): when no
        // declared page matches, error. Catches navigate calls injected
        // by molecule-level `effects: { ... }` overrides AND navigate
        // targets that resolved to imported pages from `uses` refs that
        // L1's pre-inline check couldn't see.
        //
        // Skip warning the dynamic-prefix case where the path was
        // synthesised via `extract_static_path_prefix` from a
        // `["str/concat", ...]` expression. Those are best-effort
        // partial paths; the page_init_payloads side does prefix
        // matching for them already, so no match here genuinely means
        // no page begins with that prefix.
        // A target that is entirely a binding reference (`@payload.href`,
        // `@entity.route`) is runtime-resolved data — reachability is
        // undecidable statically (the nav-stack CRUMB_NAV idiom navigates
        // to whichever level the payload carries). Only paths with a
        // static component are checkable.
        let fully_dynamic_target = nav_path.starts_with('@');
        if matching_page.is_none() && !page_init_payloads.is_empty() && !fully_dynamic_target {
            // Skip when the schema is genuinely page-less (early-stage
            // scaffolding). The check fires only when the schema
            // declares pages but none match this navigate target.
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectNavigateTargetUnreachable,
                    path,
                    format!(
                        "Navigate target '{}' doesn't resolve to any declared page",
                        nav_path
                    ),
                )
                .with_suggestion(format!(
                    "Declare a matching `pages[].path` (e.g. `page \"{}\" -> ...`) or correct the navigate path",
                    nav_path
                )),
            );
        }

        if let Some((page_path, init_payload)) = matching_page {
            // Check if navigate provides required payload
            if !init_payload.required_fields.is_empty() {
                let provided_fields = extract_payload_fields(arr);

                // Check each required field
                for required_field in &init_payload.required_fields {
                    if !provided_fields.contains(required_field) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectNavigateMissingPayload,
                                path,
                                format!(
                                    "Navigation to '{}' requires payload field '{}' for INIT event",
                                    page_path, required_field
                                ),
                            )
                            .with_suggestion(format!(
                                "Add payload to navigate effect: [\"navigate\", \"{}\", {{ \"{}\": \"@entity.{}\" }}]",
                                nav_path, required_field, required_field
                            ))
                            .with_expected_shape(format!(
                                r#"["navigate", "{}", {{ {} }}]"#,
                                nav_path,
                                init_payload
                                    .required_fields
                                    .iter()
                                    .map(|f| format!("\"{}\": \"...\"", f))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )),
                        );
                    }
                }
            }
        }
    }

    result
}

pub(crate) use orbital_core::route::{extract_static_path_prefix, path_matches_pattern};

/// Find a matching page path pattern for a given navigation path
pub(crate) fn find_matching_page_path<'a>(
    nav_path: &str,
    page_init_payloads: &'a HashMap<String, PageInitPayload>,
) -> Option<(&'a String, &'a PageInitPayload)> {
    // First try exact match
    if let Some(payload) = page_init_payloads.get(nav_path) {
        return Some((
            page_init_payloads.keys().find(|k| *k == nav_path).unwrap(),
            payload,
        ));
    }

    // Try pattern matching (e.g., "/inspection/123" matches "/inspection/:id")
    for (page_path, payload) in page_init_payloads {
        if path_matches_pattern(nav_path, page_path) {
            return Some((page_path, payload));
        }
    }

    // For partial matches (from dynamic paths like "/inspection/"),
    // try to find a pattern that starts with the same prefix
    // A prefix ending in `/` continues with a further segment (`/feed/` + id), so
    // only a page beneath it serves it — `/feed` itself does not.
    for (page_path, payload) in page_init_payloads {
        if page_path.starts_with(nav_path.trim_end_matches('/')) {
            return Some((page_path, payload));
        }
    }

    None
}

/// A page a dynamic `str/concat` target can reach: its path starts with the
/// literal prefix (`/feed/` + id reaches `/feed/:id`, never `/feed`).
fn find_page_beneath_prefix<'a>(
    prefix: &str,
    page_init_payloads: &'a HashMap<String, PageInitPayload>,
) -> Option<(&'a String, &'a PageInitPayload)> {
    let mut beneath: Vec<(&'a String, &'a PageInitPayload)> = page_init_payloads
        .iter()
        .filter(|(page_path, _)| page_path.starts_with(prefix))
        .collect();
    beneath.sort_by(|a, b| a.0.cmp(b.0));
    beneath.into_iter().next()
}

/// Check if a concrete path matches a pattern with :param placeholders
/// Validate send-server effect: requires event name arg; optional payload.
pub(crate) fn validate_send_server(arr: &[SExpression], path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();
    if arr.len() < 2 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectUnknownType,
                path,
                "send-server requires an event name",
            )
            .with_suggestion(r#"Use: ["send-server", "EVENT_NAME"] or ["send-server", "EVENT_NAME", { payload }]"#),
        );
    }
    result
}

#[cfg(test)]
mod dynamic_prefix_tests {
    use super::*;

    fn pages(paths: &[&str]) -> HashMap<String, PageInitPayload> {
        paths.iter().map(|p| (p.to_string(), PageInitPayload::default())).collect()
    }

    fn concat(prefix: &str) -> Vec<SExpression> {
        vec![
            SExpression::String("navigate".to_string()),
            SExpression::Array(vec![
                SExpression::String("str/concat".to_string()),
                SExpression::String(prefix.to_string()),
                SExpression::String("@payload.id".to_string()),
            ]),
        ]
    }

    fn unreachable(arr: &[SExpression], declared: &[&str]) -> bool {
        validate_navigate(arr, "t", &pages(declared))
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNavigateTargetUnreachable)
    }

    #[test]
    fn a_slash_terminated_dynamic_prefix_needs_a_page_beneath_it() {
        assert!(unreachable(&concat("/feed/"), &["/feed"]));
    }

    #[test]
    fn control_a_declared_detail_page_beneath_the_prefix_satisfies_it() {
        assert!(!unreachable(&concat("/feed/"), &["/feed", "/feed/:id"]));
    }

    #[test]
    fn control_a_prefix_without_a_trailing_slash_keeps_prefix_matching() {
        assert!(!unreachable(&concat("/feed"), &["/feed"]));
    }

    #[test]
    fn a_dynamic_query_string_resolves_its_route_before_the_question_mark() {
        assert!(!unreachable(&concat("/inspection/new?companyId="), &["/inspection/new"]));
        assert!(unreachable(&concat("/inspection/new?companyId="), &["/inspection"]));
    }

    fn static_target(target: &str) -> Vec<SExpression> {
        vec![
            SExpression::String("navigate".to_string()),
            SExpression::String(target.to_string()),
        ]
    }

    #[test]
    fn the_host_sign_in_route_is_reachable_without_a_page() {
        assert!(!unreachable(&static_target("/login"), &["/feed"]));
        assert!(!unreachable(&static_target("/login/"), &["/feed"]));
        assert!(!unreachable(&static_target("/login?returnTo=/feed"), &["/feed"]));
    }

    #[test]
    fn control_an_undeclared_route_is_still_unreachable() {
        assert!(unreachable(&static_target("/nowhere"), &["/feed"]));
        assert!(unreachable(&static_target("/login-help"), &["/feed"]));
        assert!(unreachable(&static_target("/account/login"), &["/feed"]));
    }

    #[test]
    fn control_a_declared_sign_in_page_is_reachable_as_any_page() {
        assert!(!unreachable(&static_target("/welcome"), &["/welcome"]));
    }

    #[test]
    fn control_a_static_target_still_matches_its_page() {
        let arr = vec![
            SExpression::String("navigate".to_string()),
            SExpression::String("/feed".to_string()),
        ];
        assert!(!unreachable(&arr, &["/feed"]));
    }
}
