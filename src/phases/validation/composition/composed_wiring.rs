//! A composed placement wired to nothing. Once a program composes an atom
//! (`sourceBehavior` set), a control value nothing keeps is discarded. This is
//! decidable on the resolved IR:
//!
//! - `ORB_CONTROL_VALUE_DISCARDED` — a composed trait's rendered control fires
//!   an event carrying a value that no other trait hears and whose own
//!   handling reads none of it. An event the trait offers outward (`-> input`) defers to an
//!   importer, except in a finished program (`orb validate --final`, `discarded_offered_values`).

use std::collections::HashSet;

use super::final_program::{embedder_closure, routes};
use super::{ErrorCode, ValidationContext, ValidationError};
use crate::phases::embed_graph::EmbedGraph;
use orbital_core_base::schema::{
    EntityRef, EventScope, ListenDefinition, ListenSource, Orbital, OrbitalSchema, Persistence,
    SExpression, TraitDefinition, TraitReference,
};

pub fn validate_composed_wiring(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let traits: Vec<(usize, &TraitDefinition)> = orbital
            .traits
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match t {
                TraitReference::Inline(td) => Some((i, td)),
                _ => None,
            })
            .collect();
        for &(ti, td) in &traits {
            if td.source_behavior.is_none() {
                continue;
            }
            let path = format!("orbitals[{oi}].traits[{ti}]");
            discarded_values(schema, orbital, td, &traits, &path, false, ctx);
        }
    }
}

/// `ORB_CONTROL_VALUE_DISCARDED` for the values a composed trait offers outward: in a finished
/// program no importer is left to hear them.
pub fn discarded_offered_values(schema: &OrbitalSchema) -> Vec<ValidationError> {
    let mut ctx = ValidationContext::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let traits: Vec<(usize, &TraitDefinition)> = orbital
            .traits
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match t {
                TraitReference::Inline(td) => Some((i, td)),
                _ => None,
            })
            .collect();
        for &(ti, td) in &traits {
            if td.source_behavior.is_some() {
                let path = format!("orbitals[{oi}].traits[{ti}]");
                discarded_values(schema, orbital, td, &traits, &path, true, &mut ctx);
            }
        }
    }
    ctx.result.errors
}

/// True for an effect tree that keeps something beyond the screen: a `persist`, a
/// `call-service` or an `emit`, at any nesting.
fn has_consequence(node: &SExpression) -> bool {
    match node {
        SExpression::Array(items) => {
            matches!(items.first(), Some(SExpression::String(op)) if op == "persist" || op == "call-service" || op == "emit")
                || items.iter().any(has_consequence)
        }
        _ => false,
    }
}

/// Fields of `td`'s runtime entity that no trait bound to it reads: a write to one keeps nothing.
fn unread_runtime_fields(
    owner: &Orbital,
    td: &TraitDefinition,
    siblings: &[(usize, &TraitDefinition)],
) -> HashSet<String> {
    let primary = match &owner.entity {
        EntityRef::Inline(def) => Some(def.name.as_str()),
        _ => None,
    };
    let Some(entity_name) = td.linked_entity.as_deref().or(primary) else {
        return HashSet::new();
    };
    let entity = std::iter::once(&owner.entity)
        .chain(owner.auxiliary_entities.iter().flatten())
        .find_map(|e| match e {
            EntityRef::Inline(def) if def.name == entity_name => Some(def),
            _ => None,
        });
    let Some(entity) = entity.filter(|e| matches!(e.persistence, Some(Persistence::Runtime)))
    else {
        return HashSet::new();
    };
    let mut read: HashSet<String> = HashSet::new();
    for (_, t) in siblings {
        if t.linked_entity.as_deref().or(primary) != Some(entity_name) {
            continue;
        }
        for tr in &t.state_machine.transitions {
            tr.guard
                .iter()
                .for_each(|g| g.collect_entity_reads(&mut read));
            tr.effects
                .iter()
                .flatten()
                .for_each(|e| e.collect_entity_reads(&mut read));
        }
        for tick in t.ticks.iter().flatten() {
            tick.effects
                .iter()
                .for_each(|e| e.collect_entity_reads(&mut read));
        }
    }
    entity
        .fields
        .iter()
        .filter_map(|f| f.name.clone())
        .filter(|f| !read.contains(f))
        .collect()
}

/// `reads_payload`, except that storing the value in an unread runtime field keeps none of it.
fn keeps_payload(node: &SExpression, unread: &HashSet<String>) -> bool {
    match node {
        SExpression::Array(items) => {
            let unread_write = matches!(
                (items.first(), items.get(1)),
                (Some(SExpression::String(op)), Some(SExpression::String(target)))
                    if op == "set"
                        && target
                            .strip_prefix("@entity.")
                            .and_then(|p| p.split(['.', '[']).next())
                            .is_some_and(|f| unread.contains(f))
            );
            !unread_write && items.iter().any(|i| keeps_payload(i, unread))
        }
        SExpression::Object(map) => map.values().any(|v| keeps_payload(v, unread)),
        other => reads_payload(other),
    }
}

fn reads_payload(node: &SExpression) -> bool {
    match node {
        SExpression::String(s) => s.starts_with("@payload") || s.starts_with('?'),
        SExpression::Array(items) => items.iter().any(reads_payload),
        SExpression::Object(map) => map.values().any(reads_payload),
        _ => false,
    }
}

/// The value-carrying control events rendered by `node`: every event prop of a form-contract
/// pattern (`fieldsContract: form`) and every event prop that declares an `emitPayloadSchema`,
/// except an `@notification` prop (it reports a change already applied, e.g. hover),
/// `@config.<knob>` resolved through the trait's own config.
fn value_control_events(node: &SExpression, td: &TraitDefinition, out: &mut HashSet<String>) {
    let resolve = |v: &SExpression| -> Option<String> {
        let SExpression::String(s) = v else {
            return None;
        };
        match s.strip_prefix("@config.") {
            None => Some(s.clone().into_inner()),
            Some(knob) => {
                (td.config
                    .as_ref()
                    .and_then(|c| c.get(knob))
                    .and_then(|f| match &f.default {
                        Some(SExpression::String(v)) => Some(v.clone()),
                        _ => None,
                    })).map(|v| v.into_inner())
            }
        }
    };
    match node {
        SExpression::Object(map) => {
            let pattern = map
                .get("type")
                .and_then(resolve)
                .and_then(|t| crate::embedded::get_pattern(&t));
            if let Some(pattern) = pattern {
                let form = pattern.fields_contract.as_deref() == Some("form");
                for (prop, spec) in &pattern.props_schema {
                    let is_event = !spec.notification
                        && matches!(spec.kind.as_deref(), Some("event") | Some("event-ref"));
                    let carries = form
                        || spec
                            .emit_payload_schema
                            .as_ref()
                            .is_some_and(|p| !p.is_empty());
                    if let (true, Some(value)) =
                        (is_event && carries, map.get(prop).and_then(resolve))
                    {
                        out.insert(value);
                    }
                }
            }
            map.values().for_each(|v| value_control_events(v, td, out));
        }
        SExpression::Array(items) => items.iter().for_each(|i| value_control_events(i, td, out)),
        _ => {}
    }
}

fn discarded_values(
    schema: &OrbitalSchema,
    owner: &Orbital,
    td: &TraitDefinition,
    siblings: &[(usize, &TraitDefinition)],
    path: &str,
    final_program: bool,
    ctx: &mut ValidationContext,
) {
    let orbital = owner.name.as_str();
    let mut submits: HashSet<String> = HashSet::new();
    for t in &td.state_machine.transitions {
        for effect in t.effects.iter().flatten() {
            value_control_events(effect, td, &mut submits);
        }
    }
    let unread = unread_runtime_fields(owner, td, siblings);
    let embed = EmbedGraph::build(&owner.traits);
    let scope = embedder_closure(&embed, &td.name);
    let owner_listens: Vec<&ListenDefinition> = owner
        .listens
        .iter()
        .flatten()
        .chain(
            siblings
                .iter()
                .flat_map(|(_, t)| t.listens.iter().flatten()),
        )
        .collect();
    let mut events: Vec<String> = submits.into_iter().collect();
    events.sort_unstable();
    for event in events.iter().map(String::as_str) {
        let carries = td
            .state_machine
            .events
            .iter()
            .flatten()
            .find(|e| e.key == event)
            .is_some_and(|e| e.payload_schema.as_ref().is_some_and(|p| !p.is_empty()));
        if !carries {
            continue;
        }
        let heard_by_sibling = siblings.iter().any(|(_, other)| {
            other.name != td.name
                && scope.contains(other.name.as_str())
                && other
                    .state_machine
                    .transitions
                    .iter()
                    .any(|t| t.event == event)
        }) || owner_listens.iter().any(|l| routes(l, event, &scope));
        // A route from another orbital names this one and this trait (`Orbital.Trait.EVENT`).
        let heard_across = schema.orbitals.iter().filter(|o| o.name != orbital).any(|o| {
            o.traits.iter().any(|t| match t {
                TraitReference::Inline(other) => other.listens.iter().flatten().any(|l| {
                    l.event == event
                        && matches!(&l.source, Some(ListenSource::Orbital { orbital: src, trait_name, .. }) if src == orbital && trait_name == &td.name)
                }),
                _ => false,
            })
        });
        if heard_by_sibling || heard_across {
            continue;
        }
        let offered_outward = td
            .emits
            .iter()
            .flatten()
            .any(|e| e.event == event && matches!(e.scope, Some(EventScope::External)));
        if offered_outward && !final_program {
            continue;
        }
        let kept_by_self = td
            .state_machine
            .transitions
            .iter()
            .filter(|t| t.event == event)
            .any(|t| {
                t.guard.as_ref().is_some_and(reads_payload)
                    || t.effects
                        .iter()
                        .flatten()
                        .any(|e| keeps_payload(e, &unread) || has_consequence(e))
            });
        if kept_by_self || final_program != offered_outward {
            continue;
        }
        ctx.add_error(
            ValidationError::error(
                ErrorCode::ControlValueDiscarded,
                path.to_string(),
                format!(
                    "'{}' renders a control that fires {} with a value, but no other trait hears {} and its own {} arm reads none of it — what the user entered is discarded",
                    td.name, event, event, event
                ),
            )
            .with_suggestion(format!("route it to a trait that keeps it: `listens {{ {}.{} -> <its input> }}` on that trait", td.name, event)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phases::validation::ValidationError;

    fn hits(schema: serde_json::Value, code: ErrorCode) -> Vec<ValidationError> {
        let schema: OrbitalSchema =
            serde_json::from_value(schema).expect("schema fixture deserializes");
        let mut ctx = ValidationContext::new();
        validate_composed_wiring(&schema, &mut ctx);
        ctx.result
            .errors
            .into_iter()
            .filter(|e| e.code == code)
            .collect()
    }

    fn composed(name: &str, behavior: &str) -> serde_json::Value {
        serde_json::json!({ "alias": "A", "behavior": behavior, "originalName": name })
    }

    /// A composed form: `OPEN` enters `open`, its rendered submit control fires `SAVE { data }`,
    /// and its own `SAVE` arm only closes it.
    fn form(name: &str, listens: serde_json::Value, source: bool) -> serde_json::Value {
        let mut t = serde_json::json!({
            "name": name, "linkedEntity": "Item",
            "listens": listens,
            "stateMachine": {
                "states": [{ "name": "closed", "isInitial": true }, { "name": "open" }],
                "events": [
                    { "key": "INIT" },
                    { "key": "OPEN", "payloadSchema": [{ "name": "id", "type": "string" }] },
                    { "key": "SAVE", "payloadSchema": [{ "name": "data", "type": "object" }] }
                ],
                "transitions": [
                    { "from": "closed", "to": "closed", "event": "INIT" },
                    { "from": "closed", "to": "open", "event": "OPEN",
                      "effects": [["render-ui", "modal", { "type": "stack", "children": [{ "type": "form-section", "entity": "Item", "submitEvent": "SAVE", "cancelEvent": "CLOSE" }] }]] },
                    { "from": "open", "to": "closed", "event": "SAVE", "effects": [["render-ui", "modal", null]] }
                ]
            }
        });
        if source {
            t["sourceBehavior"] = composed("ModalRecordModal", "std/behaviors/std-modal");
        }
        t
    }

    fn opener(action: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "AddButton", "linkedEntity": "Item",
            "sourceBehavior": composed("ButtonRender", "std/behaviors/ui-button"),
            "config": { "action": { "type": "event", "default": action } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": action }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "INIT",
                    "effects": [["render-ui", "main", { "type": "button", "label": "Add", "action": "@config.action" }]] }]
            }
        })
    }

    fn store(listens: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": "Store", "linkedEntity": "Item",
            "sourceBehavior": composed("RecordStore", "std/behaviors/std-record-store"),
            "listens": listens,
            "stateMachine": {
                "states": [{ "name": "ready", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": "DO_CREATE", "input": true, "payloadSchema": [{ "name": "data", "type": "object" }] }],
                "transitions": [
                    { "from": "ready", "to": "ready", "event": "INIT" },
                    { "from": "ready", "to": "ready", "event": "DO_CREATE", "effects": [["persist", "create", "Item", "@payload.data"]] }
                ]
            }
        })
    }

    fn program(traits: Vec<serde_json::Value>) -> serde_json::Value {
        let refs: Vec<serde_json::Value> = traits
            .iter()
            .map(|t| serde_json::json!({ "ref": t["name"] }))
            .collect();
        serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "ItemOrbital",
                "entity": { "name": "Item", "persistence": "persistent", "collection": "items", "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": traits,
                "pages": [{ "name": "ItemsPage", "path": "/items", "traits": refs }]
            }]
        })
    }

    fn route(source: &str, event: &str, triggers: &str) -> serde_json::Value {
        serde_json::json!({ "event": event, "source": { "kind": "trait", "trait": source }, "triggers": triggers })
    }

    // --- ORB_COMPOSED_STATE_UNREACHABLE ---

    // --- ORB_COMPOSED_TRANSITION_UNREACHABLE (G-CROSS-144) ---

    /// A composed list: `LOADED` (its fetch's success) settles it, `REFETCH` reloads it from
    /// `browsing`. Both states are reachable, so only the transition can be dead.
    fn browse(listens: serde_json::Value, source: bool) -> serde_json::Value {
        let mut t = serde_json::json!({
            "name": "ItemBrowse", "linkedEntity": "Item",
            "listens": listens,
            "stateMachine": {
                "states": [{ "name": "loading", "isInitial": true }, { "name": "browsing" }],
                "events": [{ "key": "INIT" }, { "key": "LOADED" }, { "key": "REFETCH" }],
                "transitions": [
                    { "from": "loading", "to": "loading", "event": "INIT",
                      "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                    { "from": "loading", "to": "browsing", "event": "LOADED" },
                    { "from": "browsing", "to": "loading", "event": "REFETCH",
                      "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] }
                ]
            }
        });
        if source {
            t["sourceBehavior"] = composed("BrowseList", "std/behaviors/std-browse");
        }
        t
    }

    fn with_declared(mut t: serde_json::Value, key: &str) -> serde_json::Value {
        for ev in t["stateMachine"]["events"].as_array_mut().expect("events") {
            if ev["key"] == key {
                ev["listened"] = serde_json::Value::Bool(true);
            }
        }
        t
    }

    // --- ORB_CONTROL_VALUE_DISCARDED ---

    /// A composed std-search: its search box fires `SEARCH { searchTerm }`, offered outward,
    /// and its own arm only stores the term on its runtime entity and repaints the box.
    fn search(effects_extra: Vec<serde_json::Value>) -> serde_json::Value {
        let render =
            serde_json::json!(["render-ui", "main", { "type": "search-input", "event": "SEARCH" }]);
        let mut arm = vec![
            serde_json::json!(["set", "@entity.searchTerm", "?searchTerm"]),
            render.clone(),
        ];
        arm.extend(effects_extra);
        serde_json::json!({
            "name": "NoteSearch", "linkedEntity": "SearchResult",
            "sourceBehavior": composed("SearchResultSearch", "std/behaviors/std-search"),
            "emits": [{ "event": "SEARCH", "scope": "external", "payloadSchema": [{ "name": "searchTerm", "type": "string", "required": true }] }],
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": "SEARCH", "listened": true, "payloadSchema": [{ "name": "searchTerm", "type": "string", "required": true }] }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [render] },
                    { "from": "idle", "to": "idle", "event": "SEARCH", "effects": arm }
                ]
            }
        })
    }

    fn search_program(traits: Vec<serde_json::Value>) -> serde_json::Value {
        let mut p = program(traits);
        p["orbitals"][0]["entity"] = serde_json::json!({
            "name": "SearchResult", "persistence": "runtime",
            "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "searchTerm", "type": "string" }]
        });
        p
    }

    fn final_hits(schema: serde_json::Value) -> Vec<ValidationError> {
        let schema: OrbitalSchema =
            serde_json::from_value(schema).expect("schema fixture deserializes");
        discarded_offered_values(&schema)
    }

    #[test]
    fn final_a_search_box_that_only_stores_an_unread_term_discards_it() {
        let found = final_hits(search_program(vec![search(vec![])]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, ErrorCode::ControlValueDiscarded);
        assert!(
            found[0].message.contains("NoteSearch") && found[0].message.contains("SEARCH"),
            "{}",
            found[0].message
        );
    }

    /// A composed landing section: a `box` reporting `HOVER { hovered }` through its
    /// `@notification` hoverEvent, offered outward and heard by nothing.
    fn hover_box() -> serde_json::Value {
        let render =
            serde_json::json!(["render-ui", "main", { "type": "box", "hoverEvent": "HOVER" }]);
        serde_json::json!({
            "name": "TaskIntro", "linkedEntity": "SearchResult",
            "sourceBehavior": composed("BoxRender", "std/behaviors/ui-box"),
            "emits": [{ "event": "HOVER", "scope": "external", "payloadSchema": [{ "name": "hovered", "type": "boolean", "required": true }] }],
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": "HOVER", "payloadSchema": [{ "name": "hovered", "type": "boolean", "required": true }] }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [render.clone()] },
                    { "from": "idle", "to": "idle", "event": "HOVER", "effects": [render] }
                ]
            }
        })
    }

    #[test]
    fn final_a_notification_event_nothing_hears_is_not_discarded_input() {
        assert!(final_hits(search_program(vec![hover_box()])).is_empty());
        assert_eq!(
            final_hits(search_program(vec![hover_box(), search(vec![])])).len(),
            1
        );
    }

    fn layout_with_empty_search_arm() -> serde_json::Value {
        serde_json::json!({
            "name": "DetailLayout", "linkedEntity": "SearchResult",
            "sourceBehavior": composed("AppLayout", "std/behaviors/std-app-layout"),
            "stateMachine": {
                "states": [{ "name": "composing", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": "SEARCH" }],
                "transitions": [
                    { "from": "composing", "to": "composing", "event": "INIT" },
                    { "from": "composing", "to": "composing", "event": "SEARCH" }
                ]
            }
        })
    }

    #[test]
    fn final_a_same_named_arm_on_a_trait_that_does_not_embed_the_search_does_not_hear_it() {
        let found = final_hits(search_program(vec![
            search(vec![]),
            layout_with_empty_search_arm(),
        ]));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn control_final_an_embedder_that_handles_the_event_hears_it() {
        let mut layout = layout_with_empty_search_arm();
        layout["stateMachine"]["transitions"][0]["effects"] = serde_json::json!([["render-ui", "main", { "type": "stack", "children": ["@trait.NoteSearch"] }]]);
        assert!(final_hits(search_program(vec![search(vec![]), layout])).is_empty());
    }

    #[test]
    fn control_without_final_an_outward_offer_still_defers_to_an_importer() {
        assert!(hits(
            search_program(vec![search(vec![])]),
            ErrorCode::ControlValueDiscarded
        )
        .is_empty());
    }

    #[test]
    fn control_final_a_sibling_routing_the_search_keeps_it() {
        let consumer = with_declared(
            browse(
                serde_json::json!([route("NoteSearch", "SEARCH", "REFETCH")]),
                true,
            ),
            "REFETCH",
        );
        assert!(final_hits(search_program(vec![search(vec![]), consumer])).is_empty());
    }

    #[test]
    fn control_final_a_term_another_trait_reads_is_kept() {
        let mut reader = browse(serde_json::json!([]), true);
        reader["linkedEntity"] = serde_json::json!("SearchResult");
        reader["stateMachine"]["transitions"][1]["effects"] = serde_json::json!([["render-ui", "main", { "type": "typography", "content": "@entity.searchTerm" }]]);
        assert!(final_hits(search_program(vec![search(vec![]), reader])).is_empty());
    }

    #[test]
    fn control_final_an_arm_that_shows_the_term_keeps_it() {
        let shown = serde_json::json!(["render-ui", "aside", { "type": "typography", "content": "?searchTerm" }]);
        assert!(final_hits(search_program(vec![search(vec![shown])])).is_empty());
    }

    #[test]
    fn edge_a_persistent_entity_write_is_not_judged_by_reads() {
        let mut p = search_program(vec![search(vec![])]);
        p["orbitals"][0]["entity"]["persistence"] = serde_json::json!("persistent");
        p["orbitals"][0]["entity"]["collection"] = serde_json::json!("results");
        assert!(final_hits(p).is_empty());
    }

    #[test]
    fn edge_a_control_without_a_declared_value_payload_is_not_a_value_control() {
        let mut t = search(vec![]);
        t["stateMachine"]["transitions"][0]["effects"] = serde_json::json!([["render-ui", "main", { "type": "button", "label": "Go", "action": "SEARCH" }]]);
        t["stateMachine"]["transitions"][1]["effects"] =
            serde_json::json!([["set", "@entity.searchTerm", "?searchTerm"]]);
        assert!(final_hits(search_program(vec![t])).is_empty());
    }

    #[test]
    fn control_a_button_firing_an_id_is_not_a_submission() {
        let mut f = form(
            "ItemForm",
            serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
            true,
        );
        f["stateMachine"]["transitions"][1]["effects"] = serde_json::json!([["render-ui", "modal", { "type": "button", "label": "Back", "action": "SAVE" }]]);
        assert!(hits(
            program(vec![opener("ACTION"), f]),
            ErrorCode::ControlValueDiscarded
        )
        .is_empty());
    }

    #[test]
    fn edge_an_event_the_atom_offers_outward_is_the_consumer_s_choice() {
        let mut f = form(
            "ItemForm",
            serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
            true,
        );
        f["emits"] = serde_json::json!([{ "event": "SAVE", "scope": "external" }]);
        assert!(hits(
            program(vec![opener("ACTION"), f]),
            ErrorCode::ControlValueDiscarded
        )
        .is_empty());
    }

    #[test]
    fn control_an_event_the_atom_keeps_internal_is_still_discarded() {
        let mut f = form(
            "ItemForm",
            serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
            true,
        );
        f["emits"] = serde_json::json!([{ "event": "SAVE" }]);
        assert_eq!(
            hits(
                program(vec![opener("ACTION"), f]),
                ErrorCode::ControlValueDiscarded
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_submission_nothing_keeps_is_discarded() {
        let traits = vec![
            opener("ACTION"),
            form(
                "ItemForm",
                serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
                true,
            ),
        ];
        let found = hits(program(traits), ErrorCode::ControlValueDiscarded);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].message.contains("ItemForm") && found[0].message.contains("SAVE"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn control_a_submission_routed_to_a_store_is_kept() {
        let traits = vec![
            opener("ACTION"),
            form(
                "ItemForm",
                serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
                true,
            ),
            store(serde_json::json!([route("ItemForm", "SAVE", "DO_CREATE")])),
        ];
        assert!(hits(program(traits), ErrorCode::ControlValueDiscarded).is_empty());
    }

    #[test]
    fn edge_a_form_whose_own_save_arm_reads_the_value_keeps_it() {
        let mut f = form(
            "ItemForm",
            serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
            true,
        );
        f["stateMachine"]["transitions"][2]["effects"] =
            serde_json::json!([["persist", "create", "Item", "@payload.data"]]);
        assert!(hits(
            program(vec![opener("ACTION"), f]),
            ErrorCode::ControlValueDiscarded
        )
        .is_empty());
    }

    #[test]
    fn control_a_submission_routed_to_another_orbital_is_kept() {
        let mut app = program(vec![
            opener("ACTION"),
            form(
                "ItemForm",
                serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
                true,
            ),
        ]);
        let mut store_orbital = app["orbitals"][0].clone();
        store_orbital["name"] = serde_json::json!("StoreOrbital");
        store_orbital["traits"] = serde_json::json!([store(serde_json::json!([{
            "event": "SAVE", "source": { "kind": "orbital", "orbital": "ItemOrbital", "trait": "ItemForm" }, "triggers": "DO_CREATE"
        }]))]);
        store_orbital["pages"] = serde_json::json!([{ "name": "StorePage", "path": "/store", "traits": [{ "ref": "Store" }] }]);
        app["orbitals"].as_array_mut().unwrap().push(store_orbital);
        assert!(hits(app, ErrorCode::ControlValueDiscarded).is_empty());
    }

    #[test]
    fn edge_a_form_whose_own_save_arm_persists_its_drafts_keeps_them() {
        let mut f = form(
            "ItemForm",
            serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
            true,
        );
        f["stateMachine"]["transitions"][2]["effects"] = serde_json::json!([["persist", "create", "Item", { "comment": "@entity.draftComment" }]]);
        assert!(hits(
            program(vec![opener("ACTION"), f]),
            ErrorCode::ControlValueDiscarded
        )
        .is_empty());
    }

    #[test]
    fn edge_an_atom_authoring_its_own_form_offers_the_value() {
        let traits = vec![
            opener("OPEN"),
            form("ModalRecordModal", serde_json::json!([]), false),
        ];
        assert!(hits(program(traits), ErrorCode::ControlValueDiscarded).is_empty());
    }
}
