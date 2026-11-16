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
            "traits": [{ "ref": "CategoryArticlesOverlay" }]
        }]
    });
    if let Some(expects) = expects {
        orbital["expects"] = expects;
    }
    json!({ "name": "App", "version": "1.0.0", "orbitals": [orbital] })
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
