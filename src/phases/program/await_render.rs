//! Awaiting-server render prediction.
//!
//! A server-backed trait renders through a server round trip. While it waits,
//! an empty slot shows a skeleton shaped like what the server WILL render. That
//! shape is declared data, so it is computed here once, from the resolved
//! schema, and carried to both execution paths (the `orbital resolve` output the
//! runtime reads, and the OIR the shells generate from) — never persisted in a
//! registry `.orb`.
//!
//! Rule (one hop, declared program only): for a transition `T: s --e--> s2`,
//! every `runsOn: server` effect in `T` with `emit: { success: L }` leads to
//! - the same trait's arms `s2 --L--> …`, and
//! - each listener's arms on its local trigger for a qualified listen of
//!   `<trait>.L` (a cross-trait cascade the server fans out).
//!
//! Each such arm's `render-ui <slot> <tree>` folds into a `SkeletonNode`
//! (container → stack of its children; leaf → the node's own `skeleton` prop
//! when set, else the pattern's registry default; `none` / no shape → nothing).
//! A slot `T` renders itself is excluded (its own render paints). When the
//! candidates for one (trait, slot) disagree, that slot gets no prediction —
//! never a guess.

use crate::pattern_registry::get_props_schema;
use orbital_core_base::runs_on::RunsOn;
use orbital_core_base::ir::{AwaitRender, SkeletonNode, SkeletonShape, SKELETON_VARIANTS};
use orbital_core_base::schema::{
    ConfigField, OrbitalSchema, SExpression, StringOrArray, TraitDefinition, TraitReference,
};
use std::collections::{BTreeMap, HashMap};

/// Predictions per (trait name, transition index within that trait).
pub type AwaitRenderMap = HashMap<(String, usize), Vec<AwaitRender>>;

pub fn predict_await_renders(schema: &OrbitalSchema) -> AwaitRenderMap {
    let traits: Vec<&TraitDefinition> = schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter_map(|t| match t {
            TraitReference::Inline(def) => Some(def),
            _ => None,
        })
        .collect();

    let residence =
        orbital_core_base::runs_on::Residence::of_entities(schema.orbitals.iter().flat_map(|o| {
            std::iter::once(&o.entity)
                .chain(o.auxiliary_entities.iter().flatten())
                .filter_map(|e| match e {
                    orbital_core_base::schema::EntityRef::Inline(def) => Some(def),
                    _ => None,
                })
        }));
    let mut out = AwaitRenderMap::new();
    for seed in &traits {
        for (idx, transition) in seed.state_machine.transitions.iter().enumerate() {
            let success_events =
                server_success_events(transition.effects.as_deref().unwrap_or(&[]), &residence);
            if success_events.is_empty() {
                continue;
            }
            let own_slots: Vec<String> =
                render_targets(transition.effects.as_deref().unwrap_or(&[]))
                    .into_iter()
                    .map(|(slot, _)| slot)
                    .collect();
            // (trait, slot) → candidate skeletons (None = an arm renders it with no shape)
            let mut candidates: BTreeMap<(String, String), Vec<Option<SkeletonNode>>> =
                BTreeMap::new();
            for event in &success_events {
                for arm in seed
                    .state_machine
                    .transitions
                    .iter()
                    .filter(|t| t.event == *event && from_matches(&t.from, &transition.to))
                {
                    collect(
                        &mut candidates,
                        seed,
                        arm.effects.as_deref().unwrap_or(&[]),
                        &own_slots,
                        true,
                    );
                }
                for listener in &traits {
                    for listen in listener.listens.iter().flatten() {
                        if !listens_to(&listen.event, &seed.name, event) {
                            continue;
                        }
                        let trigger = listen.triggers.as_deref().unwrap_or(event.as_str());
                        for arm in listener
                            .state_machine
                            .transitions
                            .iter()
                            .filter(|t| t.event == trigger)
                        {
                            collect(
                                &mut candidates,
                                listener,
                                arm.effects.as_deref().unwrap_or(&[]),
                                &own_slots,
                                listener.name == seed.name,
                            );
                        }
                    }
                }
            }
            let renders: Vec<AwaitRender> = candidates
                .into_iter()
                .filter_map(|((trait_name, slot), shapes)| {
                    let first = shapes.first()?.clone()?;
                    shapes
                        .iter()
                        .all(|s| s.as_ref() == Some(&first))
                        .then_some(AwaitRender {
                            trait_name,
                            slot,
                            skeleton: first,
                        })
                })
                .collect();
            if !renders.is_empty() {
                out.insert((seed.name.clone(), idx), renders);
            }
        }
    }
    out
}

fn collect(
    candidates: &mut BTreeMap<(String, String), Vec<Option<SkeletonNode>>>,
    owner: &TraitDefinition,
    effects: &[SExpression],
    own_slots: &[String],
    same_trait: bool,
) {
    for (slot, tree) in render_targets(effects) {
        if same_trait && own_slots.contains(&slot) {
            continue;
        }
        let node = tree.and_then(|t| fold(t, owner.config.as_ref()));
        candidates
            .entry((owner.name.clone(), slot))
            .or_default()
            .push(node);
    }
}

/// `<Trait>.EVENT` or `<Orbital>.<Trait>.EVENT` naming `trait_name`'s `event`.
fn listens_to(source: &str, trait_name: &str, event: &str) -> bool {
    let qualified = format!("{trait_name}.{event}");
    source == qualified || source.ends_with(&format!(".{qualified}"))
}

fn from_matches(from: &StringOrArray, state: &str) -> bool {
    match from {
        StringOrArray::Single(s) => s == "*" || s == state,
        StringOrArray::Multiple(states) => states.iter().any(|s| s == state),
    }
}

fn op(expr: &SExpression) -> Option<&str> {
    match expr {
        SExpression::Array(items) => match items.first() {
            Some(SExpression::String(name)) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// `success` event names of every server-run effect, at any nesting depth. A data
/// effect on a browser-stored entity runs locally, so it awaits no server render.
fn server_success_events(
    effects: &[SExpression],
    residence: &orbital_core_base::runs_on::Residence,
) -> Vec<String> {
    let mut out = Vec::new();
    for effect in effects {
        walk(effect, &mut |e| {
            if let (Some(name), SExpression::Array(items)) = (op(e), e) {
                if orbital_core_base::runs_on::effect_site(name, &items[1..], residence)
                    == RunsOn::Server
                {
                    for arg in items.iter().skip(1) {
                        if let Some(success) = emit_success(arg) {
                            if !out.contains(&success) {
                                out.push(success);
                            }
                        }
                    }
                }
            }
        });
    }
    out
}

fn emit_success(arg: &SExpression) -> Option<String> {
    let SExpression::Object(opts) = arg else {
        return None;
    };
    let SExpression::Object(emit) = opts.get("emit")? else {
        return None;
    };
    match emit.get("success")? {
        SExpression::String(event) => Some(event.clone().into_inner()),
        _ => None,
    }
}

/// Every `render-ui <slot> <tree>` at any nesting depth (both arms of an `if` included).
fn render_targets(effects: &[SExpression]) -> Vec<(String, Option<&SExpression>)> {
    let mut out = Vec::new();
    for effect in effects {
        walk(effect, &mut |e| {
            if op(e) == Some("render-ui") {
                if let SExpression::Array(items) = e {
                    if let Some(SExpression::String(slot)) = items.get(1) {
                        let tree = items.get(2).filter(|t| matches!(t, SExpression::Object(_)));
                        out.push((slot.to_string(), tree));
                    }
                }
            }
        });
    }
    out
}

fn walk<'a>(expr: &'a SExpression, visit: &mut dyn FnMut(&'a SExpression)) {
    visit(expr);
    if let SExpression::Array(items) = expr {
        // Never descend into a render tree or an options object as effects.
        if op(expr) == Some("render-ui") {
            return;
        }
        for item in items {
            if matches!(item, SExpression::Array(_)) {
                walk(item, visit);
            }
        }
    }
}

/// Fold a render tree into a skeleton node.
fn fold(tree: &SExpression, config: Option<&HashMap<String, ConfigField>>) -> Option<SkeletonNode> {
    let SExpression::Object(node) = tree else {
        return None;
    };
    let pattern = match node.get("type") {
        Some(SExpression::String(t)) => t.as_str(),
        _ => return None,
    };
    // A declared spec decides (`none` included); a `@config.<knob>` resolves to
    // the knob's default, and an unset knob or a non-spec value (draw-group's
    // boolean rig scope) leaves the element to its children / registry default.
    if let Some(spec) = node.get("skeleton").and_then(|v| declared_spec(v, config)) {
        return shape_from_spec(spec, config).map(|shape| SkeletonNode::Shape { shape });
    }
    if let Some(SExpression::Array(children)) = node.get("children") {
        let stack: Vec<SkeletonNode> = children.iter().filter_map(|c| fold(c, config)).collect();
        if !stack.is_empty() {
            return Some(if stack.len() == 1 {
                stack.into_iter().next()?
            } else {
                SkeletonNode::Stack { stack }
            });
        }
    }
    let default = get_props_schema(pattern)?
        .get("skeleton")?
        .default
        .as_ref()?;
    shape_from_json(default)
}

fn shape_from_spec(
    spec: &SExpression,
    config: Option<&HashMap<String, ConfigField>>,
) -> Option<SkeletonShape> {
    match spec {
        SExpression::String(variant) if is_variant(variant) => Some(SkeletonShape {
            variant: variant.clone().into_inner(),
            rows: None,
            columns: None,
            fields: None,
        }),
        SExpression::Object(o) => {
            let variant = match o.get("variant")? {
                SExpression::String(v) if is_variant(v) => v.clone(),
                _ => return None,
            };
            Some(SkeletonShape {
                variant: variant.into_inner(),
                rows: count(o.get("rows"), config),
                columns: count(o.get("columns"), config),
                fields: count(o.get("fields"), config),
            })
        }
        _ => None,
    }
}

fn declared_spec<'a>(
    value: &'a SExpression,
    config: Option<&'a HashMap<String, ConfigField>>,
) -> Option<&'a SExpression> {
    match value {
        SExpression::String(s) => match s.strip_prefix("@config.") {
            Some(knob) => config?
                .get(knob)?
                .default
                .as_ref()
                .and_then(|d| declared_spec(d, None)),
            None => (s == "none" || is_variant(s)).then_some(value),
        },
        SExpression::Object(_) => Some(value),
        _ => None,
    }
}

fn is_variant(name: &str) -> bool {
    SKELETON_VARIANTS.contains(&name)
}

/// A literal count, or an `@config.<knob>` resolved against the trait's config default.
fn count(
    value: Option<&SExpression>,
    config: Option<&HashMap<String, ConfigField>>,
) -> Option<u32> {
    match value? {
        SExpression::Number(n) if *n >= 1.0 => Some(*n as u32),
        SExpression::String(binding) => {
            let knob = binding.strip_prefix("@config.")?;
            match config?.get(knob)?.default.as_ref()? {
                SExpression::Number(n) if *n >= 1.0 => Some(*n as u32),
                _ => None,
            }
        }
        _ => None,
    }
}

fn shape_from_json(value: &serde_json::Value) -> Option<SkeletonNode> {
    let shape = match value {
        serde_json::Value::String(s) if s == "none" => return None,
        serde_json::Value::String(variant) => SkeletonShape {
            variant: variant.clone(),
            rows: None,
            columns: None,
            fields: None,
        },
        serde_json::Value::Object(_) => serde_json::from_value(value.clone()).ok()?,
        _ => return None,
    };
    Some(SkeletonNode::Shape { shape })
}

/// Write the predictions into a serialized resolved schema (`orbital resolve`
/// output, which the runtime path reads): each matching transition object gains
/// `awaitRender`. In-memory only — the registry `.orb` never carries it.
pub fn attach_await_renders(schema: &OrbitalSchema, value: &mut serde_json::Value) {
    let predictions = predict_await_renders(schema);
    if predictions.is_empty() {
        return;
    }
    let Some(orbitals) = value.get_mut("orbitals").and_then(|o| o.as_array_mut()) else {
        return;
    };
    for orbital in orbitals {
        let Some(traits) = orbital.get_mut("traits").and_then(|t| t.as_array_mut()) else {
            continue;
        };
        for trait_value in traits {
            let Some(name) = trait_value
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Some(transitions) = trait_value
                .get_mut("stateMachine")
                .and_then(|sm| sm.get_mut("transitions"))
                .and_then(|t| t.as_array_mut())
            else {
                continue;
            };
            for (idx, transition) in transitions.iter_mut().enumerate() {
                if let (Some(renders), Some(obj)) = (
                    predictions.get(&(name.clone(), idx)),
                    transition.as_object_mut(),
                ) {
                    if let Ok(v) = serde_json::to_value(renders) {
                        obj.insert("awaitRender".to_string(), v);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(traits: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{ "name": "O", "entity": { "name": "Item", "fields": [] }, "traits": traits, "pages": [] }]
        }))
        .expect("schema")
    }

    fn trait_def(
        name: &str,
        transitions: serde_json::Value,
        listens: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "listens": listens,
            "config": { "pageSize": { "type": "number", "default": 9 } },
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }, { "name": "loading" }, { "name": "ready" }], "transitions": transitions }
        })
    }

    fn shape(variant: &str) -> SkeletonNode {
        SkeletonNode::Shape {
            shape: SkeletonShape {
                variant: variant.to_string(),
                rows: None,
                columns: None,
                fields: None,
            },
        }
    }

    #[test]
    fn a_fetch_success_arm_render_is_predicted_for_its_slot() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "data-grid", "skeleton": "grid" }]] }
            ]),
            serde_json::json!([])
        )]));
        let p = predict_await_renders(&s);
        assert_eq!(
            p.get(&("Browse".to_string(), 0)).unwrap(),
            &vec![AwaitRender {
                trait_name: "Browse".into(),
                slot: "main".into(),
                skeleton: shape("grid")
            }]
        );
    }

    #[test]
    fn a_fetch_of_a_browser_stored_entity_predicts_no_server_skeleton() {
        let mut s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "data-grid", "skeleton": "grid" }]] }
            ]),
            serde_json::json!([])
        )]));
        if let orbital_core_base::schema::EntityRef::Inline(item) = &mut s.orbitals[0].entity {
            item.browser = true;
            item.collection = Some("items".into());
        }
        assert!(predict_await_renders(&s)
            .get(&("Browse".to_string(), 0))
            .is_none());
    }

    #[test]
    fn a_container_folds_to_a_stack_and_config_rows_resolve() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "stack", "children": [
                    { "type": "page-header", "skeleton": "header" },
                    { "type": "table-view", "skeleton": { "variant": "table", "rows": "@config.pageSize" } }
                ] }]] }
            ]),
            serde_json::json!([])
        )]));
        let p = predict_await_renders(&s);
        let SkeletonNode::Stack { stack } = &p.get(&("Browse".to_string(), 0)).unwrap()[0].skeleton
        else {
            panic!("stack")
        };
        assert_eq!(stack[0], shape("header"));
        assert_eq!(
            stack[1],
            SkeletonNode::Shape {
                shape: SkeletonShape {
                    variant: "table".into(),
                    rows: Some(9),
                    columns: None,
                    fields: None
                }
            }
        );
    }

    #[test]
    fn edge_a_non_spec_skeleton_value_does_not_hide_the_subtree() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "stack", "skeleton": true, "children": [
                    { "type": "data-grid", "skeleton": "grid" }
                ] }]] }
            ]),
            serde_json::json!([])
        )]));
        let p = predict_await_renders(&s);
        assert_eq!(
            p.get(&("Browse".to_string(), 0)).unwrap()[0].skeleton,
            shape("grid")
        );
    }

    #[test]
    fn a_config_bound_skeleton_resolves_to_the_knob_default_and_an_unset_knob_falls_through() {
        let mut t = trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "stack", "children": [
                    { "type": "typography", "skeleton": "@config.look" },
                    { "type": "typography", "skeleton": "@config.unset", "children": [{ "type": "data-grid", "skeleton": "grid" }] }
                ] }]] }
            ]),
            serde_json::json!([]),
        );
        t["config"]["look"] = serde_json::json!({ "type": "string", "default": "list" });
        t["config"]["unset"] = serde_json::json!({ "type": "string" });
        let p = predict_await_renders(&schema(serde_json::json!([t])));
        let SkeletonNode::Stack { stack } = &p.get(&("Browse".to_string(), 0)).unwrap()[0].skeleton
        else {
            panic!("stack")
        };
        assert_eq!(stack, &vec![shape("list"), shape("grid")]);
    }

    #[test]
    fn edge_an_unknown_variant_or_binding_string_is_no_shape() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [["render-ui", "main", { "type": "stack", "children": [
                    { "type": "typography", "skeleton": "carousel" },
                    { "type": "typography", "skeleton": { "variant": "carousel" } }
                ] }]] }
            ]),
            serde_json::json!([])
        )]));
        assert!(predict_await_renders(&s)
            .get(&("Browse".to_string(), 0))
            .is_none());
    }

    #[test]
    fn skeleton_none_drops_the_element_and_a_locally_rendered_slot_is_excluded() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [
                    ["fetch", "Item", { "emit": { "success": "LOADED" } }],
                    ["render-ui", "main", { "type": "skeleton" }]
                ] },
                { "from": "loading", "to": "ready", "event": "LOADED", "effects": [
                    ["render-ui", "main", { "type": "data-grid", "skeleton": "grid" }],
                    ["render-ui", "sidebar", { "type": "data-list", "skeleton": "none" }]
                ] }
            ]),
            serde_json::json!([])
        )]));
        assert!(predict_await_renders(&s)
            .get(&("Browse".to_string(), 0))
            .is_none());
    }

    #[test]
    fn a_listener_reached_across_the_listens_graph_gets_its_own_prediction() {
        let s = schema(serde_json::json!([
            trait_def(
                "Browse",
                serde_json::json!([
                    { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] }
                ]),
                serde_json::json!([])
            ),
            trait_def(
                "Stats",
                serde_json::json!([
                    { "from": "idle", "to": "ready", "event": "REFRESH", "effects": [["render-ui", "sidebar", { "type": "stat-display", "skeleton": "stats" }]] }
                ]),
                serde_json::json!([{ "event": "Browse.LOADED", "triggers": "REFRESH" }])
            )
        ]));
        let p = predict_await_renders(&s);
        assert_eq!(
            p.get(&("Browse".to_string(), 0)).unwrap(),
            &vec![AwaitRender {
                trait_name: "Stats".into(),
                slot: "sidebar".into(),
                skeleton: shape("stats")
            }]
        );
    }

    #[test]
    fn disagreeing_candidates_for_one_slot_are_never_guessed() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "loading", "event": "INIT", "effects": [["fetch", "Item", { "emit": { "success": "LOADED" } }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "guard": true, "effects": [["render-ui", "main", { "type": "data-grid", "skeleton": "grid" }]] },
                { "from": "loading", "to": "ready", "event": "LOADED", "guard": false, "effects": [["render-ui", "main", { "type": "data-list", "skeleton": "list" }]] }
            ]),
            serde_json::json!([])
        )]));
        assert!(predict_await_renders(&s)
            .get(&("Browse".to_string(), 0))
            .is_none());
    }

    #[test]
    fn control_a_transition_with_no_server_effect_predicts_nothing() {
        let s = schema(serde_json::json!([trait_def(
            "Browse",
            serde_json::json!([
                { "from": "idle", "to": "ready", "event": "INIT", "effects": [["render-ui", "main", { "type": "data-grid", "skeleton": "grid" }]] }
            ]),
            serde_json::json!([])
        )]));
        assert!(predict_await_renders(&s).is_empty());
    }
}
