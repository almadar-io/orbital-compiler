//! Cross-Orbital Validation
//!
//! Validates cross-orbital concerns: entity uniqueness, event wiring, circular dependencies.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EmitDefinition, EventScope, FieldDefinition, FieldType, ListenSource, OrbitalSchema,
    TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Validate cross-orbital concerns
pub fn validate_cross_orbital(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    // Validate schema-level requirements
    if schema.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaMissingName,
                "name",
                "Schema name is required",
            )
            .with_suggestion("Add a name for your schema"),
        );
    }

    if schema.orbitals.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaNoOrbitals,
                "orbitals",
                "Schema must have at least one orbital",
            )
            .with_suggestion("Add at least one orbital to define application features"),
        );
    }

    // Check version if present
    if let Some(version) = &schema.version {
        if version.is_empty() {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::SchemaEmptyVersion,
                    "version",
                    "Schema version should not be empty",
                )
                .with_suggestion("Use semantic versioning (e.g., '1.0.0')"),
            );
        }
    }

    // Validate entity uniqueness
    result.merge(validate_entity_uniqueness(schema));

    // Validate event wiring
    result.merge(validate_event_wiring(schema));

    // Validate circular dependencies
    result.merge(validate_circular_dependencies(schema));

    // Validate external emit payloads
    result.merge(validate_external_emit_payloads(schema));

    // COMP-7: validate that cross-trait override wires only touch emits
    // declared `scope: 'external'`. Staged as a warning per Phase 2.A.
    result.merge(validate_scope_boundary(schema));

    // COMP-9: validate that every `type: 'relation'` field's target entity
    // resolves to an entity declared in the same schema. Staged as a warning
    // per Phase 2.A; the entity-scope validator still emits the blocking
    // `ORB_E_INVALID_RELATION` error for the same condition.
    result.merge(validate_relation_targets(schema));

    // Cross-orbital trait isolation (gap #13). Listens integrity (error):
    // cross-trait/cross-orbital listens must resolve to a real emit on the
    // named source trait. Backstop for the L1 (`orbital-lolo`) check;
    // covers `use`-imported behaviors that inline new listens after
    // lowering. NOTE: a transition-event collision warning was considered
    // and rejected — once Phase 4 codegen emits qualified bus keys per
    // trait (`UI:ContactCreate.EDIT` vs `UI:DealCreate.EDIT`), two
    // orbitals sharing the bare identifier `EDIT` is the expected
    // consequence of CRUD being a uniform vocabulary, not a smell.
    result.merge(validate_listens_integrity(schema));

    // Validate orbital names
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.name.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalMissingName,
                    format!("orbitals[{}].name", i),
                    "Orbital name is required",
                )
                .with_suggestion("Add a descriptive name for the orbital"),
            );
        }
    }

    ctx.merge(result);
}

/// Validate entity names are unique across all orbitals
fn validate_entity_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut entity_locations: HashMap<&str, Vec<usize>> = HashMap::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        // Only check inline entities, not references
        if let Some(entity) = orbital.entity.as_inline() {
            entity_locations.entry(&entity.name).or_default().push(i);
        }
    }

    for (entity_name, locations) in entity_locations {
        if locations.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalDuplicateEntity,
                    "orbitals",
                    format!(
                        "Entity '{}' is defined in multiple orbitals: {:?}",
                        entity_name,
                        locations
                            .iter()
                            .map(|i| &schema.orbitals[*i].name)
                            .collect::<Vec<_>>()
                    ),
                )
                .with_suggestion("Each entity name must be unique across all orbitals"),
            );
        }
    }

    result
}

/// Validate event wiring between orbitals
fn validate_event_wiring(schema: &OrbitalSchema) -> ValidationResult {
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
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                collect_trait_events(
                    trait_def,
                    &mut emitted_events,
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
        if !emitted_events.contains_key(event) && !trigger_events.contains_key(event) {
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
        if !listened_events_all.contains_key(event)
            && !trigger_events.contains_key(event)
        {
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
fn collect_trait_events(
    trait_def: &TraitDefinition,
    emitted: &mut HashMap<String, HashSet<String>>,
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
            let is_cross_orbital =
                matches!(listen.source, Some(ListenSource::Orbital { .. }));
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
}

/// Extract event name from an emit effect
/// True when at least one transition with the given event can propagate
/// — i.e., its effects include an `(emit ...)` op or a `fetch` /
/// `persist` / `call-service` / `ref` / `deref` carrying an
/// `emit: { success: ... }` (or `failure: ...`) entry. A trait that
/// only `set`s state, renders UI, or otherwise has no event-producing
/// effect is terminal: the cycle-detector should skip its inbound edges.
fn transition_can_propagate(trait_def: &TraitDefinition, event: &str) -> bool {
    use orbital_core::schema::SExpression;
    for transition in &trait_def.state_machine.transitions {
        if transition.event != event {
            continue;
        }
        let Some(effects) = &transition.effects else { continue };
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

fn extract_emit_event(effect: &orbital_core::schema::SExpression) -> Option<String> {
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

/// Validate that external emits have payload definitions
fn validate_external_emit_payloads(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::EventScope;

    let mut result = ValidationResult::new();

    let all_traits = collect_all_traits(schema);
    for trait_def in &all_traits {
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                // Check if scope is external
                let is_external = emit
                    .scope
                    .as_ref()
                    .map(|s| *s == EventScope::External)
                    .unwrap_or(false);

                if is_external {
                    // External emits MUST have payload with at least one field
                    let has_payload = emit
                        .payload_schema
                        .as_ref()
                        .map(|p| !p.is_empty())
                        .unwrap_or(false);

                    if !has_payload {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::TraitEventExternalMissingPayload,
                                format!("traits.{}.emits.{}", trait_def.name, emit.event),
                                format!(
                                    "External event '{}' in trait '{}' should have a payload definition with typed fields",
                                    emit.event, trait_def.name
                                ),
                            )
                            .with_suggestion(
                                "Add a payload array with fields: { \"name\": \"fieldName\", \"type\": \"string\", \"required\": true, \"description\": \"...\" }"
                            )
                            .with_expected_shape(r#"{
  "event": "EVENT_NAME",
  "scope": "external",
  "description": "What this event means",
  "payload": [
    { "name": "entityId", "type": "string", "required": true, "description": "ID of the entity" }
  ]
}"#),
                        );
                    }
                }
            }
        }
    }

    result
}

/// Validate circular event dependencies.
/// Checks both `emits` declarations AND emit effects in transitions.
fn validate_circular_dependencies(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build dependency graph: trait -> [traits it depends on (via listen -> emit)]
    let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
    let mut emitters: HashMap<String, HashSet<String>> = HashMap::new(); // event -> {emitting traits}

    // Collect emitters from both declarations and transition effects
    let all_traits = collect_all_traits(schema);
    for trait_def in &all_traits {
        // From emits declarations
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                emitters
                    .entry(emit.event.clone())
                    .or_default()
                    .insert(trait_def.name.clone());
            }
        }
        // From transition effects (["emit", "EVENT_NAME"])
        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    if let Some(event_name) = extract_emit_event(effect) {
                        emitters
                            .entry(event_name)
                            .or_default()
                            .insert(trait_def.name.clone());
                    }
                }
            }
        }
    }

    // Build dependency edges: trait A listens to event X → trait B emits X → A depends on B
    //
    // Skip listens whose `triggers` resolves to a lifecycle event (`INIT`,
    // `LOAD`). Those represent a "reset/refresh on signal" pattern, not a
    // data dependency: when ContactCreate emits CONTACT_CREATED and
    // ContactBrowse responds with INIT (re-fetch its list), the state
    // machine reset breaks the bus loop. Counting them as edges flagged
    // every legitimate browse/create cycle as a dependency cycle, even
    // though the runtime can't loop infinitely (INIT just re-fetches).
    //
    // Skip also when source and listening trait are in different
    // orbitals — orbital scope already isolates the bus key, so a
    // "depends on" edge across orbitals only matters when the listening
    // trait actually drives state on that emit (which the cycle check
    // can't tell from the static graph). Cross-orbital cycles still
    // surface via the `cross_orbital` ORB_X_LISTEN_SOURCE_UNRESOLVED
    // path when they're truly broken.
    let lifecycle_triggers: std::collections::HashSet<&str> =
        ["INIT", "LOAD", "RESET"].into_iter().collect();
    for trait_def in &all_traits {
        if let Some(listens) = &trait_def.listens {
            for listen in listens {
                let trigger = listen.triggers.as_deref().unwrap_or(&listen.event);
                if lifecycle_triggers.contains(trigger) {
                    continue;
                }
                // Skip terminal listens: if the trigger's transition
                // produces no emit effects (and no emit declarations
                // beyond the transition body), the edge can't propagate
                // an event back into the source emitter's listens. The
                // pagination pattern relies on this — Pagination listens
                // to Browse.BrowseItemLoaded with an ITEMS_LOADED
                // transition that only `set`s totalPages and renders the
                // pagination control; nothing it does can drive a fire-
                // loop, so flagging the structural cycle Browse↔Pagination
                // would be a false positive.
                if !transition_can_propagate(trait_def, trigger) {
                    continue;
                }
                if let Some(emitter_traits) = emitters.get(&listen.event) {
                    for emitter in emitter_traits {
                        if emitter != &trait_def.name {
                            dependencies
                                .entry(trait_def.name.clone())
                                .or_default()
                                .insert(emitter.clone());
                        }
                    }
                }
            }
        }
    }

    // Detect cycles using DFS
    let mut visited: HashSet<&str> = HashSet::new();
    let mut rec_stack: HashSet<&str> = HashSet::new();

    for trait_name in dependencies.keys() {
        if let Some(cycle) = detect_cycle(trait_name, &dependencies, &mut visited, &mut rec_stack) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalCircularDependency,
                    "traits",
                    format!("Circular event dependency detected: {}", cycle.join(" -> ")),
                )
                .with_suggestion(
                    "Break the cycle by removing one of the listener/emitter relationships",
                ),
            );
            break; // Only report first cycle
        }
    }

    result
}

/// Detect cycle in dependency graph using DFS
fn detect_cycle<'a>(
    node: &'a str,
    graph: &'a HashMap<String, HashSet<String>>,
    visited: &mut HashSet<&'a str>,
    rec_stack: &mut HashSet<&'a str>,
) -> Option<Vec<String>> {
    if rec_stack.contains(node) {
        // Found cycle
        return Some(vec![node.to_string()]);
    }

    if visited.contains(node) {
        return None;
    }

    visited.insert(node);
    rec_stack.insert(node);

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            if let Some(mut cycle) = detect_cycle(dep, graph, visited, rec_stack) {
                cycle.insert(0, node.to_string());
                return Some(cycle);
            }
        }
    }

    rec_stack.remove(node);
    None
}

/// Collect all trait definitions from schema
/// Note: Traits are now only defined inline within orbitals, not at schema level.
fn collect_all_traits(schema: &OrbitalSchema) -> Vec<&TraitDefinition> {
    let mut traits: Vec<&TraitDefinition> = Vec::new();

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                traits.push(trait_def);
            }
        }
    }

    traits
}

// =============================================================================
// COMP-7: validate_scope_boundary
// =============================================================================

/// Validate that composing traits only remap events declared `scope: 'external'`.
///
/// A `TraitReference::Reference` can carry `events: { SRC: DST }` (event-key
/// rename map) or `listens: [...]` (replacement listens list). Each entry in
/// those maps is a "wire" into the imported trait. If the wire touches an
/// event the imported atom declared `scope: 'internal'`, the composition is
/// reaching past the atom's encapsulation and exposing an internal event to
/// cross-orbital traffic.
///
/// This validator is intentionally best-effort:
///   * When the reference's target trait can be resolved **within the same
///     schema** (another orbital inlines a same-named trait, or the author
///     hand-inlined it), we consult its `emits[]` and warn on internal-scoped
///     overrides.
///   * When the reference targets a std behavior (`from: "std/behaviors/..."`)
///     we parse the embedded .orb registry to read the imported trait's
///     `emits[]`. This covers the most common composition path.
///   * When neither applies (an external .orb file that hasn't been loaded by
///     the time this validator runs, e.g. pre-inline CLI validation of a
///     workspace-local import), we skip the check. A future phase that wires
///     the loader into validation can graduate these skips into checks.
///
/// Severity: warning. The plan graduates this to an error after Phase 4 once
/// every atom declares its `scope` explicitly
/// (see `docs/Almadar_Agent_Behaviors_Plan.md`).
fn validate_scope_boundary(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build a fast trait lookup: trait name -> its emits[] (if any). This
    // covers the in-schema case: the `Alias.traits.X` target also appears
    // inline in one of the schema's orbitals (true after the inline phase
    // for anything referenced from another orbital in the same workspace).
    let mut in_schema_emits: HashMap<String, &Vec<EmitDefinition>> = HashMap::new();
    for trait_def in collect_all_traits(schema) {
        if let Some(emits) = &trait_def.emits {
            in_schema_emits.insert(trait_def.name.clone(), emits);
        }
    }

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Build alias -> source path map from `uses:` so we can try to
        // resolve std-behavior references. Only std paths
        // ("std/behaviors/...") are resolvable here without the full
        // loader.
        let mut alias_source: HashMap<&str, &str> = HashMap::new();
        if let Some(uses) = &orbital.uses {
            for decl in uses {
                alias_source.insert(decl.alias.as_str(), decl.from.as_str());
            }
        }

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Reference {
                reference,
                events,
                listens,
                ..
            } = trait_ref
            else {
                continue;
            };

            // Nothing to check if there are no overrides that can pierce
            // scope.
            if events.is_none() && listens.is_none() {
                continue;
            }

            // Parse "Alias.traits.TraitName" — only this shape exposes a
            // per-trait scope surface. Other shapes (bare name fallback)
            // are skipped.
            let Some(ref_str) = reference.as_deref() else {
                continue;
            };
            let parts: Vec<&str> = ref_str.split('.').collect();
            if parts.len() != 3 || parts[1] != "traits" {
                continue;
            }
            let alias = parts[0];
            let target_trait_name = parts[2];

            // Try to get the target trait's emits from one of two sources.
            // `owned_holder` keeps the owned Vec alive when we read it
            // from an embedded std behavior; the in-schema path uses a
            // borrow from `in_schema_emits`.
            let owned_holder: Option<Vec<EmitDefinition>>;
            let target_emits: &Vec<EmitDefinition> =
                if let Some(emits) = in_schema_emits.get(target_trait_name) {
                    *emits
                } else if let Some(src) = alias_source.get(alias) {
                    // Attempt to resolve std-behavior content. Other
                    // sources (local .orb paths) aren't loaded here —
                    // skip silently.
                    if let Some(content) = crate::embedded::get_std_behavior_content(src) {
                        owned_holder = parse_emits_from_std_content(content, target_trait_name);
                        match &owned_holder {
                            Some(e) => e,
                            // Target trait found but has no emits[] —
                            // nothing internal to guard against.
                            None => continue,
                        }
                    } else {
                        continue;
                    }
                } else {
                    // No `uses:` entry for the alias and not in-schema —
                    // we can't verify scope. Skip silently.
                    continue;
                };

            let path_base = format!("orbitals[{}].traits[{}]", oi, ti);

            // Check `events:` rename map entries. The SOURCE key is the
            // upstream atom's event name — that's what we look up in
            // emits[].
            if let Some(map) = events {
                for (source_event, target_event) in map {
                    if let Some(emit) = target_emits.iter().find(|e| &e.event == source_event) {
                        if emit.scope == Some(EventScope::Internal) {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::InternalEventExposed,
                                    format!("{}.events", path_base),
                                    format!(
                                        "Event '{}' on trait '{}' is declared scope: 'internal'; \
                                         composing traits can only remap events declared scope: 'external'. \
                                         Remove the rename '{}': '{}' or change the upstream atom's scope.",
                                        source_event, target_trait_name, source_event, target_event
                                    ),
                                )
                                .with_suggestion(
                                    "Promote the upstream event to scope: 'external' if it is truly \
                                     part of the atom's external surface, or drop the override.",
                                ),
                            );
                        }
                    }
                }
            }

            // Check `listens:` replacement entries. Each entry names an
            // event the composed trait consumes. If the upstream atom's
            // declaration of that same event key is internal-scoped, the
            // override is reaching into private traffic.
            if let Some(listen_overrides) = listens {
                for listen in listen_overrides {
                    if let Some(emit) = target_emits.iter().find(|e| e.event == listen.event) {
                        if emit.scope == Some(EventScope::Internal) {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::InternalEventExposed,
                                    format!("{}.listens", path_base),
                                    format!(
                                        "Listen override for event '{}' touches trait '{}', \
                                         which declares that event as scope: 'internal'. \
                                         Composing traits can only wire events declared scope: 'external'.",
                                        listen.event, target_trait_name
                                    ),
                                )
                                .with_suggestion(
                                    "Promote the upstream event to scope: 'external' or remove this \
                                     listen override.",
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    result
}

/// Parse an embedded std-behavior JSON blob and extract the `emits[]` for a
/// specific trait name. Returns `None` when parsing fails or the trait isn't
/// present. Failures are treated as "skip the check" rather than errors so a
/// malformed embedded payload doesn't derail unrelated validation; if that
/// ever happens the embedded-registry tests will flag it.
fn parse_emits_from_std_content(content: &str, trait_name: &str) -> Option<Vec<EmitDefinition>> {
    let schema: OrbitalSchema = serde_json::from_str(content).ok()?;
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                if trait_def.name == trait_name {
                    return trait_def.emits.clone();
                }
            }
        }
    }
    None
}

// =============================================================================
// COMP-9: validate_relation_targets
// =============================================================================

/// Validate that every `type: 'relation'` field's `relation.entity` resolves
/// to an entity declared in the same schema.
///
/// Runs alongside the existing entity-scope check (`EntityInvalidRelation`,
/// `ORB_E_INVALID_RELATION`) which reports the same condition at error
/// severity. The cross-orbital pass is intentionally staged as a warning so
/// tooling that consumes warnings-only streams still sees unresolved relations
/// while we gate the error-side behavior behind Phase 4 (when the plan calls
/// for graduating these to errors and dropping the duplicate).
fn validate_relation_targets(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect declared entity names (inline entities only — references are
    // resolved by the inline phase; if a reference somehow reaches this point
    // we don't know its final name and would produce a spurious warning, so
    // we only assert on inline declarations).
    let mut entity_names: HashSet<&str> = HashSet::new();
    for orbital in &schema.orbitals {
        if let Some(entity) = orbital.entity.as_inline() {
            entity_names.insert(entity.name.as_str());
        }
    }

    // Walk every inline entity's fields and recurse into array `items` and
    // object `properties` so nested relations are caught too.
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let Some(entity) = orbital.entity.as_inline() else {
            continue;
        };
        let base = format!("orbitals[{}].entity", oi);
        for (fi, field) in entity.fields.iter().enumerate() {
            let field_path = format!("{}.fields[{}]", base, fi);
            check_field_relation(&entity.name, field, &field_path, &entity_names, &mut result);
        }
    }

    result
}

/// Recursively walk a `FieldDefinition`, emitting an `UnresolvedRelationTarget`
/// warning for any `type: 'relation'` whose `relation.entity` is not in
/// `declared_entities`.
fn check_field_relation(
    owning_entity: &str,
    field: &FieldDefinition,
    path: &str,
    declared_entities: &HashSet<&str>,
    result: &mut ValidationResult,
) {
    if field.field_type == FieldType::Relation {
        if let Some(relation) = &field.relation {
            if !declared_entities.contains(relation.entity.as_str()) {
                let field_label = field.name.as_deref().unwrap_or("<anonymous>");
                let mut declared: Vec<&&str> = declared_entities.iter().collect();
                declared.sort();
                let declared_list = declared
                    .iter()
                    .map(|s| (**s).to_string())
                    .collect::<Vec<_>>();
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::UnresolvedRelationTarget,
                        format!("{}.relation.entity", path),
                        format!(
                            "Relation field '{}' on '{}' points at '{}', which is not declared in this schema",
                            field_label, owning_entity, relation.entity
                        ),
                    )
                    .with_suggestion(
                        "Declare the target entity in one of the schema's orbitals, or correct the relation.entity name.",
                    )
                    .with_valid_values(declared_list),
                );
            }
        }
    }

    // Recurse into array item type — e.g. `fields[].items.relation.entity`.
    if let Some(item) = &field.items {
        let item_path = format!("{}.items", path);
        check_field_relation(owning_entity, item, &item_path, declared_entities, result);
    }

    // Recurse into object property types — e.g. `fields[].properties.foo.relation.entity`.
    if let Some(props) = &field.properties {
        for (key, sub) in props {
            let sub_path = format!("{}.properties.{}", path, key);
            check_field_relation(owning_entity, sub, &sub_path, declared_entities, result);
        }
    }
}

/// Listens-integrity backstop (error).
///
/// Defense-in-depth duplicate of the L1 `LowerError::ListenSourceUnresolved`
/// check, run on the resolved IR so `use`-imported behaviors that inline
/// listens after lowering are also covered. Reads the typed
/// `ListenSource` enum produced by `split_listen_source`; never reparses
/// the original event string.
fn validate_listens_integrity(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let mut trait_emits: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut orbitals_in_schema: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        orbitals_in_schema.insert(orbital.name.clone());
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let entry = trait_emits
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(emits) = &td.emits {
                for emit in emits {
                    entry.insert(emit.event.clone());
                }
            }
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital, source_trait) = match &listen.source {
                    Some(ListenSource::Trait { trait_name }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                // Single-orbital validation context: when the listen's source
                // orbital isn't part of this schema, we can't resolve it. Skip
                // — the composed-schema validation pass catches truly unresolved
                // cross-orbital references once all sibling orbitals are loaded.
                if !orbitals_in_schema.contains(&source_orbital) {
                    continue;
                }
                let resolved = trait_emits
                    .get(&(source_orbital.clone(), source_trait.clone()))
                    .map(|events| events.contains(&listen.event))
                    .unwrap_or(false);
                if !resolved {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CrossOrbitalListenSourceUnresolved,
                            format!(
                                "orbitals.{}.traits.{}.listens",
                                orbital.name, td.name
                            ),
                            format!(
                                "Listen entry in '{}.{}' references source '{}.{}' event '{}' \
                                 which is not declared on the source trait.",
                                orbital.name,
                                td.name,
                                source_orbital,
                                source_trait,
                                listen.event
                            ),
                        )
                        .with_suggestion(
                            "Either declare the event in the source trait's `emits` block, or \
                             remove the listens entry. Cross-orbital channels require the source \
                             trait to declare the emit explicitly.",
                        ),
                    );
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EmitDefinition, EntityDefinition, EntityRef, EventScope, Orbital, PayloadField,
        StateMachine,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
                uses: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
            }],
            metadata: None,
        }
    }

    #[test]
    fn test_valid_schema() {
        let schema = create_minimal_schema();
        let result = validate_entity_uniqueness(&schema);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_duplicate_entity() {
        let mut schema = create_minimal_schema();
        schema.orbitals.push(Orbital {
            name: "AnotherOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Task".to_string(), // Duplicate
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        });

        let result = validate_entity_uniqueness(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalDuplicateEntity));
    }

    #[test]
    fn test_missing_schema_name() {
        let mut schema = create_minimal_schema();
        schema.name = String::new();

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaMissingName));
    }

    #[test]
    fn test_empty_orbitals() {
        let mut schema = create_minimal_schema();
        schema.orbitals = vec![];

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaNoOrbitals));
    }

    #[test]
    fn test_external_emit_missing_payload() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit but no payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None, // Missing payload!
            }]),
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
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventExternalMissingPayload));
    }

    #[test]
    fn test_external_emit_with_payload_passes() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit WITH payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: Some(vec![PayloadField {
                    name: "orderId".to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: Some("The order ID".to_string()),
                }]),
            }]),
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
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
    }

    #[test]
    fn test_internal_emit_no_payload_ok() {
        let mut schema = create_minimal_schema();

        // Internal emits don't require payloads - add as inline trait
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "STATUS_CHANGED".to_string(),
                scope: Some(EventScope::Internal),
                description: Some("Internal status tracking".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None,
            }]),
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
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
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
    // COMP-7: validate_scope_boundary
    // =========================================================================

    /// Helper: build an orbital that declares an atom trait with a given
    /// emit scope, and a second orbital that composes it via an
    /// `Alias.traits.Atom` reference with an `events:` rename override.
    fn build_scope_boundary_fixture(atom_scope: EventScope) -> OrbitalSchema {
        let atom = TraitDefinition {
            name: "Atom".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "OPEN".to_string(),
                scope: Some(atom_scope),
                description: None,
                synonyms: None,
                tier: None,
                payload_schema: None,
            }]),
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
        };
        let atom_orbital = Orbital {
            name: "AtomHost".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "AtomEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(atom)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let mut events_rename = HashMap::new();
        events_rename.insert("OPEN".to_string(), "ADD_ITEM".to_string());
        let composer_ref = TraitReference::Reference {
            reference: Some("Lib.traits.Atom".to_string()),
            name: Some("Composed".to_string()),
            linked_entity: Some("Cart".to_string()),
            config: None,
            events: Some(events_rename),
            fields: None,
            listens: None,
            emits_scope: None,
        };
        let composer_orbital = Orbital {
            name: "Composer".to_string(),
            uses: Some(vec![orbital_core::schema::UseDeclaration {
                from: "local/does-not-matter".to_string(),
                alias: "Lib".to_string(),
            }]),
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Cart".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![composer_ref],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        OrbitalSchema {
            name: "CompositionApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![atom_orbital, composer_orbital],
            metadata: None,
        }
    }

    /// Positive case: the atom declares `scope: 'external'`, so remapping it
    /// is allowed. Expect no ORB_X_INTERNAL_EVENT_EXPOSED warnings.
    #[test]
    fn test_scope_boundary_external_ok() {
        let schema = build_scope_boundary_fixture(EventScope::External);
        let result = validate_scope_boundary(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::InternalEventExposed),
            "External-scoped atom events must allow remapping. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the atom declares `scope: 'internal'`, so a composing
    /// `events:` rename touches a private event. Expect exactly one
    /// ORB_X_INTERNAL_EVENT_EXPOSED warning on the Composer orbital.
    #[test]
    fn test_scope_boundary_internal_warns() {
        let schema = build_scope_boundary_fixture(EventScope::Internal);
        let result = validate_scope_boundary(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::InternalEventExposed)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Exactly one ORB_X_INTERNAL_EVENT_EXPOSED expected. Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("orbitals[1].traits[0].events"));
    }

    /// Bare reference without `events:` / `listens:` overrides must not
    /// produce any COMP-7 warnings regardless of atom scope. This guards
    /// against a naive walker that fires on any `TraitReference::Reference`.
    #[test]
    fn test_scope_boundary_no_override_is_silent() {
        let mut schema = build_scope_boundary_fixture(EventScope::Internal);
        // Drop the events override from the composer trait reference.
        if let Some(TraitReference::Reference {
            events,
            listens,
            ..
        }) = schema.orbitals[1].traits.first_mut()
        {
            *events = None;
            *listens = None;
        }
        let result = validate_scope_boundary(&schema);
        assert!(
            result.errors.is_empty(),
            "References without overrides must be silent. Warnings: {:?}",
            result.errors
        );
    }

    // =========================================================================
    // COMP-9: validate_relation_targets
    // =========================================================================

    /// Positive case: a relation field whose `relation.entity` is declared in
    /// another orbital in the same schema must not produce a warning.
    #[test]
    fn test_relation_targets_in_schema_ok() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        // Add a second orbital that declares "Patient".
        schema.orbitals.push(Orbital {
            name: "PatientOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Patient".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        });
        // Point Task.patient at Patient — should resolve cleanly.
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::UnresolvedRelationTarget),
            "Resolvable relation must not warn. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the relation points at an entity that is not declared
    /// anywhere in the schema — expect ORB_X_UNRESOLVED_RELATION_TARGET.
    #[test]
    fn test_relation_targets_dangling_warns() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Dangling relation must produce exactly one ORB_X_UNRESOLVED_RELATION_TARGET. \
             Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("orbitals[0].entity.fields[0]"));
    }

    /// Relations nested inside an array's `items.relation.entity` must also
    /// be validated, proving the recursive walker works.
    #[test]
    fn test_relation_targets_nested_in_array_items() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patientList".to_string()),
                field_type: FieldType::Array,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: Some(Box::new(FieldDefinition {
                    name: None,
                    field_type: FieldType::Relation,
                    required: None,
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: Some(RelationDefinition {
                        entity: "Patient".to_string(),
                        cardinality: None,
                    }),
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                })),
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains(".items.relation.entity"));
    }
}
