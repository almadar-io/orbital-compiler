//! `orb validate --final`: the checks that hold only where a generation concludes — the program is
//! the whole app, so what validation otherwise defers to an importer is an error here.

use super::{ErrorCode, ValidationError, ValidationResult};
use crate::embedded::{patterns, EmbeddedPatternProp};
use crate::phases::embed_graph::EmbedGraph;
use crate::phases::validation::value_walk::live_sources;
use orbital_core_base::events::is_bus_excluded_event;
use orbital_core_base::schema::{
    ListenDefinition, ListenSource, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{BTreeMap, HashSet};

/// Control event -> the `pattern.prop` sites that fire it.
type Controls = BTreeMap<String, Vec<String>>;

/// Every finished-program check (G-ORB-184 unsatisfied expectations, G-ORB-187 unhandled controls,
/// G-CROSS-144 control values offered outward that nothing hears).
pub fn validate_final_program(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    result.merge(super::expectations::validate_final_expectations(schema));
    result.merge(validate_controls_handled(schema));
    result.merge(super::effect::validate_final_os_targets(schema));
    result.merge(super::effect::validate_final_cancel_keys(schema));
    for error in super::composed_wiring::discarded_offered_values(schema) {
        result.add_error(error);
    }
    result
}

/// G-ORB-187: a control event reaches the rendering trait and its embedders (the bus scope a
/// rendered control fires in) plus every `listens` route out of them. A control none of those
/// handle does nothing when operated. `@notification` props report a change the component already
/// applied, and a control named by a config knob (`EmitDefinition::definer_knob`) is the atom
/// offering an affordance, so both are exempt (the same offer rule as `ORB_X_UNUSED_EMISSION`).
fn validate_controls_handled(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for c in unhandled_controls(schema) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EmitControlUnhandled,
                format!("orbitals.{}.traits.{}", c.orbital, c.trait_name),
                format!(
                    "A control rendered by '{}' ({}) fires '{}', which no transition of '{}' or an embedder of it handles and no `listens` routes",
                    c.trait_name,
                    c.sites.join(", "),
                    c.event,
                    c.trait_name
                ),
            )
            .with_suggestion(format!(
                "Handle '{}' in '{}' (or the trait that embeds it), route it with `listens`, or remove the control",
                c.event, c.trait_name
            )),
        );
    }
    result
}

/// A rendered control whose event nothing in the program handles (the G-ORB-187 finding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnhandledControl {
    pub orbital: String,
    pub trait_name: String,
    pub event: String,
    /// The `pattern.prop` sites that fire it.
    pub sites: Vec<String>,
}

/// Every unhandled control in `schema`: what `ORB_EMIT_CONTROL_UNHANDLED` reports.
pub fn unhandled_controls(schema: &OrbitalSchema) -> Vec<UnhandledControl> {
    let mut found = Vec::new();
    let listens: Vec<&ListenDefinition> = schema
        .orbitals
        .iter()
        .flat_map(|o| {
            let trait_listens = o.traits.iter().filter_map(|t| match t {
                TraitReference::Inline(td) => td.listens.as_ref(),
                _ => None,
            });
            o.listens.iter().chain(trait_listens).flatten()
        })
        .collect();
    for orbital in &schema.orbitals {
        let embed = EmbedGraph::build(&orbital.traits);
        let traits: Vec<&TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some(td),
                _ => None,
            })
            .collect();
        let bridged = super::embed_bridge::bridged_listens(traits.iter().copied());
        for td in &traits {
            let mut controls = Controls::new();
            let reachable = super::slot::get_initial_state(td)
                .map(|initial| super::slot::reachable_states(&td.state_machine, &initial));
            for transition in &td.state_machine.transitions {
                let dead_guard = transition
                    .guard
                    .as_ref()
                    .is_some_and(|g| g.is_statically_false());
                let unreachable = reachable.as_ref().is_some_and(|states| {
                    !super::slot::transition_from_states(&transition.from)
                        .iter()
                        .any(|s| *s == "*" || states.contains(*s))
                });
                if dead_guard || unreachable {
                    continue;
                }
                for effect in transition.effects.iter().flatten() {
                    collect_controls(effect, &mut controls);
                }
            }
            for tick in td.ticks.iter().flatten() {
                for effect in &tick.effects {
                    collect_controls(effect, &mut controls);
                }
            }
            for emit in td.emits.iter().flatten() {
                if emit.definer_knob.is_some() {
                    controls.remove(&emit.event);
                }
            }
            if controls.is_empty() {
                continue;
            }
            let scope = embedder_closure(&embed, &td.name);
            for (event, sites) in controls {
                if bridged.contains(&event) {
                    continue;
                }
                let handled = traits.iter().any(|t| {
                    scope.contains(t.name.as_str())
                        && t.state_machine
                            .transitions
                            .iter()
                            .any(|tr| tr.event == event)
                }) || listens.iter().any(|l| routes(l, &event, &scope));
                if handled {
                    continue;
                }
                found.push(UnhandledControl {
                    orbital: orbital.name.clone(),
                    trait_name: td.name.clone(),
                    event,
                    sites,
                });
            }
        }
    }
    found
}

/// The unhandled controls a trait forwards on purpose: its composer declared `emitsScope external`, so
/// the event is left for an embedding organism to handle (each emit for it carries `scope: external`
/// with `scopeOverridden`). An unhandled control without that declaration is a defect, not a forward.
pub fn forwarded_controls(schema: &OrbitalSchema) -> Vec<UnhandledControl> {
    use orbital_core_base::schema::EventScope;
    unhandled_controls(schema)
        .into_iter()
        .filter(|c| {
            schema
                .orbitals
                .iter()
                .filter(|o| o.name == c.orbital)
                .flat_map(|o| o.traits.iter())
                .filter_map(|t| match t {
                    TraitReference::Inline(td) if td.name == c.trait_name => Some(td),
                    _ => None,
                })
                .flat_map(|td| td.emits.iter().flatten())
                .any(|e| {
                    e.event == c.event
                        && e.scope == Some(EventScope::External)
                        && e.scope_overridden == Some(true)
                })
        })
        .collect()
}

pub(crate) fn routes(listen: &ListenDefinition, event: &str, scope: &HashSet<&str>) -> bool {
    if listen.event != event {
        return false;
    }
    match &listen.source {
        None => false,
        Some(ListenSource::Any) => true,
        Some(ListenSource::Trait { trait_name, .. })
        | Some(ListenSource::Orbital { trait_name, .. }) => scope.contains(trait_name.as_str()),
    }
}

/// The trait plus every trait that embeds it, directly or transitively, through any referrer.
pub(crate) fn embedder_closure<'a>(embed: &'a EmbedGraph, trait_name: &'a str) -> HashSet<&'a str> {
    let mut seen: HashSet<&str> = HashSet::from([trait_name]);
    let mut frontier = vec![trait_name];
    while let Some(current) = frontier.pop() {
        for (child, hosts) in embed.referrers_iter() {
            if child != current {
                continue;
            }
            for host in hosts {
                if seen.insert(host.as_str()) {
                    frontier.push(host.as_str());
                }
            }
        }
    }
    seen
}

fn collect_controls(expr: &SExpression, out: &mut Controls) {
    match expr {
        SExpression::Array(items) => {
            let conditional =
                matches!(items.first().and_then(|e| e.as_str()), Some("if")) && items.len() >= 3;
            if conditional && items[1].is_statically_false() {
                if let Some(else_branch) = items.get(3) {
                    collect_controls(else_branch, out);
                }
                return;
            }
            for item in items {
                collect_controls(item, out);
            }
        }
        SExpression::Object(obj) => {
            if let Some(SExpression::String(pattern_type)) = obj.get("type") {
                if let Some(pattern) = patterns().get(pattern_type.as_str()) {
                    for (key, value) in obj.iter() {
                        if let Some(prop) = pattern.props_schema.get(key) {
                            let mut events = HashSet::new();
                            control_events(prop, value, &mut events);
                            for event in events {
                                let sites = out.entry(event).or_default();
                                let site = format!("{pattern_type}.{key}");
                                if !sites.contains(&site) {
                                    sites.push(site);
                                }
                            }
                        }
                    }
                }
            }
            for value in obj.values() {
                collect_controls(value, out);
            }
        }
        _ => {}
    }
}

fn control_events(prop: &EmbeddedPatternProp, value: &SExpression, out: &mut HashSet<String>) {
    if prop.notification {
        return;
    }
    match prop.kind.as_deref() {
        Some("event" | "event-ref" | "callback") => literal_events(value, out),
        Some("event-list") => {
            let field = prop.event_field.as_deref().unwrap_or("event");
            for leaf in value.value_leaves(&live_sources()) {
                if let SExpression::Array(entries) = leaf {
                    for entry in entries {
                        if let SExpression::Object(descriptor) = entry {
                            if let Some(v) = descriptor.get(field) {
                                literal_events(v, out);
                            }
                        }
                    }
                }
            }
        }
        Some("event-map") => {
            if let SExpression::Object(map) = value {
                for v in map.values() {
                    literal_events(v, out);
                }
            }
        }
        _ => {}
    }
}

fn literal_events(value: &SExpression, out: &mut HashSet<String>) {
    for leaf in value.value_leaves(&live_sources()) {
        if let SExpression::String(s) = leaf {
            if !s.is_empty() && !s.starts_with('@') && !is_bus_excluded_event(s) {
                out.insert(s.clone().into_inner());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(traits: serde_json::Value, orbital_listens: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "Shop",
                "entity": { "name": "Item", "fields": [] },
                "traits": traits,
                "listens": orbital_listens,
            }]
        }))
        .expect("schema json")
    }

    fn renderer(name: &str, node: serde_json::Value, handles: &[&str]) -> serde_json::Value {
        let mut transitions = vec![serde_json::json!({
            "from": "idle", "to": "idle", "event": "INIT",
            "effects": [["render-ui", "main", node]]
        })];
        for event in handles {
            transitions.push(serde_json::json!({ "from": "idle", "to": "idle", "event": event }));
        }
        serde_json::json!({
            "name": name,
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": transitions }
        })
    }

    fn codes(s: &OrbitalSchema) -> Vec<String> {
        validate_controls_handled(s)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::EmitControlUnhandled)
            .map(|e| e.message.clone())
            .collect()
    }

    fn button(event: &str) -> serde_json::Value {
        serde_json::json!({ "type": "button", "label": "Go", "action": event })
    }

    fn forwarding(name: &str, event: &str, overridden: bool) -> serde_json::Value {
        let mut t = renderer(name, button(event), &[]);
        let mut emit = serde_json::json!({ "event": event, "scope": "external" });
        if overridden {
            emit["scopeOverridden"] = serde_json::json!(true);
        }
        t["emits"] = serde_json::json!([emit]);
        t
    }

    fn forwards(s: &OrbitalSchema) -> Vec<String> {
        forwarded_controls(s)
            .into_iter()
            .map(|c| format!("{}.{}", c.trait_name, c.event))
            .collect()
    }

    fn with_bridge(control: &str, bridged: &[&str]) -> serde_json::Value {
        renderer(
            "Workspace",
            serde_json::json!({ "type": "stack", "children": [
                button(control),
                ["behavior/apply", "@entity.plugin", { "bridge": { "listens": bridged, "emits": [] } }]
            ] }),
            &[],
        )
    }

    #[test]
    fn a_control_a_bridged_embed_listens_for_is_handled() {
        let s = schema(serde_json::json!([with_bridge("SHELL_KEY", &["SHELL_KEY"])]), serde_json::json!(null));
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_a_bridge_listening_for_something_else_leaves_the_control_unhandled() {
        let s = schema(serde_json::json!([with_bridge("SHELL_KEY", &["OTHER"])]), serde_json::json!(null));
        assert_eq!(codes(&s).len(), 1);
    }

    #[test]
    fn a_composer_declared_external_control_nothing_handles_is_a_forward() {
        let s = schema(
            serde_json::json!([forwarding("NoteDoc", "EDIT", true)]),
            serde_json::json!(null),
        );
        assert_eq!(forwards(&s), vec!["NoteDoc.EDIT".to_string()]);
    }

    #[test]
    fn control_an_external_emit_without_a_composer_declaration_is_not_a_forward() {
        let s = schema(
            serde_json::json!([forwarding("Plain", "DELETE", false)]),
            serde_json::json!(null),
        );
        assert!(forwards(&s).is_empty());
        assert_eq!(codes(&s).len(), 1, "still a dead control");
    }

    #[test]
    fn control_a_forward_something_routes_is_not_listed() {
        let mut listener = renderer("Editor", button("X"), &["X", "OPEN"]);
        listener["listens"] = serde_json::json!([{ "event": "EDIT", "triggers": "OPEN", "source": { "kind": "trait", "trait": "NoteDoc" } }]);
        let s = schema(
            serde_json::json!([forwarding("NoteDoc", "EDIT", true), listener]),
            serde_json::json!(null),
        );
        assert!(forwards(&s).is_empty(), "{:?}", forwards(&s));
    }

    #[test]
    fn a_control_nobody_handles_is_an_error() {
        let s = schema(
            serde_json::json!([renderer("Cart", button("CHECKOUT"), &[])]),
            serde_json::json!(null),
        );
        let found = codes(&s);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("CHECKOUT"));
    }

    #[test]
    fn control_handled_by_the_rendering_trait_passes() {
        let s = schema(
            serde_json::json!([renderer("Cart", button("CHECKOUT"), &["CHECKOUT"])]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn control_handled_by_an_embedding_host_passes() {
        let host = renderer(
            "Page",
            serde_json::json!({ "type": "stack", "children": ["@trait.Cart"] }),
            &["CHECKOUT"],
        );
        let s = schema(
            serde_json::json!([renderer("Cart", button("CHECKOUT"), &[]), host]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_a_sibling_that_handles_the_same_name_does_not_count() {
        let s = schema(
            serde_json::json!([
                renderer("Cart", button("CHECKOUT"), &[]),
                renderer("Other", button("X"), &["CHECKOUT", "X"])
            ]),
            serde_json::json!(null),
        );
        assert_eq!(codes(&s).len(), 1);
    }

    #[test]
    fn control_routed_by_a_listens_entry_passes() {
        let mut listener = renderer("Billing", button("PAY"), &["PAY", "CART_DONE"]);
        listener["listens"] = serde_json::json!([{ "event": "CHECKOUT", "triggers": "CART_DONE", "source": { "kind": "trait", "trait": "Cart" } }]);
        let s = schema(
            serde_json::json!([renderer("Cart", button("CHECKOUT"), &[]), listener]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_a_listens_route_from_another_trait_does_not_count() {
        let mut listener = renderer("Billing", button("PAY"), &["PAY", "CART_DONE"]);
        listener["listens"] = serde_json::json!([{ "event": "CHECKOUT", "triggers": "CART_DONE", "source": { "kind": "trait", "trait": "Elsewhere" } }]);
        let s = schema(
            serde_json::json!([renderer("Cart", button("CHECKOUT"), &[]), listener]),
            serde_json::json!(null),
        );
        assert_eq!(codes(&s).len(), 1);
    }

    #[test]
    fn a_notification_prop_is_exempt() {
        let node = serde_json::json!({ "type": "box", "hoverEvent": "HOVERED" });
        let s = schema(
            serde_json::json!([renderer("Card", node, &[])]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_a_non_notification_prop_on_the_same_pattern_is_checked() {
        let node = serde_json::json!({ "type": "box", "action": "OPEN" });
        let s = schema(
            serde_json::json!([renderer("Card", node, &[])]),
            serde_json::json!(null),
        );
        assert_eq!(codes(&s).len(), 1);
    }

    #[test]
    fn a_control_behind_a_statically_false_branch_is_not_checked() {
        let node = serde_json::json!(["if", "", button("DEAD"), button("LIVE")]);
        let s = schema(
            serde_json::json!([renderer("Cart", node, &["LIVE"])]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn bindings_and_lifecycle_events_are_not_controls() {
        let node = serde_json::json!({ "type": "stack", "children": [button("@config.action"), button("INIT")] });
        let s = schema(
            serde_json::json!([renderer("Cart", node, &[])]),
            serde_json::json!(null),
        );
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn a_control_named_by_a_config_knob_is_an_offer() {
        let mut list = renderer("List", button("SWIPE_LEFT"), &[]);
        list["emits"] =
            serde_json::json!([{ "event": "SWIPE_LEFT", "definerKnob": "swipeLeftEvent" }]);
        let s = schema(serde_json::json!([list]), serde_json::json!(null));
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_a_declared_emit_without_a_knob_is_still_checked() {
        let mut list = renderer("List", button("SWIPE_LEFT"), &[]);
        list["emits"] = serde_json::json!([{ "event": "SWIPE_LEFT" }]);
        let s = schema(serde_json::json!([list]), serde_json::json!(null));
        assert_eq!(codes(&s).len(), 1);
    }

    fn arm(
        from: &str,
        event: &str,
        to: &str,
        guard: serde_json::Value,
        node: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({ "from": from, "to": to, "event": event, "guard": guard, "effects": [["render-ui", "main", node]] })
    }

    fn machine(name: &str, transitions: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }, { "name": "editing" }, { "name": "viewing" }],
                "transitions": transitions
            }
        })
    }

    #[test]
    fn an_arm_behind_a_statically_false_guard_renders_nothing() {
        let t = machine(
            "Doc",
            serde_json::json!([arm(
                "idle",
                "LOADED",
                "editing",
                serde_json::json!(["=", "panel", "document"]),
                button("EDIT")
            )]),
        );
        let s = schema(serde_json::json!([t]), serde_json::json!(null));
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn an_arm_out_of_a_state_only_a_dead_guard_reaches_renders_nothing() {
        let t = machine(
            "Doc",
            serde_json::json!([
                arm("idle", "LOADED", "editing", serde_json::json!(["=", "panel", "document"]), button("OPEN")),
                arm("editing", "DONE", "viewing", serde_json::json!(null), button("EDIT")),
                { "from": "idle", "to": "idle", "event": "OPEN" }
            ]),
        );
        let s = schema(serde_json::json!([t]), serde_json::json!(null));
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    #[test]
    fn control_an_arm_behind_an_undecidable_guard_is_checked() {
        let t = machine(
            "Doc",
            serde_json::json!([
                arm("idle", "LOADED", "editing", serde_json::json!(["=", "@config.look", "document"]), button("OPEN")),
                arm("editing", "DONE", "viewing", serde_json::json!(null), button("EDIT")),
                { "from": "idle", "to": "idle", "event": "OPEN" }
            ]),
        );
        let s = schema(serde_json::json!([t]), serde_json::json!(null));
        let found = codes(&s);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("EDIT"));
    }

    #[test]
    fn item_action_events_are_controls() {
        let node = serde_json::json!({ "type": "data-list", "itemActions": [{ "label": "Delete", "event": "DELETE" }] });
        let s = schema(
            serde_json::json!([renderer("List", node, &[])]),
            serde_json::json!(null),
        );
        let found = codes(&s);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("DELETE"));
    }
}
