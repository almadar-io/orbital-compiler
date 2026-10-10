//! A composed placement wired to nothing. An atom's own inputs and outputs are
//! its offer; once a program composes it (`sourceBehavior` set), an input
//! nothing produces leaves part of it dead, and a control value nothing keeps
//! is discarded. Both are decidable on the resolved IR:
//!
//! - `ORB_COMPOSED_STATE_UNREACHABLE` — a state of a composed trait that no
//!   event the program can produce leads to.
//! - `ORB_CONTROL_VALUE_DISCARDED` — a composed trait's rendered control fires
//!   an event carrying a value that no other trait hears and whose own
//!   handling reads none of it.

use std::collections::{HashSet, VecDeque};

use super::fired_events::{collect_schema_fired_events, FiredEvents};
use super::{ErrorCode, ValidationContext, ValidationError};
use orbital_core::schema::{
    EventScope, ListenSource, OrbitalSchema, SExpression, StringOrArray, TraitDefinition,
    TraitReference,
};

pub fn validate_composed_wiring(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let fired = collect_schema_fired_events(schema);
    let produced = produced_by_name(schema, &fired);
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
            unreachable_states(td, &produced, &path, ctx);
            discarded_values(schema, td, &traits, &orbital.name, &path, ctx);
        }
    }
}

/// Every event name the program can produce, by name: control and effect emits,
/// plus every value of an `event`-typed config knob (a button's `action`), which
/// `FiredEvents` leaves out on purpose — over-counting producers only silences.
fn produced_by_name(schema: &OrbitalSchema, fired: &FiredEvents) -> HashSet<String> {
    let mut out: HashSet<String> = fired
        .by_control
        .iter()
        .chain(fired.by_effect.iter())
        .map(|(_, e)| e.clone())
        .collect();
    for orbital in &schema.orbitals {
        for t in &orbital.traits {
            let TraitReference::Inline(td) = t else {
                continue;
            };
            for field in td.config.iter().flat_map(|c| c.values()) {
                if field.field_type == "event" {
                    if let Some(default) = &field.default {
                        strings_in(default, &mut out);
                    }
                }
            }
        }
    }
    out
}

fn strings_in(node: &SExpression, out: &mut HashSet<String>) {
    match node {
        SExpression::String(s) => {
            out.insert(s.clone());
        }
        SExpression::Array(items) => items.iter().for_each(|i| strings_in(i, out)),
        _ => {}
    }
}

fn from_states(from: &StringOrArray) -> Vec<&str> {
    match from {
        StringOrArray::Single(s) => vec![s.as_str()],
        StringOrArray::Multiple(v) => v.iter().map(String::as_str).collect(),
    }
}

fn unreachable_states(
    td: &TraitDefinition,
    produced: &HashSet<String>,
    path: &str,
    ctx: &mut ValidationContext,
) {
    let sm = &td.state_machine;
    let Some(initial) = sm
        .states
        .iter()
        .find(|s| s.is_initial == Some(true))
        .or(sm.states.first())
    else {
        return;
    };
    let mut inputs: HashSet<String> = produced.clone();
    inputs.insert("INIT".to_string());
    for l in td.listens.iter().flatten() {
        inputs.insert(l.triggers.clone().unwrap_or_else(|| l.event.clone()));
    }
    for e in sm.events.iter().flatten() {
        if e.external == Some(true) {
            inputs.insert(e.key.clone());
        }
    }
    // A transition whose guard folded to `false` is switched off by the composition's config.
    let live: Vec<_> = sm
        .transitions
        .iter()
        .filter(|t| !t.guard.as_ref().is_some_and(switched_off))
        .collect();
    let mut reached: HashSet<&str> = HashSet::from([initial.name.as_str()]);
    let mut walk: VecDeque<&str> = VecDeque::from([initial.name.as_str()]);
    while let Some(state) = walk.pop_front() {
        for t in &live {
            let from = from_states(&t.from);
            if !(from.contains(&state) || from.contains(&"*")) || !inputs.contains(&t.event) {
                continue;
            }
            if reached.insert(t.to.as_str()) {
                walk.push_back(t.to.as_str());
            }
        }
    }
    // Unreached states entered by a live transition on an event nothing produces.
    let dead: Vec<&str> = sm
        .states
        .iter()
        .map(|s| s.name.as_str())
        .filter(|s| !reached.contains(s))
        .filter(|s| {
            live.iter()
                .any(|t| t.to == *s && !inputs.contains(&t.event))
        })
        .collect();
    if dead.is_empty() {
        return;
    }
    let mut missing: Vec<&str> = live
        .iter()
        .filter(|t| dead.contains(&t.to.as_str()) && !inputs.contains(&t.event))
        .map(|t| t.event.as_str())
        .collect();
    missing.sort_unstable();
    missing.dedup();
    ctx.add_error(
        ValidationError::error(
            ErrorCode::ComposedStateUnreachable,
            path.to_string(),
            format!(
                "'{}' can never enter {} — it is entered only on {}, which nothing in the program sends: no listens route, no control or effect fires it, and it is not declared external",
                td.name,
                dead.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "),
                missing.join(", ")
            ),
        )
        .with_suggestion(format!("route a control's event to it: `listens {{ <Trait>.<EVENT> -> {} }}` on '{}'", missing.first().copied().unwrap_or("<EVENT>"), td.name)),
    );
}

/// True for an effect tree that keeps something beyond the screen: a `persist`, a
/// `call-service` or an `emit`, at any nesting.
/// A guard the composition's config folded to false: `false`, or an `and` with a false operand.
fn switched_off(guard: &SExpression) -> bool {
    match guard {
        SExpression::Bool(false) => true,
        SExpression::Array(items) => {
            matches!(items.first(), Some(SExpression::String(op)) if op == "and")
                && items[1..].iter().any(switched_off)
        }
        _ => false,
    }
}

fn has_consequence(node: &SExpression) -> bool {
    match node {
        SExpression::Array(items) => {
            matches!(items.first(), Some(SExpression::String(op)) if op == "persist" || op == "call-service" || op == "emit")
                || items.iter().any(has_consequence)
        }
        _ => false,
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

/// The event props of every form-contract pattern (`fieldsContract: form` — a pattern that
/// collects field values and submits them) rendered by `node`, `@config.<knob>` resolved
/// through the trait's own config.
fn form_submit_events(node: &SExpression, td: &TraitDefinition, out: &mut HashSet<String>) {
    let resolve = |v: &SExpression| -> Option<String> {
        let SExpression::String(s) = v else {
            return None;
        };
        match s.strip_prefix("@config.") {
            None => Some(s.clone()),
            Some(knob) => {
                td.config
                    .as_ref()
                    .and_then(|c| c.get(knob))
                    .and_then(|f| match &f.default {
                        Some(SExpression::String(v)) => Some(v.clone()),
                        _ => None,
                    })
            }
        }
    };
    match node {
        SExpression::Object(map) => {
            let pattern = map
                .get("type")
                .and_then(resolve)
                .and_then(|t| crate::embedded::get_pattern(&t))
                .filter(|p| p.fields_contract.as_deref() == Some("form"));
            if let Some(pattern) = pattern {
                for (prop, spec) in &pattern.props_schema {
                    let is_event =
                        matches!(spec.kind.as_deref(), Some("event") | Some("event-ref"));
                    if let (true, Some(value)) = (is_event, map.get(prop).and_then(resolve)) {
                        out.insert(value);
                    }
                }
            }
            map.values().for_each(|v| form_submit_events(v, td, out));
        }
        SExpression::Array(items) => items.iter().for_each(|i| form_submit_events(i, td, out)),
        _ => {}
    }
}

fn discarded_values(
    schema: &OrbitalSchema,
    td: &TraitDefinition,
    siblings: &[(usize, &TraitDefinition)],
    orbital: &str,
    path: &str,
    ctx: &mut ValidationContext,
) {
    let mut submits: HashSet<String> = HashSet::new();
    for t in &td.state_machine.transitions {
        for effect in t.effects.iter().flatten() {
            form_submit_events(effect, td, &mut submits);
        }
    }
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
                && (other
                    .state_machine
                    .transitions
                    .iter()
                    .any(|t| t.event == event)
                    || other.listens.iter().flatten().any(|l| {
                        l.event == event
                            && match &l.source {
                                None | Some(ListenSource::Any) => true,
                                Some(ListenSource::Trait { trait_name, .. }) => {
                                    trait_name == &td.name
                                }
                                Some(ListenSource::Orbital {
                                    orbital: o,
                                    trait_name,
                                    ..
                                }) => o == orbital && trait_name == &td.name,
                            }
                    }))
        });
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
        if offered_outward {
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
                        .any(|e| reads_payload(e) || has_consequence(e))
            });
        if kept_by_self {
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
            "name": name, "linkedEntity": "Item", "category": "interaction",
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
            "name": "AddButton", "linkedEntity": "Item", "category": "interaction",
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
            "name": "Store", "linkedEntity": "Item", "category": "lifecycle",
            "sourceBehavior": composed("RecordStore", "std/behaviors/std-record-store"),
            "listens": listens,
            "stateMachine": {
                "states": [{ "name": "ready", "isInitial": true }],
                "events": [{ "key": "INIT" }, { "key": "DO_CREATE", "external": true, "payloadSchema": [{ "name": "data", "type": "object" }] }],
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

    #[test]
    fn a_composed_form_nothing_opens_is_unreachable() {
        let found = hits(
            program(vec![form("ItemForm", serde_json::json!([]), true)]),
            ErrorCode::ComposedStateUnreachable,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].message.contains("ItemForm")
                && found[0].message.contains("open")
                && found[0].message.contains("OPEN"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn control_a_routed_opener_reaches_the_form() {
        let traits = vec![
            opener("ACTION"),
            form(
                "ItemForm",
                serde_json::json!([route("AddButton", "ACTION", "OPEN")]),
                true,
            ),
        ];
        assert!(hits(program(traits), ErrorCode::ComposedStateUnreachable).is_empty());
    }

    #[test]
    fn control_a_control_firing_the_input_by_name_reaches_the_form() {
        let traits = vec![
            opener("OPEN"),
            form("ItemForm", serde_json::json!([]), true),
        ];
        assert!(hits(program(traits), ErrorCode::ComposedStateUnreachable).is_empty());
    }

    #[test]
    fn edge_an_atom_authoring_its_own_trait_offers_the_input() {
        assert!(hits(
            program(vec![form("ModalRecordModal", serde_json::json!([]), false)]),
            ErrorCode::ComposedStateUnreachable
        )
        .is_empty());
    }

    #[test]
    fn edge_an_input_declared_external_is_the_host_s_to_send() {
        let mut f = form("ItemForm", serde_json::json!([]), true);
        f["stateMachine"]["events"][1]["external"] = serde_json::json!(true);
        assert!(hits(program(vec![f]), ErrorCode::ComposedStateUnreachable).is_empty());
    }

    #[test]
    fn edge_a_state_the_composition_switched_off_by_config_is_not_unwired() {
        let mut f = form("ItemForm", serde_json::json!([]), true);
        f["stateMachine"]["transitions"][1]["guard"] = serde_json::json!(false);
        assert!(hits(program(vec![f]), ErrorCode::ComposedStateUnreachable).is_empty());
    }

    #[test]
    fn edge_a_conjunction_with_a_config_off_operand_is_switched_off() {
        let mut f = form("ItemForm", serde_json::json!([]), true);
        f["stateMachine"]["transitions"][1]["guard"] =
            serde_json::json!(["and", false, ["=", "@payload.id", "x"]]);
        assert!(hits(program(vec![f]), ErrorCode::ComposedStateUnreachable).is_empty());
    }

    #[test]
    fn control_a_live_conjunction_still_counts() {
        let mut f = form("ItemForm", serde_json::json!([]), true);
        f["stateMachine"]["transitions"][1]["guard"] =
            serde_json::json!(["and", true, ["=", "@payload.id", "x"]]);
        assert_eq!(
            hits(program(vec![f]), ErrorCode::ComposedStateUnreachable).len(),
            1
        );
    }

    // --- ORB_CONTROL_VALUE_DISCARDED ---

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
