//! G-ORB-042: an embedder rebind retargets only the pulled siblings bound to
//! the embedder atom's OWN entity. A sibling bound to another entity of the
//! atom (std-helpdesk-ticket's TicketCorrespondence -> TicketMessage, embedded
//! by TicketManage -> Ticket) keeps its binding when a consumer rebinds the
//! embedder (`TicketManage -> SupportTicket`).

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_core_base::schema::TraitReference;

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
        { "name": "TicketsPage", "path": "/tickets", "traits": [ { "ref": "TicketManage" } ] }
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
        { "name": "SupportPage", "path": "/support", "traits": [ { "ref": "SupportManage" } ] }
      ]
    }
  ]
}"#;

fn linked(schema: &orbital_core_base::schema::OrbitalSchema, name: &str) -> Option<String> {
    schema.orbitals[0].traits.iter().find_map(|t| match t {
        TraitReference::Inline(td) if td.name == name => td.linked_entity.clone(),
        _ => None,
    })
}

fn inline_consumer(
    atom: &str,
    dependencies: &[(&str, &str)],
) -> orbital_core_base::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom).expect("write atom");
    for (name, source) in dependencies {
        std::fs::write(dir.path().join(name), source).expect("write dependency");
    }
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, CONSUMER).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

#[test]
fn pulled_sibling_bound_to_another_entity_keeps_its_binding() {
    let schema = inline_consumer(ATOM, &[]);
    assert_eq!(
        linked(&schema, "TicketCorrespondence").as_deref(),
        Some("TicketMessage")
    );
}

#[test]
fn control_pulled_sibling_bound_to_the_embedder_entity_follows_the_rebind() {
    let schema = inline_consumer(ATOM, &[]);
    assert_eq!(
        linked(&schema, "TicketBadge").as_deref(),
        Some("SupportTicket")
    );
}

#[test]
fn pulled_auxiliary_sibling_keeps_its_own_source_entity() {
    let schema = inline_consumer(ATOM, &[]);
    let source = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|entry| match entry {
            TraitReference::Inline(def) if def.name == "TicketCorrespondence" => def
                .source_entity_definition
                .as_ref()
                .map(|entity| entity.name.as_str()),
            _ => None,
        });
    assert_eq!(source, Some("TicketMessage"));
}

#[test]
fn pulled_composed_sibling_preserves_nested_source_entity() {
    let mut atom: serde_json::Value = serde_json::from_str(ATOM).expect("atom");
    atom["orbitals"][0]["uses"] =
        serde_json::json!([{ "from": "./messages.orb", "as": "Messages" }]);
    atom["orbitals"][0]["traits"][1] =
        serde_json::json!({ "ref": "Messages.traits.MessageView", "name": "TicketCorrespondence" });
    let messages = serde_json::json!({
        "name": "messages", "orbitals": [{
            "name": "MessageOrbital",
            "entity": { "name": "TicketMessage", "persistence": "persistent", "fields": [{ "name": "id", "type": "string" }, { "name": "content", "type": "string" }] },
            "traits": [{ "name": "MessageView", "linkedEntity": "TicketMessage", "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "INIT" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [["fetch", "TicketMessage", { "emit": { "success": "INIT" } }]] }]
            }}], "pages": []
        }]
    });
    let schema = inline_consumer(
        &atom.to_string(),
        &[("messages.orb", &messages.to_string())],
    );
    let sibling = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|entry| match entry {
            TraitReference::Inline(def) if def.name == "TicketCorrespondence" => Some(def),
            _ => None,
        })
        .expect("pulled nested sibling");
    assert_eq!(sibling.linked_entity.as_deref(), Some("TicketMessage"));
    assert_eq!(
        sibling
            .source_entity_definition
            .as_ref()
            .map(|entity| entity.name.as_str()),
        Some("TicketMessage")
    );
}

fn atom_with_action(field_type: &str, event: &str, source: &str) -> String {
    let mut atom: serde_json::Value = serde_json::from_str(ATOM).expect("atom");
    atom["orbitals"][0]["traits"][1]["config"] = serde_json::json!({
        "actions": { "type": "[object]", "items": { "name": "action", "type": "object", "properties": {
            "action": { "name": "action", "type": field_type }
        } }, "default": [{ "action": event }] }
    });
    atom["orbitals"][0]["traits"].as_array_mut().unwrap().push(serde_json::json!({
        "name": "RemoveMessages", "linkedEntity": "TicketMessage", 
        "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "REMOVE" }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "REMOVE", "effects": [["set", "@entity.content", "removed"]] }] },
        "listens": [{ "event": "REMOVE", "triggers": "REMOVE", "source": { "kind": "trait", "trait": source } }]
    }));
    atom.to_string()
}

#[test]
fn typed_config_action_pulls_its_declared_sibling_receiver() {
    let schema = inline_consumer(
        &atom_with_action("event", "REMOVE", "TicketCorrespondence"),
        &[],
    );
    assert_eq!(
        linked(&schema, "RemoveMessages").as_deref(),
        Some("TicketMessage")
    );
    let findings = orbital_compiler::phases::validation::validate_comprehensive_without_services(&schema);
    assert!(
        !findings.errors.iter().any(|error| error.code
            == orbital_compiler::phases::validation::ErrorCode::TraitInvalidEventDefault),
        "{:?}",
        findings.errors
    );
}

#[test]
fn action_without_a_declared_receiver_is_not_invented() {
    let schema = inline_consumer(
        &atom_with_action("event", "UNKNOWN", "TicketCorrespondence"),
        &[],
    );
    assert_eq!(linked(&schema, "RemoveMessages"), None);
    let findings = orbital_compiler::phases::validation::validate_comprehensive_without_services(&schema);
    assert!(findings.errors.iter().any(|error| error.code
        == orbital_compiler::phases::validation::ErrorCode::TraitInvalidEventDefault));
}

#[test]
fn ordinary_string_and_foreign_listener_do_not_pull_receivers() {
    for (kind, source) in [("string", "TicketCorrespondence"), ("event", "TicketBadge")] {
        let schema = inline_consumer(&atom_with_action(kind, "REMOVE", source), &[]);
        assert_eq!(linked(&schema, "RemoveMessages"), None);
    }
}

#[test]
fn struct_config_action_pulls_the_declared_receiver_only_for_event_fields() {
    for (kind, event, expected) in [
        ("event", "REMOVE", Some("TicketMessage")),
        ("string", "REMOVE", None),
        ("event", "UNKNOWN", None),
    ] {
        let mut atom: serde_json::Value =
            serde_json::from_str(&atom_with_action(kind, event, "TicketCorrespondence"))
                .expect("atom");
        let action = &mut atom["orbitals"][0]["traits"][1]["config"]["actions"];
        *action = serde_json::json!({ "type": "object", "properties": action["items"]["properties"].clone(), "default": action["default"][0].clone() });
        let schema = inline_consumer(&atom.to_string(), &[]);
        assert_eq!(
            linked(&schema, "RemoveMessages").as_deref(),
            expected,
            "{kind}: {event}"
        );
    }
}

fn receiver_consumer(host_route: bool) -> String {
    let remover = if host_route {
        r#", { "name": "MyRemover", "linkedEntity": "TicketMessage", 
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "REMOVE" }],
              "transitions": [{ "from": "idle", "to": "idle", "event": "REMOVE", "effects": [["set", "@entity.content", "mine"]] }] },
            "listens": [{ "event": "REMOVE", "triggers": "REMOVE", "source": { "kind": "trait", "trait": "Correspondence" } }] }"#
    } else {
        ""
    };
    let mounted = if host_route { r#", { "ref": "MyRemover" }"# } else { "" };
    format!(
        r#"{{
  "name": "test-receiver-consumer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "InboxOrbital",
      "uses": [ {{ "from": "./atom.orb", "as": "Helpdesk" }} ],
      "entity": {{ "name": "TicketMessage", "persistence": "persistent", "fields": [ {{ "name": "id", "type": "string", "required": true }}, {{ "name": "content", "type": "string" }} ] }},
      "traits": [ {{ "ref": "Helpdesk.traits.TicketCorrespondence", "linkedEntity": "TicketMessage", "name": "Correspondence" }}{remover} ],
      "pages": [ {{ "name": "InboxPage", "path": "/inbox", "traits": [ {{ "ref": "Correspondence" }}{mounted} ] }} ]
    }}
  ]
}}"#
    )
}

fn inline_receiver_consumer(host_route: bool) -> orbital_core_base::schema::OrbitalSchema {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("atom.orb"),
        atom_with_action("event", "REMOVE", "TicketCorrespondence"),
    )
    .expect("write atom");
    let consumer_path = dir.path().join("consumer.orb");
    std::fs::write(&consumer_path, receiver_consumer(host_route)).expect("write consumer");
    let mut schema = parse_file(&consumer_path).expect("consumer parses");
    inline_schema(&mut schema, &InlineConfig::default(), Some(&consumer_path))
        .expect("inline succeeds");
    schema
}

fn page_mounts(schema: &orbital_core_base::schema::OrbitalSchema) -> Vec<String> {
    schema.orbitals[0]
        .pages
        .iter()
        .flatten()
        .filter_map(|p| match p {
            orbital_core_base::schema::PageRef::Inline(page) => Some(page),
            _ => None,
        })
        .flat_map(|page| page.traits.iter().flatten().filter_map(|t| t.trait_name().map(str::to_string)))
        .collect()
}

#[test]
fn a_host_route_for_the_action_is_the_receiver_and_nothing_is_pulled() {
    let schema = inline_receiver_consumer(true);
    assert_eq!(linked(&schema, "RemoveMessages"), None);
    assert_eq!(linked(&schema, "MyRemover").as_deref(), Some("TicketMessage"));
}

#[test]
fn control_an_unrouted_action_pulls_its_receiver_and_mounts_it_where_the_emitter_is_mounted() {
    let schema = inline_receiver_consumer(false);
    assert_eq!(linked(&schema, "RemoveMessages").as_deref(), Some("TicketMessage"));
    let mounts = page_mounts(&schema);
    assert!(mounts.iter().any(|m| m == "RemoveMessages"), "{mounts:?}");
}
