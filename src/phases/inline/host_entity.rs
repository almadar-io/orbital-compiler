//! Inline's call of `OrbitalSchema::bind_unlinked_traits_to_host_entity`, and
//! its end-to-end tests through `inline_schema` (`tools/parity-cascade` fixture
//! `dispatch-mode`).

use orbital_core::schema::OrbitalSchema;

pub(super) fn bind_unlinked_traits_to_host_entity(schema: &mut OrbitalSchema) {
    schema.bind_unlinked_traits_to_host_entity();
}

#[cfg(test)]
mod tests {
    use super::super::{inline_schema, InlineConfig};
    use orbital_core::runtime::DispatchMode;
    use orbital_core::schema::{EntityRef, OrbitalSchema, TraitDefinition, TraitReference};

    fn inlined(traits: serde_json::Value) -> OrbitalSchema {
        let mut schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "HostEntity",
            "orbitals": [
                {
                    "name": "Frames",
                    "entity": { "name": "Frame", "persistence": "runtime",
                                "fields": [{ "name": "id", "type": "string" }, { "name": "n", "type": "number" }] },
                    "traits": traits
                },
                {
                    "name": "Rows",
                    "entity": { "name": "Row", "fields": [{ "name": "id", "type": "string" }] },
                    "traits": []
                }
            ]
        }))
        .unwrap();
        inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
        schema
    }

    fn trait_named<'a>(schema: &'a OrbitalSchema, name: &str) -> &'a TraitDefinition {
        schema.orbitals[0]
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(td) if td.name == name => Some(td),
                _ => None,
            })
            .unwrap()
    }

    fn machine(name: &str, effects: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": effects }]
            }
        })
    }

    #[test]
    fn an_unlinked_trait_binds_its_host_entity_with_its_id() {
        let schema = inlined(serde_json::json!([machine(
            "Pure",
            serde_json::json!([["set", "@entity.n", 1]])
        )]));
        let td = trait_named(&schema, "Pure");
        assert_eq!(td.linked_entity.as_deref(), Some("Frame"));
        let EntityRef::Inline(frame) = &schema.orbitals[0].entity else {
            panic!("inline entity")
        };
        assert!(frame.id.is_some());
        assert_eq!(td.linked_entity_id, frame.id);
    }

    #[test]
    fn a_declared_binding_is_left_alone() {
        let mut declared = machine("Declared", serde_json::json!([]));
        declared["linkedEntity"] = serde_json::json!("Row");
        let schema = inlined(serde_json::json!([declared]));
        let td = trait_named(&schema, "Declared");
        assert_eq!(td.linked_entity.as_deref(), Some("Row"));
        assert_eq!(td.linked_entity_id, None);
    }

    #[test]
    fn the_host_binding_drives_the_runtime_dispatch_rule() {
        let schema = inlined(serde_json::json!([
            machine("Pure", serde_json::json!([["set", "@entity.n", 1]])),
            machine(
                "Saves",
                serde_json::json!([["persist", "update", "Frame", "@entity"]])
            )
        ]));
        let EntityRef::Inline(frame) = &schema.orbitals[0].entity else {
            panic!("inline entity")
        };
        // As the kernel does: the entity is whatever the trait's binding names.
        let mode = |name: &str| {
            let td = trait_named(&schema, name);
            td.dispatch_mode(
                td.linked_entity
                    .as_deref()
                    .filter(|e| *e == frame.name)
                    .map(|_| frame),
            )
        };
        assert_eq!(mode("Pure"), DispatchMode::HybridClientOnly);
        assert_eq!(mode("Saves"), DispatchMode::RuntimeOptimistic);
    }
}
