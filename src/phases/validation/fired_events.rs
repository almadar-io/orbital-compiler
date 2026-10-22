//! Schema-wide REACHABILITY of an event: which event names does this
//! composition actually produce?
//!
//! One walker, two consumers, because the two questions are duals and must
//! never be able to disagree:
//!
//! - `emit_payload`'s `ORB_EMIT_DECLARED_BUT_UNFIRED` — declared in `emits`,
//!   produced by nobody.
//! - `cross_orbital::event_wiring`'s `ORB_X_UNUSED_EMISSION` — produced, heard
//!   by nobody.
//!
//! Joining them is what makes the second one mean something. A declared emit
//! nothing fires is a LATENT OFFER (an atom exposes `SORT`; this app never
//! configured it) and reporting it is noise. A declared emit something DOES
//! fire — an `(emit …)` effect, a rendered `action:`, an `itemActions` entry —
//! with no listener is a live wire terminating in nothing, and that is a defect
//! whatever the trait's provenance. The provenance test this replaced
//! (imported ⇒ offer) could not tell the two apart: measured over the 1200-file
//! corpus, roughly half the "declined offers" were explicit `(emit …)` effects
//! and rendered row actions.
//!
//! Everything here is source-tagged against the pattern/operator registries —
//! `kind: "event" | "event-ref" | "event-listen" | "callback"` props,
//! `kind: "event-list"` inner fields, and `kind: "event-map"` object VALUES
//! (a d-pad's direction→event map, a canvas's keycode→event map), written by
//! pattern-sync because a component author typed the prop that way. No name
//! heuristics.

use orbital_core_base::events::is_bus_excluded_event;
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};

use crate::embedded::patterns;
use crate::phases::validation::value_walk::live_sources;

/// Every event name fired anywhere in the schema: transition effects, tick
/// effects, and RESOLVED config defaults.
///
/// The config pass is not optional. An atom's `itemActions` default
/// (`[{ event: "VIEW", … }]`) is bound into the render effect as
/// `@config.itemActions`, which the effect walk skips as a runtime binding — so
/// the fired event lives only in the config value. The inline phase folds every
/// call-site override into that same `default`, so walking it reflects the
/// resolved config for the standalone atom and for any consumer that inlines it.
///
/// ⚠️ STRUCTURED config values only — the walk matches an event name by the
/// OBJECT KEY it sits under, so a SCALAR knob (`action : event = "ACTION"`) is
/// deliberately NOT counted. Counting it was measured and rejected: it makes
/// "produced" mean "the atom ships a default name for this control", which is
/// true almost everywhere, and took the corpus from 951 findings to 9,773 —
/// re-admitting exactly the optional-affordance population this predicate
/// exists to exclude. Distinguishing a knob the call site DELIBERATELY set from
/// one left at the atom default needs the pre-override default captured at
/// inline; until then `definer_knob` (an emit whose NAME came from a knob is an
/// offer) carries that half of the classification.
/// The produced set, split by HOW the event is produced. The distinction is
/// what separates a dead affordance from a broadcast nobody happens to collect:
///
/// - `by_control` — the name sits in a render-ui event PROP (`action: "SAVE"`,
///   `itemActions: [{ event: "DELETE" }]`, `onTabChange: …`). There is a control
///   on screen wired to fire it, so no listener means a control the user can
///   operate that does nothing, and the value it carries is discarded.
/// - `by_effect` — an `(emit …)` effect or operator emit-config. Programmatic:
///   a simulation broadcasting `PENDULUM_UPDATED`, an asset catalog publishing
///   its art. No listener means the composition did not include a collector,
///   which is a composition choice.
///
/// Trait-level "does this trait render?" was tried first and is too coarse: a
/// learning `Sim` renders its canvas AND broadcasts telemetry from a tick
/// effect, so ~100 broadcasts read as dead controls. The question is how the
/// EVENT is produced, not what the trait does elsewhere.
// Keyed by (trait_name, event): a schema-wide-by-NAME set let a sibling
// trait's unrelated emitter (e.g. another trait's `EDIT` button) mark THIS
// trait's same-named event as control-fired, a measured false positive
// (`ArticleBrowse.EDIT` in std-cms-article credited by a sibling's button).
#[derive(Default)]
pub(crate) struct FiredEvents {
    pub(crate) by_control: std::collections::HashSet<(String, String)>,
    pub(crate) by_effect: std::collections::HashSet<(String, String)>,
}

impl FiredEvents {
    /// Produced by THIS trait, either way — what the unused-emission check
    /// (per-emitter reachability) asks.
    pub(crate) fn contains_for_trait(&self, trait_name: &str, event: &str) -> bool {
        let key = (trait_name.to_string(), event.to_string());
        self.by_control.contains(&key) || self.by_effect.contains(&key)
    }

    /// Produced by ANY trait in the schema, regardless of which one — the
    /// file-scope query `emit_payload`'s declared-but-unfired check
    /// deliberately wants (its "namespace owner" pattern: declared on trait
    /// A, fired by sibling B is legitimate). Kept as an explicit separate
    /// query rather than silently changing that check's semantics.
    pub(crate) fn contains_any_trait(&self, event: &str) -> bool {
        self.by_control.iter().any(|(_, e)| e == event)
            || self.by_effect.iter().any(|(_, e)| e == event)
    }
}

pub(crate) fn collect_schema_fired_events(schema: &OrbitalSchema) -> FiredEvents {
    let mut fired = FiredEvents::default();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let knobs = literal_knobs(td);
            for transition in &td.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        collect_fired_with_knobs(&td.name, effect, &knobs, &mut fired);
                    }
                }
            }
            if let Some(ticks) = &td.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        collect_fired_with_knobs(&td.name, effect, &knobs, &mut fired);
                    }
                }
            }
            if let Some(config) = &td.config {
                for field in config.values() {
                    if let Some(default) = &field.default {
                        collect_fired_with_knobs(&td.name, default, &knobs, &mut fired);
                    }
                }
            }
        }
    }
    fired
}

/// A trait's config knobs whose resolved value is a literal (the composition's call-site value,
/// folded into `default` by the inline phase): what a `(if @config.<knob> …)` condition reads.
pub(crate) fn literal_knobs(
    td: &TraitDefinition,
) -> orbital_core_base::schema::SObject {
    td.config
        .iter()
        .flat_map(|c| c.iter())
        .filter_map(|(name, field)| match &field.default {
            Some(
                v @ (SExpression::Bool(_)
                | SExpression::Number(_)
                | SExpression::String(_)
                | SExpression::Null),
            ) => Some((name.clone(), v.clone())),
            _ => None,
        })
        .collect()
}

/// `condition` with every `@config.<knob>` the trait resolved to a literal replaced by that
/// literal, so a branch the composition's config switched off folds to statically false.
pub(crate) fn fold_knobs(
    condition: &SExpression,
    knobs: &orbital_core_base::schema::SObject,
) -> SExpression {
    match condition {
        SExpression::String(s) => s
            .strip_prefix("@config.")
            .and_then(|knob| knobs.get(knob))
            .cloned()
            .unwrap_or_else(|| condition.clone()),
        SExpression::Array(items) => {
            SExpression::Array(items.iter().map(|i| fold_knobs(i, knobs)).collect())
        }
        other => other.clone(),
    }
}

/// Trait names whose event surface is DYNAMIC — the fired name is a binding
/// (`pageChangeEvent: "@config.event"`), resolved at runtime.
///
/// Reachability is undecidable for these in both directions, so neither
/// consumer may draw a conclusion: `unfired` must not report (the event may
/// well be fired) and `unused` must not escalate or silence (we cannot prove
/// either). They keep the pre-existing non-blocking treatment.
pub(crate) fn collect_dynamic_event_traits(
    schema: &OrbitalSchema,
) -> std::collections::HashSet<String> {
    let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(td) = trait_ref {
                if trait_has_dynamic_event_prop_binding(td) {
                    out.insert(td.name.clone());
                }
            }
        }
    }
    out
}

/// Set of object-key names that, per the patterns registry, identify a
/// string value as a bus event-name reference. Mirrors the lolo
/// `event_prop_names()` derivation:
///
/// 1. Top-level pattern prop names with `kind` in `{event, event-ref,
///    event-listen, callback}` (e.g. `action`, `submitEvent`,
///    `tabChangeEvent`, `pageChangeEvent`, `onTabChange`, ...).
/// 2. Inner descriptor field names from `kind: "event-list"` entries'
///    `eventField` metadata (e.g. data-list.itemActions's `event`
///    inner key).
///
/// All entries are source-tagged: pattern-sync wrote each `kind` mark
/// because a component author typed the prop as `EventEmit<P>` /
/// `EventKey` / a function (callback) / an array of descriptors with an
/// event field. No hardcoded names, no heuristics.
/// True when any render-ui pattern in the trait's effects binds a
/// kind:{event, event-ref, event-listen, callback} prop to a `@`-prefixed
/// binding (config / payload / entity / state). The runtime event name
/// is dynamic; we cannot statically prove the trait's declared emits
/// are unfired. Examples: `pageChangeEvent: "@config.event"`,
/// `tabChangeEvent: "@config.event"`. Used by L2.7 to suppress
/// false-positive `EmitDeclaredButUnfired` findings on atoms that
/// expose their event names through configuration.
pub(crate) fn trait_has_dynamic_event_prop_binding(td: &TraitDefinition) -> bool {
    let event_keys = ui_event_prop_names();
    for transition in &td.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if sexpr_has_dynamic_event_binding(effect, event_keys) {
                    return true;
                }
            }
        }
    }
    if let Some(ticks) = &td.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                if sexpr_has_dynamic_event_binding(effect, event_keys) {
                    return true;
                }
            }
        }
    }
    false
}

fn sexpr_has_dynamic_event_binding(
    expr: &SExpression,
    event_keys: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        SExpression::Object(obj) => {
            for (key, value) in obj.iter() {
                if event_keys.contains(key) {
                    if let SExpression::String(s) = value {
                        if s.starts_with('@') {
                            return true;
                        }
                    }
                }
                if sexpr_has_dynamic_event_binding(value, event_keys) {
                    return true;
                }
            }
            false
        }
        SExpression::Array(arr) => arr
            .iter()
            .any(|item| sexpr_has_dynamic_event_binding(item, event_keys)),
        _ => false,
    }
}

// Shared with `closed_circuit`'s `ORB_RENDER_ACTION_LIFECYCLE_EVENT` check —
// same registry-driven vocabulary, not a second hand-rolled list.
pub(crate) fn ui_event_prop_names() -> &'static std::collections::HashSet<String> {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static EVENT_PROP_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    EVENT_PROP_NAMES.get_or_init(|| {
        let mut names: HashSet<String> = HashSet::new();
        for pattern in patterns().values() {
            for (prop_name, prop) in &pattern.props_schema {
                let Some(kind) = prop.kind.as_deref() else {
                    continue;
                };
                match kind {
                    "event" | "event-ref" | "event-listen" | "callback" => {
                        names.insert(prop_name.clone());
                    }
                    "event-list" => {
                        if let Some(field) = prop.event_field.as_deref() {
                            names.insert(field.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        names
    })
}

/// Prop names carrying `kind: "event-map"` — a dynamic-key object whose
/// VALUES (not the prop's own value) are declared event keys, e.g. a d-pad's
/// direction→event map (`control-grid.directionEvents`) or a canvas's
/// keycode→event map (`canvas.keyMap`). Kept separate from
/// `ui_event_prop_names()`: that flat set assumes the prop's OWN value is (a
/// conditional tree of) event-name strings, whereas an event-map's value is
/// an object whose VALUES need the same treatment — a different shape of
/// walk, not just a different set of names.
fn event_map_prop_names() -> &'static std::collections::HashSet<String> {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static EVENT_MAP_PROP_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    EVENT_MAP_PROP_NAMES.get_or_init(|| {
        let mut names: HashSet<String> = HashSet::new();
        for pattern in patterns().values() {
            for (prop_name, prop) in &pattern.props_schema {
                if prop.kind.as_deref() == Some("event-map") {
                    names.insert(prop_name.clone());
                }
            }
        }
        names
    })
}

/// Recursively walk an SExpression collecting every event-name string
/// fired by:
///
/// 1. `["emit", "EVENT_NAME", ...]` standalone effect.
/// 2. `[op, ..., {emit: {success: "X", failure: "Y"}}]` operator emit-
///    config (fetch / persist / call-service / ref / deref / agent-
///    invoke — generic scan, every operator with an `emit` config-key
///    is covered).
/// 3. UI render-ui pattern props that carry event names — the registry's
///    `kind: "event"|"event-ref"|"event-listen"|"callback"` props (e.g.
///    `action: "SAVE"`, `tabChangeEvent: "TAB_CHANGED"`,
///    `submitEvent: "FORM_SAVED"`, `onTabChange: "TAB_CHANGED"`) — and
///    `kind: "event-list"` inner-descriptor fields (e.g.
///    `itemActions: [{ event: "VIEW", ... }]`).
///
/// Source-tagged: every UI prop name comes from a registry `kind` mark
/// pattern-sync wrote because a component author typed the prop
/// accordingly. The set extends automatically as new event-bearing
/// props ship in @almadar/ui.
fn collect_fired_with_knobs(
    trait_name: &str,
    expr: &SExpression,
    knobs: &orbital_core_base::schema::SObject,
    fired: &mut FiredEvents,
) {
    match expr {
        SExpression::Array(arr) => {
            // `["emit", "EVENT_NAME", ...]`
            if let (Some(SExpression::String(op)), Some(SExpression::String(event))) =
                (arr.first(), arr.get(1))
            {
                if op == "emit" {
                    fired
                        .by_effect
                        .insert((trait_name.to_string(), event.clone()));
                }
            }
            // `[op, ..., {emit: {success: "X", failure: "Y", on_message:
            // "Z", ...}}]` — scan every string value under any `emit`
            // sub-key. Done generically so every operator + every
            // fire-key (success / failure / on_message / on_change /
            // future additions) is covered without per-key maintenance.
            for item in arr {
                if let SExpression::Object(obj) = item {
                    if let Some(SExpression::Object(emit_cfg)) = obj.get("emit") {
                        for (_k, v) in emit_cfg.iter() {
                            if let SExpression::String(s) = v {
                                fired.by_effect.insert((trait_name.to_string(), s.clone()));
                            }
                        }
                    }
                }
            }
            // Fold-aware descent: `["if", cond, then, else?]` whose `cond`
            // provably never fires (`SExpression::is_statically_false` —
            // the SAME folding `slot.rs`'s guard filter already applies,
            // not a second one) contributes no emitters from its `then`
            // branch. A `Button action={SAVE}` sitting behind
            // `(if @config.knob ...)` on a call site that left the knob at
            // its `""`/`false`/unset default can never render, so counting
            // it as an emitter would make the very branch that is dead
            // suppress the `ORB_EMIT_DECLARED_BUT_UNFIRED` finding that
            // should catch it.
            let is_conditional =
                matches!(arr.first().and_then(|e| e.as_str()), Some("if")) && arr.len() >= 3;
            if is_conditional {
                let condition = &arr[1];
                if !fold_knobs(condition, knobs).is_statically_false() {
                    collect_fired_with_knobs(trait_name, &arr[2], knobs, fired);
                }
                if let Some(else_branch) = arr.get(3) {
                    collect_fired_with_knobs(trait_name, else_branch, knobs, fired);
                }
                collect_fired_with_knobs(trait_name, condition, knobs, fired);
            } else {
                for item in arr {
                    collect_fired_with_knobs(trait_name, item, knobs, fired);
                }
            }
        }
        SExpression::Object(obj) => {
            // UI pattern props that carry event-name strings. The
            // registry-driven event-prop set covers all four kinds of
            // declarative event reference (event / event-ref /
            // event-listen / callback) plus event-list inner-field
            // names — pattern-sync writes the `kind` mark; we read it.
            let event_keys = ui_event_prop_names();
            for (key, value) in obj.iter() {
                if event_keys.contains(key) {
                    // A value contributes every declared source that can
                    // actually render — a branch whose condition provably
                    // never fires contributes none (`live_sources`; renames
                    // and descriptor rewrites use `all_sources`, every branch
                    // even when dead).
                    for leaf in value.value_leaves(&live_sources()) {
                        if let SExpression::String(s) = leaf {
                            // Skip bindings (`@entity.X`, `@payload.Y`) —
                            // those resolve at runtime to event-name
                            // strings, but at static analysis we can't know
                            // the value. Treating `@`-prefixed strings as
                            // fired would suppress legitimate orphan
                            // detection.
                            // …and skip bus-excluded names. A `Button
                            // action={INIT}` is a DEAD control (the bus never
                            // delivers INIT/LOAD/$MOUNT), so counting it as an
                            // emitter let the dead button suppress the very
                            // warning that would have found it.
                            if !s.starts_with('@') && !is_bus_excluded_event(s) {
                                fired.by_control.insert((trait_name.to_string(), s.clone()));
                            }
                        }
                    }
                }
                // `event-map` props (`control-grid.directionEvents`,
                // `canvas.keyMap`): the prop's own value is an OBJECT whose
                // VALUES — not the prop's value itself — are event names. A
                // whole-map binding (`directionEvents: "@config.…"`) is
                // skipped, same as any other unresolvable binding.
                if event_map_prop_names().contains(key) {
                    if let SExpression::Object(map_values) = value {
                        for map_value in map_values.values() {
                            for leaf in map_value.value_leaves(&live_sources()) {
                                if let SExpression::String(s) = leaf {
                                    if !s.starts_with('@') && !is_bus_excluded_event(s) {
                                        fired
                                            .by_control
                                            .insert((trait_name.to_string(), s.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
                collect_fired_with_knobs(trait_name, value, knobs, fired);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        EntityDefinition, EntityRef, Orbital, StateDefinition, StateMachine, StringOrArray,
        TransitionDefinition,
    };

    fn minimal_schema(traits: Vec<TraitReference>) -> OrbitalSchema {
        OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                spec: false,
                name: "ArticleOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    timestamps: None,
                    soft_delete: None,
                    description: None,
                    visual_prompt: None,
                    asset_ref: None,
                    store: None,
                    shared: false,
                    identity: false,
                    local: false,
                    seed_mock: false,
                    locale_instances: None,
                    read_policy: None,
                    create_policy: None,
                    update_policy: None,
                    delete_policy: None,
                    access_waivers: None,
                    name: "Article".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                    id: None,
                    invariants: None,
                }),
                auxiliary_entities: None,
                traits,
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                os_grants: None,
                design: None,
                types: None,
                id: None,
                reference: None,
                config: None,
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        }
    }

    fn bare_trait(name: &str) -> TraitDefinition {
        TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Article".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                    holds: None,
                }],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_declaration: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    /// A trait rendering a `button` whose `action` prop literally fires
    /// `event`.
    fn trait_rendering_control(name: &str, event: &str) -> TraitDefinition {
        let mut td = bare_trait(name);
        td.state_machine.transitions = vec![TransitionDefinition {
            event_id: None,
            from: StringOrArray::Single("idle".to_string()),
            to: "idle".to_string(),
            event: "INIT".to_string(),
            guard: None,
            effects: Some(vec![SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String("main".to_string()),
                SExpression::Object(
                    [
                        (
                            "type".to_string(),
                            SExpression::String("button".to_string()),
                        ),
                        ("action".to_string(), SExpression::String(event.to_string())),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ])]),
            ensures: None,
        }];
        td
    }

    /// The ArticleBrowse.EDIT false positive: `ArticleBrowse` declares `EDIT`
    /// (documentation-only, namespace-owner pattern) but renders no control
    /// for it; a SIBLING trait `ArticleEdit` renders the actual button. A
    /// schema-wide-by-NAME set would credit `ArticleBrowse` with control
    /// production it doesn't have — keying by (trait, event) must not.
    /// A list whose search control sits behind `(if @config.bodySearch …)`, both in a transition
    /// effect and inside another knob's default (std-browse's `bodyContent`), with `bodySearch`
    /// resolved to `body_search` (G-CROSS-144).
    fn gated_search(body_search: serde_json::Value) -> OrbitalSchema {
        let gated = serde_json::json!(["if", "@config.bodySearch", { "type": "search-input", "event": "REFETCH_QUERY" }]);
        let mut config = serde_json::json!({
            "bodyContent": { "type": "SExpr", "default": gated.clone() }
        });
        if !body_search.is_null() {
            config["bodySearch"] = serde_json::json!({ "type": "boolean", "default": body_search });
        }
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "Items",
                "entity": { "name": "Item", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [{
                    "name": "Browse",
                    "config": config,
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": [["render-ui", "main", gated]] }]
                    }
                }]
            }]
        }))
        .expect("fixture deserializes")
    }

    #[test]
    fn a_control_behind_a_config_off_knob_is_not_produced() {
        let fired = collect_schema_fired_events(&gated_search(serde_json::json!(false)));
        assert!(!fired.contains_for_trait("Browse", "REFETCH_QUERY"));
    }

    #[test]
    fn control_a_control_behind_a_config_on_knob_is_produced() {
        let fired = collect_schema_fired_events(&gated_search(serde_json::json!(true)));
        assert!(fired.contains_for_trait("Browse", "REFETCH_QUERY"));
    }

    #[test]
    fn edge_a_knob_with_no_literal_value_keeps_both_branches() {
        let fired = collect_schema_fired_events(&gated_search(serde_json::Value::Null));
        assert!(
            fired.contains_for_trait("Browse", "REFETCH_QUERY"),
            "undecidable: counted, never guessed dead"
        );
    }

    #[test]
    fn by_control_is_keyed_per_trait_not_per_event_name() {
        let mut browse = bare_trait("ArticleBrowse");
        browse.emits = Some(vec![orbital_core_base::schema::EmitDefinition {
            definer_knob: None,
            scope_overridden: None,
            event: "EDIT".to_string(),
            event_id: None,
            description: None,
            synonyms: None,
            tier: None,
            scope: None,
            payload_schema: None,
            payload_entity: None,
        }]);
        let edit = trait_rendering_control("ArticleEdit", "EDIT");

        let schema = minimal_schema(vec![
            TraitReference::Inline(browse),
            TraitReference::Inline(edit),
        ]);
        let fired = collect_schema_fired_events(&schema);

        assert!(
            fired
                .by_control
                .contains(&("ArticleEdit".to_string(), "EDIT".to_string())),
            "the actual renderer must be credited"
        );
        assert!(
            !fired.contains_for_trait("ArticleBrowse", "EDIT"),
            "a sibling's rendered control must NOT credit ArticleBrowse's declared-only EDIT"
        );
    }

    /// The dual: `emit_payload`'s declared-but-unfired check deliberately
    /// wants FILE-scope firing (the "namespace owner" pattern — declared on
    /// one trait, fired by a sibling is legitimate), so `contains_any_trait`
    /// must still see the sibling's production.
    #[test]
    fn contains_any_trait_preserves_file_scope_semantics() {
        let mut browse = bare_trait("ArticleBrowse");
        browse.emits = Some(vec![orbital_core_base::schema::EmitDefinition {
            definer_knob: None,
            scope_overridden: None,
            event: "EDIT".to_string(),
            event_id: None,
            description: None,
            synonyms: None,
            tier: None,
            scope: None,
            payload_schema: None,
            payload_entity: None,
        }]);
        let edit = trait_rendering_control("ArticleEdit", "EDIT");

        let schema = minimal_schema(vec![
            TraitReference::Inline(browse),
            TraitReference::Inline(edit),
        ]);
        let fired = collect_schema_fired_events(&schema);

        assert!(
            fired.contains_any_trait("EDIT"),
            "file-scope query must see EDIT fired by the sibling renderer"
        );
    }

    // ---- Rule 5: fold-aware emitter counting -------------------------------

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }

    #[test]
    fn dead_if_branch_emit_is_not_counted_as_fired() {
        // (if "" (emit "DEAD") (emit "LIVE")) — the condition is a
        // string-typed config knob's `""` default, provably falsy.
        let tree = SExpression::Array(vec![
            s("if"),
            s(""),
            SExpression::Array(vec![s("emit"), s("DEAD")]),
            SExpression::Array(vec![s("emit"), s("LIVE")]),
        ]);
        let mut fired = FiredEvents::default();
        collect_fired_with_knobs("T", &tree, &orbital_core_base::schema::SObject::new(), &mut fired);
        assert!(
            !fired
                .by_effect
                .contains(&("T".to_string(), "DEAD".to_string())),
            "an emit behind a provably-false condition must not count as fired: {:?}",
            fired.by_effect
        );
        assert!(
            fired
                .by_effect
                .contains(&("T".to_string(), "LIVE".to_string())),
            "the always-taken else branch must still count as fired: {:?}",
            fired.by_effect
        );
    }

    #[test]
    fn undecidable_if_condition_still_counts_both_branches() {
        // (if @config.knob (emit "A") (emit "B")) — a binding condition is
        // undecidable, so both branches must still count (no regression on
        // the pre-existing "only one renders but each is a live emitter"
        // treatment for the general case).
        let tree = SExpression::Array(vec![
            s("if"),
            s("@config.knob"),
            SExpression::Array(vec![s("emit"), s("A")]),
            SExpression::Array(vec![s("emit"), s("B")]),
        ]);
        let mut fired = FiredEvents::default();
        collect_fired_with_knobs("T", &tree, &orbital_core_base::schema::SObject::new(), &mut fired);
        assert!(fired
            .by_effect
            .contains(&("T".to_string(), "A".to_string())));
        assert!(fired
            .by_effect
            .contains(&("T".to_string(), "B".to_string())));
    }

    #[test]
    fn live_sources_skips_false_branch() {
        let value = SExpression::Array(vec![s("if"), s(""), s("SAVE"), s("CANCEL")]);
        let leaves = value.value_leaves(&live_sources());
        assert_eq!(leaves, vec![&s("CANCEL")]);
    }

    #[test]
    fn live_sources_keeps_both_when_undecidable() {
        let value = SExpression::Array(vec![s("if"), s("@config.knob"), s("SAVE"), s("CANCEL")]);
        let leaves = value.value_leaves(&live_sources());
        assert_eq!(leaves, vec![&s("SAVE"), &s("CANCEL")]);
    }

    #[test]
    fn live_sources_returns_self_for_non_conditional() {
        let value = s("SAVE");
        let leaves = value.value_leaves(&live_sources());
        assert_eq!(leaves, vec![&value]);
    }

    // ---- `kind: "event-map"` — a d-pad/canvas direction/key→event object --

    #[test]
    fn event_map_values_count_as_by_control_producers() {
        // { type: "control-grid", directionEvents: { left: "STEER_LEFT", right: "STEER_RIGHT" } }
        let node = SExpression::Object(
            [
                ("type".to_string(), s("control-grid")),
                (
                    "directionEvents".to_string(),
                    SExpression::Object(
                        [
                            ("left".to_string(), s("STEER_LEFT")),
                            ("right".to_string(), s("STEER_RIGHT")),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let mut fired = FiredEvents::default();
        collect_fired_with_knobs("T", &node, &orbital_core_base::schema::SObject::new(), &mut fired);
        assert!(
            fired
                .by_control
                .contains(&("T".to_string(), "STEER_LEFT".to_string())),
            "{:?}",
            fired.by_control
        );
        assert!(
            fired
                .by_control
                .contains(&("T".to_string(), "STEER_RIGHT".to_string())),
            "{:?}",
            fired.by_control
        );
    }

    #[test]
    fn event_map_binding_to_the_whole_object_is_skipped_not_crashed() {
        // directionEvents: "@config.directionEvents" — a whole-map binding,
        // not itself an object of literal event names; unresolvable, skipped.
        let node = SExpression::Object(
            [
                ("type".to_string(), s("control-grid")),
                ("directionEvents".to_string(), s("@config.directionEvents")),
            ]
            .into_iter()
            .collect(),
        );
        let mut fired = FiredEvents::default();
        collect_fired_with_knobs("T", &node, &orbital_core_base::schema::SObject::new(), &mut fired);
        assert!(fired.by_control.is_empty(), "{:?}", fired.by_control);
    }
}
