use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EventScope, OrbitalSchema, TraitDefinition, TraitReference};
use std::collections::{HashMap, HashSet};

/// Validate event wiring between orbitals
pub(crate) fn validate_event_wiring(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Track events declared as `scope: external` anywhere in the schema.
    // These are explicitly meant to flow OUT to sibling orbitals — when
    // validating a single-orbital file, the consumer isn't visible, so the
    // unused-emission check would false-positive. The composed-schema
    // validation (where every consumer orbital is loaded) catches the
    // genuinely-unused case via `validate_listens_integrity` instead.
    let mut external_emit_events: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(td) = trait_ref {
                if let Some(emits) = &td.emits {
                    for emit in emits {
                        if emit.scope == Some(EventScope::External) {
                            external_emit_events.insert(emit.event.clone());
                        }
                    }
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
                    ValidationError::warning(
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
        // Events declared `scope: external` may have their consumer in a
        // sibling orbital not present in this schema. Don't flag them as
        // unused — composed-schema validation handles the genuine case.
        if external_emit_events.contains(event) {
            continue;
        }
        if !listened_events_all.contains_key(event) && !trigger_events.contains_key(event) {
            for emitter in emitters {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::CrossOrbitalUnusedEmission,
                        format!("traits.{}", emitter),
                        format!(
                            "Trait '{}' emits event '{}' but no trait listens for it",
                            emitter, event
                        ),
                    )
                    .with_suggestion("Add a listener for the event or remove the emission"),
                );
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
                            credit(v, out);
                        }
                        if let Some(field) = list_props.and_then(|m| m.get(k.as_str())) {
                            if let SExpression::Array(descriptors) = v {
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

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EntityDefinition, EntityRef, Orbital, StateMachine, TraitDefinition,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
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

        let result = validate_event_wiring(&schema);
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

        let result = validate_event_wiring(&schema);
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
        let result = validate_event_wiring(&schema);
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
        let result = validate_event_wiring(&schema);
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
            TraitReference::Inline(scoped_listener("SummaryStats", "SummaryData", "SummaryLoaded")),
        ];
        let result = validate_event_wiring(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "fetch emit-map event must satisfy the scoped listener; got {:?}",
            result.errors
        );
    }
}
