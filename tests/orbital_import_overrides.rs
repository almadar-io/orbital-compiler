//! The orbital-import body additions that retire the factory parameter layer:
//! persistence + collection on the `entity` line, `retype { … }`, and per-trait
//! `traits { Up { … } }` overrides. Each positive case has a paired control, and each
//! new error code has a firing case.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::orbital_ref_config::validate_orbital_ref_diagnostics;
use orbital_compiler::phases::validation::orbital_ref_extend::validate_orbital_ref_extend_diagnostics;
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, EventScope, FieldDefinition, FieldType, OrbitalSchema,
    Persistence, SExpression, TraitDefinition, TraitReference,
};

fn upstream(persistence: &str) -> String {
    format!(
        r#"{{
  "name": "test-overrides-upstream",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "NoteOrbital",
      "entity": {{
        "name": "Note",
        "persistence": "{persistence}",
        "collection": "upstream_notes",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "title", "type": "string", "description": "the headline" }},
          {{ "name": "count", "type": "number", "default": 1, "mock": "1, 2, 3" }}
        ]
      }},
      "traits": [
        {{
          "name": "NoteList",
          "linkedEntity": "Note",
          "category": "interaction",
          "config": {{ "limit": {{ "type": "number", "default": 10 }} }},
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "OPEN" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "OPEN", "effects": [ ["render-ui", "main", {{ "type": "typography", "content": "@config.limit" }}], ["render-ui", "main", {{ "type": "typography", "content": "@entity.title" }}] ] }} ]
          }},
          "emits": [ {{ "event": "OPENED", "scope": "external" }} ],
          "listens": [ {{ "event": "SAVED", "triggers": "OPEN" }} ]
        }},
        {{
          "name": "NoteForm",
          "linkedEntity": "Note",
          "category": "interaction",
          "config": {{ "limit": {{ "type": "number", "default": 5 }} }},
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [ {{ "key": "OPEN" }} ],
            "transitions": [ {{ "from": "idle", "to": "idle", "event": "OPEN", "effects": [ ["render-ui", "main", {{ "type": "typography", "content": "@config.limit" }}], ["render-ui", "main", {{ "type": "typography", "content": "@entity.title" }}] ] }} ]
          }},
          "emits": [ {{ "event": "SAVED", "scope": "external" }} ],
          "listens": [ {{ "event": "SAVED", "triggers": "OPEN" }} ]
        }}
      ],
      "pages": [ {{ "name": "NotesPage", "path": "/notes", "traits": [ {{ "ref": "NoteList" }}, {{ "ref": "NoteForm" }} ] }} ]
    }}
  ]
}}"#
    )
}

fn consumer(reference_extra: &str) -> String {
    format!(
        r#"{{
  "name": "test-overrides-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "Account",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.NoteOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.NoteOrbital"{reference_extra} }}
    }}
  ]
}}"#
    )
}

fn inline_with(persistence: &str, reference_extra: &str) -> (OrbitalSchema, Vec<String>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), upstream(persistence)).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(reference_extra)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let (mut trait_ref, mut orbital_ref, mut uses_config) = (Vec::new(), Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut trait_ref,
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let codes = validate_orbital_ref_diagnostics(&orbital_ref, &schema)
        .errors
        .iter()
        .chain(
            validate_orbital_ref_extend_diagnostics(&orbital_ref)
                .errors
                .iter(),
        )
        .map(|e| e.code.as_str().to_string())
        .collect();
    (schema, codes)
}

fn inline(reference_extra: &str) -> (OrbitalSchema, Vec<String>) {
    inline_with("persistent", reference_extra)
}

fn entity(schema: &OrbitalSchema) -> &EntityDefinition {
    let EntityRef::Inline(def) = &schema.orbitals[0].entity else {
        panic!("materialized entity");
    };
    def
}

fn field<'a>(schema: &'a OrbitalSchema, name: &str) -> &'a FieldDefinition {
    entity(schema)
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("field {name}"))
}

fn trait_def<'a>(schema: &'a OrbitalSchema, name: &str) -> &'a TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == name => Some(td),
            _ => None,
        })
        .unwrap_or_else(|| panic!("trait {name}"))
}

fn limit_default(td: &TraitDefinition) -> Option<SExpression> {
    td.config
        .as_ref()
        .and_then(|c| c.get("limit"))
        .and_then(|f| f.default.clone())
}

fn first_event(td: &TraitDefinition) -> String {
    td.state_machine.transitions[0].event.clone()
}

fn fires(codes: &[String], code: &str) -> bool {
    codes.iter().any(|c| c == code)
}

// ---- entity line: persistence + collection --------------------------------

#[test]
fn entity_override_changes_persistence_and_collection() {
    let (schema, codes) = inline(r#", "persistence": "persistent", "collection": "scratch_notes""#);
    assert!(codes.is_empty(), "{codes:?}");
    let e = entity(&schema);
    assert_eq!(e.persistence, Some(Persistence::Persistent));
    assert_eq!(e.collection.as_deref(), Some("scratch_notes"));
}

#[test]
fn entity_override_collection_alone_and_flags() {
    let (schema, codes) =
        inline(r#", "collection": "only_collection", "browser": true, "shared": true"#);
    assert!(codes.is_empty(), "{codes:?}");
    let e = entity(&schema);
    assert_eq!(e.collection.as_deref(), Some("only_collection"));
    assert_eq!(e.persistence, Some(Persistence::Persistent));
    assert!(e.browser && e.shared && !e.identity);
}

#[test]
fn control_without_entity_override_the_upstream_persistence_and_collection_stay() {
    let (schema, codes) = inline("");
    assert!(codes.is_empty(), "{codes:?}");
    let e = entity(&schema);
    assert_eq!(e.persistence, Some(Persistence::Persistent));
    assert_eq!(e.collection.as_deref(), Some("upstream_notes"));
    assert!(!e.browser && !e.shared && !e.identity);
}

#[test]
fn a_persistence_override_on_a_runtime_upstream_entity_is_refused_not_ignored() {
    let (schema, codes) = inline_with(
        "runtime",
        r#", "persistence": "persistent", "collection": "x""#,
    );
    assert!(
        fires(&codes, "ORB_O_ENTITY_PERSISTENCE_LOCKED"),
        "{codes:?}"
    );
    let e = entity(&schema);
    assert_eq!(e.persistence, Some(Persistence::Runtime));
    assert_eq!(e.collection.as_deref(), Some("upstream_notes"));
}

#[test]
fn a_collection_only_override_on_a_runtime_upstream_entity_is_refused_too() {
    let (_, codes) = inline_with("runtime", r#", "collection": "x""#);
    assert!(
        fires(&codes, "ORB_O_ENTITY_PERSISTENCE_LOCKED"),
        "{codes:?}"
    );
}

#[test]
fn control_a_runtime_upstream_without_override_is_clean() {
    let (schema, codes) = inline_with("runtime", "");
    assert!(
        !fires(&codes, "ORB_O_ENTITY_PERSISTENCE_LOCKED"),
        "{codes:?}"
    );
    assert_eq!(entity(&schema).persistence, Some(Persistence::Runtime));
}

// ---- retype -----------------------------------------------------------------

#[test]
fn retype_replaces_the_type_and_default_and_keeps_docs_and_mock() {
    let (schema, codes) =
        inline(r#", "retype": [ { "name": "count", "type": "string", "default": "zero" } ]"#);
    assert!(codes.is_empty(), "{codes:?}");
    let f = field(&schema, "count");
    assert_eq!(f.field_type, FieldType::String);
    assert_eq!(f.default, Some(SExpression::String("zero".to_string())));
    assert_eq!(f.mock.as_deref(), Some("1, 2, 3"), "mock carries over");
    assert_eq!(entity(&schema).fields.len(), 3, "no field added or dropped");
    assert_eq!(
        field(&schema, "title").description.as_deref(),
        Some("the headline")
    );
}

#[test]
fn retype_keeps_the_upstream_description_when_the_new_field_has_none() {
    let (schema, _) =
        inline(r#", "retype": [ { "name": "title", "type": "string", "default": "t" } ]"#);
    assert_eq!(
        field(&schema, "title").description.as_deref(),
        Some("the headline")
    );
}

#[test]
fn control_without_retype_the_upstream_field_is_untouched() {
    let (schema, _) = inline("");
    let f = field(&schema, "count");
    assert_eq!(f.field_type, FieldType::Number);
    assert_eq!(f.default, Some(SExpression::Number(1.0)));
}

#[test]
fn retype_names_the_field_after_fields_renames() {
    let (schema, codes) = inline(
        r#", "fields": { "count": "total" }, "retype": [ { "name": "total", "type": "string" } ]"#,
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(field(&schema, "total").field_type, FieldType::String);

    let (_, codes) = inline(
        r#", "fields": { "count": "total" }, "retype": [ { "name": "count", "type": "string" } ]"#,
    );
    assert!(
        fires(&codes, "ORB_O_RETYPE_UNKNOWN_FIELD"),
        "pre-rename name is unknown: {codes:?}"
    );
}

#[test]
fn retype_of_an_unknown_field_is_diagnosed() {
    let (schema, codes) = inline(r#", "retype": [ { "name": "nope", "type": "string" } ]"#);
    assert!(fires(&codes, "ORB_O_RETYPE_UNKNOWN_FIELD"), "{codes:?}");
    assert_eq!(
        entity(&schema).fields.len(),
        3,
        "an unknown retype adds nothing"
    );
}

#[test]
fn a_field_both_extended_and_retyped_is_an_error() {
    let (_, codes) = inline(
        r#", "extend": [ { "name": "tier", "type": "string" } ], "retype": [ { "name": "tier", "type": "number" } ]"#,
    );
    assert!(fires(&codes, "ORB_O_RETYPE_FIELD_CONFLICT"), "{codes:?}");
}

#[test]
fn a_field_retyped_twice_is_an_error() {
    let (_, codes) = inline(
        r#", "retype": [ { "name": "count", "type": "string" }, { "name": "count", "type": "boolean" } ]"#,
    );
    assert!(fires(&codes, "ORB_O_RETYPE_FIELD_CONFLICT"), "{codes:?}");
}

#[test]
fn extend_still_collides_with_an_existing_field_unchanged() {
    let (schema, codes) = inline(r#", "extend": [ { "name": "count", "type": "string" } ]"#);
    assert!(fires(&codes, "ORB_O_EXTEND_FIELD_COLLISION"), "{codes:?}");
    assert!(!fires(&codes, "ORB_O_RETYPE_FIELD_CONFLICT"), "{codes:?}");
    assert_eq!(
        field(&schema, "count").field_type,
        FieldType::Number,
        "the upstream field stays"
    );
}

#[test]
fn mock_applies_after_retype_and_overrides_the_carried_mock() {
    let (schema, codes) = inline(
        r#", "retype": [ { "name": "count", "type": "string" } ], "mock": { "count": "a, b" }"#,
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(field(&schema, "count").mock.as_deref(), Some("a, b"));
}

// ---- traits {} --------------------------------------------------------------

const LIST_OVERRIDE: &str = r#", "traits": { "NoteList": {
    "config": { "limit": { "type": "unknown", "default": 50 } },
    "events": { "OPEN": "VIEW" },
    "emitsScope": "internal"
} }"#;

#[test]
fn per_trait_overrides_apply_to_the_named_trait_only() {
    let (schema, codes) = inline(LIST_OVERRIDE);
    assert!(codes.is_empty(), "{codes:?}");
    let list = trait_def(&schema, "AccountNoteList");
    let form = trait_def(&schema, "AccountNoteForm");

    assert_eq!(limit_default(list), Some(SExpression::Number(50.0)));
    assert_eq!(first_event(list), "VIEW");
    let emit = &list.emits.as_ref().expect("emits")[0];
    assert_eq!(emit.scope, Some(EventScope::Internal));

    assert_eq!(
        limit_default(form),
        Some(SExpression::Number(5.0)),
        "sibling config untouched"
    );
    assert_eq!(first_event(form), "OPEN", "sibling events untouched");
    let form_emit = &form.emits.as_ref().expect("emits")[0];
    assert_eq!(
        form_emit.scope,
        Some(EventScope::External),
        "sibling emit scope untouched"
    );
}

#[test]
fn control_without_traits_overrides_every_trait_keeps_its_upstream_surface() {
    let (schema, codes) = inline("");
    assert!(codes.is_empty(), "{codes:?}");
    let list = trait_def(&schema, "AccountNoteList");
    let form = trait_def(&schema, "AccountNoteForm");
    assert_eq!(limit_default(list), Some(SExpression::Number(10.0)));
    assert_eq!(limit_default(form), Some(SExpression::Number(5.0)));
    assert_eq!(first_event(list), "OPEN");
    assert_eq!(
        list.emits.as_ref().expect("emits")[0].scope,
        Some(EventScope::External)
    );
}

#[test]
fn per_trait_listens_replace_the_traits_listens() {
    let (schema, codes) = inline(
        r#", "traits": { "NoteList": { "listens": [ { "event": "PING", "triggers": "OPEN" } ] } }"#,
    );
    assert!(codes.is_empty(), "{codes:?}");
    let list = trait_def(&schema, "AccountNoteList");
    let events: Vec<&str> = list
        .listens
        .iter()
        .flatten()
        .map(|l| l.event.as_str())
        .collect();
    assert_eq!(events, vec!["PING"]);
    let form = trait_def(&schema, "AccountNoteForm");
    let form_events: Vec<&str> = form
        .listens
        .iter()
        .flatten()
        .map(|l| l.event.as_str())
        .collect();
    assert_eq!(form_events, vec!["SAVED"]);
}

fn effects_json(td: &TraitDefinition) -> String {
    serde_json::to_string(&td.state_machine.transitions[0].effects).expect("serializes")
}

#[test]
fn per_trait_fields_rename_applies_to_that_trait_only() {
    let (schema, codes) =
        inline(r#", "traits": { "NoteList": { "fields": { "title": "subject" } } }"#);
    assert!(codes.is_empty(), "{codes:?}");
    let list = effects_json(trait_def(&schema, "AccountNoteList"));
    let form = effects_json(trait_def(&schema, "AccountNoteForm"));
    assert!(
        list.contains("@entity.subject") && !list.contains("@entity.title"),
        "{list}"
    );
    assert!(
        form.contains("@entity.title") && !form.contains("@entity.subject"),
        "{form}"
    );
}

#[test]
fn control_without_a_per_trait_fields_rename_both_traits_read_the_upstream_field() {
    let (schema, _) = inline("");
    for name in ["AccountNoteList", "AccountNoteForm"] {
        assert!(
            effects_json(trait_def(&schema, name)).contains("@entity.title"),
            "{name}"
        );
    }
}

#[test]
fn a_traits_key_naming_no_upstream_trait_is_diagnosed() {
    let (_, codes) = inline(r#", "traits": { "Nope": { "events": { "A": "B" } } }"#);
    assert!(fires(&codes, "ORB_O_TRAITS_UNKNOWN_TRAIT"), "{codes:?}");
}

#[test]
fn a_prefixed_trait_name_is_not_an_upstream_name() {
    let (_, codes) = inline(r#", "traits": { "AccountNoteList": { "events": { "A": "B" } } }"#);
    assert!(fires(&codes, "ORB_O_TRAITS_UNKNOWN_TRAIT"), "{codes:?}");
}

#[test]
fn a_traits_key_that_is_also_omitted_is_an_error() {
    let (_, codes) = inline(
        r#", "omit": [ "NoteForm" ], "traits": { "NoteForm": { "events": { "OPEN": "VIEW" } } }"#,
    );
    assert!(fires(&codes, "ORB_O_TRAITS_OMITTED_TRAIT"), "{codes:?}");
    assert!(!fires(&codes, "ORB_O_TRAITS_UNKNOWN_TRAIT"), "{codes:?}");
}

#[test]
fn a_traits_key_dropped_by_only_is_an_error() {
    let (_, codes) = inline(
        r#", "only": [ "NoteList" ], "traits": { "NoteForm": { "events": { "OPEN": "VIEW" } } }"#,
    );
    assert!(fires(&codes, "ORB_O_TRAITS_OMITTED_TRAIT"), "{codes:?}");
}

#[test]
fn control_omitting_a_trait_without_overriding_it_raises_no_traits_error() {
    let (_, codes) = inline(
        r#", "omit": [ "NoteForm" ], "traits": { "NoteList": { "events": { "OPEN": "VIEW" } } }"#,
    );
    assert!(!fires(&codes, "ORB_O_TRAITS_OMITTED_TRAIT"), "{codes:?}");
}

#[test]
fn a_trait_config_key_the_trait_never_declares_is_diagnosed() {
    let (_, codes) = inline(
        r#", "traits": { "NoteList": { "config": { "bogus": { "type": "unknown", "default": 1 } } } }"#,
    );
    assert!(fires(&codes, "ORB_O_CONFIG_UNKNOWN_KEY"), "{codes:?}");
}

#[test]
fn a_config_key_the_trait_declares_raises_no_error() {
    let (_, codes) = inline(
        r#", "traits": { "NoteList": { "config": { "limit": { "type": "unknown", "default": 7 } } } }"#,
    );
    assert!(codes.is_empty(), "NoteList declares limit: {codes:?}");
}

#[test]
fn per_trait_overrides_compose_with_orbital_level_events() {
    let (schema, codes) = inline(
        r#", "events": { "OPENED": "WAS_OPENED" }, "traits": { "NoteList": { "events": { "OPEN": "VIEW" } } }"#,
    );
    assert!(codes.is_empty(), "{codes:?}");
    let list = trait_def(&schema, "AccountNoteList");
    assert_eq!(first_event(list), "VIEW");
    assert_eq!(list.emits.as_ref().expect("emits")[0].event, "WAS_OPENED");
}

// ---------------------------------------------------------------------------------------------
// An imported orbital's obligations follow the import's renames (G-ORB-378): the trait prefix, the
// event renames, the page remap and the omitted traits.
// ---------------------------------------------------------------------------------------------

const OBLIGATED_UPSTREAM: &str = r#"{
  "name": "test-obligated-upstream", "version": "1.0.0",
  "orbitals": [{
    "name": "CounterOrbital",
    "entity": { "name": "Counter", "persistence": "runtime", "fields": [
      { "name": "id", "type": "string", "required": true },
      { "name": "count", "type": "number", "default": 0 }
    ] },
    "expects": [
      { "kind": "entity", "name": "Counter", "invariants": [[">=", "@entity.count", 0]], "strict": true },
      { "kind": "state", "traitName": "Count", "state": "idle", "holds": [">=", "@entity.count", 0], "strict": true },
      { "kind": "transition", "traitName": "Count", "from": "idle", "event": "BUMP", "to": "idle", "strict": true },
      { "kind": "event", "traitName": "Count", "event": "BUMPED", "strict": true },
      { "kind": "state", "traitName": "Sibling", "state": "idle", "strict": true },
      { "kind": "page", "path": "/counter", "traits": ["Count"], "strict": true }
    ],
    "traits": [
      { "name": "Count", "linkedEntity": "Counter", "category": "interaction",
        "stateMachine": {
          "states": [{ "name": "idle", "isInitial": true }],
          "events": [{ "key": "BUMP" }],
          "transitions": [{ "from": "idle", "to": "idle", "event": "BUMP",
            "effects": [["set", "@entity.count", ["+", "@entity.count", 1]], ["emit", "BUMPED", {}]] }]
        },
        "emits": [{ "event": "BUMPED", "scope": "external" }] },
      { "name": "Sibling", "linkedEntity": "Counter", "category": "interaction",
        "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [], "transitions": [] } }
    ],
    "pages": [{ "name": "CounterPage", "path": "/counter", "traits": [{ "ref": "Count" }] }]
  }]
}"#;

fn imported_obligations(reference_extra: &str) -> (OrbitalSchema, Vec<serde_json::Value>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), OBLIGATED_UPSTREAM).expect("write upstream");
    let consumer = format!(
        r#"{{
  "name": "test-obligated-consumer", "version": "1.0.0",
  "orbitals": [{{
    "name": "Local",
    "uses": [{{ "from": "./upstream.orb", "as": "Up" }}],
    "entity": "Up.orbitals.CounterOrbital.entity",
    "traits": [],
    "reference": {{ "ref": "Up.orbitals.CounterOrbital"{reference_extra} }}
  }}]
}}"#
    );
    let path = dir.path().join("consumer.orb");
    std::fs::write(&path, consumer).expect("write consumer");
    let mut schema = parse_file(&path).expect("consumer parses");
    let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(&mut schema, &InlineConfig::default(), Some(&path), &mut a, &mut b, &mut c)
        .expect("inline succeeds");
    let expects = schema.orbitals[0].expects.iter().flatten().map(|e| serde_json::to_value(e).unwrap()).collect();
    (schema, expects)
}

fn trait_ending(schema: &OrbitalSchema, suffix: &str) -> Option<String> {
    schema.orbitals[0].traits.iter().find_map(|t| match t {
        TraitReference::Inline(td) if td.name.ends_with(suffix) => Some(td.name.clone()),
        _ => None,
    })
}

#[test]
fn an_imported_orbitals_obligations_follow_the_import() {
    let (schema, all) = imported_obligations(r#", "events": { "BUMP": "ADD", "BUMPED": "ADDED" }, "pages": { "/counter": "/tally" }, "omit": ["Sibling"]"#);
    let count = trait_ending(&schema, "Count").expect("the imported Count trait");
    assert!(trait_ending(&schema, "Sibling").is_none(), "omitted");
    let by_kind = |k: &str| all.iter().filter(|e| e["kind"] == k).collect::<Vec<_>>();
    let states = by_kind("state");
    assert_eq!(states.len(), 1, "the omitted sibling's obligation does not travel: {all:#?}");
    assert_eq!(states[0]["traitName"], count.as_str());
    let transition = by_kind("transition");
    assert_eq!((transition[0]["traitName"].as_str(), transition[0]["event"].as_str()), (Some(count.as_str()), Some("ADD")));
    let event = by_kind("event");
    assert_eq!((event[0]["traitName"].as_str(), event[0]["event"].as_str()), (Some(count.as_str()), Some("ADDED")));
    let page = by_kind("page");
    assert_eq!(page[0]["path"], "/tally");
    assert_eq!(page[0]["traits"], serde_json::json!([count]));
    let EntityRef::Inline(local) = &schema.orbitals[0].entity else { panic!("materialized entity") };
    assert_eq!(by_kind("entity")[0]["name"], local.name.as_str());
}

#[test]
fn control_an_import_without_renames_owes_its_obligations_under_the_imported_names() {
    let (schema, all) = imported_obligations("");
    let count = trait_ending(&schema, "Count").unwrap();
    let sibling = trait_ending(&schema, "Sibling").unwrap();
    let names: Vec<&str> = all.iter().filter(|e| e["kind"] == "state").filter_map(|e| e["traitName"].as_str()).collect();
    assert_eq!(names, vec![count.as_str(), sibling.as_str()]);
    assert_eq!(all.iter().find(|e| e["kind"] == "page").unwrap()["path"], "/counter");
}
