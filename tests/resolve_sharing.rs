//! One program resolve shares its loaded files and each imported behavior's
//! inlined result across every orbital and every lazy behavior. Sharing must
//! change nothing: each piece resolves exactly as it would alone.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::lazy::{resolve_program_with_path, split_lazy};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::{resolve_schema_with_path, CompilerOptions};
use orbital_core_base::schema::{Orbital, OrbitalSchema};
use std::path::{Path, PathBuf};

const LEAF: &str = r#"{
  "name": "leaf",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Leaf",
      "entity": { "name": "LeafState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "LeafView", "linkedEntity": "LeafState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Leaf" }] ] } ] } }
      ],
      "pages": [ { "name": "LeafPage", "path": "/leaf", "traits": [ { "ref": "LeafView" } ] } ]
    }
  ]
}"#;

/// `post` imports `leaf` with a call-site config knob `leaf` never declares,
/// so every inline of `post` records one `uses` config site.
const POST: &str = r#"{
  "name": "post",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Post",
      "uses": [ { "from": "./leaf.orb", "as": "L", "config": { "bogus": { "type": "number", "default": 1 } } } ],
      "entity": { "name": "PostState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "PostView", "linkedEntity": "PostState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Why we started" }] ] } ] } }
      ],
      "pages": [ { "name": "PostPage", "path": "/post", "traits": [ { "ref": "PostView" } ] } ]
    }
  ]
}"#;

const OTHER: &str = r#"{
  "name": "other",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "Other",
      "entity": { "name": "OtherState", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
      "traits": [
        { "name": "OtherView", "linkedEntity": "OtherState", 
          "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "events": [],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["render-ui", "main", { "type": "typography", "content": "Other" }] ] } ] } }
      ],
      "pages": [ { "name": "OtherPage", "path": "/other", "traits": [ { "ref": "OtherView" } ] } ]
    }
  ]
}"#;

fn reference(
    name: &str,
    from: &str,
    alias: &str,
    upstream: &str,
    page_from: &str,
    page_to: &str,
    lazy: bool,
) -> String {
    format!(
        r#"{{
      "name": "{name}",
      "uses": [ {{ "from": "{from}", "as": "{alias}"{lazy} }} ],
      "entity": "{alias}.orbitals.{upstream}.entity",
      "traits": [],
      "reference": {{ "ref": "{alias}.orbitals.{upstream}", "pages": {{ "{page_from}": "{page_to}" }} }}
    }}"#,
        lazy = if lazy { r#", "lazy": true"# } else { "" }
    )
}

struct Program {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn program(orbitals: &[String], locales: bool) -> Program {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (file, body) in [("leaf.orb", LEAF), ("post.orb", POST), ("other.orb", OTHER)] {
        std::fs::write(dir.path().join(file), body).expect("write behavior");
    }
    let path = dir.path().join("site.orb");
    let locales = if locales {
        r#", "locales": ["en"]"#
    } else {
        ""
    };
    let body = format!(
        r#"{{ "name": "site", "version": "1.0.0"{locales}, "orbitals": [ {} ] }}"#,
        orbitals.join(",")
    );
    std::fs::write(&path, body).expect("write site");
    Program { _dir: dir, path }
}

fn resolved(path: &Path) -> OrbitalSchema {
    resolve_schema_with_path(
        parse_file(path).expect("parses"),
        &CompilerOptions::default(),
        Some(path),
    )
    .expect("resolves")
}

/// Fixtures carry no identity ledger, so each resolve mints fresh ids; compare everything else.
fn without_ids(value: serde_json::Value) -> serde_json::Value {
    fn is_identity_key(k: &str) -> bool {
        k == "id"
            || k == "sourcePage"
            || k == "ledger"
            || k == "schemaVersion"
            || k.ends_with("Id")
            || k.ends_with("Ids")
    }
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(k, _)| !is_identity_key(k))
                .map(|(k, v)| (k, without_ids(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(without_ids).collect())
        }
        other => other,
    }
}

/// The orbital as JSON, its imports written relative to the program's directory: resolution anchors a relative import to
/// the file it names (G-CROSS-358), and two programs in different directories name the same sibling.
fn orbital_json(schema: &OrbitalSchema, name: &str, program: &Path) -> serde_json::Value {
    let orbital: &Orbital = schema
        .orbitals
        .iter()
        .find(|o| o.name == name)
        .expect("orbital present");
    let dir = format!("{}/", std::path::absolute(program.parent().expect("a program directory")).expect("absolute").display());
    let mut json = without_ids(serde_json::to_value(orbital).expect("serializes"));
    for decl in json["uses"].as_array_mut().into_iter().flatten() {
        if let Some(from) = decl["from"].as_str().and_then(|f| f.strip_prefix(&dir)).map(|rest| format!("./{rest}")) {
            decl["from"] = serde_json::Value::String(from);
        }
    }
    json
}

fn blog(name: &str, to: &str) -> String {
    reference(name, "./post.orb", "Why", "Post", "/post", to, false)
}

#[test]
fn an_orbital_that_reuses_an_import_resolves_as_it_would_alone() {
    let both = program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false);
    let only_b = program(&[blog("BlogB", "/b")], false);
    assert_eq!(
        orbital_json(&resolved(&both.path), "BlogB", &both.path),
        orbital_json(&resolved(&only_b.path), "BlogB", &only_b.path)
    );
}

#[test]
fn control_the_first_user_of_an_import_is_unchanged_too() {
    let both = program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false);
    let only_a = program(&[blog("BlogA", "/a")], false);
    assert_eq!(
        orbital_json(&resolved(&both.path), "BlogA", &both.path),
        orbital_json(&resolved(&only_a.path), "BlogA", &only_a.path)
    );
}

fn uses_config_site_count(p: &Program) -> usize {
    let mut schema = parse_file(&p.path).expect("parses");
    let (mut orbital_diags, mut uses_diags) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&p.path),
        &mut orbital_diags,
        &mut uses_diags,
    )
    .expect("inlines");
    uses_diags.iter().filter(|d| d.alias == "L").count()
}

#[test]
fn every_use_of_an_import_still_records_its_uses_config_sites() {
    let one = uses_config_site_count(&program(&[blog("BlogA", "/a")], false));
    assert!(
        one >= 1,
        "post's own `uses` config call site is recorded"
    );
    let two =
        uses_config_site_count(&program(&[blog("BlogA", "/a"), blog("BlogB", "/b")], false));
    assert_eq!(two, 2 * one);
}

fn lazy_program() -> Program {
    program(
        &[
            reference("BlogA", "./post.orb", "Why", "Post", "/post", "/a", true),
            reference("BlogB", "./post.orb", "Why", "Post", "/post", "/b", true),
            reference(
                "Elsewhere",
                "./other.orb",
                "O",
                "Other",
                "/other",
                "/elsewhere",
                true,
            ),
        ],
        true,
    )
}

#[test]
fn each_lazy_behavior_resolves_as_it_would_alone() {
    let p = lazy_program();
    let (_, lazy) = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .expect("resolves");
    let mut split_source = parse_file(&p.path).unwrap();
    for artifact in split_lazy(&mut split_source) {
        let alone = resolve_schema_with_path(
            artifact.schema.clone(),
            &CompilerOptions::default(),
            Some(&p.path),
        )
        .expect("alone");
        let shared = lazy
            .iter()
            .find(|l| l.orbital == artifact.orbital)
            .expect("resolved lazily");
        assert_eq!(
            without_ids(serde_json::to_value(&shared.schema).unwrap()),
            without_ids(serde_json::to_value(&alone).unwrap()),
            "{}",
            artifact.orbital
        );
    }
}

#[test]
fn a_lazy_behavior_lists_only_the_catalogs_it_loaded() {
    let p = lazy_program();
    let (_, lazy) = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .expect("resolves");
    let sources = |name: &str| {
        let item = lazy
            .iter()
            .find(|l| l.orbital == name)
            .expect("lazy behavior");
        item.schema
            .metadata
            .as_ref()
            .and_then(|m| m.message_sources.clone())
            .expect("message sources")
    };
    let b = sources("BlogB");
    assert!(
        b.contains_key("post") && b.contains_key("leaf"),
        "the second user of post still lists post and leaf: {b:?}"
    );
    let elsewhere = sources("Elsewhere");
    assert!(elsewhere.contains_key("other"), "{elsewhere:?}");
    assert!(
        !elsewhere.contains_key("post") && !elsewhere.contains_key("leaf"),
        "no catalogs from other lazy behaviors: {elsewhere:?}"
    );
}

#[test]
fn resolution_puts_each_programs_own_messages_in_it() {
    let p = lazy_program();
    let dir = p.path.parent().unwrap();
    std::fs::create_dir_all(dir.join("locales/en")).unwrap();
    std::fs::write(dir.join("locales/en/post.json"), r#"{"title":"Why"}"#).unwrap();
    std::fs::write(dir.join("locales/en/other.json"), r#"{"title":"Elsewhere"}"#).unwrap();
    let (_, lazy) = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .expect("resolves");
    let messages = |name: &str| {
        lazy.iter()
            .find(|l| l.orbital == name)
            .and_then(|l| l.schema.messages.clone())
            .expect("messages")
    };
    assert_eq!(messages("BlogA")["en"].get("post:title").map(String::as_str), Some("Why"));
    let elsewhere = messages("Elsewhere");
    assert_eq!(elsewhere["en"].get("other:title").map(String::as_str), Some("Elsewhere"));
    assert!(elsewhere["en"].get("post:title").is_none(), "control: no text from another lazy behavior: {elsewhere:?}");
}

#[test]
fn edge_a_catalog_that_does_not_parse_fails_the_resolve_naming_it() {
    let p = lazy_program();
    let dir = p.path.parent().unwrap();
    std::fs::create_dir_all(dir.join("locales/en")).unwrap();
    std::fs::write(dir.join("locales/en/post.json"), "{ broken").unwrap();
    let error = resolve_program_with_path(
        parse_file(&p.path).unwrap(),
        &CompilerOptions::default(),
        Some(&p.path),
    )
    .err()
    .expect("a broken catalog fails");
    assert!(error.to_string().contains("post.json"), "{error}");
}

/// A lazy-resolve cache that holds its entries in memory and counts each store.
#[derive(Default)]
struct MemoryLazyCache {
    entries: std::sync::Mutex<std::collections::HashMap<String, orbital_compiler::phases::lazy::CachedLazy>>,
    stores: std::sync::atomic::AtomicUsize,
}

impl orbital_compiler::phases::lazy::LazyResolveCache for MemoryLazyCache {
    fn get(&self, key: &str) -> Option<orbital_compiler::phases::lazy::CachedLazy> {
        self.entries.lock().unwrap().get(key).cloned()
    }
    fn put(&self, _slot: &str, key: &str, entry: orbital_compiler::phases::lazy::CachedLazy) {
        self.stores.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entries.lock().unwrap().insert(key.to_string(), entry);
    }
}

impl MemoryLazyCache {
    fn stores(&self) -> usize {
        self.stores.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn reads_of(&self, orbital: &str) -> Vec<String> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.values().find(|e| e.resolved.orbital == orbital).expect("a cached lazy behavior");
        entry.reads.paths().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect()
    }
}

/// Two lazy blogs over `post` (which imports `leaf`), and an eager orbital over `other`.
fn lazy_blogs() -> Program {
    program(
        &[
            reference("BlogA", "./post.orb", "Why", "Post", "/post", "/a", true),
            reference("BlogB", "./post.orb", "Why", "Post", "/post", "/b", true),
            reference("Oth", "./other.orb", "O", "Other", "/other", "/oth", false),
        ],
        false,
    )
}

fn resolve_with(path: &Path, cache: Option<std::sync::Arc<MemoryLazyCache>>) -> (serde_json::Value, serde_json::Value) {
    let options = CompilerOptions {
        lazy_cache: orbital_compiler::phases::lazy::LazyCache(cache.map(|c| c as std::sync::Arc<dyn orbital_compiler::phases::lazy::LazyResolveCache>)),
        ..CompilerOptions::default()
    };
    let (schema, lazy) = resolve_program_with_path(parse_file(path).expect("parses"), &options, Some(path)).expect("resolves");
    let children: Vec<serde_json::Value> = lazy.iter().map(|l| without_ids(serde_json::to_value(&l.schema).expect("serializes"))).collect();
    (without_ids(serde_json::to_value(&schema).expect("serializes")), serde_json::Value::Array(children))
}

#[test]
fn a_cached_lazy_behavior_resolves_exactly_as_a_fresh_one_and_is_not_resolved_again() {
    let site = lazy_blogs();
    let cache = std::sync::Arc::new(MemoryLazyCache::default());
    let first = resolve_with(&site.path, Some(cache.clone()));
    assert_eq!(cache.stores(), 2, "each lazy behavior is resolved and stored once");
    let second = resolve_with(&site.path, Some(cache.clone()));
    assert_eq!(cache.stores(), 2, "nothing it read changed, so neither is resolved again");
    assert_eq!(first, second);
    assert_eq!(resolve_with(&site.path, None), first, "control: the same program without a cache resolves the same");
}

#[test]
fn a_lazy_behavior_records_every_file_it_resolved_from_even_when_a_sibling_loaded_it_first() {
    let site = lazy_blogs();
    let cache = std::sync::Arc::new(MemoryLazyCache::default());
    resolve_with(&site.path, Some(cache.clone()));
    for blog in ["BlogA", "BlogB"] {
        let reads = cache.reads_of(blog);
        assert!(reads.contains(&"post.orb".to_string()) && reads.contains(&"leaf.orb".to_string()), "{blog} read {reads:?}");
    }
}

#[test]
fn editing_a_file_a_lazy_behavior_read_resolves_it_again_and_an_unrelated_edit_does_not() {
    let site = lazy_blogs();
    let dir = site.path.parent().expect("a program directory").to_path_buf();
    let cache = std::sync::Arc::new(MemoryLazyCache::default());
    resolve_with(&site.path, Some(cache.clone()));
    std::fs::write(dir.join("other.orb"), OTHER.replace("\"Other\" }", "\"Other, edited\" }")).expect("edit other");
    resolve_with(&site.path, Some(cache.clone()));
    assert_eq!(cache.stores(), 2, "control: a file only the eager orbital read changes no lazy behavior");
    std::fs::write(dir.join("leaf.orb"), LEAF.replace("\"Leaf\" }", "\"Leaf, edited\" }")).expect("edit leaf");
    resolve_with(&site.path, Some(cache.clone()));
    assert_eq!(cache.stores(), 4, "both blogs read leaf, so both resolve again");
}

fn describing_stub(value: &serde_json::Value, _from: Option<&Path>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "value": value }))
}

/// `post` with a literal `behavior/describe` on INIT, which resolution answers at build time.
fn describing_post() -> String {
    POST.replace(
        r#""events": [],"#,
        r#""events": [ { "key": "DESCRIBED", "name": "Described" }, { "key": "DESCRIBE_FAILED", "name": "Describe failed" } ],"#,
    )
    .replace(
        r#"["render-ui", "main", { "type": "typography", "content": "Why we started" }] ]"#,
        r#"["render-ui", "main", { "type": "typography", "content": "Why we started" }], ["behavior/describe", { "behavior": "std/std-door", "trait": "Door" }, { "emit": { "success": "DESCRIBED", "failure": "DESCRIBE_FAILED" } }] ]"#,
    )
}

/// Whether `value` holds a `behavior/describe` call (an effect-row entry only names its kind).
fn holds_describe(value: &serde_json::Value) -> bool {
    value.to_string().contains(r#"["behavior/describe","#)
}

#[test]
fn a_cached_lazy_behavior_holds_what_the_resolve_answers_for_it_build_time_answers_included() {
    orbital_compiler::phases::reflect::register_trait_describer(describing_stub);
    let site = lazy_blogs();
    let post = describing_post();
    assert!(post.contains("behavior/describe") && post.contains("DESCRIBE_FAILED"), "the fixture edit applied");
    std::fs::write(site.path.parent().expect("a program directory").join("post.orb"), post).expect("write post");
    let cache = std::sync::Arc::new(MemoryLazyCache::default());
    let (_, children) = resolve_with(&site.path, Some(cache.clone()));
    assert!(!holds_describe(&children), "the resolve answers each lazy behavior's literal describe: {children}");
    let cached: Vec<serde_json::Value> = cache.entries.lock().unwrap().values().map(|e| serde_json::to_value(&e.resolved.schema).expect("serializes")).collect();
    assert_eq!(cached.len(), 2);
    assert!(cached.iter().all(|c| !holds_describe(c)), "what the cache holds is what the resolve answered, not the page before its build-time answers");
    assert_eq!(resolve_with(&site.path, Some(cache)).1, children, "a resolve served from the cache answers the same");
}

#[test]
fn control_a_lazy_behavior_without_a_describe_is_cached_as_before() {
    let site = lazy_blogs();
    let cache = std::sync::Arc::new(MemoryLazyCache::default());
    let (_, children) = resolve_with(&site.path, Some(cache.clone()));
    assert!(!holds_describe(&children));
    assert_eq!(cache.stores(), 2);
}

fn resolve_on(path: &Path, workers: usize) -> (serde_json::Value, serde_json::Value) {
    let options = CompilerOptions { lazy_workers: workers, ..CompilerOptions::default() };
    let (schema, lazy) = resolve_program_with_path(parse_file(path).expect("parses"), &options, Some(path)).expect("resolves");
    let children: Vec<serde_json::Value> = lazy.iter().map(|l| without_ids(serde_json::to_value(&l.schema).expect("serializes"))).collect();
    (without_ids(serde_json::to_value(&schema).expect("serializes")), serde_json::Value::Array(children))
}

#[test]
fn lazy_behaviors_resolved_on_several_threads_resolve_exactly_as_on_one() {
    let site = program(
        &[
            reference("BlogA", "./post.orb", "Why", "Post", "/post", "/a", true),
            reference("BlogB", "./post.orb", "Why", "Post", "/post", "/b", true),
            reference("OthA", "./other.orb", "O", "Other", "/other", "/oa", true),
            reference("OthB", "./other.orb", "O", "Other", "/other", "/ob", true),
        ],
        false,
    );
    assert_eq!(resolve_on(&site.path, 4), resolve_on(&site.path, 1), "the same program, its lazy behaviors in the same order");
}

#[test]
fn edge_a_lazy_behavior_that_fails_on_a_worker_fails_the_resolve_as_it_does_on_one_thread() {
    let site = program(
        &[
            reference("BlogA", "./post.orb", "Why", "Post", "/post", "/a", true),
            reference("Gone", "./missing.orb", "M", "Missing", "/m", "/gone", true),
        ],
        false,
    );
    let fail = |workers: usize| {
        let options = CompilerOptions { lazy_workers: workers, ..CompilerOptions::default() };
        resolve_program_with_path(parse_file(&site.path).expect("parses"), &options, Some(&site.path)).expect_err("a missing import fails").to_string()
    };
    assert_eq!(fail(4), fail(1));
}
