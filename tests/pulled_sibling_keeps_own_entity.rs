//! G-ORB-042: an embedder rebind retargets only the pulled siblings bound to
//! the embedder atom's OWN entity. A sibling bound to another entity of the
//! atom (std-helpdesk-ticket's TicketCorrespondence -> TicketMessage, embedded
//! by TicketManage -> Ticket) keeps its binding when a consumer rebinds the
//! embedder (`TicketManage -> SupportTicket`).

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core::schema::TraitReference;

const ATOM: &str = r#"{
  "name": "test-ticket-atom",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "TicketOrbital",
      "entity": {
        "name": "Ticket",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "subject", "type": "string" } ]
      },
      "auxiliaryEntities": [
        {
          "name": "TicketMessage",
          "persistence": "persistent",
          "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "content", "type": "string" } ]
        }
      ],
      "traits": [
        {
          "name": "TicketManage",
          "linkedEntity": "Ticket",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["render-ui", "main", { "type": "stack", "children": [ "@trait.TicketCorrespondence", "@trait.TicketBadge" ] }] ] }
            ]
          }
        },
        {
          "name": "TicketCorrespondence",
          "linkedEntity": "TicketMessage",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["fetch", "TicketMessage", { "emit": { "success": "INIT" } }] ] }
            ]
          }
        },
        {
          "name": "TicketBadge",
          "linkedEntity": "Ticket",
          "category": "interaction",
          "stateMachine": {
            "states": [ { "name": "idle", "isInitial": true } ],
            "events": [ { "key": "INIT" } ],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT",
                "effects": [ ["fetch", "Ticket", { "emit": { "success": "INIT" } }] ] }
            ]
          }
        }
      ],
      "pages": [
        { "name": "TicketsPage", "path": "/tickets", "linkedEntity": "Ticket", "traits": [ { "ref": "TicketManage" } ] }
      ]
    }
  ]
}"#;

const CONSUMER: &str = r#"{
  "name": "test-ticket-consumer",
  "version": "1.0.0",
  "orbitals": [
    {
      "name": "SupportOrbital",
      "uses": [ { "from": "./atom.orb", "as": "Helpdesk" } ],
      "entity": {
        "name": "SupportTicket",
        "persistence": "persistent",
        "fields": [ { "name": "id", "type": "string", "required": true }, { "name": "subject", "type": "string" } ]
      },
      "traits": [
        { "ref": "Helpdesk.traits.TicketManage", "linkedEntity": "SupportTicket", "name": "SupportManage" }
      ],
      "pages": [
        { "name": "SupportPage", "path": "/support", "linkedEntity": "SupportTicket", "traits": [ { "ref": "SupportManage" } ] }
      ]
    }
  ]
}"#;

fn linked(schema: &orbital_core::schema::OrbitalSchema, name: &str) -> Option<String> {
    schema.orbitals[0].traits.iter().find_map(|t| match t {
        TraitReference::Inline(td) if td.name == name => td.linked_entity.clone(),
        _ => None,
    })
}

fn inline_consumer() -> orbital_core::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), ATOM).expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

#[test]
fn pulled_sibling_bound_to_another_entity_keeps_its_binding() {
    let schema = inline_consumer();
    assert_eq!(
        linked(&schema, "TicketCorrespondence").as_deref(),
        Some("TicketMessage")
    );
}

#[test]
fn control_pulled_sibling_bound_to_the_embedder_entity_follows_the_rebind() {
    let schema = inline_consumer();
    assert_eq!(
        linked(&schema, "TicketBadge").as_deref(),
        Some("SupportTicket")
    );
}
