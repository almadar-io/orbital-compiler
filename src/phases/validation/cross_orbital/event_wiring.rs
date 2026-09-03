use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EventScope, OrbitalSchema, TraitDefinition, TraitReference};
use std::collections::{HashMap, HashSet};

/// Validate event wiring between orbitals
pub(crate) fn validate_event_wiring(schema: &OrbitalSchema) -> ValidationResult {
    // Schema-wide event reachability, from the walker `emit_payload`'s
    // declared-but-unfired check shares. Computed here rather than threaded
    // through `ValidationContext` so the function stays self-sufficient: a
    // caller that forgets to populate a context field would silently
    // reclassify every finding as a latent offer, and an empty set is a
    // legitimate value that no assertion could catch.
    let fired_events = &super::super::fired_events::collect_schema_fired_events(schema);
    let dynamic_event_traits = &super::super::fired_events::collect_dynamic_event_traits(schema);
    let mut result = ValidationResult::new();

    // `scope: external` is an offer for A consumer to wire, wherever that
    // trait was declared — so `exempt_external` is built uniformly for
    // authored AND imported traits. For an authored trait the deferred
    // consumer may be a sibling orbital outside this file, so the exemption
    // always holds. For an imported trait THIS file is the consumer the atom
    // declared external for, so honouring the exemption unconditionally would
    // re-evaporate the whole check (measured: 947 findings to 0, the composed
    // `ui-input` `ACTION` knob alone exempting every composed input in the
    // corpus) — the consumption site below therefore skips this pre-filter
    // for an imported trait's DEAD-CONTROL case (`by_control`), where a
    // rendered affordance firing the event makes the question decidable
    // regardless of scope; every other imported case is additionally covered
    // by the reachability classification (a/b/d) further down.
    //
    // Keyed per (trait, event), not by event NAME — a single `ACTION ->
    // external` anywhere must not exempt every trait emitting `ACTION`
    // everywhere. See ORB-X-EXTERNAL-EMIT-EXEMPTION in
    // `docs/Almadar_Compiler_Gaps.md`.
    let mut exempt_external: HashSet<(String, String)> = HashSet::new();
    // The subset of `exempt_external` whose scope came from a CALL-SITE
    // `emitsScope` override (`scope_overridden`), not the atom's own
    // declaration. For an imported trait's dead control, only THIS form
    // exempts: the composition consciously forwarded the obligation
    // ("my consumer wires this"), which is a different statement from the
    // atom's unexamined `-> external` default.
    let mut call_site_external: HashSet<(String, String)> = HashSet::new();
    let mut imported_traits: HashSet<String> = HashSet::new();
    // An emit whose event NAME came from a config knob (`emits { @config.X }`)
    // is the atom OFFERING an affordance, not this file promising to use it.
    // `EmitDefinition::definer_knob` records that at L1, where the `@config.X`
    // spelling still exists; the inline phase then overwrites `event` with the
    // resolved literal and the provenance would otherwise be lost.
    let mut offer_emits: HashSet<(String, String)> = HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(td) = trait_ref {
                if let Some(emits) = &td.emits {
                    for emit in emits {
                        if emit.definer_knob.is_some() {
                            offer_emits.insert((td.name.clone(), emit.event.clone()));
                        }
                        if emit.scope == Some(EventScope::External) {
                            exempt_external.insert((td.name.clone(), emit.event.clone()));
                            if emit.scope_overridden == Some(true) {
                                call_site_external.insert((td.name.clone(), emit.event.clone()));
                            }
                        }
                    }
                }
                if td.source_behavior.is_some() {
                    imported_traits.insert(td.name.clone());
                }
            }
        }
    }

    // Collect all emitted events with their sources (HashSet deduplicates same-trait emissions)
    let mut emitted_events: HashMap<String, HashSet<String>> = HashMap::new(); // event -> {trait names}

    // Collect all listened events with their listeners (source-scoped AND wildcard).
    // Used by the unused-emission check — a wildcard listen `* X -> TRIG`
    // counts as a consumer of event X, so declared emits remain "used."
    let mut listened_events_all: HashMap<String, HashSet<String>> = HashMap::new(); // event -> {trait names}

    // Subset: listens with explicit source (Trait/Orbital) only. Wildcard
    // listens are excluded because they declare "any source," which includes
    // runtime-fired events that don't appear in the static schema (e.g.
    // `agent/memorize` firing MEMORIZED). Orphan-listener checks use this
    // stricter set so wildcards never register as orphans.
    let mut listened_events_strict: HashMap<String, HashSet<String>> = HashMap::new();

    // Collect all transition triggers per trait. These are events a trait REACTS
    // to (inbound from the bus) — not declared emits, but since the bus
    // broadcasts every dispatched event to every subscriber, another trait that
    // `listens { TraitA X -> ... }` is validly wired when TraitA has a
    // transition with trigger X. The orphan-listener check below accepts this
    // as a valid producer; the unused-emission check does NOT (declared emits
    // still require a listener).
    let mut trigger_events: HashMap<String, HashSet<String>> = HashMap::new(); // event -> {trait names}

    // Process inline traits in orbitals
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    let mut soft_emitted_events: HashMap<String, HashSet<String>> = HashMap::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                collect_trait_events(
                    trait_def,
                    &mut emitted_events,
                    &mut soft_emitted_events,
                    &mut listened_events_all,
                    &mut listened_events_strict,
                );
                for transition in &trait_def.state_machine.transitions {
                    trigger_events
                        .entry(transition.event.clone())
                        .or_default()
                        .insert(trait_def.name.clone());
                }
            }
        }
    }

    // Check for orphan listeners (listening to events that are never emitted).
    // Accept both declared emits AND transition triggers as valid producers —
    // the bus broadcasts dispatched events (whether fired by `(emit X)` or by
    // a UI event dispatch targeting a trait's transition trigger), so a
    // subscriber wired to `TraitA X` is valid if TraitA reacts to X either way.
    //
    // Only source-scoped listens are checked here; wildcard listens intentionally
    // declare "any source" and may subscribe to runtime-fired events that never
    // appear in the static schema.
    for (event, listeners) in &listened_events_strict {
        if !emitted_events.contains_key(event)
            && !soft_emitted_events.contains_key(event)
            && !trigger_events.contains_key(event)
        {
            for listener in listeners {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CrossOrbitalOrphanListener,
                        format!("traits.{}", listener),
                        format!(
                            "Trait '{}' listens for event '{}' but no trait emits it",
                            listener, event
                        ),
                    )
                    .with_suggestion(
                        "Ensure the event is emitted by another trait or remove the listener",
                    ),
                );
            }
        }
    }

    // Check for unused emissions (emitting events that nothing listens to)
    // Skip INIT (reserved self-targeting event used for re-rendering).
    // Uses the "all" listen set (includes wildcards) so a wildcard subscriber
    // counts as a valid consumer. Also accepts transition triggers —
    // a trait whose transition reacts to X is a valid consumer of X on the
    // bus, even without an explicit listens entry (common pattern for atoms
    // like std-push where an `os/watch-http` emits PUSH_RECEIVED which the
    // trait's own PUSH_RECEIVED transition reacts to).
    for (event, emitters) in &emitted_events {
        if event == "INIT" {
            continue;
        }
        if !listened_events_all.contains_key(event) && !trigger_events.contains_key(event) {
            for emitter in emitters {
                // HOW is it produced? `by_control` means a control on screen is
                // wired to fire it (`action:`, `itemActions[].event`, `onX:`) —
                // no listener there is a control the user can operate that does
                // nothing, and the value it carries is discarded. `by_effect`
                // means an `(emit …)`/emit-config broadcast: a learning `Sim`
                // publishing `PENDULUM_UPDATED`, an asset catalog publishing its
                // art. No listener there means the collector — which lives in
                // another behavior — was not composed here, a composition
                // choice rather than a broken wire.
                //
                // A trait-level "does it render?" proxy was tried first and is
                // too coarse: a Sim renders its canvas AND broadcasts telemetry
                // from a tick effect, so ~100 broadcasts read as dead controls.
                let by_control = fired_events
                    .by_control
                    .contains(&(emitter.clone(), event.clone()));
                let imported = imported_traits.contains(emitter);

                // Exemption is per-emitter, not per event name — see the
                // `exempt_external` construction above. For an imported
                // trait's dead-control case only the CALL-SITE `emitsScope`
                // form exempts — honouring the SOURCE atom's `-> external`
                // there was measured and rejected (947→0, the check
                // evaporates — see ORB-X-EXTERNAL-EMIT-EXEMPTION in
                // Almadar_Compiler_Gaps.md), while a composition restating
                // it is the consumer's own obligation-forward
                // (C-X-ATOM-CHAIN-AFFORDANCE).
                let key = (emitter.clone(), event.clone());
                if (!(imported && by_control) && exempt_external.contains(&key))
                    || call_site_external.contains(&key)
                {
                    continue;
                }

                // The `definer_knob` offer exemption is UNCONDITIONAL — a
                // control whose event NAME is a config knob is the atom
                // offering an affordance whatever fires it. Guarding it behind
                // `!(imported && by_control)` was tried and re-admitted the
                // whole optional-affordance population (`SELECTION`,
                // `LOAD_MORE`, `DISMISS`, …) as dead controls: 7 → 1,260
                // measured. The composed-input worry it answered is owned by
                // ORB_ENTITY_SPLIT_MUTATION (§61) — a knob-named control's
                // unheard emit shows up there as the reader-less service
                // entity, not here.
                if offer_emits.contains(&(emitter.clone(), event.clone())) {
                    continue;
                }

                // REACHABILITY — does anything here PRODUCE this event?
                // `fired_events` is the schema-wide produced set — `(emit …)`
                // effects, operator emit-config, and STRUCTURED config event
                // lists (an `itemActions: [{ event: "DELETE" }]` the consumer
                // configured). An emit something fires with no listener is a
                // live wire terminating in nothing, whatever the emitting
                // trait's provenance; an emit nothing fires is at worst a stale
                // declaration, which `emit_payload`'s declared-but-unfired
                // check owns.
                let produced = fired_events.contains_for_trait(emitter, event);
                let undecidable = dynamic_event_traits.contains(emitter);

                // For an IMPORTED trait every non-dead-control case is a
                // consequence-free finding and is not constructed at all:
                // (a) BROADCAST — produced programmatically (by_effect) with
                //     no rendered control; the collector living in another
                //     behavior and not being composed here is a composition
                //     choice, not a defect.
                // (b) UNDECIDABLE — the fired name is runtime-bound; neither
                //     confirming nor denying reachability is sound.
                // (d) UNEXERCISED OFFER — nothing in this composition
                //     produces it at all (the "stale" class): normally a
                //     latent affordance the call site never configured, UNLESS
                //     the file's own persist/effect logic still references
                //     the event name, which is a real (if indirect) use and
                //     keeps the finding.
                // The dead-control case (`by_control`) is deliberately NOT
                // touched here — that is the composed-input class (a rendered
                // control firing an event nothing consumes, e.g. the
                // Stripe-amount-0 family) and must keep reporting.
                if imported {
                    if produced && !by_control {
                        continue; // (a) broadcast
                    }
                    if undecidable {
                        continue; // (b) undecidable
                    }
                    if !produced && !event_referenced_in_persist_or_effect_flow(schema, event) {
                        continue; // (d) unexercised offer
                    }
                }

                let suggestion = if imported {
                    format!(
                        "This trait is composed from an imported behavior, so THIS file is the consumer its `-> external` emit was declared for. Wire it at the call site: rename the event onto one you handle (`events {{ {event}: <YOUR_EVENT> }}`) and add a matching transition arm, or add `listens {{ {emitter}.{event} -> <TRIGGER> }}`. Re-declaring it `-> external` no longer suppresses this — that only defers an emit whose trait is DECLARED here."
                    )
                } else {
                    format!(
                        "Consume it where it fires: add a transition arm `{event} -> <state>` in the emitting trait (the usual pattern for persist/fetch success|failure events), declare the event `-> external` if a SIBLING orbital consumes it, or remove it from the emit config"
                    )
                };
                let message = format!(
                    "Trait '{}' emits event '{}' but no trait listens for it{}",
                    emitter,
                    event,
                    if undecidable {
                        " (the event name is bound at runtime, so reachability cannot be decided statically)"
                    } else if by_control {
                        " — and a RENDERED control here fires it, so the affordance is dead and the value it carries is discarded"
                    } else if produced {
                        " — this composition broadcasts it programmatically, and the collector for it was not composed here"
                    } else {
                        " (nothing in this composition fires it, so this is a stale declaration rather than a dead control)"
                    }
                );
                // The dead-control class is the real defect and is now
                // blocking severity, corpus-wide.
                const DEAD_CONTROL_IS_BLOCKING: bool = true;
                let blocking = by_control && !undecidable && DEAD_CONTROL_IS_BLOCKING;
                let finding = if !blocking && imported {
                    ValidationError::error(
                        ErrorCode::CrossOrbitalUnusedEmission,
                        format!("traits.{}", emitter),
                        message,
                    )
                } else {
                    ValidationError::error(
                        ErrorCode::CrossOrbitalUnusedEmission,
                        format!("traits.{}", emitter),
                        message,
                    )
                };
                result.add_error(finding.with_suggestion(suggestion));
            }
        }
    }

    // (Removed) Same-event-name-multiple-traits collision check.
    //
    // The runtime now qualifies every emit at the producer side via
    // `TraitScopeProvider` (in `@almadar/ui` from 4.10.0): a button
    // rendered inside trait T fires on bus key `UI:Orbital.T.EVENT`,
    // and trait subscribers listen on the same qualified form. So when
    // multiple traits in the same orbital declare `emits { CLOSE }`
    // (e.g. std-cart's CartItemAddItem + CartItemRemoveConfirm both
    // inheriting CLOSE from std-modal / std-confirmation), they emit on
    // distinct bus keys and don't collide at runtime. The check was
    // designed for a pre-Phase-4 model where emits flowed on bare
    // `UI:EVENT` keys; that model is gone.

    result
}

/// Collect events from a trait definition
/// Uses HashSet to deduplicate same-trait emissions from multiple transitions.
///
/// `listened_all` includes every non-external listen (source-scoped + wildcard),
/// used by the unused-emission check. `listened_strict` excludes wildcards and
/// is used by the orphan-listener check.
pub(crate) fn collect_trait_events(
    trait_def: &TraitDefinition,
    emitted: &mut HashMap<String, HashSet<String>>,
    // Producible-but-undeclared events (effect emit-option maps, pattern
    // event props). These SATISFY listeners (orphan/source checks) but do
    // not DEMAND one — a configured extra channel with no subscriber is not
    // an unused emission, because the canonical event still drives the
    // circuit.
    soft_emitted: &mut HashMap<String, HashSet<String>>,
    listened_all: &mut HashMap<String, HashSet<String>>,
    listened_strict: &mut HashMap<String, HashSet<String>>,
) {
    // Build set of events declared as internal scope — these are self-consumed
    let internal_events: HashSet<String> = trait_def
        .emits
        .as_ref()
        .map(|emits| {
            emits
                .iter()
                .filter(|e| e.scope == Some(EventScope::Internal))
                .map(|e| e.event.clone())
                .collect()
        })
        .unwrap_or_default();

    // Collect from emits declaration (skip internal scope - they're not for cross-orbital).
    // External-scope emits stay in the `emitted` map: same-orbital listeners can
    // still consume them via `source: { kind: 'trait', ... }`, and we need that
    // intra-orbital wiring to register so the orphan-listener check doesn't
    // false-positive. The unused-emission iteration at the call site handles
    // external emits separately.
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            if emit.scope == Some(EventScope::Internal) {
                continue;
            }
            emitted
                .entry(emit.event.clone())
                .or_default()
                .insert(trait_def.name.clone());
        }
    }

    // Collect from listens declaration
    // COMP-2 fix: mirror the emit-side skip for `scope: external`. An external
    // listen is wired to another orbital's external emitter (typically via the
    // agent/integration bus), not to a sibling trait's emits[]. Counting it
    // here produced false-positive ORB_X_ORPHAN_LISTENER warnings whenever a
    // trait consumed an external event without a co-located emitter.
    //
    // Phase 9.5.G fix: also skip wildcard-source listens (`* EVENT -> TRIG`).
    // A wildcard explicitly declares "any source," which includes runtime-fired
    // events that never appear in the static schema (e.g. `agent/memorize`
    // firing MEMORIZED). If a wildcard has no intra-orbital source, that is
    // the author's explicit intent, not an orphan.
    if let Some(listens) = &trait_def.listens {
        use orbital_core::schema::ListenSource;
        for listen in listens {
            if listen.scope == Some(EventScope::External) {
                continue;
            }
            // Cross-orbital listens (`source: { kind: 'orbital', ... }`)
            // resolve to a sibling orbital's trait. Track them in
            // `listened_all` so a same-orbital external emit with only a
            // cross-orbital consumer still passes the unused-emission
            // check; exclude them from `listened_strict` so the orphan-
            // listener check doesn't fire when the schema only contains
            // this orbital.
            let is_cross_orbital = matches!(listen.source, Some(ListenSource::Orbital { .. }));
            listened_all
                .entry(listen.event.clone())
                .or_default()
                .insert(trait_def.name.clone());
            if !matches!(listen.source, Some(ListenSource::Any)) && !is_cross_orbital {
                listened_strict
                    .entry(listen.event.clone())
                    .or_default()
                    .insert(trait_def.name.clone());
            }
        }
    }

    // Collect from transition effects (emit calls)
    // Skip events declared as internal in the emits array — they're self-consumed
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if let Some(event_name) = extract_emit_event(effect) {
                    if !internal_events.contains(&event_name) {
                        emitted
                            .entry(event_name)
                            .or_default()
                            .insert(trait_def.name.clone());
                    }
                }
            }
        }
    }

    // Collect the trait's remaining PRODUCIBLE events — effect emit-option
    // maps and render-ui pattern event props — so scoped listeners on them
    // don't false-positive as orphans. Soft: satisfies listeners, never
    // demands one.
    for event_name in collect_effect_produced_events(trait_def) {
        if !internal_events.contains(&event_name) {
            soft_emitted
                .entry(event_name)
                .or_default()
                .insert(trait_def.name.clone());
        }
    }
}

/// Events a trait can PRODUCE beyond `emits` declarations and literal
/// `["emit", X]` arrays:
/// - effect emit-option maps — `emit: {success|failure|on_message|on_change}`
///   on fetch / persist / call-service / ref / os-watch / set effects (the
///   slot-key universe of every `validate_emit_config` call site), plus the
///   string form `emit: "X"`. A `[lifecycle]` fetcher whose only production
///   of `FooLoaded` is its fetch emit map was invisible to the emitter index
///   (C-VALIDATE-ORPHAN-LISTENER-LIFECYCLE-FETCH).
/// - render-ui pattern event props (registry `kind: event|event-ref|callback`,
///   e.g. search-input / filter-group `event`) and event-list descriptor
///   fields — the component fires `UI:<value>`, so the configured name IS an
///   emission of this trait. Mirrors the runtime wiring lint's
///   `producibleEvents` crediting.
///
/// `@`-prefixed values (unresolved bindings) are never credited.
pub(crate) fn collect_effect_produced_events(trait_def: &TraitDefinition) -> HashSet<String> {
    use orbital_core::schema::SExpression;

    fn credit(value: &SExpression, out: &mut HashSet<String>) {
        if let SExpression::String(ev) = value {
            if !ev.is_empty() && !ev.starts_with('@') {
                out.insert(ev.clone());
            }
        }
    }

    fn walk(expr: &SExpression, out: &mut HashSet<String>) {
        match expr {
            SExpression::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            SExpression::Object(map) => {
                match map.get("emit") {
                    Some(SExpression::Object(emit_map)) => {
                        for (slot, slot_v) in emit_map {
                            if matches!(
                                slot.as_str(),
                                "success" | "failure" | "on_message" | "on_change"
                            ) {
                                credit(slot_v, out);
                            }
                        }
                    }
                    Some(other) => credit(other, out),
                    None => {}
                }
                if let Some(SExpression::String(pattern_type)) = map.get("type") {
                    use crate::phases::validation::sync::patterns::{
                        PATTERN_EVENT_LIST_PROPS, PATTERN_EVENT_PROPS,
                    };
                    let static_props = PATTERN_EVENT_PROPS.get(pattern_type.as_str());
                    let list_props = PATTERN_EVENT_LIST_PROPS.get(pattern_type.as_str());
                    let meta = crate::embedded::get_pattern(pattern_type);
                    for (k, v) in map {
                        let is_event_prop = static_props
                            .is_some_and(|props| props.contains(k.as_str()))
                            || meta.is_some_and(|p| {
                                p.props_schema
                                    .get(k.as_str())
                                    .and_then(|prop| prop.kind.as_deref())
                                    .is_some_and(|kind| {
                                        matches!(kind, "event" | "event-ref" | "callback")
                                    })
                            });
                        if is_event_prop {
                            for leaf in v.conditional_leaf_values() {
                                credit(leaf, out);
                            }
                        }
                        if let Some(field) = list_props.and_then(|m| m.get(k.as_str())) {
                            // A conditional (`if`-wrapped) event-list config
                            // contributes the union of both branches — the
                            // runtime picks one at render time, but either is
                            // a legal listen source.
                            for leaf in v.conditional_leaf_values() {
                                if let SExpression::Array(descriptors) = leaf {
                                    for descriptor in descriptors {
                                        if let SExpression::Object(dm) = descriptor {
                                            if let Some(ev) = dm.get(*field) {
                                                credit(ev, out);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                for v in map.values() {
                    walk(v, out);
                }
            }
            _ => {}
        }
    }

    let mut out = HashSet::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                walk(effect, &mut out);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                walk(effect, &mut out);
            }
        }
    }
    out
}

/// Extract event name from an emit effect
/// True when at least one transition with the given event can propagate
/// — i.e., its effects include an `(emit ...)` op or a `fetch` /
/// `persist` / `call-service` / `ref` / `deref` carrying an
/// `emit: { success: ... }` (or `failure: ...`) entry. A trait that
/// only `set`s state, renders UI, or otherwise has no event-producing
/// effect is terminal: the cycle-detector should skip its inbound edges.
pub(crate) fn transition_can_propagate(trait_def: &TraitDefinition, event: &str) -> bool {
    use orbital_core::schema::SExpression;
    for transition in &trait_def.state_machine.transitions {
        if transition.event != event {
            continue;
        }
        let Some(effects) = &transition.effects else {
            continue;
        };
        for effect in effects {
            if extract_emit_event(effect).is_some() {
                return true;
            }
            if let SExpression::Array(arr) = effect {
                if arr.len() >= 2 {
                    if let SExpression::String(op) = &arr[0] {
                        if matches!(
                            op.as_str(),
                            "fetch" | "persist" | "ref" | "deref" | "call-service" | "agent/invoke"
                        ) {
                            for item in arr.iter().skip(1) {
                                if let SExpression::Object(opts) = item {
                                    if opts.contains_key("emit") {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

pub(crate) fn extract_emit_event(effect: &orbital_core::schema::SExpression) -> Option<String> {
    use orbital_core::schema::SExpression;

    match effect {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(op) = &arr[0] {
                if op == "emit" {
                    if let SExpression::String(event) = &arr[1] {
                        return Some(event.clone());
                    }
                }
            }
        }
        SExpression::Object(obj) => {
            if let Some(SExpression::String(event)) = obj.get("emit") {
                return Some(event.clone());
            }
        }
        _ => {}
    }
    None
}

/// True when `event` appears as a literal string anywhere inside a `persist`
/// effect (guard or transition/tick effect) in the schema — broader than
/// `produced` (which only credits emit-shaped positions: `(emit …)`, operator
/// emit-config, render-ui event props). An imported trait's declared-but-
/// unproduced emit is normally an unexercised OFFER (see the (d) arm above),
/// but if the composition's own persistence logic still keys off the event
/// name (e.g. an audit/filter payload literally naming it), that is a real
/// use and the finding should stay reported rather than being silently
/// dropped as stale.
fn event_referenced_in_persist_or_effect_flow(schema: &OrbitalSchema, event: &str) -> bool {
    use orbital_core::schema::SExpression;

    fn is_persist_effect(expr: &SExpression) -> bool {
        matches!(expr, SExpression::Array(arr) if matches!(arr.first(), Some(SExpression::String(op)) if op == "persist"))
    }

    fn contains_literal(expr: &SExpression, event: &str) -> bool {
        match expr {
            SExpression::String(s) => s == event,
            SExpression::Array(items) => items.iter().any(|i| contains_literal(i, event)),
            SExpression::Object(map) => map.values().any(|v| contains_literal(v, event)),
            _ => false,
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for transition in &td.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        if is_persist_effect(effect) && contains_literal(effect, event) {
                            return true;
                        }
                    }
                }
            }
            if let Some(ticks) = &td.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        if is_persist_effect(effect) && contains_literal(effect, event) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {

    /// Drive the check the way `validate_cross_orbital` does — the fired-event
    /// and dynamic-trait sets come from the same shared walker production uses,
    /// so a fixture that fires nothing is classified a latent offer here too.
    fn wire(schema: &OrbitalSchema) -> ValidationResult {
        validate_event_wiring(schema)
    }
    use super::*;
    use orbital_core::schema::{
        EntityDefinition, EntityRef, Orbital, StateMachine, TraitDefinition,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    shared: false,
                    identity: false,
                    read_policy: None,
                    create_policy: None,
                    update_policy: None,
                    delete_policy: None,
                    access_waivers: None,
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                    id: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
                types: None,
                id: None,
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
        }
    }

    // =========================================================================
    // COMP-2: external listen no longer counts as an orphan
    // =========================================================================

    /// Positive case for COMP-2 fix: a trait with an `external`-scoped listen
    /// and no in-schema emitter must NOT produce `ORB_X_ORPHAN_LISTENER`. The
    /// expectation is that the agent bus / another orbital is responsible for
    /// emitting this event; the sibling-only check was producing false
    /// positives.
    #[test]
    fn test_external_listen_not_reported_as_orphan() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            type_params: None,
            type_args: None,
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: None,
            listens: Some(vec![orbital_core::schema::ListenDefinition {
                event: "USER_LOGGED_IN".to_string(),
                triggers: None,
                scope: Some(EventScope::External),
                source: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
                event_id: None,
                triggers_id: None,
            }]),
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        })];

        let result = wire(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "External listen should not produce ORB_X_ORPHAN_LISTENER. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case for COMP-2 fix: an `internal`-scoped listen (or one with
    /// no scope, defaulting to internal-equivalent for orphan checks — but we
    /// only check explicit External here) with no in-schema emitter SHOULD
    /// still produce the ORB_X_ORPHAN_LISTENER warning so regressions don't
    /// silently drop coverage.
    #[test]
    fn test_internal_listen_still_reports_orphan() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            type_params: None,
            type_args: None,
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: None,
            listens: Some(vec![orbital_core::schema::ListenDefinition {
                event: "SOMETHING_WITHIN_ORBITAL".to_string(),
                triggers: None,
                scope: Some(EventScope::Internal),
                source: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
                event_id: None,
                triggers_id: None,
            }]),
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        })];

        let result = wire(&schema);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "Internal listen with no emitter should still produce ORB_X_ORPHAN_LISTENER. \
             Warnings: {:?}",
            result.errors
        );
    }

    // =========================================================================
    // Producible-event crediting (soft emissions): pattern event props +
    // effect emit-option maps satisfy listeners without demanding one.
    // =========================================================================

    fn bare_trait(name: &str) -> TraitDefinition {
        TraitDefinition {
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    fn render_ui_with_event_prop(pattern_type: &str, prop: &str, event: &str) -> SExpression {
        use std::collections::HashMap as Map;
        let mut node: Map<String, SExpression> = Map::new();
        node.insert(
            "type".to_string(),
            SExpression::String(pattern_type.to_string()),
        );
        node.insert(prop.to_string(), SExpression::String(event.to_string()));
        SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(node),
        ])
    }

    fn scoped_listener(name: &str, source_trait: &str, event: &str) -> TraitDefinition {
        let mut td = bare_trait(name);
        td.listens = Some(vec![orbital_core::schema::ListenDefinition {
            event: event.to_string(),
            triggers: Some("REFRESH".to_string()),
            scope: None,
            source: Some(orbital_core::schema::ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            event_id: None,
            triggers_id: None,
        }]);
        td
    }

    use orbital_core::schema::{SExpression, StateDefinition, TransitionDefinition};

    fn trait_with_effect(name: &str, effect: SExpression) -> TraitDefinition {
        let mut td = bare_trait(name);
        td.state_machine = StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }],
            events: None,
            transitions: vec![TransitionDefinition {
                from: orbital_core::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                event_id: None,
                guard: None,
                effects: Some(vec![effect]),
            }],
        };
        td
    }

    /// A scoped listener on a filter-group's configured `event` prop value is
    /// satisfied (the component fires `UI:<value>`), and the configured value
    /// alone does NOT trip the unused-emission check when nobody listens.
    #[test]
    fn pattern_event_prop_satisfies_scoped_listener_without_demanding_one() {
        // Listener present: no orphan.
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![
            TraitReference::Inline(trait_with_effect(
                "TaskFilter",
                render_ui_with_event_prop("filter-group", "event", "TASK_FILTER"),
            )),
            TraitReference::Inline(scoped_listener("TaskList", "TaskFilter", "TASK_FILTER")),
        ];
        let result = wire(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "configured pattern event prop must satisfy the scoped listener; got {:?}",
            result.errors
        );

        // No listener: soft emission demands nothing.
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![TraitReference::Inline(trait_with_effect(
            "TaskFilter",
            render_ui_with_event_prop("filter-group", "event", "TASK_FILTER"),
        ))];
        let result = wire(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalUnusedEmission),
            "a configured-but-unconsumed pattern event prop is a soft emission, \
             not an unused one; got {:?}",
            result.errors
        );
    }

    /// A `[lifecycle]`-style fetcher whose only production of an event is its
    /// fetch `emit: {success}` map satisfies a scoped std-stats-style listener.
    #[test]
    fn fetch_emit_map_satisfies_scoped_listener() {
        use std::collections::HashMap as Map;
        let mut emit_map: Map<String, SExpression> = Map::new();
        emit_map.insert(
            "success".to_string(),
            SExpression::String("SummaryLoaded".to_string()),
        );
        let mut options: Map<String, SExpression> = Map::new();
        options.insert("emit".to_string(), SExpression::Object(emit_map));
        let fetch_effect = SExpression::Array(vec![
            SExpression::String("fetch".to_string()),
            SExpression::String("Task".to_string()),
            SExpression::Object(options),
        ]);

        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![
            TraitReference::Inline(trait_with_effect("SummaryData", fetch_effect)),
            TraitReference::Inline(scoped_listener(
                "SummaryStats",
                "SummaryData",
                "SummaryLoaded",
            )),
        ];
        let result = wire(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "fetch emit-map event must satisfy the scoped listener; got {:?}",
            result.errors
        );
    }

    /// Build `["if", cond, then_descriptors, else_descriptors]` where each
    /// branch is an itemActions-style array of `{event: <name>}` descriptors.
    fn conditional_item_actions(then_event: &str, else_event: &str) -> SExpression {
        use std::collections::HashMap as Map;
        let descriptor = |event: &str| {
            let mut dm: Map<String, SExpression> = Map::new();
            dm.insert("name".to_string(), SExpression::String("Edit".to_string()));
            dm.insert("event".to_string(), SExpression::String(event.to_string()));
            SExpression::Object(dm)
        };
        SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.role".to_string()),
            SExpression::Array(vec![descriptor(then_event)]),
            SExpression::Array(vec![descriptor(else_event)]),
        ])
    }

    fn render_ui_with_item_actions(pattern_type: &str, item_actions: SExpression) -> SExpression {
        use std::collections::HashMap as Map;
        let mut node: Map<String, SExpression> = Map::new();
        node.insert(
            "type".to_string(),
            SExpression::String(pattern_type.to_string()),
        );
        node.insert("itemActions".to_string(), item_actions);
        SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(node),
        ])
    }

    /// A conditional (`if`-wrapped) `itemActions` config must credit BOTH
    /// branches' events into the derived producible-event set — a listener
    /// on a branch-only event is legal even though the runtime only fires it
    /// under one condition.
    #[test]
    fn conditional_item_actions_credits_both_branches_as_producible_events() {
        let td = trait_with_effect(
            "TaskList",
            render_ui_with_item_actions(
                "data-list",
                conditional_item_actions("EDIT_ITEM", "VIEW_ITEM"),
            ),
        );
        let produced = collect_effect_produced_events(&td);
        assert!(
            produced.contains("EDIT_ITEM"),
            "then-branch event missing from {:?}",
            produced
        );
        assert!(
            produced.contains("VIEW_ITEM"),
            "else-branch event missing from {:?}",
            produced
        );
    }

    /// Nested conditional (`if` inside a branch) must still be walked all the
    /// way down: every leaf branch's events are credited.
    #[test]
    fn nested_conditional_item_actions_credits_every_leaf_branch() {
        use std::collections::HashMap as Map;
        let descriptor = |event: &str| {
            let mut dm: Map<String, SExpression> = Map::new();
            dm.insert("name".to_string(), SExpression::String("Edit".to_string()));
            dm.insert("event".to_string(), SExpression::String(event.to_string()));
            SExpression::Object(dm)
        };
        let inner_if = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.tier".to_string()),
            SExpression::Array(vec![descriptor("PROMOTE_ITEM")]),
            SExpression::Array(vec![descriptor("DEMOTE_ITEM")]),
        ]);
        let outer_if = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.role".to_string()),
            inner_if,
            SExpression::Array(vec![descriptor("VIEW_ITEM")]),
        ]);
        let td = trait_with_effect(
            "TaskList",
            render_ui_with_item_actions("data-list", outer_if),
        );
        let produced = collect_effect_produced_events(&td);
        for expected in ["PROMOTE_ITEM", "DEMOTE_ITEM", "VIEW_ITEM"] {
            assert!(
                produced.contains(expected),
                "{expected} missing from {:?}",
                produced
            );
        }
    }

    /// End-to-end: a scoped listener on a branch-only emit (event fired only
    /// in the `if`'s else branch) resolves without `ORB_X_ORPHAN_LISTENER` —
    /// the full-schema counterpart of the derived-set test above.
    #[test]
    fn scoped_listener_on_conditional_branch_only_event_is_not_orphan() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![
            TraitReference::Inline(trait_with_effect(
                "TaskList",
                render_ui_with_item_actions(
                    "data-list",
                    conditional_item_actions("EDIT_ITEM", "VIEW_ITEM"),
                ),
            )),
            TraitReference::Inline(scoped_listener("TaskDetail", "TaskList", "VIEW_ITEM")),
        ];
        let result = wire(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "listener on the conditional's else-branch-only event must resolve; got {:?}",
            result.errors
        );
    }

    /// A trait emitting `event` with `scope: external` and nothing listening.
    /// `imported` mirrors what the inline phase stamps for a `uses[]` clone.
    /// A trait that PRODUCES an event through an `(emit …)` transition effect —
    /// the unambiguous producer, and a literal-named obligation (no
    /// `definer_knob`).
    fn emitting_trait_with_effect(name: &str, event: &str) -> TraitReference {
        let mut td = externally_emitting_trait(name, event, true);
        if let TraitReference::Inline(def) = &mut td {
            def.config = None;
            def.state_machine.transitions = vec![orbital_core::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core::schema::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "TICK".to_string(),
                guard: None,
                effects: Some(vec![orbital_core::schema::SExpression::Array(vec![
                    orbital_core::schema::SExpression::String("emit".to_string()),
                    orbital_core::schema::SExpression::String(event.to_string()),
                ])]),
            }];
        }
        td
    }

    fn externally_emitting_trait(name: &str, event: &str, imported: bool) -> TraitReference {
        TraitReference::Inline(TraitDefinition {
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![orbital_core::schema::EmitDefinition {
                definer_knob: None,
                scope_overridden: None,
                event: event.to_string(),
                event_id: None,
                description: None,
                synonyms: None,
                tier: None,
                scope: Some(EventScope::External),
                payload_schema: None,
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            // The config default alone (a SCALAR knob) is deliberately NOT
            // counted as production (see fired_events.rs) — a trait built
            // from just this is the "latent offer" class. The composed-input
            // regression below additionally renders a control bound to this
            // same resolved default, which post-inline is exactly what a
            // real composed input looks like.
            config: Some(
                [(
                    "action".to_string(),
                    orbital_core::schema::ConfigField {
                        field_type: "event".to_string(),
                        default: Some(orbital_core::schema::SExpression::String(event.to_string())),
                        label: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        values: None,
                        items: None,
                        properties: None,
                        required: None,
                        ref_id: None,
                    },
                )]
                .into_iter()
                .collect(),
            ),
            data_entities: None,
            scope: None,
            source_behavior: if imported {
                Some(orbital_core::schema::SourceBehaviorMetadata {
                    behavior: "ui-input".to_string(),
                    alias: "Input".to_string(),
                    original_name: "InputRender".to_string(),
                })
            } else {
                None
            },
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        })
    }

    /// Give a trait built by `externally_emitting_trait` an actual rendered
    /// control firing `event` — post-inline, this is what a real composed
    /// input's resolved `action` knob looks like (the config default alone
    /// is a scalar, deliberately not counted as production; the RENDERED
    /// prop is).
    fn render_control_firing(td: &mut TraitReference, event: &str) {
        let TraitReference::Inline(def) = td else {
            return;
        };
        def.state_machine.states = vec![orbital_core::schema::StateDefinition {
            name: "idle".to_string(),
            is_initial: Some(true),
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        }];
        def.state_machine.transitions = vec![orbital_core::schema::TransitionDefinition {
            event_id: None,
            from: orbital_core::schema::StringOrArray::Single("idle".to_string()),
            to: "idle".to_string(),
            event: "INIT".to_string(),
            guard: None,
            effects: Some(vec![orbital_core::schema::SExpression::Array(vec![
                orbital_core::schema::SExpression::String("render-ui".to_string()),
                orbital_core::schema::SExpression::String("main".to_string()),
                orbital_core::schema::SExpression::Object(
                    [
                        (
                            "type".to_string(),
                            orbital_core::schema::SExpression::String("button".to_string()),
                        ),
                        (
                            "action".to_string(),
                            orbital_core::schema::SExpression::String(event.to_string()),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ])]),
        }];
    }

    /// `(blocking, non_blocking)` — `add_error` routes by severity.
    /// `CrossOrbitalUnusedEmission` is hard-error-only now, so the second
    /// slot stays 0; kept as a pair so a future non-blocking class doesn't
    /// need every call site rewritten.
    fn unused_emission_counts(schema: &OrbitalSchema) -> (usize, usize) {
        let r = wire(schema);
        let count = |v: &Vec<ValidationError>| {
            v.iter()
                .filter(|e| e.code == ErrorCode::CrossOrbitalUnusedEmission)
                .count()
        };
        (count(&r.errors), count(&r.warnings))
    }

    /// An `external` emit on a trait AUTHORED in this schema stays exempt: its
    /// consumer may be a sibling orbital that isn't loaded here, so "nobody
    /// listens" is not decidable. This is the producer-side case and must keep
    /// working — validating `ui-input.lolo` alone may not warn.
    #[test]
    fn external_emit_on_authored_trait_is_exempt() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![externally_emitting_trait("InputRender", "ACTION", false)];
        assert_eq!(
            unused_emission_counts(&schema),
            (0, 0),
            "an external emit declared by a trait authored in this file defers to a sibling orbital"
        );
    }

    /// The SAME emit reached through a `uses` import is reported: this file is
    /// the consumer the atom declared `external` for, every party is present,
    /// and a missing listener is a dead affordance rather than a deferred one.
    ///
    /// Regression for the composed-input class — `std-service-marketplace`'s
    /// payment amount field emitted `CHANGE`/`ACTION` that nothing consumed, so
    /// `@entity.amount` stayed at its initialised `0` and Stripe was called
    /// with a zero amount, while `orb validate` reported 0 errors.
    #[test]
    fn external_emit_on_imported_trait_is_reported() {
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("OrderPaymentNumberInput", "ACTION", true);
        render_control_firing(&mut td, "ACTION");
        schema.orbitals[0].traits = vec![td];
        assert_eq!(
            unused_emission_counts(&schema),
            (1, 0),
            "an external emit on an imported trait is decidable here and must report"
        );
    }

    /// An imported trait's declared-but-unproduced emit ("stale" class) is an
    /// unexercised OFFER — nothing constructed at all, per the "stop
    /// constructing" doctrine — because nothing in this composition fires it
    /// and the file's persist/effect logic doesn't reference it either.
    #[test]
    fn an_unproduced_emit_on_an_imported_trait_is_silent() {
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("GridRender", "SORT", true);
        if let TraitReference::Inline(def) = &mut td {
            def.config = None; // nothing in this composition fires SORT
        }
        schema.orbitals[0].traits = vec![td];
        assert_eq!(
            unused_emission_counts(&schema),
            (0, 0),
            "an unexercised offer on an imported trait must not be constructed at all"
        );
    }

    /// A trait with a dynamic event-prop binding elsewhere (`action:
    /// "@config.event"`) is UNDECIDABLE for the whole trait. Imported: not
    /// constructed at all — neither confirming nor denying reachability is
    /// sound. Local (authored): unaffected, still reported — the undecidable
    /// suppression is imported-only, mirroring every other class in 1a/1b/1d.
    fn trait_with_dynamic_binding_and_declared_emit(
        name: &str,
        event: &str,
        imported: bool,
    ) -> TraitReference {
        use orbital_core::schema::SExpression;
        // Unscoped (not `external`), so `exempt_external` never applies —
        // isolates the undecidable predicate from the separate scope-based
        // exemption tested elsewhere.
        let mut td = externally_emitting_trait(name, event, imported);
        if let TraitReference::Inline(def) = &mut td {
            def.emits = Some(vec![orbital_core::schema::EmitDefinition {
                definer_knob: None,
                scope_overridden: None,
                event: event.to_string(),
                event_id: None,
                description: None,
                synonyms: None,
                tier: None,
                scope: None,
                payload_schema: None,
            }]);
            def.config = None;
            def.state_machine.states = vec![orbital_core::schema::StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }];
            def.state_machine.transitions = vec![orbital_core::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core::schema::StringOrArray::Single("idle".to_string()),
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
                            (
                                "action".to_string(),
                                SExpression::String("@config.event".to_string()),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ])]),
            }];
        }
        td
    }

    #[test]
    fn undecidable_imported_trait_is_silent() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![trait_with_dynamic_binding_and_declared_emit(
            "PagerInput",
            "PAGE_CHANGED",
            true,
        )];
        assert_eq!(
            unused_emission_counts(&schema),
            (0, 0),
            "an undecidable imported trait's declared emit must not be constructed at all"
        );
    }

    #[test]
    fn undecidable_local_trait_is_unaffected_and_still_reports() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![trait_with_dynamic_binding_and_declared_emit(
            "PagerRender",
            "PAGE_CHANGED",
            false,
        )];
        assert_eq!(
            unused_emission_counts(&schema),
            (1, 0),
            "the undecidable suppression is imported-only; local-trait behavior is unchanged"
        );
    }

    /// The residual case: even though nothing PRODUCES the event (no control,
    /// no `(emit …)`), the file's own persist logic literally references the
    /// event name — a real use, so the finding stays reported rather than
    /// being silently dropped as stale.
    #[test]
    fn an_unproduced_emit_referenced_by_persist_logic_still_reports() {
        use orbital_core::schema::SExpression;
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("GridRender", "SORT", true);
        if let TraitReference::Inline(def) = &mut td {
            def.config = None; // nothing in this composition fires SORT
            def.state_machine.transitions = vec![orbital_core::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core::schema::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(vec![SExpression::Array(vec![
                    SExpression::String("persist".to_string()),
                    SExpression::String("create".to_string()),
                    SExpression::String("AuditLog".to_string()),
                    SExpression::Object(
                        [(
                            "action".to_string(),
                            SExpression::String("SORT".to_string()),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                ])]),
            }];
        }
        schema.orbitals[0].traits = vec![td];
        assert_eq!(
            unused_emission_counts(&schema),
            (1, 0),
            "a persist effect literally referencing the event keeps the finding"
        );
    }

    /// A PROGRAMMATIC broadcast with no listener on an imported trait is the
    /// design working — the atom fires it and the collector for it (which
    /// lives in another behavior) simply wasn't composed here — so nothing is
    /// constructed at all.
    #[test]
    fn a_programmatic_broadcast_on_an_imported_trait_is_silent() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![emitting_trait_with_effect("Sim", "GAME_END")];
        assert_eq!(
            unused_emission_counts(&schema),
            (0, 0),
            "a broadcast on an imported trait must not be constructed at all"
        );
    }

    /// The dual: the same event named by a rendered control's event PROP is a
    /// dead affordance — something on screen fires it and nothing hears it.
    /// Note the emitting trait is identical in both tests; only HOW the event is
    /// produced differs, which is the whole point of the split.
    #[test]
    fn an_event_named_by_a_rendered_control_is_the_dead_control_class() {
        let mut schema = create_minimal_schema();
        let mut td = emitting_trait_with_effect("Sim", "GAME_END");
        if let TraitReference::Inline(def) = &mut td {
            if let Some(effects) = def.state_machine.transitions[0].effects.as_mut() {
                effects.clear();
                effects.push(orbital_core::schema::SExpression::Array(vec![
                    orbital_core::schema::SExpression::String("render-ui".to_string()),
                    orbital_core::schema::SExpression::String("main".to_string()),
                    orbital_core::schema::SExpression::Object(
                        [
                            (
                                "type".to_string(),
                                orbital_core::schema::SExpression::String("button".to_string()),
                            ),
                            (
                                "action".to_string(),
                                orbital_core::schema::SExpression::String("GAME_END".to_string()),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ]));
            }
        }
        schema.orbitals[0].traits = vec![td];
        let msgs: Vec<String> = wire(&schema)
            .errors
            .iter()
            .map(|e| e.message.clone())
            .collect();
        assert!(
            msgs.iter().any(|m| m.contains("RENDERED control")),
            "an event named by a control's action prop must be the dead-control class: {msgs:?}"
        );
    }

    /// The exemption is per (trait, event), not per event NAME. One authored
    /// trait declaring `ACTION -> external` must not exempt a DIFFERENT,
    /// imported trait that also emits `ACTION` — the global-by-name set was
    /// the original defect, and `ACTION` is `ui-input`'s default knob value, so
    /// a single atom silently exempted every composed input in the corpus.
    #[test]
    fn authored_external_emit_does_not_exempt_imported_namesake() {
        let mut schema = create_minimal_schema();
        let mut authored = externally_emitting_trait("AuthoredRender", "ACTION", false);
        render_control_firing(&mut authored, "ACTION");
        let mut imported = externally_emitting_trait("ImportedInput", "ACTION", true);
        render_control_firing(&mut imported, "ACTION");
        schema.orbitals[0].traits = vec![authored, imported];
        assert_eq!(
            unused_emission_counts(&schema),
            (1, 0),
            "only the imported emitter is reported; the authored namesake stays exempt"
        );
    }

    /// A CALL-SITE `emitsScope external` (recorded as `scope_overridden` by
    /// the inline phase) is the composition's own obligation-forward and
    /// exempts even an imported trait's dead control — the
    /// C-X-ATOM-CHAIN-AFFORDANCE forwarding surface.
    #[test]
    fn call_site_emits_scope_exempts_imported_dead_control() {
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("ArticleDoc", "EDIT", true);
        render_control_firing(&mut td, "EDIT");
        if let TraitReference::Inline(def) = &mut td {
            for e in def.emits.as_mut().unwrap() {
                e.scope_overridden = Some(true);
            }
        }
        schema.orbitals[0].traits = vec![td];
        assert_eq!(
            unused_emission_counts(&schema),
            (0, 0),
            "a call-site emitsScope forward exempts the imported dead control"
        );
    }

    /// SAFETY INVERSE: the source atom's own `-> external` (no call-site
    /// override, `scope_overridden` unset) still does NOT exempt an imported
    /// dead control — dropping that override was measured at 947→0.
    #[test]
    fn atom_declared_external_alone_still_reports_imported_dead_control() {
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("ArticleDoc", "EDIT", true);
        render_control_firing(&mut td, "EDIT");
        schema.orbitals[0].traits = vec![td];
        assert_eq!(
            unused_emission_counts(&schema),
            (1, 0),
            "atom-declared external without a call-site forward keeps reporting"
        );
    }
}
