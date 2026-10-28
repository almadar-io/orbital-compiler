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
use std::sync::Arc;
use orbital_core_base::schema::{
    ConfigField, EntityRef, Orbital, OrbitalSchema, PageRef, SExpression, TraitDefinition,
    TraitReference,
};
use std::collections::HashSet;

/// Rewrite every id-carrying reference token to its current declaration name
/// across the whole schema. Runs after `ensure_stamped`, before inline/validate.
pub fn normalize_identity_tokens(schema: &mut OrbitalSchema) {
    for orbital in schema.orbitals.iter_mut() {
        let entity_id_to_name = collect_entity_id_names(orbital);
        let trait_id_to_name = collect_trait_id_names(orbital);
        let event_id_to_name = collect_event_id_names(orbital);
        if entity_id_to_name.is_empty()
            && trait_id_to_name.is_empty()
            && event_id_to_name.is_empty()
        {
            continue;
        }

        for tr in orbital.traits.iter_mut() {
            normalize_trait_reference(tr, &entity_id_to_name, &trait_id_to_name, &event_id_to_name);
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
                        normalize_trait_reference(
                            tr,
                            &entity_id_to_name,
                            &trait_id_to_name,
                            &event_id_to_name,
                        );
                    }
                }
            }
        }
    }
}

/// Insert `id → name`, refusing ambiguity: an id carried by two nodes that
/// disagree on the name resolves to NOTHING (removed + poisoned) — resolution
/// must be a function, and an arbitrary winner would silently rewrite the
/// other owners' tokens (the multi-instance shared-id clobber).
fn insert_unambiguous(
    map: &mut orbital_core_base::collections::IndexMap<String, String>,
    poisoned: &mut HashSet<String>,
    id: &str,
    name: &str,
) {
    if poisoned.contains(id) {
        return;
    }
    match map.get(id) {
        Some(existing) if existing != name => {
            map.shift_remove(id);
            poisoned.insert(id.to_string());
        }
        Some(_) => {}
        None => {
            map.insert(id.to_string(), name.to_string());
        }
    }
}

/// `entity_id → current declaration name` for the orbital's primary + auxiliary
/// inline entities. Reference/extends entities carry no local id to rename by.
fn collect_entity_id_names(orbital: &Orbital) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    let mut poisoned = HashSet::new();
    if let EntityRef::Inline(e) = &orbital.entity {
        if let Some(id) = &e.id {
            insert_unambiguous(&mut out, &mut poisoned, id, &e.name);
        }
    }
    if let Some(aux) = orbital.auxiliary_entities.as_ref() {
        for a in aux {
            if let EntityRef::Inline(e) = a {
                if let Some(id) = &e.id {
                    insert_unambiguous(&mut out, &mut poisoned, id, &e.name);
                }
            }
        }
    }
    out
}

/// `trait_id → current declaration name` for the orbital's own trait
/// declarations — inline bodies AND Reference-form declarations (which carry a
/// LOCAL decl `id` since the composed-surface backbone; their current name is
/// the `name:` override else the reference's last segment). The imported
/// TARGET's `ref_id` still resolves against the source registry during inline.
fn collect_trait_id_names(orbital: &Orbital) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    let mut poisoned = HashSet::new();
    for tr in &orbital.traits {
        match tr {
            TraitReference::Inline(td) => {
                if let Some(id) = &td.id {
                    insert_unambiguous(&mut out, &mut poisoned, id, &td.name);
                }
            }
            TraitReference::Reference {
                id: Some(id),
                name,
                reference,
                ..
            } => {
                let current = name.clone().or_else(|| {
                    reference
                        .as_ref()
                        .map(|r| r.rsplit('.').next().unwrap_or(r).to_string())
                });
                if let Some(current) = current {
                    insert_unambiguous(&mut out, &mut poisoned, id, &current);
                }
            }
            TraitReference::Reference { .. } => {}
        }
    }
    out
}

/// `event_id → current event key` across the orbital's own inline traits'
/// declared events (the `emits` block). Events are first-class — a config knob
/// typed `event` holds a dispatchable event key, resolved by id so a call-site
/// `events: { OLD: NEW }` rename threads into the knob value like entity/trait.
fn collect_event_id_names(orbital: &Orbital) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    let mut poisoned = HashSet::new();
    for tr in &orbital.traits {
        if let TraitReference::Inline(td) = tr {
            for ev in td.state_machine.events.iter().flatten() {
                if let Some(id) = &ev.id {
                    insert_unambiguous(&mut out, &mut poisoned, id, &ev.key);
                }
            }
        }
    }
    out
}

/// Dispatch identity normalization over one trait reference (own or page),
/// covering both the inline body tokens / trait ref name AND the config-ref
/// defaults (`entity`/`trait`-typed knobs carrying a stamped `refId`).
fn normalize_trait_reference(
    tr: &mut TraitReference,
    entity_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    trait_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    event_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
) {
    match tr {
        TraitReference::Inline(td) => {
            let td = Arc::make_mut(td);
            normalize_inline_trait(td, entity_id_to_name, trait_id_to_name);
            normalize_config_ref_tokens(
                td.config.as_mut(),
                entity_id_to_name,
                trait_id_to_name,
                event_id_to_name,
            );
        }
        TraitReference::Reference {
            reference,
            ref_id,
            config,
            ..
        } => {
            normalize_trait_ref(reference, ref_id.as_deref(), trait_id_to_name);
            normalize_config_ref_tokens(
                config.as_mut(),
                entity_id_to_name,
                trait_id_to_name,
                event_id_to_name,
            );
        }
    }
}

/// Rewrite one inline trait's entity tokens, `linkedEntity`, and `@trait.<x>`
/// embeds to the current declaration names its id side-maps point at.
fn normalize_inline_trait(
    td: &mut TraitDefinition,
    entity_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    trait_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
) {
    normalize_trait_entity_tokens(td, entity_id_to_name);
    normalize_trait_embed_tokens(td, trait_id_to_name);
}

/// Config-ref side: `entity`/`trait`/`event`-typed config knobs whose `default`
/// holds a reference name carry a stamped `refId` (V4-W5). Resolve the current
/// declaration name/key of the node the `refId` points at and rewrite the
/// default so a renamed entity/trait/event flows into the config value the same
/// way body tokens do.
fn normalize_config_ref_tokens(
    config: Option<&mut orbital_core_base::collections::IndexMap<String, ConfigField>>,
    entity_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    trait_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    event_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
) {
    let Some(config) = config else { return };
    for cf in config.values_mut() {
        let Some(ref_id) = cf.ref_id.as_deref() else {
            continue;
        };
        let cur = match cf.field_type.as_str() {
            "entity" => entity_id_to_name.get(ref_id).cloned(),
            "trait" => trait_id_to_name.get(ref_id).map(|n| format!("@trait.{n}")),
            "event" => event_id_to_name.get(ref_id).cloned(),
            _ => None,
        };
        if let Some(cur) = cur {
            if cf.default.as_ref() != Some(&SExpression::string(cur.clone())) {
                cf.default = Some(SExpression::string(cur));
            }
        }
    }
}

/// Entity side: `entity_ref_ids` body tokens (`@entity`, positional
/// `fetch`/`persist`/`ref`/`spawn`, payload types) + `linked_entity_id`.
fn normalize_trait_entity_tokens(
    td: &mut TraitDefinition,
    entity_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
) {
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

    let mut entity_subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
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

    let no_events: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    for p in td.propositions_mut() {
        rewrite_identifiers(p, &entity_subs, &no_events);
    }
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
fn normalize_trait_embed_tokens(
    td: &mut TraitDefinition,
    trait_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if trait_id_to_name.is_empty() {
        return;
    }
    let mut trait_subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
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
    rewrite_trait_embed_tokens(td, &trait_subs);
}

/// Apply a `@trait.<from>` → `@trait.<to>` substitution map across every token
/// position a trait body carries: transition effects, tick effects, AND
/// config-field defaults (std-browse's `bodyContent`/`denseBodyContent` shape —
/// `apply_inline_config_defaults`/`rewrite_config_bindings` substitute defaults
/// into effect trees, so a rename that skips them leaves dangling tokens in the
/// default and in every substituted copy). Shared by the id-driven
/// normalization above and the name-driven uniquify rename in `inline/mod.rs`
/// (whose Reference-trait siblings carry no embed id to normalize by).
pub(super) fn rewrite_trait_embed_tokens(
    td: &mut TraitDefinition,
    trait_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    for transition in td.state_machine.transitions.iter_mut() {
        if let Some(effects) = transition.effects.as_mut() {
            for effect in effects.iter_mut() {
                rewrite_trait_embed_in_sexpr(effect, trait_subs);
            }
        }
    }
    if let Some(ticks) = td.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            for effect in tick.effects.iter_mut() {
                rewrite_trait_embed_in_sexpr(effect, trait_subs);
            }
        }
    }
    if let Some(config) = td.config.as_mut() {
        for cf in config.values_mut() {
            if let Some(default) = cf.default.as_mut() {
                rewrite_trait_embed_in_sexpr(default, trait_subs);
            }
        }
    }
}

/// Rewrite `@trait.<from>` embed roots to `@trait.<to>` in an S-expression tree.
/// Boundary-checked (next char `.`/`[`/end) so `@trait.FooBar` is not matched by
/// a `Foo` sub.
/// `@trait.<X>` embeds inside config defaults (a composed trait's call-site
/// `config {}` override, e.g. an `AppLayout`'s `contentTrait`).
pub(super) fn rewrite_config_embed_tokens(
    config: &mut orbital_core_base::collections::IndexMap<String, ConfigField>,
    trait_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    for cf in config.values_mut() {
        if let Some(default) = cf.default.as_mut() {
            rewrite_trait_embed_in_sexpr(default, trait_subs);
        }
    }
}

fn rewrite_trait_embed_in_sexpr(expr: &mut SExpression, subs: &orbital_core_base::collections::IndexMap<String, String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                let split_at = rest.find(['.', '[']).unwrap_or(rest.len());
                let (name, suffix) = rest.split_at(split_at);
                if let Some(to) = subs.get(name) {
                    *s = format!("@trait.{}{}", to, suffix).into();
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
    trait_id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
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

    /// A renamed inline-entity declaration drags its
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
        assert_eq!(
            effects[0],
            SExpression::Array(vec![
                SExpression::string("persist"),
                SExpression::string("create"),
                SExpression::string("Record"),
            ])
        );
        assert_eq!(
            effects[1],
            SExpression::Array(vec![
                SExpression::string("fetch"),
                SExpression::string("Record"),
            ])
        );
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
        assert_eq!(
            *effect,
            SExpression::Array(vec![
                SExpression::string("render-ui"),
                SExpression::string("main"),
                SExpression::string("@trait.BrowseView"),
            ])
        );
    }

    /// An `entity`-typed config knob carrying a stamped `refId` rewrites its
    /// `default` to the renamed entity's current name; a `trait`-typed knob
    /// rewrites to `@trait.<current>`. Covers both inline and page-ref config.
    #[test]
    fn rewrites_config_refs_by_id() {
        let mut s = schema(json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Record", "fields": [], "id": "ent_a" },
                "traits": [
                    {
                        "name": "Target", "id": "trt_a", "linkedEntity": "Record",
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }]
                        }
                    },
                    {
                        "name": "Lifecycle", "id": "trt_b", "linkedEntity": "Record",
                        "config": {
                            "targetEntity": { "type": "entity", "default": "Item", "refId": "ent_a" },
                            "targetTrait": { "type": "trait", "default": "@trait.Old", "refId": "trt_a" },
                            "targetEvent": { "type": "event", "default": "OLD_EVENT", "refId": "evt_a" }
                        },
                        "stateMachine": {
                            "states": [{ "name": "idle", "isInitial": true }],
                            "transitions": [{ "from": "idle", "to": "idle", "event": "SAVE" }],
                            "events": [{ "key": "SAVE", "id": "evt_a" }]
                        }
                    }
                ]
            }]
        }));
        normalize_identity_tokens(&mut s);
        let td = match &s.orbitals[0].traits[1] {
            TraitReference::Inline(td) => td,
            _ => panic!("inline"),
        };
        let config = td.config.as_ref().unwrap();
        assert_eq!(
            config["targetEntity"].default,
            Some(SExpression::string("Record"))
        );
        assert_eq!(
            config["targetTrait"].default,
            Some(SExpression::string("@trait.Target"))
        );
        // event ref resolves by id to the event's current key
        assert_eq!(
            config["targetEvent"].default,
            Some(SExpression::string("SAVE"))
        );
    }

    /// An id carried by two declarations that DISAGREE on the name resolves to
    /// nothing: neither knob default is rewritten (no arbitrary winner). This
    /// is the stale-artifact heal for the multi-instance shared-id clobber —
    /// a fresh artifact never reaches here with a shared id (inline's event-id
    /// hygiene re-derives per instance).
    #[test]
    fn ambiguous_shared_event_id_resolves_to_nothing() {
        let trait_json = |name: &str, key: &str| {
            json!({
                "name": name, "linkedEntity": "Note",
                "config": {
                    "searchEvent": { "type": "event", "default": key, "refId": "evt_shared" }
                },
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": key }],
                    "events": [{ "key": key, "id": "evt_shared" }]
                }
            })
        };
        let mut s = schema(json!({
            "version": "2.0", "name": "T",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Note", "fields": [], "id": "ent_a" },
                "traits": [trait_json("A", "NOTE_SEARCH"), trait_json("B", "SEARCH")]
            }]
        }));
        normalize_identity_tokens(&mut s);
        let default_of = |i: usize| match &s.orbitals[0].traits[i] {
            TraitReference::Inline(td) => {
                td.config.as_ref().unwrap()["searchEvent"].default.clone()
            }
            _ => panic!("inline"),
        };
        assert_eq!(
            default_of(0),
            Some(SExpression::string("NOTE_SEARCH"))
        );
        assert_eq!(default_of(1), Some(SExpression::string("SEARCH")));
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
                            "effects": [["fetch", "Item"]]
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
