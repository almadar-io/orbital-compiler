use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::events::is_bus_excluded_event;
use orbital_core_base::schema::{EventScope, OrbitalSchema, TraitDefinition, TraitReference};
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

    validate_sibling_handlers_routed(schema, &mut result);

    result
}

/// Every event a trait fires from its own logic — `(emit …)` effects (nested
/// in `when`/`if`/… too) and operator emit-config results — reaches another
/// trait only through a declared route. A sibling trait in the same orbital
/// with an arm for it and no route from this emitter is an intent that
/// terminates in nothing. UI-control events (render-ui event props) are not
/// in scope: the bus delivers those by name.
fn validate_sibling_handlers_routed(schema: &OrbitalSchema, result: &mut ValidationResult) {
    use orbital_core_base::schema::{ListenSource, SExpression};

    fn walk_emits(expr: &SExpression, out: &mut HashSet<String>) {
        if let Some(event) = extract_emit_event(expr) {
            out.insert(event);
        }
        if let SExpression::Array(items) = expr {
            for item in items {
                walk_emits(item, out);
            }
        }
    }

    let ui_fired = crate::phases::validation::fired_events::collect_schema_fired_events(schema);
    for orbital in &schema.orbitals {
        let traits: Vec<&TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def),
                _ => None,
            })
            .collect();
        let fired_by = |def: &TraitDefinition| -> HashSet<String> {
            let mut fired = HashSet::new();
            let mut async_results = HashSet::new();
            for transition in &def.state_machine.transitions {
                let effects = transition.effects.as_deref().unwrap_or(&[]);
                for effect in effects {
                    walk_emits(effect, &mut fired);
                }
                async_results.extend(collect_async_result_events(effects));
            }
            // A tick's emits travel the same declared routes.
            for tick in def.ticks.as_deref().unwrap_or(&[]) {
                for effect in &tick.effects {
                    walk_emits(effect, &mut fired);
                }
            }
            // Only emits that can fire: a branch folded statically false
            // (`(if (not @config.loopRound) (emit …))` with `loopRound: true`)
            // is the same dead producer ORB_X_LISTEN_SOURCE_NEVER_PRODUCES
            // rejects a route to — the two checks read one walker.
            fired.retain(|e| ui_fired.by_effect.contains(&(def.name.clone(), e.clone())));
            fired.extend(async_results);
            fired
        };
        let handles = |def: &TraitDefinition, event: &str| {
            def.state_machine
                .transitions
                .iter()
                .any(|t| t.event == event)
        };
        let routes_from = |handler: &TraitDefinition, event: &str, emitter: &TraitDefinition| {
            handler.listens.as_deref().unwrap_or(&[]).iter().any(|l| {
                l.event == event
                    && match &l.source {
                        None | Some(ListenSource::Any) => true,
                        Some(ListenSource::Trait { trait_name, .. }) => trait_name == &emitter.name,
                        Some(ListenSource::Orbital {
                            orbital: o,
                            trait_name,
                            ..
                        }) => o == &orbital.name && trait_name == &emitter.name,
                    }
            })
        };
        let fired: HashMap<&str, HashSet<String>> = traits
            .iter()
            .map(|t| (t.name.as_str(), fired_by(t)))
            .collect();
        for emitter in &traits {
            let mut events: Vec<&String> = fired[emitter.name.as_str()].iter().collect();
            events.sort();
            for event in events {
                if is_bus_excluded_event(event) {
                    continue;
                }
                for handler in &traits {
                    // Two instances of one atom each fire AND handle the event
                    // themselves (same-trait continuation on both sides) —
                    // neither is the other's audience.
                    let both_self_consume =
                        handles(emitter, event) && fired[handler.name.as_str()].contains(event);
                    if handler.name == emitter.name || !handles(handler, event) || both_self_consume
                    {
                        continue;
                    }
                    // A handler that routes the event from some source has
                    // chosen its audience (a lab's sim hears its own
                    // transport, not the other instrument's) — it is owed
                    // this emitter only when the emitter reaches no one.
                    let has_audience = handler.listens.as_deref().unwrap_or(&[]).iter().any(|l| {
                        l.triggers.as_deref().unwrap_or(&l.event) == event.as_str()
                            && matches!(
                                l.source,
                                Some(ListenSource::Trait { .. })
                                    | Some(ListenSource::Orbital { .. })
                            )
                    });
                    // Likewise an arm fed by the handler's own on-screen
                    // control is live without this emitter.
                    let self_fed = ui_fired
                        .by_control
                        .contains(&(handler.name.clone(), event.clone()));
                    if routes_from(handler, event, emitter)
                        || ((has_audience || self_fed)
                            && traits.iter().any(|t| {
                                t.name != emitter.name
                                    && handles(t, event)
                                    && routes_from(t, event, emitter)
                            }))
                    {
                        continue;
                    }
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EmitUnroutedToHandler,
                            format!("orbitals.{}.traits.{}", orbital.name, emitter.name),
                            format!(
                                "Trait '{}' emits '{}' and trait '{}' has an arm for it, but no declared route delivers it — events reach another trait only through `listens`",
                                emitter.name, event, handler.name
                            ),
                        )
                        .with_suggestion(format!(
                            "Add `listens {{ {}.{} -> {} }}` on '{}'",
                            emitter.name, event, event, handler.name
                        )),
                    );
                }
            }
        }
    }
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
        use orbital_core_base::schema::ListenSource;
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
/// - effect emit-option maps — `emit: {success|failure|on_message|on_change|cancelled}`
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
    use crate::phases::validation::value_walk::all_sources;
    use orbital_core_base::schema::SExpression;

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
                                "success" | "failure" | "on_message" | "onMessage" | "on_change" | "cancelled"
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
                            for leaf in v.value_leaves(&all_sources()) {
                                credit(leaf, out);
                            }
                        }
                        if let Some(field) = list_props.and_then(|m| m.get(k.as_str())) {
                            // Every declared value source of an event-list
                            // config contributes (both `if` branches, a
                            // sliced/sorted/concatenated list, a mapped
                            // lambda body): the runtime picks at render time,
                            // but any of them is a legal listen source. A
                            // source is a descriptor list or one descriptor.
                            for leaf in v.value_leaves(&all_sources()) {
                                let descriptors = match leaf {
                                    SExpression::Array(items) => items.as_slice(),
                                    SExpression::Object(_) => std::slice::from_ref(leaf),
                                    _ => &[],
                                };
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

/// Failure-only counterpart to [`collect_effect_produced_events`]: every
/// `emit.failure` string on a `fetch`/`persist`/`call-service` node anywhere
/// in `effects`, including nested inside `if`/`do` — the async-result
/// contract's failure half, isolated for the closed-circuit arm-completeness
/// checks (`ORB_CIRCUIT_FAILURE_ARM_*`). Mirrors `effect-emits.ts`'s
/// `collectFailureEvents` EXACTLY, including its operator allowlist: an
/// `emit.failure` on any OTHER op (e.g. `llm/generate`, `os-watch`) is not
/// this contract and must stay uncredited, or a headless/agent atom with no
/// UI of its own over-fires on every state-only failure arm (measured: a
/// generic "any node with an emit key" walk hit 121 files vs. the JS tool's
/// 0 on the same corpus sample). Declared data only: reads the effect
/// tuple's own operator name + `emit: {failure: X}` map, no name/text
/// matching beyond that fixed 3-operator allowlist.
pub(crate) fn collect_failure_events(
    effects: &[orbital_core_base::schema::SExpression],
) -> HashSet<String> {
    use orbital_core_base::schema::SExpression;
    fn walk(expr: &SExpression, out: &mut HashSet<String>) {
        let SExpression::Array(items) = expr else {
            return;
        };
        if let Some(SExpression::String(op)) = items.first() {
            if matches!(
                op.as_str(),
                "fetch" | "persist" | "call-service" | "http" | "redis" | "postgres" | "random-bytes" | "credential-status" | "credential-forget" | "oauth-authorize" | "oauth-connection-status" | "oauth-connection-forget" | "program/eval" | "program/compose"
            ) || orbital_core_base::credential_admin::CredentialAdminOp::from_name(op).is_some() {
                for item in items {
                    if let SExpression::Object(map) = item {
                        if let Some(SExpression::Object(emit_map)) = map.get("emit") {
                            if let Some(SExpression::String(ev)) = emit_map.get("failure") {
                                if !ev.is_empty() {
                                    out.insert(ev.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        for item in items {
            walk(item, out);
        }
    }
    let mut out = HashSet::new();
    for effect in effects {
        walk(effect, &mut out);
    }
    out
}

/// Collect EVERY value of the `emit` map (not `failure` alone, and not
/// success/failure by name) of every `fetch`/`persist` effect in one
/// transition's effect list, recursing through `if`/`do`. Sibling of
/// [`collect_failure_events`] for `async-result-deaf-target`, which needs
/// the destination state to hear the WHOLE result contract a request can
/// produce, not just its failure half. Two deliberate divergences from
/// `collect_failure_events`, both mirroring the JS wiring lint's own
/// `collectAsyncResultEvents` (`effect-emits.ts`) rather than reusing its
/// `collectFailureEvents` (`wiring-lint.ts`): narrower op scope (`fetch`/
/// `persist` only — no `call-service`, a real asymmetry in the JS source,
/// not a bug this port silently fixes) and wider emit-key scope (every
/// key in the `emit` object, since a target state must hear success too,
/// not only failure).
pub(crate) fn collect_async_result_events(
    effects: &[orbital_core_base::schema::SExpression],
) -> HashSet<String> {
    use orbital_core_base::schema::SExpression;
    fn walk(expr: &SExpression, out: &mut HashSet<String>) {
        let SExpression::Array(items) = expr else {
            return;
        };
        if let Some(SExpression::String(op)) = items.first() {
            if op == "fetch" || op == "persist" {
                for item in items {
                    if let SExpression::Object(map) = item {
                        if let Some(SExpression::Object(emit_map)) = map.get("emit") {
                            for v in emit_map.values() {
                                if let SExpression::String(ev) = v {
                                    if !ev.is_empty() {
                                        out.insert(ev.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for item in items {
            walk(item, out);
        }
    }
    let mut out = HashSet::new();
    for effect in effects {
        walk(effect, &mut out);
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
    use orbital_core_base::schema::SExpression;
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
                            "fetch" | "persist" | "ref" | "deref" | "call-service" | "http" | "redis" | "postgres" | "random-bytes" | "credential-status" | "credential-forget" | "oauth-authorize" | "oauth-connection-status" | "oauth-connection-forget" | "agent/invoke"
                        ) || orbital_core_base::credential_admin::CredentialAdminOp::from_name(op).is_some() {
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

pub(crate) fn extract_emit_event(effect: &orbital_core_base::schema::SExpression) -> Option<String> {
    use orbital_core_base::schema::SExpression;

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
    use orbital_core_base::schema::SExpression;

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
    use orbital_core_base::schema::{
        EntityDefinition, EntityRef, Orbital, StateMachine, TraitDefinition,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
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
                name: "TaskOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
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
            local: false,
            effect_row: None,
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
            listens: Some(vec![orbital_core_base::schema::ListenDefinition {
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
            local: false,
            effect_row: None,
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
            listens: Some(vec![orbital_core_base::schema::ListenDefinition {
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
        let mut node: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        td.listens = Some(vec![orbital_core_base::schema::ListenDefinition {
            event: event.to_string(),
            triggers: Some("REFRESH".to_string()),
            scope: None,
            source: Some(orbital_core_base::schema::ListenSource::Trait {
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

    use orbital_core_base::schema::{SExpression, StateDefinition, TransitionDefinition};

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
                from: orbital_core_base::schema::types::StringOrArray::Single("idle".to_string()),
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
        let mut emit_map: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
        emit_map.insert(
            "success".to_string(),
            SExpression::String("SummaryLoaded".to_string()),
        );
        let mut options: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        let descriptor = |event: &str| {
            let mut dm: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        let mut node: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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
        let descriptor = |event: &str| {
            let mut dm: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
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

    /// `(array/filter <descriptors> (fn …))` only narrows its collection, so a
    /// list bound through the filter credits exactly the events a bare list
    /// does; the bare binding is the paired control.
    #[test]
    fn filtered_item_actions_credit_the_same_events_as_a_bare_binding() {
        let descriptor = |event: &str| {
            let mut dm: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
            dm.insert("event".to_string(), SExpression::String(event.to_string()));
            SExpression::Object(dm)
        };
        let bare = || SExpression::Array(vec![descriptor("EDIT"), descriptor("DELETE")]);
        let filtered = SExpression::Array(vec![
            SExpression::String("array/filter".to_string()),
            bare(),
            SExpression::Array(vec![
                SExpression::String("fn".to_string()),
                SExpression::String("item".to_string()),
                SExpression::Bool(true),
            ]),
        ]);
        let produced_bare = collect_effect_produced_events(&trait_with_effect(
            "TaskList",
            render_ui_with_item_actions("data-list", bare()),
        ));
        let produced_filtered = collect_effect_produced_events(&trait_with_effect(
            "TaskList",
            render_ui_with_item_actions("data-list", filtered),
        ));
        assert!(produced_bare.contains("EDIT") && produced_bare.contains("DELETE"));
        assert_eq!(produced_filtered, produced_bare);
    }

    fn descriptor_list(events: &[&str]) -> SExpression {
        SExpression::Array(
            events
                .iter()
                .map(|event| {
                    let mut dm: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
                    dm.insert(
                        "event".to_string(),
                        SExpression::String((*event).to_string()),
                    );
                    SExpression::Object(dm)
                })
                .collect(),
        )
    }

    fn call(op: &str, args: Vec<SExpression>) -> SExpression {
        let mut items = vec![SExpression::String(op.to_string())];
        items.extend(args);
        SExpression::Array(items)
    }

    fn lambda(body: SExpression) -> SExpression {
        call("fn", vec![SExpression::String("item".to_string()), body])
    }

    fn produced_through(item_actions: SExpression) -> HashSet<String> {
        collect_effect_produced_events(&trait_with_effect(
            "TaskList",
            render_ui_with_item_actions("data-list", item_actions),
        ))
    }

    fn set_of(events: &[&str]) -> HashSet<String> {
        events.iter().map(|e| e.to_string()).collect()
    }

    /// Every operator whose registry entry declares `identity-of-arg<0>`
    /// (subsets and reorders) is transparent: the list's events survive.
    #[test]
    fn identity_declared_operators_credit_the_wrapped_list() {
        let n = || SExpression::Number(1.0);
        for wrapped in [
            call("array/slice", vec![descriptor_list(&["A", "B"]), n(), n()]),
            call(
                "array/sort",
                vec![
                    descriptor_list(&["A", "B"]),
                    SExpression::String("k".into()),
                ],
            ),
            call("array/take", vec![descriptor_list(&["A", "B"]), n()]),
            call("array/reverse", vec![descriptor_list(&["A", "B"])]),
            call("array/unique", vec![descriptor_list(&["A", "B"])]),
            call(
                "array/filter",
                vec![
                    descriptor_list(&["A", "B"]),
                    lambda(SExpression::Bool(true)),
                ],
            ),
        ] {
            assert_eq!(
                produced_through(wrapped.clone()),
                set_of(&["A", "B"]),
                "{wrapped:?}"
            );
        }
    }

    #[test]
    fn concat_credits_every_argument_list() {
        let concat = call(
            "array/concat",
            vec![descriptor_list(&["A"]), descriptor_list(&["B", "C"])],
        );
        assert_eq!(produced_through(concat), set_of(&["A", "B", "C"]));
    }

    #[test]
    fn append_and_prepend_credit_the_list_and_the_added_descriptor() {
        let extra = descriptor_list(&["EXTRA"]).clone();
        let SExpression::Array(mut one) = extra else {
            unreachable!()
        };
        let element = one.remove(0);
        for op in ["array/append", "array/prepend"] {
            let call_expr = call(op, vec![descriptor_list(&["A"]), element.clone()]);
            assert_eq!(produced_through(call_expr), set_of(&["A", "EXTRA"]), "{op}");
        }
        let insert = call(
            "array/insert",
            vec![descriptor_list(&["A"]), SExpression::Number(0.0), element],
        );
        assert_eq!(produced_through(insert), set_of(&["A", "EXTRA"]));
    }

    #[test]
    fn do_and_let_credit_only_their_result() {
        let effect = call(
            "set",
            vec![
                SExpression::String("@entity.x".into()),
                SExpression::Number(1.0),
            ],
        );
        let sequenced = call("do", vec![effect, descriptor_list(&["A", "B"])]);
        assert_eq!(produced_through(sequenced), set_of(&["A", "B"]));
        let bound = call(
            "let",
            vec![
                SExpression::Array(vec![SExpression::Array(vec![
                    SExpression::String("x".into()),
                    descriptor_list(&["IGNORED"]),
                ])]),
                descriptor_list(&["A"]),
            ],
        );
        assert_eq!(produced_through(bound), set_of(&["A"]));
    }

    #[test]
    fn nested_filter_inside_if_credits_every_branch() {
        let filtered = call(
            "array/filter",
            vec![
                descriptor_list(&["A", "B"]),
                lambda(SExpression::Bool(true)),
            ],
        );
        let cond = call(
            "if",
            vec![
                SExpression::String("@user.role".into()),
                filtered,
                call(
                    "array/sort",
                    vec![descriptor_list(&["C"]), SExpression::String("k".into())],
                ),
            ],
        );
        assert_eq!(produced_through(cond), set_of(&["A", "B", "C"]));
    }

    /// Paired control: an operator that creates new elements declares nothing,
    /// so the walk credits nothing through it — even around a real list.
    #[test]
    fn undeclared_operators_credit_nothing() {
        for creating in [
            call(
                "array/zip",
                vec![descriptor_list(&["A"]), descriptor_list(&["B"])],
            ),
            call("array/flatten", vec![descriptor_list(&["A"])]),
            call(
                "array/partition",
                vec![descriptor_list(&["A"]), lambda(SExpression::Bool(true))],
            ),
            call(
                "array/groupBy",
                vec![descriptor_list(&["A"]), SExpression::String("k".into())],
            ),
            call(
                "array/slice",
                vec![
                    call(
                        "array/zip",
                        vec![descriptor_list(&["A"]), descriptor_list(&["B"])],
                    ),
                    SExpression::Number(0.0),
                ],
            ),
        ] {
            assert!(
                produced_through(creating.clone()).is_empty(),
                "{creating:?}"
            );
        }
    }

    /// `array/map` is `lambda-result`: the events are the lambda body's
    /// descriptors, not the mapped collection's.
    #[test]
    fn map_credits_the_lambda_body_leaves() {
        let body = call(
            "if",
            vec![
                SExpression::String("@item.done".into()),
                descriptor_list(&["REOPEN"]),
                descriptor_list(&["COMPLETE"]),
            ],
        );
        let mapped = call(
            "array/map",
            vec![SExpression::String("@entity.items".into()), lambda(body)],
        );
        assert_eq!(produced_through(mapped), set_of(&["REOPEN", "COMPLETE"]));
        let mapped_to_descriptor = call(
            "array/map",
            vec![
                descriptor_list(&["NOT_THE_COLLECTION"]),
                lambda(descriptor_list(&["ROW"]).clone()),
            ],
        );
        assert_eq!(produced_through(mapped_to_descriptor), set_of(&["ROW"]));
    }

    /// A filter around an unresolved knob binding contributes nothing extra
    /// (the leaf is a `@` string, never credited) — same as the bare binding.
    #[test]
    fn filtered_unresolved_binding_credits_nothing() {
        let filtered = SExpression::Array(vec![
            SExpression::String("array/filter".to_string()),
            SExpression::String("@config.itemActions".to_string()),
            SExpression::Null,
        ]);
        let produced = collect_effect_produced_events(&trait_with_effect(
            "TaskList",
            render_ui_with_item_actions("data-list", filtered),
        ));
        assert!(produced.is_empty(), "{produced:?}");
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
            def.state_machine.transitions = vec![orbital_core_base::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core_base::schema::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "TICK".to_string(),
                guard: None,
                effects: Some(vec![orbital_core_base::schema::SExpression::Array(vec![
                    orbital_core_base::schema::SExpression::String("emit".to_string()),
                    orbital_core_base::schema::SExpression::String(event.to_string()),
                ])]),
            }];
        }
        td
    }

    fn externally_emitting_trait(name: &str, event: &str, imported: bool) -> TraitReference {
        TraitReference::Inline(TraitDefinition {
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
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![orbital_core_base::schema::EmitDefinition {
                definer_knob: None,
                scope_overridden: None,
                event: event.to_string(),
                event_id: None,
                description: None,
                synonyms: None,
                tier: None,
                scope: Some(EventScope::External),
                payload_schema: None,
                payload_entity: None,
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
                    orbital_core_base::schema::ConfigField {
                        field_type: "event".to_string(),
                        default: Some(orbital_core_base::schema::SExpression::String(event.to_string())),
                        label: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        values: None,
                        items: None,
                        properties: None,
                        required: None,
                        ref_id: None,
                        forwarded_from: None,
                        nested_forwarded_from: Vec::new(),
                        payload_for: None,
                        projected_from: None,
                    },
                )]
                .into_iter()
                .collect(),
            ),
            data_entities: None,
            scope: None,
            source_behavior: if imported {
                Some(orbital_core_base::schema::SourceBehaviorMetadata {
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
        def.state_machine.states = vec![orbital_core_base::schema::StateDefinition {
            name: "idle".to_string(),
            is_initial: Some(true),
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        }];
        def.state_machine.transitions = vec![orbital_core_base::schema::TransitionDefinition {
            event_id: None,
            from: orbital_core_base::schema::StringOrArray::Single("idle".to_string()),
            to: "idle".to_string(),
            event: "INIT".to_string(),
            guard: None,
            effects: Some(vec![orbital_core_base::schema::SExpression::Array(vec![
                orbital_core_base::schema::SExpression::String("render-ui".to_string()),
                orbital_core_base::schema::SExpression::String("main".to_string()),
                orbital_core_base::schema::SExpression::Object(
                    [
                        (
                            "type".to_string(),
                            orbital_core_base::schema::SExpression::String("button".to_string()),
                        ),
                        (
                            "action".to_string(),
                            orbital_core_base::schema::SExpression::String(event.to_string()),
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
        use orbital_core_base::schema::SExpression;
        // Unscoped (not `external`), so `exempt_external` never applies —
        // isolates the undecidable predicate from the separate scope-based
        // exemption tested elsewhere.
        let mut td = externally_emitting_trait(name, event, imported);
        if let TraitReference::Inline(def) = &mut td {
            def.emits = Some(vec![orbital_core_base::schema::EmitDefinition {
                definer_knob: None,
                scope_overridden: None,
                event: event.to_string(),
                event_id: None,
                description: None,
                synonyms: None,
                tier: None,
                scope: None,
                payload_schema: None,
                payload_entity: None,
            }]);
            def.config = None;
            def.state_machine.states = vec![orbital_core_base::schema::StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }];
            def.state_machine.transitions = vec![orbital_core_base::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core_base::schema::StringOrArray::Single("idle".to_string()),
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
        use orbital_core_base::schema::SExpression;
        let mut schema = create_minimal_schema();
        let mut td = externally_emitting_trait("GridRender", "SORT", true);
        if let TraitReference::Inline(def) = &mut td {
            def.config = None; // nothing in this composition fires SORT
            def.state_machine.transitions = vec![orbital_core_base::schema::TransitionDefinition {
                event_id: None,
                from: orbital_core_base::schema::StringOrArray::Single("idle".to_string()),
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
                effects.push(orbital_core_base::schema::SExpression::Array(vec![
                    orbital_core_base::schema::SExpression::String("render-ui".to_string()),
                    orbital_core_base::schema::SExpression::String("main".to_string()),
                    orbital_core_base::schema::SExpression::Object(
                        [
                            (
                                "type".to_string(),
                                orbital_core_base::schema::SExpression::String("button".to_string()),
                            ),
                            (
                                "action".to_string(),
                                orbital_core_base::schema::SExpression::String("GAME_END".to_string()),
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

/// Unit tests for the [`collect_async_result_events`]
/// collector family (siblings of [`collect_failure_events`]) — kept in their
/// own module rather than inside `mod tests` above so this file's two
/// concurrently-edited test surfaces don't collide.
#[cfg(test)]
mod async_result_collector_tests {
    use super::*;
    use orbital_core_base::schema::SExpression;

    fn effect_with_emit(op: &str, emit: &[(&str, &str)]) -> SExpression {
        let mut emit_map = orbital_core_base::schema::SObject::new();
        for (k, v) in emit {
            emit_map.insert((*k).to_string(), SExpression::String((*v).to_string()));
        }
        let mut options = orbital_core_base::schema::SObject::new();
        options.insert("emit".to_string(), SExpression::Object(emit_map));
        SExpression::Array(vec![
            SExpression::String(op.to_string()),
            SExpression::String("Item".to_string()),
            SExpression::Object(options),
        ])
    }

    #[test]
    fn async_result_events_credits_every_emit_key_for_fetch_and_persist() {
        let effects = vec![
            effect_with_emit("fetch", &[("success", "Loaded"), ("failure", "LoadFailed")]),
            effect_with_emit("persist", &[("success", "Saved")]),
        ];
        let got = collect_async_result_events(&effects);
        assert_eq!(
            got,
            HashSet::from([
                "Loaded".to_string(),
                "LoadFailed".to_string(),
                "Saved".to_string()
            ])
        );
    }

    /// `call-service` is deliberately OUT of scope (a real JS asymmetry vs.
    /// `collect_failure_events`, not a bug this port fixes); a `fetch`
    /// nested inside `if` is still credited.
    #[test]
    fn async_result_events_excludes_call_service_and_descends_through_if() {
        let call_service = effect_with_emit("call-service", &[("success", "ShouldNotCount")]);
        let wrapped_fetch = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::Bool(true),
            SExpression::Array(vec![effect_with_emit("fetch", &[("success", "Nested")])]),
            SExpression::Array(vec![]),
        ]);
        let got = collect_async_result_events(&[call_service, wrapped_fetch]);
        assert_eq!(got, HashSet::from(["Nested".to_string()]));
    }
}

/// An intent a trait emits that a SIBLING trait in the same orbital handles
/// must reach it through a declared route (owner ruling 2026-09-24: declared
/// routes only). std-pong's `PongPlay` emitted `PADDLE_MOVE`, `PongMechanic`
/// had the arm, nothing routed it — the keyboard moved nothing.
#[cfg(test)]
mod unrouted_handler_tests {
    use super::*;

    fn schema(mechanic_listens: serde_json::Value, play_emits_scope: &str) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "Pong",
            "orbitals": [{
                "name": "PongOrbital",
                "entity": { "name": "PongGame", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [
                    {
                        "name": "PongPlay",
                        "linkedEntity": "PongGame",
                        "emits": [{ "event": "PADDLE_MOVE", "scope": play_emits_scope }],
                        "stateMachine": {
                            "states": [{ "name": "playing", "isInitial": true }],
                            "transitions": [{ "from": "playing", "to": "playing", "event": "LEFT_UP",
                                "effects": [["when", true, ["emit", "PADDLE_MOVE", { "dy": -1 }]]] }]
                        }
                    },
                    {
                        "name": "PongMechanic",
                        "linkedEntity": "PongGame",
                        "listens": mechanic_listens,
                        "stateMachine": {
                            "states": [{ "name": "active", "isInitial": true }],
                            "transitions": [{ "from": "active", "to": "active", "event": "PADDLE_MOVE", "effects": [] }]
                        }
                    }
                ]
            }]
        }))
        .unwrap()
    }

    fn unrouted(schema: &OrbitalSchema) -> Vec<String> {
        validate_event_wiring(schema)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::EmitUnroutedToHandler)
            .map(|e| e.message.clone())
            .collect()
    }

    #[test]
    fn a_sibling_arm_with_no_route_is_an_error() {
        for scope in ["external", "internal"] {
            let found = unrouted(&schema(serde_json::Value::Null, scope));
            assert_eq!(found.len(), 1, "scope {scope}: {found:?}");
            assert!(
                found[0].contains("PongPlay")
                    && found[0].contains("PADDLE_MOVE")
                    && found[0].contains("PongMechanic")
            );
        }
    }

    /// A mechanic's TICK emits (std-pong-rally's paddle-hit `BURST`) travel the
    /// same declared routes as a transition's.
    #[test]
    fn a_tick_emit_to_a_sibling_arm_with_no_route_is_an_error() {
        let mut s = schema(serde_json::Value::Null, "external");
        if let TraitReference::Inline(play) = &mut s.orbitals[0].traits[0] {
            play.state_machine.transitions[0].effects = Some(vec![]);
            play.ticks = Some(vec![serde_json::from_value(serde_json::json!({
                "name": "step", "interval": 100, "effects": [["emit", "PADDLE_MOVE", {}]]
            }))
            .unwrap()]);
        }
        assert_eq!(unrouted(&s).len(), 1);
    }

    /// std-tactics-board-2d: `PlayerIntent` handles `MOVE` (the tile click) and
    /// forwards it with `(emit MOVE …)`; `Authority` only handles it. The
    /// forward is an intent for the authority and needs a route.
    #[test]
    fn a_forwarding_emitter_still_needs_a_route_to_the_handler() {
        let mut s = schema(serde_json::Value::Null, "external");
        if let TraitReference::Inline(play) = &mut s.orbitals[0].traits[0] {
            let mut own = play.state_machine.transitions[0].clone();
            own.event = "PADDLE_MOVE".to_string();
            play.state_machine.transitions.push(own);
        }
        assert_eq!(unrouted(&s).len(), 1);
    }

    #[test]
    fn control_a_declared_route_delivers_it() {
        let listens = serde_json::json!([{ "event": "PADDLE_MOVE", "source": { "kind": "trait", "trait": "PongPlay" }, "triggers": "PADDLE_MOVE" }]);
        assert!(unrouted(&schema(listens, "external")).is_empty());
    }

    /// A lab: two instruments, each a transport firing `STEP` and a sim with
    /// an arm for it. `sims` gives each sim's declared sources for `STEP`.
    fn lab(sims: &[(&str, &[&str])]) -> OrbitalSchema {
        let mut traits: Vec<serde_json::Value> = ["TransportA", "TransportB"]
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "linkedEntity": "Scene",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "RUN", "effects": [["emit", "STEP", {}]] }]
                    }
                })
            })
            .collect();
        for (name, sources) in sims {
            let listens: Vec<serde_json::Value> = sources
                .iter()
                .map(|src| serde_json::json!({ "event": "STEP", "source": { "kind": "trait", "trait": src }, "triggers": "STEP" }))
                .collect();
            traits.push(serde_json::json!({
                "name": name,
                "linkedEntity": "Scene",
                "listens": listens,
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "STEP", "effects": [] }]
                }
            }));
        }
        serde_json::from_value(serde_json::json!({
            "name": "Lab",
            "orbitals": [{
                "name": "LabOrbital",
                "entity": { "name": "Scene", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": traits
            }]
        }))
        .unwrap()
    }

    /// learning-astronomy-lab: each sim routes its OWN transport; the other
    /// instrument's transport is not its audience.
    #[test]
    fn control_a_handler_with_a_declared_audience_is_not_owed_other_emitters() {
        let s = lab(&[("SimA", &["TransportA"]), ("SimB", &["TransportB"])]);
        assert!(unrouted(&s).is_empty(), "{:?}", unrouted(&s));
    }

    /// Both sims chose TransportA — TransportB's STEP reaches no one.
    #[test]
    fn an_emitter_routed_to_no_handler_is_an_error() {
        let found = unrouted(&lab(&[
            ("SimA", &["TransportA"]),
            ("SimB", &["TransportA"]),
        ]));
        assert!(!found.is_empty());
        assert!(found.iter().all(|m| m.contains("TransportB")), "{found:?}");
    }

    /// Mechanic -> Fx routed, Sound declares nothing for the event: Sound is
    /// still owed its route even though the emitter reaches someone.
    #[test]
    fn a_handler_with_no_route_for_the_event_is_owed_every_emitter() {
        let found = unrouted(&lab(&[
            ("SimA", &["TransportA", "TransportB"]),
            ("SimB", &[]),
        ]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|m| m.contains("SimB")), "{found:?}");
    }

    /// `lab` plus a `SimC` whose `STEP` arm is fed by its own on-screen button.
    fn lab_with_self_fed(sims: &[(&str, &[&str])]) -> OrbitalSchema {
        let mut s = lab(sims);
        let sim_c: TraitDefinition = serde_json::from_value(serde_json::json!({
            "name": "SimC",
            "linkedEntity": "Scene",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT",
                      "effects": [["render-ui", "main", { "type": "button", "label": "Step", "action": "STEP" }]] },
                    { "from": "idle", "to": "idle", "event": "STEP", "effects": [] }
                ]
            }
        }))
        .unwrap();
        s.orbitals[0].traits.push(TraitReference::Inline(sim_c));
        s
    }

    /// learning-physics-lab: Projectile3DPhysics's PAUSE reaches its own sim;
    /// Orbit3DSim's PAUSE arm serves its own transport button.
    #[test]
    fn control_a_handler_fed_by_its_own_control_is_not_owed_a_reached_emitter() {
        let s = lab_with_self_fed(&[("SimA", &["TransportA", "TransportB"])]);
        assert!(unrouted(&s).is_empty(), "{:?}", unrouted(&s));
    }

    #[test]
    fn a_self_fed_handler_is_still_owed_an_emitter_that_reaches_no_one() {
        let found = unrouted(&lab_with_self_fed(&[]));
        assert!(found.iter().any(|m| m.contains("SimC")), "{found:?}");
    }

    /// learning-logic-lab: std-learn-classify's `LEARN_ROUND_COMPLETE` sits
    /// under `(if (not @config.loopRound) …)` and the call site sets
    /// `loopRound: true` — a branch that never fires owes no route (and a route
    /// to it would be ORB_X_LISTEN_SOURCE_NEVER_PRODUCES).
    #[test]
    fn control_an_emit_in_a_statically_dead_branch_owes_no_route() {
        let mut s = schema(serde_json::Value::Null, "external");
        if let TraitReference::Inline(play) = &mut s.orbitals[0].traits[0] {
            play.state_machine.transitions[0].effects =
                Some(vec![serde_json::from_value(serde_json::json!([
                    "if",
                    false,
                    ["emit", "PADDLE_MOVE", {}],
                    null
                ]))
                .unwrap()]);
        }
        assert!(unrouted(&s).is_empty(), "{:?}", unrouted(&s));
    }

    /// project-friday: `ProjectPersistor`'s `PROJECT_CREATE_REQUESTED` arm is
    /// fed by `ProjectCreate.SAVE -> PROJECT_CREATE_REQUESTED` — a route that
    /// TRIGGERS the arm is its audience, whatever the source event is called.
    #[test]
    fn control_an_arm_fed_by_a_renamed_route_has_an_audience() {
        let mut s = lab(&[("SimA", &["TransportA", "TransportB"])]);
        let sim_b: TraitDefinition = serde_json::from_value(serde_json::json!({
            "name": "SimB",
            "linkedEntity": "Scene",
            "listens": [{ "event": "SAVE", "source": { "kind": "trait", "trait": "Form" }, "triggers": "STEP" }],
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "STEP", "effects": [] }]
            }
        }))
        .unwrap();
        s.orbitals[0].traits.push(TraitReference::Inline(sim_b));
        assert!(unrouted(&s).is_empty(), "{:?}", unrouted(&s));
    }

    #[test]
    fn control_a_route_from_another_source_does_not_count() {
        let listens = serde_json::json!([{ "event": "PADDLE_MOVE", "source": { "kind": "trait", "trait": "Other" }, "triggers": "PADDLE_MOVE" }]);
        assert_eq!(unrouted(&schema(listens, "external")).len(), 1);
    }

    /// Two instances of one fetch atom each consume their OWN `Loaded`
    /// (same-trait continuation): neither is an intent for the other.
    #[test]
    fn control_self_consuming_instances_are_not_intents_for_each_other() {
        let traits: Vec<serde_json::Value> = ["A", "B"]
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "linkedEntity": "Row",
                    "stateMachine": {
                        "states": [{ "name": "loading", "isInitial": true }, { "name": "browsing" }],
                        "transitions": [
                            { "from": "loading", "to": "loading", "event": "INIT", "effects": [["emit", "Loaded", {}]] },
                            { "from": "loading", "to": "browsing", "event": "Loaded", "effects": [] }
                        ]
                    }
                })
            })
            .collect();
        let s: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "Lists",
            "orbitals": [{
                "name": "ListOrbital",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": traits
            }]
        }))
        .unwrap();
        assert!(unrouted(&s).is_empty());
    }

    #[test]
    fn control_the_emitters_own_arm_is_same_trait_continuation() {
        let mut s = schema(serde_json::Value::Null, "external");
        if let TraitReference::Inline(mechanic) = &mut s.orbitals[0].traits[1] {
            mechanic.state_machine.transitions.clear();
        }
        if let TraitReference::Inline(play) = &mut s.orbitals[0].traits[0] {
            let mut own = play.state_machine.transitions[0].clone();
            own.event = "PADDLE_MOVE".to_string();
            own.effects = Some(vec![]);
            play.state_machine.transitions.push(own);
        }
        assert!(unrouted(&s).is_empty());
    }
}
