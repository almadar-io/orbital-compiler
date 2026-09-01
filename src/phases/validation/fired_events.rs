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
//! `kind: "event" | "event-ref" | "event-listen" | "callback"` props and
//! `kind: "event-list"` inner fields, written by pattern-sync because a
//! component author typed the prop that way. No name heuristics.

use orbital_core::events::is_bus_excluded_event;
use orbital_core::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};

use crate::embedded::patterns;

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
#[derive(Default)]
pub(crate) struct FiredEvents {
    pub(crate) by_control: std::collections::HashSet<String>,
    pub(crate) by_effect: std::collections::HashSet<String>,
}

impl FiredEvents {
    /// Produced at all, either way — what the declared-but-unfired check asks.
    pub(crate) fn contains(&self, event: &str) -> bool {
        self.by_control.contains(event) || self.by_effect.contains(event)
    }
}

pub(crate) fn collect_schema_fired_events(schema: &OrbitalSchema) -> FiredEvents {
    let mut fired = FiredEvents::default();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for transition in &td.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        collect_fired_events_recursive(effect, &mut fired);
                    }
                }
            }
            if let Some(ticks) = &td.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        collect_fired_events_recursive(effect, &mut fired);
                    }
                }
            }
            if let Some(config) = &td.config {
                for field in config.values() {
                    if let Some(default) = &field.default {
                        collect_fired_events_recursive(default, &mut fired);
                    }
                }
            }
        }
    }
    fired
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
/// false-positive `EmitDeclaredButUnfired` warnings on atoms that
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

fn ui_event_prop_names() -> &'static std::collections::HashSet<String> {
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
pub(crate) fn collect_fired_events_recursive(expr: &SExpression, fired: &mut FiredEvents) {
    match expr {
        SExpression::Array(arr) => {
            // `["emit", "EVENT_NAME", ...]`
            if let (Some(SExpression::String(op)), Some(SExpression::String(event))) =
                (arr.first(), arr.get(1))
            {
                if op == "emit" {
                    fired.by_effect.insert(event.clone());
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
                                fired.by_effect.insert(s.clone());
                            }
                        }
                    }
                }
            }
            for item in arr {
                collect_fired_events_recursive(item, fired);
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
                    // A conditional value contributes every branch's leaf —
                    // only one renders at a time, but each is a live emitter.
                    for leaf in value.conditional_leaf_values() {
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
                                fired.by_control.insert(s.clone());
                            }
                        }
                    }
                }
                collect_fired_events_recursive(value, fired);
            }
        }
        _ => {}
    }
}

