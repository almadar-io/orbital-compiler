//! Composing one atom in two orbitals (or twice in one) copies its traits with the source atom's ids, so one trait id
//! ends up on several declarations and every by-id reader (listen sources, page references, embeds) confuses them. The
//! first declaration keeps the id; each later one gets `derive_id(id, "<orbital>.<trait>")`, the references inside its
//! own orbital follow it by name, and the ledger gains the derived row.

use orbital_core_base::identity::{derive_id, LedgerEntry};
use orbital_core_base::schema::{ListenSource, Orbital, OrbitalSchema, PageRef, TraitReference};
use std::collections::HashMap;

pub(super) fn uniquify_instance_trait_ids(schema: &mut OrbitalSchema) {
    let mut owners: HashMap<String, (String, String)> = HashMap::new();
    let mut derived: Vec<(String, String, String)> = Vec::new();
    for orbital in schema.orbitals.iter_mut() {
        let mut declared: Vec<(String, String)> = Vec::new();
        for_each_trait_ref_mut(orbital, &mut |tr| {
            if let TraitReference::Inline(td) = tr {
                if let Some(id) = &td.id {
                    declared.push((td.name.clone(), id.clone()));
                }
            }
        });
        let mut renamed: HashMap<String, String> = HashMap::new();
        for (name, id) in declared {
            let owner = (orbital.name.clone(), name.clone());
            match owners.get(&id) {
                None => {
                    owners.insert(id, owner);
                }
                Some(first) if *first == owner => {}
                Some(_) => {
                    if !renamed.contains_key(&name) {
                        let fresh = derive_id(&id, &format!("{}.{}", orbital.name, name));
                        derived.push((id, fresh.clone(), name.clone()));
                        renamed.insert(name, fresh);
                    }
                }
            }
        }
        if !renamed.is_empty() {
            follow(orbital, &renamed);
        }
    }
    let Some(ledger) = schema.ledger.as_mut() else {
        return;
    };
    for (old, fresh, name) in derived {
        if let Some(row) = ledger.entries.get(&old).cloned() {
            ledger.entries.insert(
                fresh.clone(),
                LedgerEntry {
                    id: fresh,
                    cur_name: name,
                    renames: Vec::new(),
                    ..row
                },
            );
        }
    }
}

/// Points every declaration and by-id reference to a renamed trait in `orbital` at its derived id.
fn follow(orbital: &mut Orbital, renamed: &HashMap<String, String>) {
    for_each_trait_ref_mut(orbital, &mut |tr| match tr {
        TraitReference::Inline(td) => {
            if let Some(fresh) = renamed.get(&td.name) {
                td.id = Some(fresh.clone());
            }
            for listen in td.listens.iter_mut().flatten() {
                if let Some(ListenSource::Trait { trait_name, trait_id }) = listen.source.as_mut() {
                    if let Some(fresh) = renamed.get(trait_name.as_str()) {
                        *trait_id = Some(fresh.clone());
                    }
                }
            }
            for (embedded, id) in td.trait_embed_ids.iter_mut().flatten() {
                if let Some(fresh) = renamed.get(embedded) {
                    *id = fresh.clone();
                }
            }
        }
        TraitReference::Reference { reference: Some(name), ref_id, .. } => {
            if let Some(fresh) = renamed.get(name.as_str()) {
                *ref_id = Some(fresh.clone());
            }
        }
        TraitReference::Reference { .. } => {}
    });
    for page in orbital.pages.iter_mut().flatten() {
        if let PageRef::Object(po) = page {
            if let (Some(traits), Some(ids)) = (po.traits.as_ref(), po.trait_ref_ids.as_mut()) {
                for (tr, id) in traits.iter().zip(ids.iter_mut()) {
                    let name = match tr {
                        TraitReference::Inline(td) => Some(td.name.as_str()),
                        TraitReference::Reference { reference, .. } => reference.as_deref(),
                    };
                    if let Some(fresh) = name.and_then(|n| renamed.get(n)) {
                        *id = fresh.clone();
                    }
                }
            }
        }
    }
}

fn for_each_trait_ref_mut(orbital: &mut Orbital, f: &mut impl FnMut(&mut TraitReference)) {
    for tr in orbital.traits.iter_mut() {
        f(tr);
    }
    for page in orbital.pages.iter_mut().flatten() {
        let traits = match page {
            PageRef::Inline(pd) => pd.traits.as_mut(),
            PageRef::Object(po) => po.traits.as_mut(),
            PageRef::Reference(_) => None,
        };
        for tr in traits.into_iter().flatten() {
            f(tr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(second_orbital_trait: &str) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "p",
            "orbitals": [
                {
                    "name": "Desk",
                    "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                    "traits": [
                        { "name": "CancelDivider", "id": "trt_SHARED", "linkedEntity": "Row",
                          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } }
                    ]
                },
                {
                    "name": "View",
                    "entity": { "name": "Visit", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                    "traits": [
                        { "name": second_orbital_trait, "id": "trt_SHARED", "linkedEntity": "Visit",
                          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } },
                        { "name": "Listener", "id": "trt_LISTENER", "linkedEntity": "Visit",
                          "listens": [{ "event": "DONE", "triggers": "GO",
                                        "source": { "kind": "trait", "trait": second_orbital_trait, "traitId": "trt_SHARED" } }],
                          "traitEmbedIds": { second_orbital_trait: "trt_SHARED" },
                          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } }
                    ]
                }
            ]
        }))
        .expect("a program")
    }

    fn inline<'a>(schema: &'a OrbitalSchema, orbital: usize, name: &str) -> &'a orbital_core_base::schema::TraitDefinition {
        schema.orbitals[orbital]
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(td) if td.name == name => Some(td),
                _ => None,
            })
            .expect("the trait")
    }

    #[test]
    fn a_trait_id_carried_by_a_second_declaration_is_re_derived_and_its_references_follow() {
        let mut schema = program("ViewCancelDivider");
        uniquify_instance_trait_ids(&mut schema);
        assert_eq!(inline(&schema, 0, "CancelDivider").id.as_deref(), Some("trt_SHARED"), "the first keeps its id");
        let copy = inline(&schema, 1, "ViewCancelDivider").id.clone().expect("an id");
        assert_ne!(copy, "trt_SHARED");
        assert_eq!(copy, derive_id("trt_SHARED", "View.ViewCancelDivider"), "deterministic");
        let listener = inline(&schema, 1, "Listener");
        let source = listener.listens.as_ref().unwrap()[0].source.clone();
        assert!(matches!(source, Some(ListenSource::Trait { trait_id: Some(ref id), .. }) if *id == copy));
        assert_eq!(listener.trait_embed_ids.as_ref().unwrap()["ViewCancelDivider"], copy);
        assert_eq!(listener.id.as_deref(), Some("trt_LISTENER"), "control: an unshared id is untouched");
    }

    #[test]
    fn control_a_program_whose_trait_ids_are_unique_is_unchanged() {
        let mut schema = program("ViewCancelDivider");
        if let TraitReference::Inline(td) = &mut schema.orbitals[1].traits[0] {
            td.id = Some("trt_OWN".to_string());
        }
        let before = serde_json::to_value(&schema).unwrap();
        uniquify_instance_trait_ids(&mut schema);
        assert_eq!(serde_json::to_value(&schema).unwrap(), before);
    }
}
