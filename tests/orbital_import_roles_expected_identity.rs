//! An atom with no roster of its own declares its viewer with `expects entity Person [identity]
//! { role : "instructor" | "student" | "admin" }`. Its import's `roles {}` remaps those role
//! words onto the consumer's roster, as it does for an upstream that declares the roster:
//! the expectation's role values are the upstream vocabulary.

use orbital_compiler::phases::inline::{inline_schema_with_orbital_diagnostics, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::orbital_ref_config::validate_orbital_ref_diagnostics;
use orbital_core_base::schema::{EntityRef, OrbitalSchema};

const UPSTREAM: &str = r#"{
  "name": "test-roles-expected-identity-upstream",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "ProgressOrbital",
      "expects": [ { "kind": "identity", "name": "Person", "shape": [
        { "name": "id", "type": "string", "required": true },
        { "name": "role", "type": "string", "values": ["instructor", "student", "admin"] }
      ] } ],
      "entity": {
        "name": "Progress",
        "persistence": "persistent",
        "collection": "progresses",
        "read_policy": ["or", ["=", "@user.role", "admin"], ["=", ["object/get", "@entity", "studentId"], "@user.id"]],
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "studentId", "type": "relation", "relation": { "entity": "Person", "cardinality": "one" } }
        ]
      },
      "traits": [
        {
          "name": "ProgressList",
          "linkedEntity": "Progress",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [ { "from": "idle", "to": "idle", "event": "INIT", "effects": [ ["fetch", "Progress"], ["render-ui", "main", { "type": "stack" }] ] } ]
          }
        }
      ],
      "pages": [ { "name": "ProgressPage", "path": "/progress", "traits": [ { "ref": "ProgressList" } ] } ]
    }
  ]
}"#;

fn consumer(roles: &str) -> String {
    format!(
        r#"{{
  "name": "test-roles-expected-identity-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "MemberOrbital",
      "entity": {{
        "name": "Member",
        "persistence": "persistent",
        "identity": true,
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "role", "type": "string", "values": ["teacher", "pupil", "director"] }}
        ]
      }},
      "traits": [],
      "pages": []
    }},
    {{
      "name": "Grades",
      "uses": [ {{ "from": "./upstream.orb", "as": "Up" }} ],
      "entity": "Up.orbitals.ProgressOrbital.entity",
      "traits": [],
      "reference": {{ "ref": "Up.orbitals.ProgressOrbital", "entity": "Grade", "entities": {{ "Person": "Member" }}{roles} }}
    }}
  ]
}}"#
    )
}

fn inline(roles: &str) -> (OrbitalSchema, Vec<String>) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("upstream.orb"), UPSTREAM).expect("write upstream");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, consumer(roles)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    let (mut orbital_ref, mut uses_config) = (Vec::new(), Vec::new());
    inline_schema_with_orbital_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&consumer_path),
        &mut orbital_ref,
        &mut uses_config,
    )
    .expect("inline succeeds");
    let codes = validate_orbital_ref_diagnostics(&orbital_ref, &schema)
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect();
    (schema, codes)
}

fn grade_read_policy(schema: &OrbitalSchema) -> String {
    let grades = schema.orbitals.iter().find(|o| o.name == "Grades").expect("Grades");
    match &grades.entity {
        EntityRef::Inline(e) => serde_json::to_string(&e.read_policy).expect("json"),
        other => panic!("inline entity expected, got {other:?}"),
    }
}

#[test]
fn an_expected_identitys_role_words_are_remapped() {
    let (schema, errors) = inline(
        r#", "roles": { "admin": ["director"], "instructor": ["teacher"], "student": ["pupil"] }"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let policy = grade_read_policy(&schema);
    assert!(policy.contains("director"), "{policy}");
    assert!(!policy.contains("\"admin\""), "{policy}");
}

#[test]
fn control_a_word_the_expectation_does_not_declare_is_still_unknown() {
    let (_, errors) = inline(r#", "roles": { "ghost": ["director"] }"#);
    assert!(errors.iter().any(|c| c == "ORB_O_ROLE_UNKNOWN_UPSTREAM"), "{errors:?}");
}

#[test]
fn control_without_a_remap_the_upstream_words_stay() {
    let (schema, errors) = inline("");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(grade_read_policy(&schema).contains("\"admin\""));
}
