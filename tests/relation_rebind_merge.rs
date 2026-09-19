//! Regression: `linkedEntity` rebind must retarget a self-referential
//! `relation.entity` when the imported entity's fields are auto-merged onto the
//! caller's entity. Before the fix, a variation entity's self-relation (e.g. a
//! thread's `replies : [ThreadLike]`) was merged verbatim onto the consumer
//! entity, leaving `relation.entity` pointing at the now-nonexistent imported
//! entity → ORB_E_INVALID_RELATION / ORB_X_UNRESOLVED_RELATION_TARGET. The
//! auto-merge now rewrites self-references (and nested object-property ones) to
//! the caller entity. The atom's trait reads `@entity.replies` in its
//! render-ui so the field counts as referenced under the
//! R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY usage gate — an unreferenced
//! field no longer merges at all, self-relation or not.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::schema::EntityRef;

const ATOM: &str = r#"{
  "name": "test-rel-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "RelAtomOrbital",
      "entity": {
        "name": "ThreadLike",
        "persistence": "persistent",
        "fields": [
          { "name": "id", "type": "string", "required": true },
          { "name": "replies", "type": "relation", "relation": { "entity": "ThreadLike", "cardinality": "many" } }
        ]
      },
      "traits": [
        {
          "name": "RelAtomTrait",
          "linkedEntity": "ThreadLike",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "LOAD" } ],
            "transitions": [
              {
                "from": "idle",
                "to": "idle",
                "event": "LOAD",
                "effects": [ ["render-ui", "main", { "type": "list", "items": "@entity.replies" }] ]
              }
            ]
          }
        }
      ]
    }
  ]
}"#;

const CONSUMER: &str = r#"{
  "name": "test-rel-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "RelConsumerOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Atom" } ],
      "entity": {
        "name": "ConsumerRec",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true } ]
      },
      "traits": [
        { "ref": "Atom.traits.RelAtomTrait", "linkedEntity": "ConsumerRec" }
      ]
    }
  ]
}"#;

#[test]
fn linked_entity_rebind_retargets_self_referential_relation() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");

    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");

    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("consumer entity should resolve to an inline definition");
    };
    let replies = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("replies"))
        .expect("the imported self-referential `replies` relation must be merged onto the consumer entity");
    let rel = replies
        .relation
        .as_ref()
        .expect("`replies` must still be a relation field after merge");
    assert_eq!(
        rel.entity, "ConsumerRec",
        "the self-referential relation target must be retargeted to the caller entity \
         (got {:?}); leaving it at the imported entity name dangles the relation",
        rel.entity
    );
}
