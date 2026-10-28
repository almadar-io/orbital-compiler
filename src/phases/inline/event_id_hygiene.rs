//! Post-expansion event-id hygiene (per-instance alpha-renaming).
//!
//! An atom's baked event ids are minted ONCE and copied verbatim into every
//! `use` instance the inliner expands, so an orbital binding one atom N times
//! carries N inline traits sharing one id per event — variable capture. Every
//! id→name resolution over the composed document (identity normalization,
//! id-primary dispatch) is then ambiguous: with `events { SEARCH: NOTE_SEARCH }`
//! on one of five std-app-layout instances, the four unrenamed siblings clobber
//! the renamed instance's `searchEvent` knob default through the document-global
//! id→key map (SCAN-EVENT-REFID-SCOPE-1).
//!
//! This pass restores the document-unique-id invariant hygienically, like a
//! hygienic macro expansion: any event id declared by two or more DIFFERENT
//! owning traits is re-derived per owner — `derive_id(old_id, owner trait id)`,
//! deterministic and entropy-free, so page copies of the same instance converge
//! on the same derived id and re-runs never churn. All intra-owner positions
//! rewrite together (`stateMachine.events[].id`, `transitions[].eventId`,
//! `emits[].eventId`, listens `eventId`/`triggersId`, config `refId`); a
//! cross-trait listen that pointed at the retired shared id re-resolves by
//! event key (first declaration wins — `stamp`'s `events_any` semantics); and
//! each derived id gets its own ledger row (kind `event`, parent = the OWNING
//! trait — the ledger's documented per-(trait, declared event) model), while
//! the retired shared rows are dropped. A document with no shared event ids
//! passes through byte-identical.

use orbital_core_base::identity::{derive_id, IdKind, LedgerEntry, LedgerOwner};
use std::sync::Arc;
use orbital_core_base::schema::{
    Orbital, OrbitalSchema, PageRef, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Entry point: uniquify shared declared event ids across every orbital, then
/// reconcile the schema ledger (derived rows in, retired rows out).
pub(super) fn uniquify_instance_event_ids(schema: &mut OrbitalSchema) {
    let mut derived_rows: Vec<DerivedRow> = Vec::new();
    let mut retired_ids: HashSet<String> = HashSet::new();

    for orbital in schema.orbitals.iter_mut() {
        uniquify_orbital(orbital, &mut derived_rows, &mut retired_ids);
    }

    if derived_rows.is_empty() {
        return;
    }
    let Some(ledger) = schema.ledger.as_mut() else {
        return;
    };
    for row in derived_rows {
        let (baked_name, owner) = match ledger.entries.get(&row.old_id) {
            Some(old) => (old.baked_name.clone(), old.owner),
            None => (row.key.clone(), LedgerOwner::Workspace),
        };
        ledger.entries.insert(
            row.new_id.clone(),
            LedgerEntry {
                id: row.new_id,
                kind: IdKind::Event,
                baked_name,
                cur_name: row.key,
                renames: Vec::new(),
                owner,
                parent: row.parent,
            },
        );
    }
    // Collisions are per orbital; the ledger is the whole program's. A retired
    // id another orbital's instance still declares keeps its row.
    let mut still_declared: HashSet<String> = HashSet::new();
    for orbital in schema.orbitals.iter() {
        for_each_inline_trait(orbital, &mut |td| {
            declared_event_ids(td, &mut |id| {
                still_declared.insert(id.to_string());
            });
        });
    }
    for id in retired_ids {
        if !still_declared.contains(&id) {
            ledger.entries.remove(&id);
        }
    }
}

/// One derived per-instance event id, pending its ledger row.
struct DerivedRow {
    old_id: String,
    new_id: String,
    /// The owning instance's CURRENT event key (post-rename-fold).
    key: String,
    /// The owning trait's declaration id.
    parent: Option<String>,
}

/// The stable per-instance discriminator: the trait's NAME — unique per
/// orbital (`DuplicateTraitName`-enforced, and the sibling-pull uniquifier
/// renames cross-owner pulls before this pass runs), and shared by a page
/// copy of the same instance so the copies converge on one derived id. The
/// decl id is NOT usable here: pulled sibling copies materialized for two
/// different owners keep the SOURCE atom's trait id, so keying by id would
/// merge two distinct declarations into one owner and leave their shared
/// event ids undetected.
fn owner_key(td: &TraitDefinition) -> &str {
    &td.name
}

/// Ids this trait DECLARES: its state-machine event list plus its emit list.
fn declared_event_ids(td: &TraitDefinition, out: &mut impl FnMut(&str)) {
    for ev in td.state_machine.events.iter().flatten() {
        if let Some(id) = &ev.id {
            out(id);
        }
    }
    for e in td.emits.iter().flatten() {
        if let Some(id) = &e.event_id {
            out(id);
        }
    }
}

pub(super) fn for_each_inline_trait<'a>(orbital: &'a Orbital, f: &mut impl FnMut(&'a TraitDefinition)) {
    for tr in &orbital.traits {
        if let TraitReference::Inline(td) = tr {
            f(td);
        }
    }
    for page in orbital.pages.iter().flatten() {
        let traits = match page {
            PageRef::Inline(pd) => pd.traits.as_ref(),
            PageRef::Object(po) => po.traits.as_ref(),
            PageRef::Reference(_) => None,
        };
        for tr in traits.into_iter().flatten() {
            if let TraitReference::Inline(td) = tr {
                f(td);
            }
        }
    }
}

pub(super) fn for_each_trait_ref_mut(orbital: &mut Orbital, f: &mut impl FnMut(&mut TraitReference)) {
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

fn uniquify_orbital(
    orbital: &mut Orbital,
    derived_rows: &mut Vec<DerivedRow>,
    retired_ids: &mut HashSet<String>,
) {
    // Pass 1 — ownership census. An id is shared when two DIFFERENT owners
    // declare it; the same instance carried twice (own trait + its page copy,
    // same decl id) is one owner, not a collision.
    let mut owners: HashMap<String, HashSet<String>> = HashMap::new();
    for_each_inline_trait(orbital, &mut |td| {
        let key = owner_key(td).to_string();
        declared_event_ids(td, &mut |id| {
            owners
                .entry(id.to_string())
                .or_default()
                .insert(key.clone());
        });
    });
    let colliding: HashSet<&String> = owners
        .iter()
        .filter(|(_, o)| o.len() > 1)
        .map(|(id, _)| id)
        .collect();
    if colliding.is_empty() {
        return;
    }
    let colliding: HashSet<String> = colliding.into_iter().cloned().collect();

    // Pass 2 — per-owner re-derivation + intra-owner rewrite. derive_id is a
    // pure function of (old id, owner key), so the page copy of an instance
    // lands on the same derived id as the instance itself.
    let mut rows_by_new: HashMap<String, DerivedRow> = HashMap::new();
    for_each_trait_ref_mut(orbital, &mut |tr| {
        let TraitReference::Inline(td) = tr else {
            return;
        };
        let td = Arc::make_mut(td);
        let disc = owner_key(td).to_string();
        let mut subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
        declared_event_ids(td, &mut |id| {
            if colliding.contains(id) && !subs.contains_key(id) {
                subs.insert(id.to_string(), derive_id(id, &disc));
            }
        });
        if subs.is_empty() {
            return;
        }
        // Current key per rewritten id, for the ledger row (emits fill in ids
        // the event list doesn't carry).
        let mut key_of: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
        for e in td.emits.iter().flatten() {
            if let Some(id) = &e.event_id {
                key_of.entry(id.clone()).or_insert_with(|| e.event.clone());
            }
        }
        for ev in td.state_machine.events.iter().flatten() {
            if let Some(id) = &ev.id {
                key_of.insert(id.clone(), ev.key.clone());
            }
        }
        for (old, new) in &subs {
            rows_by_new
                .entry(new.clone())
                .or_insert_with(|| DerivedRow {
                    old_id: old.clone(),
                    new_id: new.clone(),
                    key: key_of.get(old).cloned().unwrap_or_default(),
                    parent: td.id.clone(),
                });
        }
        for ev in td.state_machine.events.iter_mut().flatten() {
            swap_id(&mut ev.id, &subs);
        }
        for t in td.state_machine.transitions.iter_mut() {
            swap_id(&mut t.event_id, &subs);
        }
        for e in td.emits.iter_mut().flatten() {
            swap_id(&mut e.event_id, &subs);
        }
        for l in td.listens.iter_mut().flatten() {
            swap_id(&mut l.event_id, &subs);
            swap_id(&mut l.triggers_id, &subs);
        }
        if let Some(config) = td.config.as_mut() {
            for cf in config.values_mut() {
                swap_id(&mut cf.ref_id, &subs);
            }
        }
    });
    derived_rows.extend(rows_by_new.into_values());
    retired_ids.extend(colliding.iter().cloned());

    // Pass 3 — the retired shared ids no longer name any node. Re-resolve every
    // position still holding one by its sibling NAME against the post-hygiene
    // declarations (first declaration wins, `events_any` semantics); a key that
    // resolves nowhere clears to None (the best-effort contract).
    let mut key_to_id: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for_each_inline_trait(orbital, &mut |td| {
        for ev in td.state_machine.events.iter().flatten() {
            if let Some(id) = &ev.id {
                key_to_id
                    .entry(ev.key.clone())
                    .or_insert_with(|| id.clone());
            }
        }
        for e in td.emits.iter().flatten() {
            if let Some(id) = &e.event_id {
                key_to_id
                    .entry(e.event.clone())
                    .or_insert_with(|| id.clone());
            }
        }
    });
    for_each_trait_ref_mut(orbital, &mut |tr| match tr {
        TraitReference::Inline(td) => {
            let td = Arc::make_mut(td);
            for l in td.listens.iter_mut().flatten() {
                reresolve(&mut l.event_id, Some(&l.event), &colliding, &key_to_id);
                reresolve(
                    &mut l.triggers_id,
                    l.triggers.as_deref(),
                    &colliding,
                    &key_to_id,
                );
            }
            reresolve_config(td.config.as_mut(), &colliding, &key_to_id);
        }
        TraitReference::Reference { config, .. } => {
            reresolve_config(config.as_mut(), &colliding, &key_to_id);
        }
    });
}

fn swap_id(slot: &mut Option<String>, subs: &orbital_core_base::collections::IndexMap<String, String>) {
    if let Some(id) = slot.as_ref() {
        if let Some(new) = subs.get(id) {
            *slot = Some(new.clone());
        }
    }
}

fn reresolve(
    slot: &mut Option<String>,
    name: Option<&str>,
    retired: &HashSet<String>,
    key_to_id: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if let Some(id) = slot.as_ref() {
        if retired.contains(id) {
            *slot = name.and_then(|n| key_to_id.get(n).cloned());
        }
    }
}

fn reresolve_config(
    config: Option<&mut orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
    retired: &HashSet<String>,
    key_to_id: &orbital_core_base::collections::IndexMap<String, String>,
) {
    let Some(config) = config else { return };
    for cf in config.values_mut() {
        if cf.field_type != "event" {
            continue;
        }
        let name = match &cf.default {
            Some(SExpression::String(s)) if !s.starts_with('@') => Some(s.as_str()),
            _ => None,
        };
        reresolve(&mut cf.ref_id, name, retired, key_to_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(v: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(v).expect("valid schema")
    }

    /// The reproduced clobber shape: one atom expanded twice, one instance
    /// renames the event and carries the rename-folded knob default; both
    /// instances share the baked event id, both knob refIds point at it.
    fn two_instance_json() -> serde_json::Value {
        json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Note", "fields": [], "id": "ent_a" },
                "traits": [
                    {
                        "name": "NoteLayout", "id": "trt_a1", "linkedEntity": "Note",
                        "config": {
                            "searchEvent": { "type": "event", "default": "NOTE_SEARCH", "refId": "evt_shared" }
                        },
                        "emits": [{ "event": "NOTE_SEARCH", "eventId": "evt_shared" }],
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "NOTE_SEARCH", "eventId": "evt_shared" }],
                            "events": [{ "key": "NOTE_SEARCH", "id": "evt_shared" }]
                        }
                    },
                    {
                        "name": "DetailLayout", "id": "trt_a2", "linkedEntity": "Note",
                        "config": {
                            "searchEvent": { "type": "event", "default": "SEARCH", "refId": "evt_shared" }
                        },
                        "emits": [{ "event": "SEARCH", "eventId": "evt_shared" }],
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "SEARCH", "eventId": "evt_shared" }],
                            "events": [{ "key": "SEARCH", "id": "evt_shared" }]
                        }
                    }
                ]
            }],
            "schemaVersion": 1,
            "ledger": { "schemaVersion": 1, "entries": {
                "evt_shared": { "id": "evt_shared", "kind": "event", "bakedName": "SEARCH", "curName": "SEARCH", "renames": [], "owner": "std", "parent": "trt_atom" }
            } }
        })
    }

    fn inline_at(s: &OrbitalSchema, i: usize) -> &TraitDefinition {
        match &s.orbitals[0].traits[i] {
            TraitReference::Inline(td) => td,
            _ => panic!("inline"),
        }
    }

    /// The collision is per orbital, the ledger is program-wide: a second
    /// orbital whose one instance still declares the shared id keeps it, and
    /// its listen keeps resolving through the ledger row.
    #[test]
    fn a_shared_id_still_declared_in_another_orbital_keeps_its_ledger_row() {
        let mut v = two_instance_json();
        v["orbitals"].as_array_mut().unwrap().push(json!({
            "name": "P",
            "entity": { "name": "Tag", "fields": [], "id": "ent_b" },
            "traits": [{
                "name": "TagLayout", "id": "trt_b1", "linkedEntity": "Tag",
                "listens": [{ "event": "SEARCH", "triggers": "SEARCH", "triggersId": "evt_shared", "source": { "kind": "any" } }],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "SEARCH", "eventId": "evt_shared" }],
                    "events": [{ "key": "SEARCH", "id": "evt_shared" }]
                }
            }]
        }));
        let mut s = schema(v);
        uniquify_instance_event_ids(&mut s);
        let entries = &s.ledger.as_ref().unwrap().entries;
        assert!(
            entries.contains_key("evt_shared"),
            "still declared by P.TagLayout"
        );
        let TraitReference::Inline(tag) = &s.orbitals[1].traits[0] else {
            panic!("inline")
        };
        assert_eq!(
            tag.listens.as_ref().unwrap()[0].triggers_id.as_deref(),
            Some("evt_shared")
        );
    }

    #[test]
    fn control_a_shared_id_declared_nowhere_after_hygiene_is_retired() {
        let mut s = schema(two_instance_json());
        uniquify_instance_event_ids(&mut s);
        assert!(!s
            .ledger
            .as_ref()
            .unwrap()
            .entries
            .contains_key("evt_shared"));
    }

    #[test]
    fn shared_event_id_is_rederived_per_owner_consistently() {
        let mut s = schema(two_instance_json());
        uniquify_instance_event_ids(&mut s);
        let a = inline_at(&s, 0);
        let b = inline_at(&s, 1);
        let a_id = a.state_machine.events.as_ref().unwrap()[0]
            .id
            .clone()
            .unwrap();
        let b_id = b.state_machine.events.as_ref().unwrap()[0]
            .id
            .clone()
            .unwrap();
        assert_ne!(a_id, b_id, "instances must not share an event id");
        assert_ne!(a_id, "evt_shared");
        assert_ne!(b_id, "evt_shared");
        assert!(a_id.starts_with("evt_"), "kind prefix kept: {a_id}");
        // Every intra-owner position moved together.
        for td in [a, b] {
            let id = td.state_machine.events.as_ref().unwrap()[0]
                .id
                .as_deref()
                .unwrap();
            assert_eq!(td.emits.as_ref().unwrap()[0].event_id.as_deref(), Some(id));
            assert_eq!(
                td.state_machine.transitions[0].event_id.as_deref(),
                Some(id)
            );
            assert_eq!(
                td.config.as_ref().unwrap()["searchEvent"].ref_id.as_deref(),
                Some(id),
                "the knob refId must follow its OWN instance's event"
            );
        }
        // Deterministic: same input, same output.
        let mut again = schema(two_instance_json());
        uniquify_instance_event_ids(&mut again);
        assert_eq!(
            inline_at(&again, 0).state_machine.events.as_ref().unwrap()[0].id,
            Some(a_id)
        );
    }

    #[test]
    fn ledger_gets_per_instance_rows_and_drops_the_retired_one() {
        let mut s = schema(two_instance_json());
        uniquify_instance_event_ids(&mut s);
        let ledger = s.ledger.as_ref().unwrap();
        assert!(
            !ledger.entries.contains_key("evt_shared"),
            "retired row dropped"
        );
        let a_id = inline_at(&s, 0).state_machine.events.as_ref().unwrap()[0]
            .id
            .clone()
            .unwrap();
        let row = ledger.entries.get(&a_id).expect("derived row present");
        assert_eq!(row.cur_name, "NOTE_SEARCH");
        assert_eq!(
            row.baked_name, "SEARCH",
            "baked name copied from the retired row"
        );
        assert_eq!(
            row.parent.as_deref(),
            Some("trt_a1"),
            "parent = the OWNING instance"
        );
    }

    #[test]
    fn cross_trait_listen_rersolves_by_key_first_declared() {
        let mut v = two_instance_json();
        v["orbitals"][0]["traits"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "name": "Watcher", "id": "trt_w", "linkedEntity": "Note",
                "listens": [{ "event": "SEARCH", "eventId": "evt_shared" }],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                }
            }));
        let mut s = schema(v);
        uniquify_instance_event_ids(&mut s);
        let watcher = inline_at(&s, 2);
        let detail_id = inline_at(&s, 1).state_machine.events.as_ref().unwrap()[0]
            .id
            .clone()
            .unwrap();
        assert_eq!(
            watcher.listens.as_ref().unwrap()[0].event_id.as_deref(),
            Some(detail_id.as_str()),
            "listen re-resolves by key to the (only) SEARCH declaration"
        );
    }

    #[test]
    fn unique_ids_pass_through_byte_identical() {
        let mut v = two_instance_json();
        // Give the second instance its own id everywhere — no collision left.
        let s2 = serde_json::to_string(&v["orbitals"][0]["traits"][1])
            .unwrap()
            .replace("evt_shared", "evt_other");
        v["orbitals"][0]["traits"][1] = serde_json::from_str(&s2).unwrap();
        let mut s = schema(v.clone());
        let before = serde_json::to_value(&s).unwrap();
        uniquify_instance_event_ids(&mut s);
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
}
