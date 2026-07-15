//! V4-W4 pre-inline identity normalization.
//!
//! Stamp-before-rename (the factory emits CANONICAL entity/trait names and the
//! persistence layer applies a display rename to the DECLARATION + ledger only,
//! keeping the reference TOKENS canonical) relies on readers resolving those
//! tokens by their stamped id side-maps onto the renamed declaration. The
//! runtime does this in `resolveEntityTokensById` + id-first ref resolution
//! (`almadar-runtime`); this pass is its compiled-path twin.
//!
//! For every inline trait (own traits + inline-page traits) and every page trait
//! ref, rewrite each id-carrying reference to the CURRENT declaration name of the
//! node its side-map (`entity_ref_ids` / `linked_entity_id` / `trait_embed_ids` /
//! page-ref `ref_id`) points at. After this pass the tokens agree with the
//! (renamed) declarations, so the downstream name-based inliner + validators
//! resolve them correctly — which is what lets the factory stop rewriting tokens
//! itself (`rebindInlineTraitEntity` / `applyTraitRenames`).
//!
//! ADDITIVE + presence-based: a node with no side-map (or whose id resolves to a
//! name already equal to the token) is left untouched, so a schema without a
//! rename is byte-identical through this pass.

use super::rewrite::{rewrite_identifiers, rewrite_payload_entity_refs};
use orbital_core::schema::{
    EntityRef, Orbital, OrbitalSchema, PageRef, SExpression, TraitDefinition, TraitReference,
};
use std::collections::HashMap;

/// Rewrite every id-carrying reference token to its current declaration name
/// across the whole schema. Runs after `ensure_stamped`, before inline/validate.
pub fn normalize_identity_tokens(schema: &mut OrbitalSchema) {
    for orbital in schema.orbitals.iter_mut() {
        let entity_id_to_name = collect_entity_id_names(orbital);
        let trait_id_to_name = collect_trait_id_names(orbital);
        if entity_id_to_name.is_empty() && trait_id_to_name.is_empty() {
            continue;
        }

        for tr in orbital.traits.iter_mut() {
            if let TraitReference::Inline(td) = tr {
                normalize_inline_trait(td, &entity_id_to_name, &trait_id_to_name);
            }
        }

        if let Some(pages) = orbital.pages.as_mut() {
            for page in pages.iter_mut() {
                let page_traits: Option<&mut Vec<TraitReference>> = match page {
                    PageRef::Inline(pd) => pd.traits.as_mut(),
                    PageRef::Object(po) => po.traits.as_mut(),
                    PageRef::Reference(_) => None,
                };
                if let Some(pts) = page_traits {
                    for tr in pts.iter_mut() {
                        match tr {
                            TraitReference::Inline(td) => {
                                normalize_inline_trait(td, &entity_id_to_name, &trait_id_to_name);
                            }
                            TraitReference::Reference {
                                reference, ref_id, ..
                            } => normalize_trait_ref(reference, ref_id.as_deref(), &trait_id_to_name),
                        }
                    }
                }
            }
        }
    }
}

/// `entity_id → current declaration name` for the orbital's primary + auxiliary
/// inline entities. Reference/extends entities carry no local id to rename by.
fn collect_entity_id_names(orbital: &Orbital) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let EntityRef::Inline(e) = &orbital.entity {
        if let Some(id) = &e.id {
            out.insert(id.clone(), e.name.clone());
        }
    }
    if let Some(aux) = orbital.auxiliary_entities.as_ref() {
        for a in aux {
            if let EntityRef::Inline(e) = a {
                if let Some(id) = &e.id {
                    out.insert(id.clone(), e.name.clone());
                }
            }
        }
    }
    out
}

/// `trait_id → current declaration name` for the orbital's own inline traits.
/// Imported reference traits resolve their `ref_id` against the source registry
/// during inline, so only inline (own) trait ids are recoverable here.
fn collect_trait_id_names(orbital: &Orbital) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for tr in &orbital.traits {
        if let TraitReference::Inline(td) = tr {
            if let Some(id) = &td.id {
                out.insert(id.clone(), td.name.clone());
            }
        }
    }
    out
}

/// Rewrite one inline trait's entity tokens, `linkedEntity`, and `@trait.<x>`
/// embeds to the current declaration names its id side-maps point at.
fn normalize_inline_trait(
    td: &mut TraitDefinition,
    entity_id_to_name: &HashMap<String, String>,
    trait_id_to_name: &HashMap<String, String>,
) {
    normalize_trait_entity_tokens(td, entity_id_to_name);
    normalize_trait_embed_tokens(td, trait_id_to_name);
}

/// Entity side: `entity_ref_ids` body tokens (`@entity`, positional
/// `fetch`/`persist`/`ref`/`spawn`, payload types) + `linked_entity_id`.
fn normalize_trait_entity_tokens(td: &mut TraitDefinition, entity_id_to_name: &HashMap<String, String>) {
    if entity_id_to_name.is_empty() {
        return;
    }
    // `linkedEntity` — resolve by its dual-carry id.
    if let Some(id) = td.linked_entity_id.as_deref() {
        if let Some(cur) = entity_id_to_name.get(id) {
            if td.linked_entity.as_deref() != Some(cur.as_str()) {
                td.linked_entity = Some(cur.clone());
            }
        }
    }

    let mut entity_subs: HashMap<String, String> = HashMap::new();
    if let Some(refs) = td.entity_ref_ids.as_ref() {
        for (token_name, entity_id) in refs {
            if let Some(cur) = entity_id_to_name.get(entity_id) {
                if cur != token_name {
                    entity_subs.insert(token_name.clone(), cur.clone());
                }
            }
        }
    }
    if entity_subs.is_empty() {
        return;
    }

    let no_events: HashMap<String, String> = HashMap::new();
    for transition in td.state_machine.transitions.iter_mut() {
        if let Some(guard) = transition.guard.as_mut() {
            rewrite_identifiers(guard, &entity_subs, &no_events);
        }
        if let Some(effects) = transition.effects.as_mut() {
            for effect in effects.iter_mut() {
                rewrite_identifiers(effect, &entity_subs, &no_events);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_identifiers(guard, &entity_subs, &no_events);
            }
            for effect in tick.effects.iter_mut() {
                rewrite_identifiers(effect, &entity_subs, &no_events);
            }
        }
    }
    rewrite_payload_entity_refs(td, &entity_subs);
}

/// Trait-embed side: `@trait.<Name>` render tokens carried in this trait's
/// effect trees, resolved by `trait_embed_ids`.
fn normalize_trait_embed_tokens(td: &mut TraitDefinition, trait_id_to_name: &HashMap<String, String>) {
    if trait_id_to_name.is_empty() {
        return;
    }
    let mut trait_subs: HashMap<String, String> = HashMap::new();
    if let Some(embeds) = td.trait_embed_ids.as_ref() {
        for (token_name, trait_id) in embeds {
            if let Some(cur) = trait_id_to_name.get(trait_id) {
                if cur != token_name {
                    trait_subs.insert(token_name.clone(), cur.clone());
                }
            }
        }
    }
    if trait_subs.is_empty() {
        return;
    }
    for transition in td.state_machine.transitions.iter_mut() {
        if let Some(effects) = transition.effects.as_mut() {
            for effect in effects.iter_mut() {
                rewrite_trait_embed_in_sexpr(effect, &trait_subs);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            for effect in tick.effects.iter_mut() {
                rewrite_trait_embed_in_sexpr(effect, &trait_subs);
            }
        }
    }
}

/// Rewrite `@trait.<from>` embed roots to `@trait.<to>` in an S-expression tree.
/// Boundary-checked (next char `.`/`[`/end) so `@trait.FooBar` is not matched by
/// a `Foo` sub.
fn rewrite_trait_embed_in_sexpr(expr: &mut SExpression, subs: &HashMap<String, String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                let split_at = rest.find(['.', '[']).unwrap_or(rest.len());
                let (name, suffix) = rest.split_at(split_at);
                if let Some(to) = subs.get(name) {
                    *s = format!("@trait.{}{}", to, suffix);
                }
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_trait_embed_in_sexpr(item, subs);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_trait_embed_in_sexpr(v, subs);
            }
        }
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// Rewrite a page/orbital trait `ref` string to the current declaration name of
/// the inline trait its `ref_id` points at (own-orbital refs only — an imported
/// `ref_id` is absent from `trait_id_to_name` and passes through untouched).
fn normalize_trait_ref(
    reference: &mut Option<String>,
    ref_id: Option<&str>,
    trait_id_to_name: &HashMap<String, String>,
) {
    if let (Some(rid), Some(reference_str)) = (ref_id, reference.as_mut()) {
        if let Some(cur) = trait_id_to_name.get(rid) {
            if reference_str != cur {
                *reference_str = cur.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(v: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(v).expect("valid schema")
    }

    /// A renamed inline-entity declaration drags its `["ref", <name>]` /
    /// `["fetch", <name>]` body tokens (keyed by `entityRefIds` id) to the new
    /// name, and updates `linkedEntity` via `linkedEntityId`.
    #[test]
    fn rewrites_entity_tokens_by_id() {
        let mut s = schema(json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Record", "fields": [], "id": "ent_a" },
                "traits": [{
                    "name": "Persistor",
                    "linkedEntity": "Item", "linkedEntityId": "ent_a",
                    "entityRefIds": { "Item": "ent_a" },
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{
                            "from": "idle", "to": "idle", "event": "SAVE",
                            "effects": [["persist", "create", "Item"], ["fetch", "Item"]]
                        }]
                    }
                }]
            }]
        }));
        normalize_identity_tokens(&mut s);
        let td = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!("inline"),
        };
        assert_eq!(td.linked_entity.as_deref(), Some("Record"));
        let effects = td.state_machine.transitions[0].effects.as_ref().unwrap();
        assert_eq!(effects[0], SExpression::Array(vec![
            SExpression::String("persist".into()),
            SExpression::String("create".into()),
            SExpression::String("Record".into()),
        ]));
        assert_eq!(effects[1], SExpression::Array(vec![
            SExpression::String("fetch".into()),
            SExpression::String("Record".into()),
        ]));
    }

    /// A page trait `ref` (canonical) is rewritten to the renamed inline trait's
    /// current name via `refId`, while an imported ref (no `refId`) is untouched.
    #[test]
    fn rewrites_page_ref_by_id() {
        let mut s = schema(json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Item", "fields": [], "id": "ent_a" },
                "traits": [{
                    "name": "BrowseView", "id": "trt_a",
                    "linkedEntity": "Item",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                    }
                }],
                "pages": [{
                    "name": "P", "path": "/p",
                    "traits": [
                        { "ref": "Browse", "refId": "trt_a" },
                        { "ref": "Imported" }
                    ]
                }]
            }]
        }));
        normalize_identity_tokens(&mut s);
        let page_traits = match &s.orbitals[0].pages.as_ref().unwrap()[0] {
            PageRef::Inline(pd) => pd.traits.as_ref().unwrap(),
            PageRef::Object(po) => po.traits.as_ref().unwrap(),
            _ => panic!("page with traits"),
        };
        let ref_name = |t: &TraitReference| match t {
            TraitReference::Reference { reference, .. } => reference.clone(),
            _ => None,
        };
        assert_eq!(ref_name(&page_traits[0]).as_deref(), Some("BrowseView"));
        assert_eq!(ref_name(&page_traits[1]).as_deref(), Some("Imported"));
    }

    /// A `@trait.<canonical>` embed token is rewritten to the renamed trait's
    /// current name via `traitEmbedIds`, boundary-checked.
    #[test]
    fn rewrites_trait_embed_by_id() {
        let mut s = schema(json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Item", "fields": [], "id": "ent_a" },
                "traits": [
                    {
                        "name": "BrowseView", "id": "trt_a", "linkedEntity": "Item",
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                        }
                    },
                    {
                        "name": "Host", "id": "trt_b", "linkedEntity": "Item",
                        "traitEmbedIds": { "Browse": "trt_a" },
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{
                                "from": "idle", "to": "idle", "event": "INIT",
                                "effects": [["render-ui", "main", "@trait.Browse"]]
                            }]
                        }
                    }
                ]
            }]
        }));
        normalize_identity_tokens(&mut s);
        let host = match &s.orbitals[0].traits[1] {
            TraitReference::Inline(td) => td,
            _ => panic!("inline"),
        };
        let effect = &host.state_machine.transitions[0].effects.as_ref().unwrap()[0];
        assert_eq!(*effect, SExpression::Array(vec![
            SExpression::String("render-ui".into()),
            SExpression::String("main".into()),
            SExpression::String("@trait.BrowseView".into()),
        ]));
    }

    /// No side-map ids present → the schema passes through byte-identical.
    #[test]
    fn no_ids_is_noop() {
        let v = json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Item", "fields": [] },
                "traits": [{
                    "name": "V", "linkedEntity": "Item",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{
                            "from": "idle", "to": "idle", "event": "S",
                            "effects": [["ref", "Item"]]
                        }]
                    }
                }]
            }]
        });
        let mut s = schema(v.clone());
        let before = serde_json::to_value(&s).unwrap();
        normalize_identity_tokens(&mut s);
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
}
