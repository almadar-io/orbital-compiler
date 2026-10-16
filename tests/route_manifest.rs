//! `routes.json` carries each route's resolved page modifiers: a literal as
//! written, an `i18n/t` message from that route's locale catalog, plus the
//! locale's text direction and the app's site modifiers.

use orbital_compiler::generate::route_manifest;
use orbital_core_base::ir::{OirModule, OirPage};
use orbital_core_base::schema::{PageAccess, PageIndexing, PageMeta, SiteMeta};
use std::collections::BTreeMap;

fn module() -> OirModule {
    let mut m = OirModule::new("site", "1.0.0");
    m.locales = vec!["en".into(), "ar".into()];
    let mut home = OirPage::new("HomeEN", "/");
    home.access = Some(PageAccess::Public);
    home.indexing = Some(PageIndexing::Index);
    home.title = Some(PageMeta::message("site:meta.home"));
    home.description = Some(PageMeta::Literal("Plain".into()));
    let mut home_ar = OirPage::new("HomeAR", "/ar");
    home_ar.title = Some(PageMeta::message("site:meta.home"));
    m.pages = vec![home, home_ar, OirPage::new("Docs", "/docs")];
    m.site = Some(SiteMeta {
        origin: Some("https://example.org".into()),
        ..SiteMeta::default()
    });
    m
}

fn catalogs() -> BTreeMap<String, BTreeMap<String, String>> {
    BTreeMap::from([
        (
            "en".into(),
            BTreeMap::from([("site:meta.home".into(), "Home".into())]),
        ),
        (
            "ar".into(),
            BTreeMap::from([("site:meta.home".into(), "الرئيسية".into())]),
        ),
    ])
}

#[test]
fn routes_carry_modifiers_resolved_for_their_locale() {
    let m = route_manifest(&module(), &catalogs()).expect("manifest");
    let routes = m["routes"].as_array().unwrap();
    assert_eq!(routes[0]["title"], "Home");
    assert_eq!(routes[0]["description"], "Plain");
    assert_eq!(routes[0]["access"], "public");
    assert_eq!(routes[0]["indexing"], "index");
    assert_eq!(routes[0]["dir"], "ltr");
    assert_eq!(routes[1]["title"], "الرئيسية");
    assert_eq!(routes[1]["locale"], "ar");
    assert_eq!(routes[1]["dir"], "rtl");
    assert_eq!(m["site"]["origin"], "https://example.org");
}

#[test]
fn control_absent_modifiers_stay_absent() {
    let m = route_manifest(&module(), &catalogs()).expect("manifest");
    let docs = m["routes"][2].as_object().unwrap();
    for key in ["access", "indexing", "title", "description"] {
        assert!(!docs.contains_key(key), "{docs:?}");
    }
}

#[test]
fn a_message_missing_from_the_route_catalog_is_an_error() {
    let mut c = catalogs();
    c.get_mut("ar").unwrap().clear();
    let err = route_manifest(&module(), &c).expect_err("missing ar message");
    assert!(
        err.contains("/ar") && err.contains("site:meta.home"),
        "{err}"
    );
}

#[test]
fn edge_a_program_without_site_or_locales_has_bare_routes() {
    let mut m = OirModule::new("app", "1.0.0");
    m.pages = vec![OirPage::new("Home", "/")];
    let manifest = route_manifest(&m, &BTreeMap::new()).expect("manifest");
    assert!(manifest.get("site").is_none());
    assert!(manifest["routes"][0]["locale"].is_null());
    assert!(manifest["routes"][0].get("dir").is_none());
}

#[test]
fn an_indexed_route_in_an_app_without_origin_is_an_error() {
    let mut m = module();
    m.site = None;
    let err = route_manifest(&m, &catalogs()).expect_err("indexed route needs origin");
    assert!(
        err.contains("ORB_P_INDEX_NO_ORIGIN") && err.contains("route /"),
        "{err}"
    );
}

#[test]
fn a_page_on_the_hosts_sign_in_route_is_an_error() {
    for path in ["/login", "/login/"] {
        let mut m = module();
        m.pages.push(OirPage::new("SignIn", path));
        let err = route_manifest(&m, &catalogs()).expect_err("reserved route");
        assert!(
            err.contains("ORB_P_HOST_RESERVED_ROUTE") && err.contains(path),
            "{err}"
        );
    }
}

fn localized() -> OirModule {
    let mut m = OirModule::new("site", "1.0.0");
    m.locales = vec!["en".into(), "ar".into(), "sl".into()];
    let mut pages = Vec::new();
    for (name, path, source) in [
        ("HomeEN", "/", Some("pag_home")),
        ("HomeSL", "/sl", Some("pag_home")),
        ("HomeAR", "/ar", Some("pag_home")),
        ("Docs", "/docs", None),
        ("PostA", "/a", Some("pag_post")),
        ("PostB", "/b", Some("pag_post")),
    ] {
        let mut p = OirPage::new(name, path);
        p.source_page = source.map(str::to_string);
        p.access = Some(PageAccess::Public);
        p.indexing = Some(PageIndexing::Index);
        p.title = Some(PageMeta::Literal(name.into()));
        p.description = Some(PageMeta::Literal(name.into()));
        pages.push(p);
    }
    m.pages = pages;
    m.site = Some(SiteMeta {
        origin: Some("https://example.org".into()),
        ..SiteMeta::default()
    });
    m
}

#[test]
fn imports_of_one_page_in_different_locales_are_alternates_in_locale_order() {
    let m = route_manifest(&localized(), &BTreeMap::new()).expect("manifest");
    assert_eq!(m["defaultLocale"], "en");
    let expected = serde_json::json!([
        { "locale": "en", "path": "/" },
        { "locale": "ar", "path": "/ar" },
        { "locale": "sl", "path": "/sl" }
    ]);
    for i in 0..3 {
        assert_eq!(m["routes"][i]["alternates"], expected, "route {i}");
    }
}

#[test]
fn control_a_page_with_no_source_has_no_alternates() {
    let m = route_manifest(&localized(), &BTreeMap::new()).expect("manifest");
    assert!(m["routes"][3].get("alternates").is_none());
}

#[test]
fn edge_two_imports_in_one_locale_are_not_alternates() {
    let m = route_manifest(&localized(), &BTreeMap::new()).expect("manifest");
    assert!(m["routes"][4].get("alternates").is_none());
    assert!(m["routes"][5].get("alternates").is_none());
}

#[test]
fn edge_a_private_or_unindexed_translation_drops_out_of_the_alternates() {
    let mut m = localized();
    m.pages[1].indexing = Some(PageIndexing::Noindex);
    m.pages[2].access = Some(PageAccess::Authenticated);
    let manifest = route_manifest(&m, &BTreeMap::new()).expect("manifest");
    assert!(
        manifest["routes"][0].get("alternates").is_none(),
        "only one indexable translation is left"
    );
}

#[test]
fn control_a_private_page_gets_no_alternates_of_its_own() {
    let mut m = localized();
    m.pages[1].indexing = Some(PageIndexing::Noindex);
    let manifest = route_manifest(&m, &BTreeMap::new()).expect("manifest");
    assert!(manifest["routes"][1].get("alternates").is_none());
    assert_eq!(
        manifest["routes"][0]["alternates"].as_array().map(Vec::len),
        Some(2)
    );
}

#[test]
fn separately_authored_translations_group_by_translation_of() {
    let mut m = localized();
    m.pages[3].translation_of = Some("docs".into());
    let mut docs_ar = OirPage::new("DocsAR", "/ar/docs");
    docs_ar.translation_of = Some("docs".into());
    docs_ar.access = Some(PageAccess::Public);
    docs_ar.indexing = Some(PageIndexing::Index);
    m.pages.push(docs_ar);
    let manifest = route_manifest(&m, &BTreeMap::new()).expect("manifest");
    assert_eq!(
        manifest["routes"][3]["alternates"],
        serde_json::json!([{ "locale": "en", "path": "/docs" }, { "locale": "ar", "path": "/ar/docs" }])
    );
}

#[test]
fn a_translation_group_holding_one_locale_twice_is_an_error() {
    let mut m = localized();
    m.pages[3].translation_of = Some("docs".into());
    m.pages[4].translation_of = Some("docs".into());
    let err = route_manifest(&m, &BTreeMap::new()).expect_err("duplicate locale");
    assert!(
        err.contains("ORB_P_TRANSLATION_DUPLICATE_LOCALE")
            && err.contains("/docs")
            && err.contains("/a"),
        "{err}"
    );
}
