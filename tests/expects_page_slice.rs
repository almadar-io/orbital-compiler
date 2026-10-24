//! `expects page "/route/:param"` — the route arm of the consumer-side
//! requirement contract (SCAN-NAV-TARGET-UNREACHABLE-1).
//!
//! Navigate reachability is a WHOLE-PROGRAM check: `validate_navigate` matches
//! the target against `page_init_payloads`, which `collect_names` builds from
//! every orbital in the schema it was handed. Slice one orbital out of an
//! organism and a navigate to a sibling's route dangles — not because anything
//! is wrong, but because the provider is not in the array.
//!
//! `expects page` is what a slice uses to say so. It is the one cross-boundary
//! edge family the effect cannot state for itself: `(str/concat "/articles/"
//! @payload.data.slug)` yields the static PREFIX `/articles/`, while the
//! provider declares the PATTERN `/articles/:slug` — the parameterisation is
//! information the usage does not carry.

use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

const NAV_UNREACHABLE: &str = "ORB_EFF_NAVIGATE_TARGET_UNREACHABLE";

fn codes(schema: Value) -> Vec<String> {
    let schema: OrbitalSchema =
        serde_json::from_value(schema).expect("schema fixture must deserialize");
    validate_comprehensive_without_services(&schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

/// CategoryOrbital, standing alone: it declares `/categories` and navigates to
/// `/articles/<slug>`, whose page ArticleOrbital owns.
fn category_slice(expects: Option<Value>) -> Value {
    let mut orbital = json!({
        "name": "CategoryOrbital",
        "entity": {
            "name": "Category",
            "collection": "categories",
            "persistence": "persistent",
            "fields": [
                { "name": "id", "type": "string", "required": true },
                { "name": "name", "type": "string" }
            ]
        },
        "traits": [{
            "name": "CategoryArticlesOverlay",
            "scope": "instance",
            "linkedEntity": "Category",
            "stateMachine": {
                "states": [{ "name": "browsing", "isInitial": true }],
                "events": [{ "key": "OPEN_ARTICLE", "name": "Open article" }],
                "transitions": [{
                    "from": "browsing",
                    "to": "browsing",
                    "event": "OPEN_ARTICLE",
                    "effects": [
                        ["navigate", ["str/concat", "/articles/", "@payload.data.slug"]]
                    ]
                }]
            }
        }],
        "pages": [{
            "name": "CategoriesPage",
            "path": "/categories",
            "linkedEntity": "Category",
            "traits": [{ "ref": "CategoryArticlesOverlay" }]
        }]
    });
    if let Some(expects) = expects {
        orbital["expects"] = expects;
    }
    json!({ "name": "App", "version": "1.0.0", "orbitals": [orbital] })
}

#[test]
fn slice_without_the_declaration_reports_the_navigate_unreachable() {
    // The baseline this feature exists to fix: nothing is wrong with the
    // program, only with checking it in isolation.
    assert!(codes(category_slice(None))
        .iter()
        .any(|c| c == NAV_UNREACHABLE));
}

#[test]
fn expects_page_makes_the_sibling_route_reachable() {
    let expects = json!([{ "kind": "page", "path": "/articles/:slug" }]);
    assert!(
        !codes(category_slice(Some(expects)))
            .iter()
            .any(|c| c == NAV_UNREACHABLE),
        "a declared route expectation must satisfy navigate reachability"
    );
}

#[test]
fn an_unrelated_expected_route_does_not_satisfy_the_navigate() {
    // The expectation is not a blanket amnesty — it registers ONE pattern, and
    // a navigate that does not match it still fails. This is what keeps a real
    // typo an error: derivation only ever emits a pattern the target matched.
    let expects = json!([{ "kind": "page", "path": "/authors/:id" }]);
    assert!(codes(category_slice(Some(expects)))
        .iter()
        .any(|c| c == NAV_UNREACHABLE));
}

#[test]
fn a_real_declaration_still_wins_over_an_expectation_of_the_same_path() {
    // Both present: the orbital declares `/categories` AND (redundantly)
    // expects it. The declared page's INIT payload must stay authoritative —
    // the expectation seeds an EMPTY payload, so a naive insert would erase
    // the payload requirements of a page the program really owns.
    let expects = json!([{ "kind": "page", "path": "/categories" }]);
    let with_both = codes(category_slice(Some(expects)));
    let declared_only = codes(category_slice(None));
    assert_eq!(
        with_both.iter().filter(|c| *c != NAV_UNREACHABLE).count(),
        declared_only
            .iter()
            .filter(|c| *c != NAV_UNREACHABLE)
            .count(),
        "expecting a route the orbital declares itself must change nothing else"
    );
}

const MISMATCH: &str = "ORB_S_EXPECTATION_PROVIDER_MISMATCH";
const UNSATISFIED: &str = "ORB_S_EXPECTATION_UNSATISFIED";

fn endpoint_program(page: Value, expects: Value) -> Value {
    let trait_def = |name: &str| {
        json!({
            "name": name,
            "linkedEntity": "Job",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT", "name": "Init" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }]
            }
        })
    };
    json!({
        "name": "App",
        "version": "1.0.0",
        "orbitals": [{
            "name": "Worker",
            "entity": {
                "name": "Job",
                "collection": "jobs",
                "persistence": "persistent",
                "fields": [{ "name": "id", "type": "string", "required": true }]
            },
            "traits": [trait_def("Seed"), trait_def("Tick")],
            "pages": [page],
            "expects": expects
        }]
    })
}

fn boot_page(traits: Value) -> Value {
    json!({ "name": "Start", "path": "", "protocol": "boot", "linkedEntity": "Job", "traits": traits })
}

#[test]
fn a_boot_expectation_is_satisfied_by_a_boot_page_mounting_the_traits() {
    let expects = json!([{ "kind": "page", "protocol": "boot", "traits": ["Seed"], "strict": true }]);
    let found = codes(endpoint_program(boot_page(json!([{ "ref": "Seed" }, { "ref": "Tick" }])), expects));
    assert!(!found.iter().any(|c| c == MISMATCH || c == UNSATISFIED), "{found:?}");
}

#[test]
fn a_boot_expectation_is_a_mismatch_when_the_boot_page_lacks_the_trait() {
    let expects = json!([{ "kind": "page", "protocol": "boot", "traits": ["Seed", "Tick"], "strict": true }]);
    let found = codes(endpoint_program(boot_page(json!([{ "ref": "Seed" }])), expects));
    assert!(found.iter().any(|c| c == MISMATCH), "{found:?}");
}

#[test]
fn a_route_expectation_is_not_satisfied_by_a_boot_page() {
    let expects = json!([{ "kind": "page", "path": "", "strict": true }]);
    let found = codes(endpoint_program(boot_page(json!([{ "ref": "Seed" }])), expects));
    assert!(found.iter().any(|c| c == UNSATISFIED), "{found:?}");
}

#[test]
fn a_boot_expectation_is_unsatisfied_when_the_program_has_no_boot_page() {
    let route_page = json!({ "name": "Home", "path": "/home", "linkedEntity": "Job", "traits": [{ "ref": "Seed" }] });
    let expects = json!([{ "kind": "page", "protocol": "boot", "strict": true }]);
    let found = codes(endpoint_program(route_page, expects));
    assert!(found.iter().any(|c| c == UNSATISFIED), "{found:?}");
}

#[test]
fn a_route_expectation_still_matches_by_path_and_checks_the_traits() {
    let route_page = json!({ "name": "Home", "path": "/home", "linkedEntity": "Job", "traits": [{ "ref": "Seed" }] });
    let ok = json!([{ "kind": "page", "path": "/home", "traits": ["Seed"], "strict": true }]);
    let found = codes(endpoint_program(route_page.clone(), ok));
    assert!(!found.iter().any(|c| c == MISMATCH || c == UNSATISFIED), "{found:?}");
    let bad = json!([{ "kind": "page", "path": "/home", "traits": ["Tick"], "strict": true }]);
    assert!(codes(endpoint_program(route_page, bad)).iter().any(|c| c == MISMATCH));
}
