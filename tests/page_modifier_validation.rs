//! Static page-modifier checks: an indexed page is public, titled, described,
//! concrete and has a canonical origin; an authenticated page has a `signIn`
//! route, and that route is a page anonymous viewers can open.

use orbital_compiler::phases::validation::page::{validate_page_modifiers, validate_sign_in_route};
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn schema(site: Value, pages: Value) -> OrbitalSchema {
    serde_json::from_value(json!({
        "name": "app",
        "site": site,
        "orbitals": [{
            "name": "Main",
            "entity": { "name": "Item", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [],
            "pages": pages
        }]
    }))
    .expect("schema")
}

fn codes(s: &OrbitalSchema) -> Vec<&'static str> {
    validate_page_modifiers(s)
        .errors
        .iter()
        .chain(validate_sign_in_route(s).errors.iter())
        .map(|e| e.code.as_str())
        .collect()
}

fn page(path: &str, extra: Value) -> Value {
    let mut p = json!({ "name": format!("P{}", path.len()), "path": path, "traits": [] });
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    p
}

const ORIGIN: &str = "https://example.org";

#[test]
fn control_a_complete_indexed_public_page_passes() {
    let s = schema(
        json!({ "origin": ORIGIN }),
        json!([page(
            "/",
            json!({ "access": "public", "indexing": "index", "title": "Home", "description": ["i18n/t", "app:meta.home"] })
        )]),
    );
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}

#[test]
fn an_indexed_page_without_public_access_fails() {
    for access in [json!({}), json!({ "access": "authenticated" })] {
        let mut extra = json!({ "indexing": "index", "title": "T", "description": "D" });
        extra
            .as_object_mut()
            .unwrap()
            .extend(access.as_object().unwrap().clone());
        let s = schema(
            json!({ "origin": ORIGIN, "signIn": "/welcome" }),
            json!([
                page("/a", extra),
                page("/welcome", json!({ "access": "public" }))
            ]),
        );
        assert_eq!(codes(&s), vec!["ORB_P_INDEX_REQUIRES_PUBLIC"]);
    }
}

#[test]
fn an_indexed_page_needs_title_description_and_origin() {
    let s = schema(
        json!({}),
        json!([page(
            "/",
            json!({ "access": "public", "indexing": "index" })
        )]),
    );
    let errs = validate_page_modifiers(&s).errors;
    assert_eq!(errs.len(), 1);
    for missing in ["`title:`", "`description:`"] {
        assert!(errs[0].message.contains(missing), "{}", errs[0].message);
    }
}

#[test]
fn control_a_noindex_page_needs_none_of_that() {
    let s = schema(
        json!({}),
        json!([page("/", json!({ "indexing": "noindex" }))]),
    );
    assert!(codes(&s).is_empty());
}

#[test]
fn an_indexed_parameter_route_fails() {
    let s = schema(
        json!({ "origin": ORIGIN }),
        json!([page(
            "/posts/:id",
            json!({ "access": "public", "indexing": "index", "title": "T", "description": "D" })
        )]),
    );
    assert_eq!(codes(&s), vec!["ORB_P_INDEX_PARAM_ROUTE"]);
}

#[test]
fn an_authenticated_page_needs_a_sign_in_route() {
    let s = schema(
        json!({}),
        json!([page("/me", json!({ "access": "authenticated" }))]),
    );
    assert_eq!(codes(&s), vec!["ORB_P_AUTH_NO_SIGN_IN"]);
}

#[test]
fn control_an_authenticated_page_with_a_public_sign_in_route_passes() {
    let s = schema(
        json!({ "signIn": "/welcome" }),
        json!([
            page("/me", json!({ "access": "authenticated" })),
            page("/welcome", json!({}))
        ]),
    );
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}

#[test]
fn a_sign_in_route_that_is_no_page_fails() {
    let s = schema(
        json!({ "signIn": "/welcome" }),
        json!([page("/me", json!({ "access": "authenticated" }))]),
    );
    assert_eq!(codes(&s), vec!["ORB_P_SIGN_IN_UNKNOWN_ROUTE"]);
}

#[test]
fn a_sign_in_route_that_is_itself_authenticated_fails() {
    let s = schema(
        json!({ "signIn": "/welcome" }),
        json!([page("/welcome", json!({ "access": "authenticated" }))]),
    );
    assert_eq!(codes(&s), vec!["ORB_P_SIGN_IN_UNKNOWN_ROUTE"]);
}

#[test]
fn edge_a_sign_in_route_served_by_a_lazy_page_passes() {
    let mut s = schema(
        json!({ "signIn": "/welcome" }),
        json!([page("/me", json!({ "access": "authenticated" }))]),
    );
    s.lazy_pages = Some(vec![serde_json::from_value(
        json!({ "path": "/welcome", "orbital": "Auth", "orbRef": "./auth.orb" }),
    )
    .unwrap()]);
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}

#[test]
fn control_omitted_modifiers_are_never_checked() {
    let s = schema(
        json!({}),
        json!([page("/", json!({})), page("/posts/:id", json!({}))]),
    );
    assert!(codes(&s).is_empty());
}

#[test]
fn a_sign_in_route_naming_the_hosts_sign_in_route_needs_no_page() {
    let s = schema(
        json!({ "signIn": "/login" }),
        json!([page("/me", json!({ "access": "authenticated" }))]),
    );
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}

#[test]
fn a_page_declared_on_the_hosts_sign_in_route_fails() {
    for site in [json!({}), json!({ "signIn": "/login" })] {
        let s = schema(site, json!([page("/login", json!({ "access": "public" }))]));
        assert_eq!(codes(&s), vec!["ORB_P_HOST_RESERVED_ROUTE"]);
    }
}

#[test]
fn edge_a_lazy_page_on_the_hosts_sign_in_route_fails() {
    let mut s = schema(json!({}), json!([page("/", json!({}))]));
    s.lazy_pages = Some(vec![serde_json::from_value(
        json!({ "path": "/login/", "orbital": "Auth", "orbRef": "./auth.orb" }),
    )
    .unwrap()]);
    assert_eq!(codes(&s), vec!["ORB_P_HOST_RESERVED_ROUTE"]);
}

#[test]
fn control_a_page_on_a_route_merely_starting_with_login_passes() {
    let s = schema(
        json!({}),
        json!([
            page("/login-help", json!({})),
            page("/login/:step", json!({}))
        ]),
    );
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}

fn localized(pages: Value) -> OrbitalSchema {
    let mut s = schema(json!({}), pages);
    s.locales = Some(vec!["en".into(), "ar".into()]);
    s
}

#[test]
fn a_translation_group_holding_one_locale_twice_fails() {
    let s = localized(json!([
        page("/blog/x", json!({ "translationOf": "blog/x" })),
        page("/blog/x-copy", json!({ "translationOf": "blog/x" })),
        page("/ar/blog/x", json!({ "translationOf": "blog/x" }))
    ]));
    assert_eq!(codes(&s), vec!["ORB_P_TRANSLATION_DUPLICATE_LOCALE"]);
}

#[test]
fn control_one_page_per_locale_in_a_group_passes() {
    let s = localized(json!([
        page("/blog/x", json!({ "translationOf": "blog/x" })),
        page("/ar/blog/x", json!({ "translationOf": "blog/x" }))
    ]));
    assert!(codes(&s).is_empty(), "{:?}", codes(&s));
}
