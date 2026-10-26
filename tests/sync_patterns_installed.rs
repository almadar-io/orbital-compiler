//! The validator's pattern tables (`phases::validation::sync::patterns`) are derived
//! from the installed `@almadar/core` registry, so a new pattern or prop is accepted
//! without a compiler rebuild. Own test binary: the language data is process-wide.

use orbital_compiler::phases::validation::sync::patterns as p;
use orbital_core_base::language_data::{install_language_data, load_language_data};
use std::fs;

#[test]
fn validator_tables_come_from_the_installed_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let core = tmp.path().join("node_modules/@almadar/core");
    let data = core.join("dist/patterns");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        core.join("package.json"),
        r#"{"name":"@almadar/core","version":"0.0.1","orbLanguage":{"compiler":">=0.0.0",
            "patterns":"dist/patterns/patterns-registry.json",
            "componentMapping":"dist/patterns/component-mapping.json",
            "eventContracts":"dist/patterns/event-contracts.json",
            "services":"dist/patterns/services-registry.json",
            "bindingRules":"dist/language/binding-rules.json",
            "programTypes":"dist/language/program-types.json",
            "protocols":"dist/language/protocols.json",
            "themes":"themes"}}"#,
    )
    .unwrap();
    fs::write(
        data.join("patterns-registry.json"),
        r#"{"patterns":{
            "zz-widget":{"type":"zz-widget","category":"zz-cat","propsSchema":{
                "zzBrandNew":{"types":["string"]},
                "zzRequired":{"types":["string","number"],"required":true},
                "zzAny":{"types":["unknown"]},
                "zzAlign":{"types":["string"],"enumValues":["start","end"]},
                "zzFire":{"types":["string"],"kind":"event"},
                "itemActions":{"types":["array"],"kind":"event-list","eventField":"on",
                    "items":{"types":["object"],"properties":{"zzTone":{"types":["string"]}}}}}},
            "zz-bare":{"type":"zz-bare"}}}"#,
    )
    .unwrap();
    fs::write(data.join("component-mapping.json"), r#"{"mappings":{}}"#).unwrap();
    fs::write(data.join("event-contracts.json"), r#"{"contracts":{}}"#).unwrap();
    fs::write(data.join("services-registry.json"), r#"{"services":{}}"#).unwrap();
    fs::create_dir_all(core.join("dist/language")).unwrap();
    fs::write(
        core.join("dist/language/binding-rules.json"),
        r#"{"guard":{"allowed":["zzRoot"]},"effect":{"allowed":["entity"]},"tick":{"allowed":[]}}"#,
    )
    .unwrap();
    fs::write(core.join("dist/language/program-types.json"), "{}").unwrap();
    fs::write(
        core.join("dist/language/protocols.json"),
        r#"[{"name":"route","address":null,"viewer":true,"grant":null,"side":"client"}]"#,
    )
    .unwrap();
    fs::create_dir_all(core.join("themes")).unwrap();
    fs::write(core.join("themes/zz.json"), r#"{"name":"zz","tokens":{}}"#).unwrap();
    install_language_data(load_language_data(tmp.path(), &tmp.path().join("store")).unwrap())
        .unwrap();

    assert!(p::is_valid_pattern("zz-widget"));
    assert!(
        !p::is_valid_pattern("table-view"),
        "control: the real registry is not in play"
    );
    let zz_root = ["zzRoot".to_string()];
    assert_eq!(
        orbital_compiler::embedded::allowed_sigils_for_context("guard"),
        Some(&zz_root[..]),
        "binding rules come from the installed core"
    );
    assert!(p::get_pattern_valid_props("zz-widget")
        .unwrap()
        .contains("zzBrandNew"));
    assert_eq!(p::get_pattern_category("zz-widget"), Some("zz-cat"));
    assert_eq!(
        p::get_pattern_category("zz-bare"),
        Some("component"),
        "missing category defaults like the generator"
    );
    assert!(
        p::get_pattern_valid_props("zz-bare").is_none(),
        "no propsSchema → no entry"
    );
    assert_eq!(p::get_pattern_props("zz-widget"), Some(&vec!["zzRequired"]));
    assert_eq!(
        p::get_prop_types("zz-widget", "zzRequired"),
        Some(&["string", "number"][..])
    );
    assert_eq!(
        p::get_prop_types("zz-widget", "zzAny"),
        None,
        "\"unknown\" types are not concrete"
    );
    assert_eq!(
        p::get_prop_enum_values("zz-widget", "zzAlign"),
        Some(&["start", "end"][..])
    );
    assert_eq!(p::get_prop_enum_values("zz-widget", "zzBrandNew"), None);
    assert!(p::PATTERN_EVENT_PROPS["zz-widget"].contains("zzFire"));
    assert!(!p::PATTERN_EVENT_PROPS["zz-widget"].contains("zzBrandNew"));
    assert_eq!(
        p::PATTERN_EVENT_LIST_PROPS["zz-widget"]["itemActions"],
        "on"
    );
    assert!(
        p::is_valid_item_action_prop("zzTone"),
        "declared itemActions item props are accepted"
    );
    assert!(
        p::is_valid_item_action_prop("label"),
        "base item-action props stay"
    );
    assert!(!p::is_valid_item_action_prop("condition"));
}
