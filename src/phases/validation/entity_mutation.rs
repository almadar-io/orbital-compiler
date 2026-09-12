//! Entity Mutation & Tick Guards
//!
//! Multi-trait entity-mutation rules (`SPLIT_ENTITY_MUTATION`,
//! `SharedEntityFieldConflict`, `SharedEntityRenderAuthority`) plus the
//! non-`Cron` tick backstop (`ORB_TICK_REQUIRES_RUNTIME`,
//! `ORB_TICK_ASYNC_EFFECT`). Runs POST-INLINE (after `uses` imports resolve)
//! so the fully composed trait set and each entity's `shared` /
//! `persistence` flags are visible.
//!
//! `EntityDefinition::shared` means several traits deliberately co-mutate ONE
//! entity instance; a non-shared entity gets an isolated copy per trait. The
//! rules below catch two mistakes: multiple traits mutating a non-shared
//! entity by accident (should have been `[shared]`), and the inverse —
//! `[shared]` traits fighting over one field or double-rendering.

use super::render_ui::is_render_ui_effect;
use super::{ErrorCode, Severity, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, ListenSource, Orbital, OrbitalSchema, PageRef, Persistence, SExpression,
    StringOrNumber, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

// ============================================================================
// Shared aggregation primitive (also consumed by binding.rs's group-aware
// setter check, Task C) — the single owner of "who mutates this field",
// built on top of `SExpression::collect_entity_field_refs`. No second
// field-ref walker.
// ============================================================================

/// Aggregate, across `traits`, which distinct trait names write each
/// `@entity.<field>` via an explicit `(set ...)` in a transition's guard or
/// effects, or in a tick effect. Keyed entity name → field name → set of
/// writer trait names.
pub(super) fn collect_entity_mutators<'a>(
    traits: impl IntoIterator<Item = &'a TraitDefinition>,
) -> HashMap<String, HashMap<String, HashSet<String>>> {
    let mut mutators: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();

    for trait_def in traits {
        let Some(linked) = &trait_def.linked_entity else {
            continue;
        };

        let mut referenced: HashMap<String, String> = HashMap::new();
        let mut set_fields: HashSet<String> = HashSet::new();
        for transition in &trait_def.state_machine.transitions {
            let event = transition.event.as_str();
            if let Some(guard) = &transition.guard {
                guard.collect_entity_field_refs(&mut referenced, &mut set_fields, event);
            }
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    effect.collect_entity_field_refs(&mut referenced, &mut set_fields, event);
                }
            }
        }
        if let Some(ticks) = &trait_def.ticks {
            for tick in ticks {
                for effect in &tick.effects {
                    effect.collect_entity_field_refs(&mut referenced, &mut set_fields, "TICK");
                }
            }
        }

        if set_fields.is_empty() {
            continue;
        }
        let field_map = mutators.entry(linked.clone()).or_default();
        for field in set_fields {
            field_map
                .entry(field)
                .or_default()
                .insert(trait_def.name.clone());
        }
    }

    mutators
}

/// Aggregate, across `traits`, which distinct trait names read ANY
/// `@entity.<field>` inside a `render-ui` effect. Keyed entity name → set of
/// reader trait names.
///
/// Render-child attribution: a trait that is `@trait.X`-referenced from
/// ANOTHER trait's render-ui tree in the same orbital is a render CHILD of
/// that referencer — its entity reads are attributed to the ROOT referencer
/// (transitively: host → child → grandchild collapse to the root). This is
/// structural, keyed purely on the `@trait.X` reference graph, so an inline
/// JSX wrapper forwarding `@entity.*` counts as part of its host authority's
/// render output, while a genuinely independent (unreferenced) second reader
/// still counts as its own authority.
/// entity name → slot name → trait roots that render-ui-read the entity
/// into that slot. Per-(entity, slot) rather than per-entity: two traits
/// bound to one `[shared]` entity may each own a DIFFERENT render slot
/// without contention — `SharedEntityRenderAuthority` only fires when two
/// distinct traits render the SAME entity into the SAME slot.
fn collect_render_ui_readers(
    traits: &[&TraitDefinition],
) -> HashMap<String, HashMap<String, HashSet<String>>> {
    // Direct readers: (trait name, linked entity, slot) triples whose
    // render-ui effects read @entity.*.
    let mut direct: Vec<(String, String, String)> = Vec::new();
    // Reverse render-reference graph: target trait name → trait names whose
    // render-ui trees reference it via `@trait.<target>`.
    let mut referenced_by: HashMap<String, HashSet<String>> = HashMap::new();

    for trait_def in traits {
        let mut read_slots: HashSet<String> = HashSet::new();
        let mut refs: HashSet<String> = HashSet::new();
        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    if !is_render_ui_effect(effect) {
                        continue;
                    }
                    if render_ui_effect_reads_entity(effect) {
                        if let Some(slot) = super::render_ui::extract_slot(effect) {
                            read_slots.insert(slot);
                        }
                    }
                    crate::phases::inline::collect_sibling_trait_refs(effect, &mut refs);
                }
            }
        }
        if let Some(ticks) = &trait_def.ticks {
            for tick in ticks {
                for effect in &tick.effects {
                    if !is_render_ui_effect(effect) {
                        continue;
                    }
                    if render_ui_effect_reads_entity(effect) {
                        if let Some(slot) = super::render_ui::extract_slot(effect) {
                            read_slots.insert(slot);
                        }
                    }
                    crate::phases::inline::collect_sibling_trait_refs(effect, &mut refs);
                }
            }
        }

        for target in refs {
            // A self-reference is not a parent edge.
            if target != trait_def.name {
                referenced_by
                    .entry(target)
                    .or_default()
                    .insert(trait_def.name.clone());
            }
        }

        if !read_slots.is_empty() {
            if let Some(linked) = &trait_def.linked_entity {
                for slot in read_slots {
                    direct.push((trait_def.name.clone(), linked.clone(), slot));
                }
            }
        }
    }

    let mut readers: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
    for (name, linked, slot) in direct {
        for root in resolve_render_roots(&name, &referenced_by) {
            readers
                .entry(linked.clone())
                .or_default()
                .entry(slot.clone())
                .or_default()
                .insert(root);
        }
    }

    readers
}

/// Walk the reverse render-reference graph upward from `name` to the root
/// referencer(s) a reader's entity reads are attributed to. A trait nobody
/// references is its own root. Cycles terminate via the visited set; a pure
/// cycle with no outside referencer attributes each member to itself.
fn resolve_render_roots(
    name: &str,
    referenced_by: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let mut roots: HashSet<String> = HashSet::new();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = vec![name];
    while let Some(n) = stack.pop() {
        if !visited.insert(n) {
            continue;
        }
        match referenced_by.get(n) {
            Some(parents) if !parents.is_empty() => {
                for p in parents {
                    stack.push(p.as_str());
                }
            }
            _ => {
                roots.insert(n.to_string());
            }
        }
    }
    if roots.is_empty() {
        roots.insert(name.to_string());
    }
    roots
}

/// True when `effect` is a `render-ui` call whose subtree contains at least
/// one `@entity.<field>` read. Scoped to a single effect node so this reuses
/// `collect_entity_field_refs` rather than adding a second render-ui prop
/// walker.
fn render_ui_effect_reads_entity(effect: &SExpression) -> bool {
    if !is_render_ui_effect(effect) {
        return false;
    }
    let mut referenced: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();
    effect.collect_entity_field_refs(&mut referenced, &mut set_fields, "RENDER");
    !referenced.is_empty()
}

/// Every `@entity.<field>` READ per trait, keyed trait name → field set,
/// across ALL of the trait's guards, transition effects, and tick effects —
/// render-ui bindings and effect arguments (call-service, persist, emit
/// payloads) alike. The consequence a split write carries is a READ that
/// misses it; this is the read side of that dependency.
fn collect_entity_field_reads(
    traits: &[&TraitDefinition],
    entity_name: &str,
) -> HashMap<String, HashSet<String>> {
    let mut reads: HashMap<String, HashSet<String>> = HashMap::new();
    for trait_def in traits {
        if trait_def.linked_entity.as_deref() != Some(entity_name) {
            continue;
        }
        let mut referenced: HashMap<String, String> = HashMap::new();
        let mut set_fields: HashSet<String> = HashSet::new();
        for transition in &trait_def.state_machine.transitions {
            if let Some(guard) = &transition.guard {
                guard.collect_entity_field_refs(&mut referenced, &mut set_fields, "GUARD");
            }
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    effect.collect_entity_field_refs(&mut referenced, &mut set_fields, "EFFECT");
                }
            }
        }
        if let Some(ticks) = &trait_def.ticks {
            for tick in ticks {
                for effect in &tick.effects {
                    effect.collect_entity_field_refs(&mut referenced, &mut set_fields, "TICK");
                }
            }
        }
        if !referenced.is_empty() {
            reads.insert(trait_def.name.clone(), referenced.into_keys().collect());
        }
    }
    reads
}

/// The consequence predicate for a split mutation: some trait READS a field
/// it never writes while a DIFFERENT trait writes it — on isolated per-trait
/// copies that read provably misses the write. Returns one witnessing
/// `(reader, field, writer)`; `None` means every reader is self-sufficient
/// (each populates its own copy, typically via bus-routed payloads — e.g.
/// `std-record-detail`'s drawer re-stashing the row the detail trait
/// broadcast) and the isolation carries no consequence.
fn cross_read_dependency(
    field_map: &HashMap<String, HashSet<String>>,
    reads_by_trait: &HashMap<String, HashSet<String>>,
) -> Option<(String, String, String)> {
    let mut readers: Vec<&String> = reads_by_trait.keys().collect();
    readers.sort();
    for reader in readers {
        let mut fields: Vec<&String> = reads_by_trait[reader].iter().collect();
        fields.sort();
        for field in fields {
            if let Some(writers) = field_map.get(field) {
                if writers.contains(reader) {
                    continue;
                }
                let mut others: Vec<&String> = writers.iter().collect();
                others.sort();
                if let Some(writer) = others.first() {
                    return Some(((*reader).clone(), (*field).clone(), (**writer).clone()));
                }
            }
        }
    }
    None
}

/// Transitive page membership: trait name → set of page names it is active
/// on. Seeded from each page's `traits[]` list, then closed over `@trait.X`
/// render-ui references (`collect_sibling_trait_refs` — the same graph
/// `collect_render_ui_readers` walks) so a child wrapped into a page-root
/// trait's render tree inherits that root's page(s) too, even though the
/// page's own `traits[]` never names the child directly.
fn collect_trait_pages(
    orbital: &Orbital,
    traits: &[&TraitDefinition],
) -> HashMap<String, HashSet<String>> {
    let mut refs_by_trait: HashMap<&str, HashSet<String>> = HashMap::new();
    for trait_def in traits {
        let mut refs: HashSet<String> = HashSet::new();
        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    crate::phases::inline::collect_sibling_trait_refs(effect, &mut refs);
                }
            }
        }
        if let Some(ticks) = &trait_def.ticks {
            for tick in ticks {
                for effect in &tick.effects {
                    crate::phases::inline::collect_sibling_trait_refs(effect, &mut refs);
                }
            }
        }
        refs.remove(&trait_def.name);
        refs_by_trait.insert(trait_def.name.as_str(), refs);
    }

    let mut trait_pages: HashMap<String, HashSet<String>> = HashMap::new();
    let Some(pages) = &orbital.pages else {
        return trait_pages;
    };
    for page_ref in pages {
        let PageRef::Inline(page) = page_ref else {
            continue;
        };
        let Some(page_traits) = &page.traits else {
            continue;
        };
        let mut closure: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = Vec::new();
        for trait_ref in page_traits {
            if let Some(name) = trait_ref.trait_name() {
                if closure.insert(name.to_string()) {
                    stack.push(name.to_string());
                }
            }
        }
        while let Some(name) = stack.pop() {
            if let Some(refs) = refs_by_trait.get(name.as_str()) {
                for r in refs {
                    if closure.insert(r.clone()) {
                        stack.push(r.clone());
                    }
                }
            }
        }
        for name in closure {
            trait_pages
                .entry(name)
                .or_default()
                .insert(page.name.clone());
        }
    }
    trait_pages
}

/// True when every mutator in `names` is a composed instance of the SAME
/// source trait (identical `(behavior, original_name)` on `source_behavior`)
/// — N independent instances of one `@rebindable` widget atom (e.g. several
/// `ui-input`s), each with its own isolated copy by design. A trait authored
/// directly on the orbital (`source_behavior == None`) never qualifies.
/// docs/Almadar_Compiler_Gaps.md:1213 (OPEN 2).
fn same_source_trait(names: &[String], trait_by_name: &HashMap<&str, &TraitDefinition>) -> bool {
    let mut source_key: Option<(&str, &str)> = None;
    for name in names {
        let Some(td) = trait_by_name.get(name.as_str()) else {
            return false;
        };
        let Some(sb) = &td.source_behavior else {
            return false;
        };
        let key = (sb.behavior.as_str(), sb.original_name.as_str());
        match source_key {
            None => source_key = Some(key),
            Some(existing) if existing == key => {}
            _ => return false,
        }
    }
    true
}

/// True when no two mutators in `names` can ever co-mount — every pair's page
/// membership is disjoint, and every mutator appears on at least one page.
/// Runtime entity stores are per-page, so mutators that never share a page
/// can never split-write the same instance. A mutator absent from every page
/// is NOT exempted (conservatively kept reported) — its runtime membership
/// isn't provable from static page wiring alone.
fn mutators_page_disjoint(
    names: &[String],
    trait_pages: &HashMap<String, HashSet<String>>,
) -> bool {
    let mut sets: Vec<&HashSet<String>> = Vec::with_capacity(names.len());
    for name in names {
        match trait_pages.get(name) {
            Some(pages) if !pages.is_empty() => sets.push(pages),
            _ => return false,
        }
    }
    for i in 0..sets.len() {
        for j in (i + 1)..sets.len() {
            if !sets[i].is_disjoint(sets[j]) {
                return false;
            }
        }
    }
    true
}

// ============================================================================
// B. Multi-trait mutation trio
// ============================================================================

/// Run the multi-trait mutation trio for every orbital in the schema.
pub fn validate_all_entity_mutations(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let orbital_path = format!("orbitals[{}]", i);
        let inline_traits: Vec<&TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def),
                TraitReference::Reference { .. } => None,
            })
            .collect();
        let trait_pages = collect_trait_pages(orbital, &inline_traits);

        let result = validate_orbital_entity_mutations(
            &inline_traits,
            &orbital_path,
            &ctx.entity_definitions,
            &trait_pages,
        );
        ctx.merge(result);
    }
}

fn validate_orbital_entity_mutations(
    traits: &[&TraitDefinition],
    orbital_path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    trait_pages: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let trait_by_name: HashMap<&str, &TraitDefinition> =
        traits.iter().map(|t| (t.name.as_str(), *t)).collect();
    let mutators_by_field = collect_entity_mutators(traits.iter().copied());
    let renderers = collect_render_ui_readers(traits);

    let mut entity_names: HashSet<&String> = HashSet::new();
    entity_names.extend(mutators_by_field.keys());
    entity_names.extend(renderers.keys());

    for entity_name in entity_names {
        let Some(entity_def) = entity_definitions.get(entity_name) else {
            continue;
        };
        let field_map = mutators_by_field.get(entity_name);
        let path = format!("{}.entity[{}]", orbital_path, entity_name);

        if entity_def.shared {
            // SharedEntityRenderAuthority — mirrors SlotContention's
            // aggregate-then-flag shape (slot.rs:112): exactly one render
            // authority is allowed per (entity, slot). Two traits bound to
            // one shared entity may each own a DIFFERENT slot without a
            // finding — only the SAME slot contended by ≥2 distinct traits
            // is a real double-render.
            if let Some(slots) = renderers.get(entity_name) {
                let mut slot_names: Vec<&String> = slots.keys().collect();
                slot_names.sort();
                for slot in slot_names {
                    let entity_renderers = &slots[slot];
                    if entity_renderers.len() > 1 {
                        let mut names: Vec<String> = entity_renderers.iter().cloned().collect();
                        names.sort();
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::SharedEntityRenderAuthority,
                                path.as_str(),
                                format!(
                                    "Shared entity '{}' is render-ui-read into slot '{}' by {} traits: {:?} — exactly one render authority is allowed per (entity, slot)",
                                    entity_name, slot, names.len(), names
                                ),
                            )
                            .with_suggestion(
                                "Keep a single trait's render-ui reading this shared entity into this slot; route the others through it, or give them a different slot",
                            ),
                        );
                    }
                }
            }

            // SharedEntityFieldConflict — same field written by >=2 traits.
            if let Some(field_map) = field_map {
                for (field, writers) in field_map {
                    if writers.len() > 1 {
                        let mut names: Vec<String> = writers.iter().cloned().collect();
                        names.sort();
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::SharedEntityFieldConflict,
                                path.as_str(),
                                format!(
                                    "Shared entity '{}' field '{}' is written by {} traits: {:?} — coupled mechanics writing one field must be one ordered pipeline, not parallel writers",
                                    entity_name, field, names.len(), names
                                ),
                            )
                            .with_suggestion(
                                "Route the write through a single ordered pipeline instead of writing the same field from multiple traits",
                            ),
                        );
                    }
                }
            }
        } else if let Some(field_map) = field_map {
            let mut all_mutators: Vec<String> = field_map
                .values()
                .flat_map(|writers| writers.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            if all_mutators.len() < 2 {
                continue;
            }
            if !matches!(entity_def.persistence, Some(Persistence::Runtime)) {
                continue;
            }
            all_mutators.sort();

            // N independent instances of the SAME source trait: isolation is
            // the design (e.g. several composed `ui-input`s), not an
            // accident. docs/Almadar_Compiler_Gaps.md:1213 (OPEN 2).
            if same_source_trait(&all_mutators, &trait_by_name) {
                continue;
            }
            // Mutators that can never co-mount can't split-write — runtime
            // entity stores are per-page.
            if mutators_page_disjoint(&all_mutators, trait_pages) {
                continue;
            }

            // Consequence = a READ that provably misses a write. A reader
            // that populates its own copy (every field it reads, it also
            // writes — usually from bus-routed payloads) is self-sufficient;
            // isolation without a cross-trait read dependency changes
            // nothing observable and is not constructed.
            let reads_by_trait = collect_entity_field_reads(traits, entity_name);
            let Some((reader, field, writer)) = cross_read_dependency(field_map, &reads_by_trait)
            else {
                continue;
            };
            let message = format!(
                "Entity '{}' (runtime, not [shared]) is mutated by {} distinct traits {:?} on ISOLATED per-trait copies, and trait '{}' reads '@entity.{}' which it never writes — only '{}' writes it, on a copy '{}' can't see; that read provably misses the write",
                entity_name, all_mutators.len(), all_mutators, reader, field, writer, reader
            );
            result.add_error(
                ValidationError::error(ErrorCode::EntitySplitMutation, path.as_str(), message)
                    .with_suggestion(format!(
                        "If these traits are meant to co-mutate one instance, add [shared] to entity '{}'",
                        entity_name
                    )),
            );
        }
    }

    result
}

// ============================================================================
// A. Non-`Cron` tick backstop
// ============================================================================

/// Non-`Cron` tick guards — the corpus-facing backstop mirroring the lolo L1
/// tick guards. `Cron` intervals are exempt (the sole server-schedulable
/// interval); every other interval (frame / duration) fires on a client
/// cadence, which is what these rules guard against.
pub fn validate_all_tick_guards(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_tick_guards(trait_def, &path, &ctx.entity_definitions);
        ctx.merge(result);
    }
}

/// Per-trait tick-guard check (both `ORB_TICK_REQUIRES_RUNTIME` and
/// `ORB_TICK_ASYNC_EFFECT`), factored out of `validate_all_tick_guards` so it
/// can run against a hand-built `TraitDefinition` fixture without needing a
/// full `OrbitalSchema`.
fn validate_trait_tick_guards(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(ticks) = &trait_def.ticks else {
        return result;
    };

    let has_non_cron_tick = ticks.iter().any(|t| !tick_is_cron(&t.interval));
    if has_non_cron_tick {
        if let Some(linked) = &trait_def.linked_entity {
            let is_persistent = entity_definitions
                .get(linked)
                .map(|e| matches!(e.persistence, None | Some(Persistence::Persistent)))
                .unwrap_or(false);
            if is_persistent {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TickRequiresRuntime,
                        format!("{}.ticks", path),
                        format!(
                            "trait {:?} runs a non-cron tick against entity {:?}, which is Persistent — a persistent entity mutated on every tick is usually meant to be [runtime]",
                            trait_def.name, linked
                        ),
                    )
                    .with_suggestion(format!(
                        "Mark entity {:?} [runtime], or switch the tick to a Cron interval if it's a server-scheduled job",
                        linked
                    )),
                );
            }
        }
    }

    for (i, tick) in ticks.iter().enumerate() {
        if tick_is_cron(&tick.interval) {
            continue;
        }
        for (j, effect) in tick.effects.iter().enumerate() {
            if let Some(op_name) = find_async_operator_call(effect) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TickAsyncEffect,
                        format!("{}.ticks[{}].effects[{}]", path, i, j),
                        format!(
                            "trait {:?} tick '{}' calls server-async operator '{}' on a non-cron interval — repeated per-tick agent/LLM calls can pile up",
                            trait_def.name, tick.name, op_name
                        ),
                    )
                    .with_suggestion(
                        "Trigger the async call from an explicit event-driven transition, or use a Cron-scheduled tick",
                    ),
                );
            }
        }
    }

    result
}

/// True when `interval` is a valid 5-field cron expression — the sole
/// server-schedulable tick interval, exempt from the guards above. Reuses
/// the canonical cron parser (`orbital_core::cron`) rather than
/// reimplementing cron syntax; mirrors the check
/// `phases::resolve::resolve_tick` runs when lowering to `OirInterval::Cron`.
fn tick_is_cron(interval: &StringOrNumber) -> bool {
    match interval {
        StringOrNumber::Number(_) => false,
        StringOrNumber::String(s) => orbital_core::cron::is_valid_cron_expression(s),
    }
}

/// Find the first server-async operator call anywhere in `expr`'s tree.
/// "Async" = the baked canonical registry's `std-llm` category (LLM calls:
/// `llm/generate`, `llm/call-tools`, `llm/embed`, ... — formerly `agent/*`
/// under `std-agent`). Consults `embedded::get_operator` (the baked
/// canonical-operators.json, the source of record) — the hand-maintained
/// static table in `canonical_operators.rs` predates the rename and never
/// carried these operators, which is how this rule went silently dead.
fn find_async_operator_call(expr: &SExpression) -> Option<&str> {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(op)) = arr.first() {
                if crate::embedded::get_operator(op).is_some_and(|m| m.category == "std-llm") {
                    return Some(op.as_str());
                }
            }
            arr.iter().find_map(|item| find_async_operator_call(item))
        }
        SExpression::Object(obj) => obj.values().find_map(find_async_operator_call),
        _ => None,
    }
}

// ============================================================================
// C. Local trait dispatch contract (Wave 2)
// ============================================================================

/// Effect operators that never fire client-only — a `local` trait carrying
/// one of these is mis-tagged, since `local` (`OirTrait::dispatch_mode`)
/// declares its transitions never await a server round trip.
const SERVER_BOUND_OPERATORS: &[&str] = &[
    "fetch",
    "persist",
    "call-service",
    "ref",
    "deref",
    "swap!",
    "watch",
    "atomic",
    "fetch-stream",
];

/// True when `expr`'s tree contains a call to any operator in
/// `SERVER_BOUND_OPERATORS`, at any depth (recurses through control-flow
/// wrappers like `if`/`do`/`let` the same way `find_async_operator_call`
/// does for the tick backstop).
fn contains_server_bound_effect(expr: &SExpression) -> bool {
    expr_calls_any_operator(expr, SERVER_BOUND_OPERATORS)
}

/// Effect operators that reach a server-owned resource — the subset of
/// `SERVER_BOUND_OPERATORS` Rule 2b gates a mutation-authorization finding
/// on (a guard that reads an `@intrinsic` field AND `@user.*` is only a
/// SECURITY concern when the transition it guards actually writes).
const MUTATING_SERVER_OPERATORS: &[&str] = &["persist", "call-service"];

/// True when `expr`'s tree contains a call to any operator in `ops`, at any
/// depth (recurses through control-flow wrappers the same way
/// `find_async_operator_call` does for the tick backstop). Single walker
/// shared by every "does this effect touch operator X" check in this module.
fn expr_calls_any_operator(expr: &SExpression, ops: &[&str]) -> bool {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(op)) = arr.first() {
                if ops.contains(&op.as_str()) {
                    return true;
                }
            }
            arr.iter().any(|item| expr_calls_any_operator(item, ops))
        }
        SExpression::Object(obj) => obj.values().any(|v| expr_calls_any_operator(v, ops)),
        _ => false,
    }
}

/// True when `expr`'s tree reads `@user` or `@user.<field>`, in either the
/// bare dotted-string form or the `(object/get @user "field")` call form —
/// the two spellings `entity_access.rs`'s `check_owner_field_identity_typed`
/// already reasons about for the same binding root.
fn references_user_binding(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) => s == "@user" || s.starts_with("@user."),
        SExpression::Array(arr) => {
            if arr.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(root)) = (&arr[0], &arr[1]) {
                    if op == "object/get" && root == "@user" {
                        return true;
                    }
                }
            }
            arr.iter().any(references_user_binding)
        }
        SExpression::Object(obj) => obj.values().any(references_user_binding),
        _ => false,
    }
}

/// The first `@intrinsic` field of `entity_def` that `expr`'s tree reads via
/// `@entity.<field>` (dotted) or `(object/get @entity "field")` — the two
/// spellings `entity_access.rs`'s owner-field extraction already handles for
/// the same binding root.
fn guard_references_intrinsic_field<'a>(
    expr: &SExpression,
    entity_def: &'a EntityDefinition,
) -> Option<&'a str> {
    let intrinsic_named = |field: &str| {
        entity_def
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(field) && f.intrinsic == Some(true))
            .and_then(|f| f.name.as_deref())
    };
    match expr {
        SExpression::String(s) => {
            let field = s.strip_prefix("@entity.")?;
            let field = field.split(['.', '[']).next().unwrap_or(field);
            intrinsic_named(field)
        }
        SExpression::Array(arr) => {
            if let (Some(SExpression::String(op)), Some(SExpression::String(root)), Some(SExpression::String(field))) =
                (arr.first(), arr.get(1), arr.get(2))
            {
                if op == "object/get" && root == "@entity" {
                    if let Some(hit) = intrinsic_named(field) {
                        return Some(hit);
                    }
                }
            }
            arr.iter()
                .find_map(|item| guard_references_intrinsic_field(item, entity_def))
        }
        SExpression::Object(obj) => obj
            .values()
            .find_map(|v| guard_references_intrinsic_field(v, entity_def)),
        _ => None,
    }
}

/// Run `ORB_GUARD_ON_FRAME_FIELD_AUTHORIZES` (guard half — the access-policy
/// half lives in `entity_access.rs`'s `check_entity_field`) for every orbital
/// in the schema.
pub fn validate_all_guard_frame_field_authorization(
    schema: &OrbitalSchema,
    ctx: &mut ValidationContext,
) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result =
            validate_trait_guard_frame_field_authorization(trait_def, &path, &ctx.entity_definitions);
        ctx.merge(result);
    }
}

/// Per-trait guard-authorization check, factored out so it can run against a
/// hand-built `TraitDefinition` fixture without a full `OrbitalSchema`.
fn validate_trait_guard_frame_field_authorization(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(linked) = &trait_def.linked_entity else {
        return result;
    };
    let Some(entity_def) = entity_definitions.get(linked) else {
        return result;
    };
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let Some(guard) = &transition.guard else {
            continue;
        };
        // A plain precondition guard (no identity comparison) is not
        // authorization — e.g. `ChatComposer.SEND`'s `activeChannel != ""`.
        if !references_user_binding(guard) {
            continue;
        }
        let Some(intrinsic_field) = guard_references_intrinsic_field(guard, entity_def) else {
            continue;
        };
        let gates_a_mutation = transition
            .effects
            .as_ref()
            .map(|effects| {
                effects
                    .iter()
                    .any(|e| expr_calls_any_operator(e, MUTATING_SERVER_OPERATORS))
            })
            .unwrap_or(false);
        if !gates_a_mutation {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::GuardOnFrameFieldAuthorizes,
                format!("{}.stateMachine.transitions[{}].guard", path, i),
                format!(
                    "trait {:?} transition '{}' guards a persist/call-service effect on @intrinsic field '{}' alongside @user — intrinsic fields are trait-owned view-state the runtime never seeds or persists, so this authorization check is trivially bypassable",
                    trait_def.name, transition.event, intrinsic_field
                ),
            )
            .with_suggestion(format!(
                "Base this guard on a real persisted field of entity {:?}, not the @intrinsic field {:?}",
                linked, intrinsic_field
            )),
        );
    }
    result
}

/// The pre-resolve schema-shape twin of `OirTrait::dispatch_mode`'s
/// client-only test (`RuntimeOptimistic | HybridClientOnly`). Validation runs
/// before `resolve`, so no `OirTrait` exists yet to call the real method on;
/// this replicates its exact three-way lookup (`[runtime]` entity →
/// optimistic, `local` trait → hybrid client-only, else server-gated) over
/// the schema types `resolve_trait`/`resolve_field` read it from — same pure
/// lookup, same inputs, one level earlier in the pipeline.
fn schema_dispatch_is_client_only(
    trait_def: &TraitDefinition,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> bool {
    let entity_is_runtime = trait_def
        .linked_entity
        .as_ref()
        .and_then(|e| entity_definitions.get(e))
        .map(|e| matches!(e.persistence, Some(Persistence::Runtime)))
        .unwrap_or(false);
    entity_is_runtime || trait_def.local
}

/// Run `ORB_AUTH_GUARD_CLIENT_ONLY` for every orbital in the schema.
pub fn validate_all_auth_guard_client_only(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result =
            validate_trait_auth_guard_client_only(trait_def, &path, &ctx.entity_definitions);
        ctx.merge(result);
    }
}

/// Per-trait `ORB_AUTH_GUARD_CLIENT_ONLY` check, factored out so it can run
/// against a hand-built `TraitDefinition` fixture without a full
/// `OrbitalSchema`.
fn validate_trait_auth_guard_client_only(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if !schema_dispatch_is_client_only(trait_def, entity_definitions) {
        return result;
    }
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let Some(guard) = &transition.guard else {
            continue;
        };
        if !references_user_binding(guard) {
            continue;
        }
        // A client-only transition with no server-touching effect has no server
        // state to protect: the guard is a UX precondition, not a finding.
        let has_server_effect = transition
            .effects
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(contains_server_bound_effect);
        if !has_server_effect {
            continue;
        }
        let message = format!(
            "trait {:?} transition '{}' guards on @user, but this trait's dispatch is client-only ({}) — the client state machine advances optimistically before the server evaluates this guard; the effects stay server-gated, the UI transition does not, since the client can construct any @user value it likes",
            trait_def.name,
            transition.event,
            if trait_def.local { "local" } else { "[runtime] entity" }
        );
        result.add_error(ValidationError {
            code: ErrorCode::AuthGuardClientOnly,
            message,
            path: format!("{}.stateMachine.transitions[{}].guard", path, i),
            severity: Severity::Warning,
            suggestion: Some(
                "If this guard is meant as an authorization boundary, move the mutation to a server-gated (non-local, non-[runtime]) trait; otherwise treat it as a UX precondition only.".to_string(),
            ),
            valid_values: None,
            expected_shape: None,
            fix_guidance: None,
        });
    }
    result
}

/// Run the `local` trait dispatch-contract pair
/// (`ORB_T_LOCAL_TRAIT_TOUCHES_SERVER` / `ORB_T_LOCAL_ON_NON_SHARED_PERSISTENT`)
/// for every orbital in the schema.
pub fn validate_all_local_trait_dispatch(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_local_dispatch(trait_def, &path, &ctx.entity_definitions);
        ctx.merge(result);
    }
}

/// Per-trait `local` dispatch-contract check, factored out so it can run
/// against a hand-built `TraitDefinition` fixture without a full
/// `OrbitalSchema` (mirrors `validate_trait_tick_guards`'s shape).
fn validate_trait_local_dispatch(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if !trait_def.local {
        return result;
    }

    // ORB_T_LOCAL_ON_NON_SHARED_PERSISTENT — `local` is only legal on an
    // entity multiple traits co-mutate client-side (`[shared]`) or one that
    // is never persisted at all (`[runtime]`).
    if let Some(linked) = &trait_def.linked_entity {
        if let Some(entity_def) = entity_definitions.get(linked) {
            let is_runtime = matches!(entity_def.persistence, Some(Persistence::Runtime));
            if !entity_def.shared && !is_runtime {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitLocalOnNonSharedPersistent,
                        path,
                        format!(
                            "trait {:?} is [local] but its linked entity {:?} is neither [shared] nor [runtime] — local dispatch would silently hide server-gated writes behind client-only optimism",
                            trait_def.name, linked
                        ),
                    )
                    .with_suggestion(format!(
                        "Add [shared] or a `runtime` persistence tag to entity {:?}, or drop [local] from trait {:?}",
                        linked, trait_def.name
                    )),
                );
            }
        }
    }

    // ORB_T_LOCAL_TRAIT_TOUCHES_SERVER — a local trait's transitions must
    // never touch the server or read viewer identity.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(guard) = &transition.guard {
            if references_user_binding(guard) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitLocalTouchesServer,
                        format!("{}.stateMachine.transitions[{}].guard", path, i),
                        format!(
                            "trait {:?} is [local] but transition '{}' guards on @user — a local trait's transitions never await a server round trip, so a viewer-identity read here is either unreachable or a false authorization boundary",
                            trait_def.name, transition.event
                        ),
                    )
                    .with_suggestion(
                        "Drop [local] from this trait, or remove the @user read from the guard",
                    ),
                );
            }
        }
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                if contains_server_bound_effect(effect) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitLocalTouchesServer,
                            format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j),
                            format!(
                                "trait {:?} is [local] but transition '{}' runs a server-bound effect — a local trait's transitions never await a server round trip",
                                trait_def.name, transition.event
                            ),
                        )
                        .with_suggestion(
                            "Drop [local] from this trait, or move the server-bound effect to a non-local trait",
                        ),
                    );
                }
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                if contains_server_bound_effect(effect) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitLocalTouchesServer,
                            format!("{}.ticks[{}].effects[{}]", path, i, j),
                            format!(
                                "trait {:?} is [local] but tick '{}' runs a server-bound effect — a local trait's transitions never await a server round trip",
                                trait_def.name, tick.name
                            ),
                        )
                        .with_suggestion(
                            "Drop [local] from this trait, or move the server-bound effect to a non-local trait",
                        ),
                    );
                }
            }
        }
    }

    result
}

// ============================================================================
// D. Set-column-without-row backstop (Wave 2 rule 9)
// ============================================================================

/// True when `expr`'s tree contains a `(persist create|update <entity_name> ...)`
/// call targeting `entity_name` specifically — any data-literal shape, not just
/// the whole-row form `expr_persists_whole_row` (binding.rs) looks for.
fn expr_persists_entity_create_or_update(expr: &SExpression, entity_name: &str) -> bool {
    match expr {
        SExpression::Array(items) => {
            let hit = matches!(items.first(), Some(SExpression::String(op)) if op == "persist")
                && matches!(items.get(1), Some(SExpression::String(action)) if action == "create" || action == "update")
                && matches!(items.get(2), Some(SExpression::String(name)) if name == entity_name);
            hit || items
                .iter()
                .any(|item| expr_persists_entity_create_or_update(item, entity_name))
        }
        SExpression::Object(obj) => obj
            .values()
            .any(|v| expr_persists_entity_create_or_update(v, entity_name)),
        _ => false,
    }
}

/// If `expr` contains `(persist create|update <entity_name> @payload.<var>...)`
/// — the whole-payload-forward persist shape — returns `<var>`, the top-level
/// payload field the persistor writes verbatim. `None` for a persist built
/// from an object literal or a different entity (those keep the strict
/// same-trait requirement).
fn persist_whole_payload_var(expr: &SExpression, entity_name: &str) -> Option<String> {
    match expr {
        SExpression::Array(items) => {
            let is_persist_call = matches!(items.first(), Some(SExpression::String(op)) if op == "persist")
                && matches!(items.get(1), Some(SExpression::String(action)) if action == "create" || action == "update")
                && matches!(items.get(2), Some(SExpression::String(name)) if name == entity_name);
            if is_persist_call {
                if let Some(SExpression::String(s)) = items.get(3) {
                    if let Some(rest) = s.strip_prefix("@payload.") {
                        let var = rest.split('.').next().unwrap_or(rest);
                        if !var.is_empty() {
                            return Some(var.to_string());
                        }
                    }
                }
            }
            items
                .iter()
                .find_map(|item| persist_whole_payload_var(item, entity_name))
        }
        SExpression::Object(obj) => obj
            .values()
            .find_map(|v| persist_whole_payload_var(v, entity_name)),
        _ => None,
    }
}

/// Cross-trait co-mutator lookup for the **restate-persistor** idiom: trait
/// `source_trait`'s event `event` fires, a `listens { source_trait.event ->
/// trigger }` restate route in a sibling persistor trait forwards the SAME
/// payload object into `trigger`, and that arm runs `(persist update
/// entity_name @payload.<var> ...)` against the whole payload. When found,
/// returns `<var>` — any `(set @entity.<field> @payload.<var>.<field>)` in
/// `source_trait`'s `event` transition is drawn from the exact object the
/// sibling persists, so the write lands even though it's in a different
/// trait than the persist call (`std-vendor-onboarding`'s `VendorPersistor`
/// is the canonical shape: `VendorOnboard.NEXT_STEP -> DO_UPDATE`, `DO_UPDATE`
/// runs `persist update Vendor ?data`). Distinct from the `[shared]`
/// co-mutator exemption above (which requires a shared entity instance) —
/// this one is payload-provenance, not instance-sharing, so it applies to
/// ordinary (non-shared) persistent entities too.
fn restate_persistor_payload_var(
    sibling_traits: &[&TraitDefinition],
    source_trait: &str,
    event: &str,
    entity_name: &str,
) -> Option<String> {
    for sibling in sibling_traits {
        if sibling.name == source_trait {
            continue;
        }
        let Some(listens) = &sibling.listens else {
            continue;
        };
        for listen in listens {
            let source_matches = match &listen.source {
                Some(ListenSource::Trait { trait_name, .. }) => trait_name == source_trait,
                Some(ListenSource::Orbital { trait_name, .. }) => trait_name == source_trait,
                _ => false,
            };
            if !source_matches || listen.event != event {
                continue;
            }
            let trigger_event = listen.triggers.as_deref().unwrap_or(&listen.event);
            for transition in &sibling.state_machine.transitions {
                if transition.event != trigger_event {
                    continue;
                }
                let Some(effects) = &transition.effects else {
                    continue;
                };
                if let Some(var) = effects
                    .iter()
                    .find_map(|e| persist_whole_payload_var(e, entity_name))
                {
                    return Some(var);
                }
            }
        }
    }
    None
}

/// True when `effects` sets `@entity.<field>` from `@payload.<var>` or
/// `@payload.<var>.<anything>` — the exact shape `restate_persistor_payload_var`
/// proves lands in a sibling's persist call.
fn set_field_from_payload_var(effects: &[SExpression], field: &str, var: &str) -> bool {
    fn walk(expr: &SExpression, target: &str, prefix: &str) -> bool {
        match expr {
            SExpression::Array(items) => {
                if items.len() >= 3 {
                    if let (SExpression::String(op), SExpression::String(dest)) =
                        (&items[0], &items[1])
                    {
                        if op == "set" && dest == target {
                            return matches!(&items[2], SExpression::String(val) if val == prefix || val.starts_with(&format!("{prefix}.")));
                        }
                    }
                }
                items.iter().any(|i| walk(i, target, prefix))
            }
            SExpression::Object(obj) => obj.values().any(|v| walk(v, target, prefix)),
            _ => false,
        }
    }
    let target = format!("@entity.{field}");
    let prefix = format!("@payload.{var}");
    effects.iter().any(|e| walk(e, &target, &prefix))
}

/// True when `effects` contains a `fetch`/`ref`/`fetch-stream` effect whose
/// `filter` config key reads `@entity.<field>` — the "scope this read to my
/// own row" idiom (`filter: ["=", (object/get @entity studentId), @user.id]`,
/// 20 files in the corpus). A field set purely to build a read's own scoping
/// filter is consumed as a query parameter in the SAME transition it's set
/// in — a real, intentional use, just never one that reaches a persisted
/// row, and the field is routinely genuine access-control data (still read
/// in `@read`/`@create`/… directives elsewhere) rather than trait-local
/// scratch, so `@intrinsic` is the wrong label for it (verified: tripped
/// `ORB_GUARD_ON_FRAME_FIELD_AUTHORIZES` when tried on `std-lms`'s
/// `Progress.studentId`).
fn field_scopes_a_read_filter(effects: &[SExpression], field: &str) -> bool {
    fn expr_reads_field(expr: &SExpression, target_dotted: &str, field: &str) -> bool {
        match expr {
            SExpression::String(s) => s == target_dotted,
            SExpression::Array(items) => {
                if items.len() == 3 {
                    if let (
                        SExpression::String(op),
                        SExpression::String(obj),
                        SExpression::String(key),
                    ) = (&items[0], &items[1], &items[2])
                    {
                        if op == "object/get" && obj == "@entity" && key == field {
                            return true;
                        }
                    }
                }
                items
                    .iter()
                    .any(|i| expr_reads_field(i, target_dotted, field))
            }
            SExpression::Object(obj) => obj
                .values()
                .any(|v| expr_reads_field(v, target_dotted, field)),
            _ => false,
        }
    }
    fn find_filtered_read(expr: &SExpression, target_dotted: &str, field: &str) -> bool {
        match expr {
            SExpression::Array(items) => {
                let is_read_call = matches!(
                    items.first(),
                    Some(SExpression::String(op)) if op == "fetch" || op == "ref" || op == "fetch-stream"
                );
                if is_read_call {
                    if let Some(SExpression::Object(cfg)) = items.get(2) {
                        if let Some(filter_expr) = cfg.get("filter") {
                            if expr_reads_field(filter_expr, target_dotted, field) {
                                return true;
                            }
                        }
                    }
                }
                items
                    .iter()
                    .any(|i| find_filtered_read(i, target_dotted, field))
            }
            SExpression::Object(obj) => obj
                .values()
                .any(|v| find_filtered_read(v, target_dotted, field)),
            _ => false,
        }
    }
    let target_dotted = format!("@entity.{field}");
    effects
        .iter()
        .any(|e| find_filtered_read(e, &target_dotted, field))
}

/// True when `trait_def` runs `persist create`/`persist update` against
/// `entity_name` anywhere (any transition's effects, or a tick) — the
/// "the write lands eventually" exemption: a draft form that sets columns
/// across several transitions before a final `persist create` is legitimate.
fn trait_lands_a_write(trait_def: &TraitDefinition, entity_name: &str) -> bool {
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            if effects
                .iter()
                .any(|e| expr_persists_entity_create_or_update(e, entity_name))
            {
                return true;
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            if tick
                .effects
                .iter()
                .any(|e| expr_persists_entity_create_or_update(e, entity_name))
            {
                return true;
            }
        }
    }
    false
}

/// Run `ORB_SET_COLUMN_WITHOUT_ROW` for every orbital in the schema.
///
/// Per-orbital (not the flat `inline_traits` iterator) because a `[shared]`
/// entity's "does the write land" question is answered at the CO-MUTATOR
/// GROUP's scope, not one trait's — several traits share one instance, and
/// the trait that actually calls `persist create`/`update` is routinely a
/// dedicated persistor sibling (`ChatMessagePersistor` in std-realtime-chat:
/// `ChatMessageAttachment` sets `@entity.attachment` on the shared frame,
/// `ChatComposer.SAVE` forwards it, `ChatMessagePersistor` is the one that
/// creates the row). A non-shared entity gets an ISOLATED per-trait copy, so
/// a sibling's persist call can never see this trait's write — there the
/// same-trait requirement stays exact.
pub fn validate_all_set_column_without_row(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (orbital_index, orbital) in schema.orbitals.iter().enumerate() {
        let inline_traits: Vec<&TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def),
                TraitReference::Reference { .. } => None,
            })
            .collect();
        let mut orbital_persisted_entities: HashSet<&str> = HashSet::new();
        for trait_def in &inline_traits {
            if let Some(linked) = &trait_def.linked_entity {
                if trait_lands_a_write(trait_def, linked) {
                    orbital_persisted_entities.insert(linked.as_str());
                }
            }
        }
        for (i, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let path = super::trait_path(orbital_index, &orbital.name, i, trait_ref);
            let result = validate_trait_set_column_without_row(
                trait_def,
                &path,
                &ctx.entity_definitions,
                &orbital_persisted_entities,
                &inline_traits,
            );
            ctx.merge(result);
        }
    }
}

/// The entity name `event`'s payload was stamped with as a WHOLE-ROW mirror —
/// `PayloadField.entity`, set by lowering for `data : Note` / `data : [Note]`
/// (the array form stamps the element entity), which flattens the field's
/// `field_type` to `"object"`/`"[object]"` rather than leaving the entity
/// name in it. `per_event_payload`'s flat `field_type` string alone can't see
/// this — it's the reason `event_has_row_payload` below needs both checks:
/// the unresolved `@entity` sentinel / concrete-name string AND this stamp.
fn event_payload_row_entity<'a>(trait_def: &'a TraitDefinition, event: &str) -> Option<&'a str> {
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            if emit.event == event {
                if let Some(schema) = &emit.payload_schema {
                    if let Some(e) = schema.iter().find_map(|f| f.entity.as_deref()) {
                        return Some(e);
                    }
                }
            }
        }
    }
    if let Some(events) = &trait_def.state_machine.events {
        for ev in events {
            if ev.key == event {
                if let Some(schema) = &ev.payload_schema {
                    if let Some(e) = schema.iter().find_map(|f| f.entity.as_deref()) {
                        return Some(e);
                    }
                }
            }
        }
    }
    None
}

/// Per-trait `ORB_SET_COLUMN_WITHOUT_ROW` check, factored out so it can run
/// against a hand-built `TraitDefinition` fixture without a full
/// `OrbitalSchema`. `orbital_persisted_entities` names every entity that
/// gets a `persist create`/`update` from ANY inline trait in the same
/// orbital — the co-mutator-group exemption for `[shared]` entities.
/// `sibling_traits` is every inline trait in the same orbital (this trait
/// included), consulted for the restate-persistor exemption
/// (`restate_persistor_payload_var`) — a non-shared entity's own
/// co-mutator-group idiom, keyed by payload provenance instead of instance
/// sharing.
fn validate_trait_set_column_without_row(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    orbital_persisted_entities: &HashSet<&str>,
    sibling_traits: &[&TraitDefinition],
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(linked) = &trait_def.linked_entity else {
        return result;
    };
    let Some(entity_def) = entity_definitions.get(linked) else {
        return result;
    };
    // Only a `[persistent]` entity has a "row" to land in at all.
    if !matches!(entity_def.persistence, None | Some(Persistence::Persistent)) {
        return result;
    }
    // The write lands eventually — a draft form spanning several transitions
    // before a final `persist create`/`update` is legitimate. For a
    // `[shared]` entity, "eventually" includes a sibling trait's persist
    // call against the same shared instance.
    let lands_a_write = trait_lands_a_write(trait_def, linked)
        || (entity_def.shared && orbital_persisted_entities.contains(linked.as_str()));
    if lands_a_write {
        return result;
    }

    // Trait-wide "entityId in scope" — the same signal
    // `validate_entity_fields_have_setters` (binding.rs) uses to decide a row
    // is bound: `(set @entity.id ...)` anywhere in the trait.
    let mut referenced_all: HashMap<String, String> = HashMap::new();
    let mut set_all: HashSet<String> = HashSet::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = &transition.guard {
            guard.collect_entity_field_refs(&mut referenced_all, &mut set_all, &transition.event);
        }
        if let Some(effects) = &transition.effects {
            for effect in effects {
                effect.collect_entity_field_refs(&mut referenced_all, &mut set_all, &transition.event);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                effect.collect_entity_field_refs(&mut referenced_all, &mut set_all, "TICK");
            }
        }
    }
    if set_all.contains("id") {
        // `@entity.id` is bound somewhere in this trait — a row is in scope.
        return result;
    }

    let per_event_payload = super::effect::collect_per_event_payload_field_types(trait_def);

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        // A payload field declared `@entity` carries the WHOLE fetched row
        // (the fetch-success-callback shape: `XxxLoaded { data : @entity! }`)
        // — that transition unambiguously knows which row it's mirroring
        // fields FROM, even though the field name isn't literally "id"/
        // "row". Without this, every read-only "fetch by id, then (set
        // @entity.field ?data.field) to feed render-ui" detail-view trait —
        // the most common shape in the corpus — false-positives as a
        // scratch write that never lands.
        // A directly-authored organism (as opposed to a generic library atom)
        // already knows its concrete entity name at author time, so its
        // fetch-success payload reads e.g. `data : Graph` / `data : [Graph]`
        // instead of the unresolved `@entity` / `[@entity]` sentinel — the
        // exact same "whole row" shape, just already resolved. Recognizing
        // only the sentinel form false-positived every directly-authored
        // fetch-then-mirror trait in the corpus (std-kflow's `GraphView`,
        // `LessonView`, `StoryView`, … all declare `data : <EntityName>`).
        let whole_row_array = format!("[{linked}]");
        let event_has_row_payload = per_event_payload
            .get(&transition.event)
            .map(|fields| {
                fields.contains_key("id")
                    || fields.contains_key("row")
                    || fields
                        .values()
                        .any(|ty| ty == "@entity" || ty == linked || ty == &whole_row_array)
            })
            .unwrap_or(false)
            || event_payload_row_entity(trait_def, &transition.event) == Some(linked.as_str());
        if event_has_row_payload {
            continue;
        }
        let Some(effects) = &transition.effects else {
            continue;
        };
        let mut local_referenced: HashMap<String, String> = HashMap::new();
        let mut local_set: HashSet<String> = HashSet::new();
        for effect in effects {
            effect.collect_entity_field_refs(&mut local_referenced, &mut local_set, &transition.event);
        }
        let mut fields: Vec<&String> = local_set.iter().filter(|f| f.as_str() != "id").collect();
        fields.sort();
        let restate_var = restate_persistor_payload_var(
            sibling_traits,
            &trait_def.name,
            &transition.event,
            linked,
        );
        for field in fields {
            let is_intrinsic = entity_def
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(field.as_str()) && f.intrinsic == Some(true));
            if is_intrinsic {
                continue;
            }
            if let Some(var) = &restate_var {
                if set_field_from_payload_var(effects, field, var) {
                    continue;
                }
            }
            if field_scopes_a_read_filter(effects, field) {
                continue;
            }
            result.add_error(
                ValidationError::error(
                    ErrorCode::SetColumnWithoutRow,
                    format!("{}.stateMachine.transitions[{}]", path, i),
                    format!(
                        "trait {:?} sets '@entity.{}' on event {:?} with no row in scope (no id/row-bearing event payload, no @entity.id bound anywhere in the trait), and never runs persist create/update on {:?} anywhere — this write can never land",
                        trait_def.name, field, transition.event, linked
                    ),
                )
                .with_suggestion(format!(
                    "Run (persist create {} ...) or (persist update {} ...) somewhere in this trait, bind a row (set @entity.id ...) before this transition, or declare '{}' @intrinsic if it is trait-local scratch state",
                    linked, linked, field
                )),
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EmitDefinition, EntityRef, FieldDefinition, FieldType, ListenDefinition, PageDefinition,
        PayloadField, StateDefinition, StateMachine, StringOrArray, TickDefinition,
        TransitionDefinition,
    };

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }
    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }

    fn entity(name: &str, shared: bool, persistence: Option<Persistence>) -> EntityDefinition {
        EntityDefinition {
            shared,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: name.to_string(),
            collection: None,
            persistence,
            instances: None,
            fields: vec![FieldDefinition {
                merged_from: None,
                name: Some("value".to_string()),
                field_type: FieldType::Number,
                required: None,
                primary_key: None,
                unique: None,
                relation: None,
                items: None,
                values: None,
                default: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            }],
            id: None,
        }
    }

    fn set_effect(field: &str) -> SExpression {
        arr(vec![
            s("set"),
            s(&format!("@entity.{}", field)),
            SExpression::Number(1.into()),
        ])
    }

    fn render_ui_reading(field: &str) -> SExpression {
        arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s(&format!("@entity.{}", field)));
                m
            }),
        ])
    }

    fn trait_with_init_effects(
        name: &str,
        linked_entity: &str,
        effects: Vec<SExpression>,
    ) -> TraitDefinition {
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
            linked_entity: Some(linked_entity.to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: None,
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(effects),
                }],
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

    /// Like `trait_with_init_effects` but `local: true` and with a caller-supplied
    /// guard on the INIT transition — the fixture Rule 8's tests build on.
    fn local_trait_with_guard(
        name: &str,
        linked_entity: &str,
        guard: Option<SExpression>,
        effects: Vec<SExpression>,
    ) -> TraitDefinition {
        let mut t = trait_with_init_effects(name, linked_entity, effects);
        t.local = true;
        t.state_machine.transitions[0].guard = guard;
        t
    }

    /// Like `local_trait_with_guard` but `local: false` — the fixture Rule 2b
    /// and Rule 4's `[runtime]`-entity cases build on.
    fn trait_with_guard(
        name: &str,
        linked_entity: &str,
        guard: Option<SExpression>,
        effects: Vec<SExpression>,
    ) -> TraitDefinition {
        let mut t = trait_with_init_effects(name, linked_entity, effects);
        t.state_machine.transitions[0].guard = guard;
        t
    }

    fn trait_with_tick(
        name: &str,
        linked_entity: &str,
        interval: &str,
        effects: Vec<SExpression>,
    ) -> TraitDefinition {
        let mut t = trait_with_init_effects(name, linked_entity, vec![]);
        t.state_machine.transitions.clear();
        t.ticks = Some(vec![TickDefinition {
            name: "Tick".to_string(),
            interval: StringOrNumber::String(interval.to_string()),
            guard: None,
            effects,
            applies_to: None,
            description: None,
        }]);
        t
    }

    // ---- B: SPLIT_ENTITY_MUTATION -----------------------------------------

    #[test]
    fn split_mutation_warning_with_render_ui_reader() {
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects(
            "Renderer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("x")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Expected EntitySplitMutation error: errors={:?} warnings={:?}",
            result.errors,
            result.warnings
        );
        assert!(
            result.warnings.is_empty(),
            "Must be an error, not a warning"
        );
    }

    /// Self-sufficient readers: each trait reads only fields IT writes (the
    /// `std-record-detail` shape — the drawer re-stashes the row the detail
    /// trait broadcast, then renders its own copy). No read misses any
    /// write, so nothing is constructed.
    #[test]
    fn split_mutation_silent_when_every_reader_writes_its_own_reads() {
        let t1 = trait_with_init_effects(
            "Detail",
            "Blob",
            vec![set_effect("x"), render_ui_reading("x")],
        );
        let t2 = trait_with_init_effects(
            "Drawer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("y")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .warnings
                .iter()
                .chain(result.errors.iter())
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "self-sufficient readers carry no consequence: {:?}",
            result.warnings
        );
    }

    /// An EFFECT-argument cross-read (not render): the reader's call-service
    /// arg reads a field only the sibling writes — the value provably never
    /// arrives. Reported, same as a render cross-read.
    #[test]
    fn split_mutation_reports_effect_arg_cross_read() {
        let t1 = trait_with_init_effects("Input", "Blob", vec![set_effect("ttl")]);
        let t2 = trait_with_init_effects(
            "Host",
            "Blob",
            vec![
                set_effect("status"),
                arr(vec![
                    s("call-service"),
                    s("redis"),
                    s("set"),
                    SExpression::Object({
                        let mut m = std::collections::HashMap::new();
                        m.insert("ttl".to_string(), s("@entity.ttl"));
                        m
                    }),
                ]),
            ],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation
                    && e.message.contains("@entity.ttl")),
            "an effect-arg read that misses the write is the consequence: {:?} {:?}",
            result.errors,
            result.warnings
        );
    }

    #[test]
    fn split_mutation_silent_without_render_ui_reader() {
        // The informational arm ("likely legitimate isolated per-trait
        // scratch") is no longer constructed at all — no reader observed the
        // split, so there is no consequence to report.
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects("Scorer", "Blob", vec![set_effect("y")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Expected no EntitySplitMutation finding without a reader: {:?}",
            result.warnings
        );
    }

    fn with_source_behavior(
        mut td: TraitDefinition,
        behavior: &str,
        original_name: &str,
    ) -> TraitDefinition {
        td.source_behavior = Some(orbital_core::schema::SourceBehaviorMetadata {
            behavior: behavior.to_string(),
            alias: td.name.clone(),
            original_name: original_name.to_string(),
        });
        td
    }

    /// N independent instances of the SAME source trait (e.g. two composed
    /// `ui-input`s) mutating disjoint fields, with a reader — isolation is
    /// the design, not an accident: no EntitySplitMutation.
    #[test]
    fn same_source_instances_do_not_split_mutation() {
        let t1 = with_source_behavior(
            trait_with_init_effects("FirstNameInput", "Form", vec![set_effect("firstName")]),
            "ui-input",
            "InputRender",
        );
        let t2 = with_source_behavior(
            trait_with_init_effects(
                "LastNameInput",
                "Form",
                vec![set_effect("lastName"), render_ui_reading("firstName")],
            ),
            "ui-input",
            "InputRender",
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Form".to_string(),
            entity("Form", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "N instances of the same source trait must not split-mutation: {:?}",
            result.warnings
        );
    }

    /// The negative: mutators composed from DIFFERENT source traits (or one
    /// authored directly) are genuinely heterogeneous — same-source must not
    /// over-suppress, and the finding stays reported when there's a reader.
    #[test]
    fn heterogeneous_source_traits_with_reader_still_report() {
        let t1 = with_source_behavior(
            trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]),
            "std-motion",
            "MoveRender",
        );
        let t2 = trait_with_init_effects(
            "Renderer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("x")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Mutators from different sources (one imported, one authored) must still report: {:?}",
            result.errors
        );
    }

    /// Mutators that can never co-mount (disjoint page membership) can't
    /// split-write the same runtime instance — no EntitySplitMutation even
    /// with a reader present.
    #[test]
    fn page_disjoint_mutators_do_not_split_mutation() {
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects(
            "Renderer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("x")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let mut trait_pages: HashMap<String, HashSet<String>> = HashMap::new();
        trait_pages.insert(
            "Mover".to_string(),
            ["PageA".to_string()].into_iter().collect(),
        );
        trait_pages.insert(
            "Renderer".to_string(),
            ["PageB".to_string()].into_iter().collect(),
        );
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &trait_pages);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Mutators that never co-mount must not split-mutation: {:?}",
            result.warnings
        );
    }

    /// The negative: mutators that DO share a page (or whose page membership
    /// is unknown) keep reporting — page-disjointness must not over-suppress.
    #[test]
    fn page_overlapping_mutators_still_report() {
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects(
            "Renderer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("x")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let mut trait_pages: HashMap<String, HashSet<String>> = HashMap::new();
        trait_pages.insert(
            "Mover".to_string(),
            ["PageA".to_string()].into_iter().collect(),
        );
        trait_pages.insert(
            "Renderer".to_string(),
            ["PageA".to_string()].into_iter().collect(),
        );
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &trait_pages);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Mutators sharing a page must still report: {:?}",
            result.errors
        );
    }

    /// `collect_trait_pages` closes the seed (`page.traits[]`) over
    /// `@trait.X` render-ui references: `Host` is directly on "Main" and
    /// `@trait`-references `Child` from its render tree, so `Child` inherits
    /// "Main" too even though the page's own `traits[]` never names it.
    #[test]
    fn collect_trait_pages_closes_over_trait_refs() {
        let host = trait_with_init_effects(
            "Host",
            "Blob",
            vec![render_ui_reading_with_refs("x", &["Child"])],
        );
        let child = trait_with_init_effects("Child", "Blob", vec![set_effect("y")]);
        let orbital = Orbital {
            name: "TestOrbital".to_string(),
            id: None,
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity("Blob", false, Some(Persistence::Runtime))),
            auxiliary_entities: None,
            traits: vec![
                TraitReference::Inline(host.clone()),
                TraitReference::Inline(child.clone()),
            ],
            pages: Some(vec![PageRef::Inline(PageDefinition {
                name: "Main".to_string(),
                id: None,
                path: "/".to_string(),
                view_type: None,
                primary_entity: None,
                is_initial: Some(true),
                traits: Some(vec![TraitReference::Inline(host.clone())]),
                sections: None,
                icon: None,
                label: None,
            })]),
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            types: None,
            reference: None,
            config: None,
        };
        let trait_pages = collect_trait_pages(&orbital, &[&host, &child]);
        assert!(
            trait_pages.get("Host").is_some_and(|p| p.contains("Main")),
            "the directly-seeded trait must carry the page: {trait_pages:?}"
        );
        assert!(
            trait_pages.get("Child").is_some_and(|p| p.contains("Main")),
            "the @trait-referenced child must inherit the host's page: {trait_pages:?}"
        );
    }

    #[test]
    fn no_split_mutation_with_single_mutator() {
        let t1 = trait_with_init_effects(
            "Mover",
            "Blob",
            vec![set_effect("x"), render_ui_reading("x")],
        );
        let traits = vec![&t1];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Single mutator must not trip SPLIT_ENTITY_MUTATION: {:?}",
            result.warnings
        );
    }

    #[test]
    fn no_split_mutation_when_persistent() {
        // Same 2-mutator + reader shape, but entity is Persistent, not
        // Runtime — the rule is Runtime-only.
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects(
            "Renderer",
            "Blob",
            vec![set_effect("y"), render_ui_reading("x")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Persistent)),
        );
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Persistent entity must not trip SPLIT_ENTITY_MUTATION: {:?}",
            result.warnings
        );
    }

    // ---- B: SharedEntityFieldConflict / SharedEntityRenderAuthority -------

    #[test]
    fn shared_entity_field_conflict_errors() {
        let t1 = trait_with_init_effects("Writer1", "Pool", vec![set_effect("hp")]);
        let t2 = trait_with_init_effects("Writer2", "Pool", vec![set_effect("hp")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SharedEntityFieldConflict),
            "Expected SharedEntityFieldConflict error: {:?}",
            result.errors
        );
    }

    #[test]
    fn shared_entity_disjoint_fields_pass() {
        let t1 = trait_with_init_effects("Writer1", "Pool", vec![set_effect("hp")]);
        let t2 = trait_with_init_effects("Writer2", "Pool", vec![set_effect("mana")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::SharedEntityFieldConflict),
            "Disjoint-field writers on a shared entity must pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn shared_entity_render_authority_errors() {
        let t1 = trait_with_init_effects("Reader1", "Pool", vec![render_ui_reading("hp")]);
        let t2 = trait_with_init_effects("Reader2", "Pool", vec![render_ui_reading("hp")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "Expected SharedEntityRenderAuthority error: {:?}",
            result.errors
        );
    }

    fn render_ui_reading_slot(field: &str, slot: &str) -> SExpression {
        arr(vec![
            s("render-ui"),
            s(slot),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s(&format!("@entity.{}", field)));
                m
            }),
        ])
    }

    #[test]
    fn shared_entity_render_authority_per_slot_different_slots_pass() {
        // Two traits bound to one [shared] entity, each owning a DIFFERENT
        // render slot — Rule 6 refinement: no contention when the slots
        // differ, even though both read the same shared entity.
        let t1 = trait_with_init_effects("Reader1", "Pool", vec![render_ui_reading_slot("hp", "main")]);
        let t2 =
            trait_with_init_effects("Reader2", "Pool", vec![render_ui_reading_slot("hp", "sidebar")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "Different render slots on a shared entity must not contend: {:?}",
            result.errors
        );
    }

    /// A render-ui effect that reads `@entity.<field>` AND references the
    /// given sibling traits via `@trait.<name>` children — the shape a host
    /// authority trait has after inline JSX wrappers are hoisted.
    fn render_ui_reading_with_refs(field: &str, refs: &[&str]) -> SExpression {
        arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s(&format!("@entity.{}", field)));
                m.insert(
                    "children".to_string(),
                    SExpression::Array(refs.iter().map(|r| s(&format!("@trait.{}", r))).collect()),
                );
                m
            }),
        ])
    }

    #[test]
    fn shared_entity_referenced_wrappers_attribute_to_host_authority() {
        // Host authority reads the shared entity AND references two wrapper
        // traits from its render tree; each wrapper also reads @entity.*.
        // Attribution collapses all reads onto the host → exactly one
        // authority → VALID.
        let host = trait_with_init_effects(
            "DeckView",
            "Pool",
            vec![render_ui_reading_with_refs("hp", &["InlineW1", "InlineW2"])],
        );
        let w1 = trait_with_init_effects("InlineW1", "Pool", vec![render_ui_reading("hp")]);
        let w2 = trait_with_init_effects("InlineW2", "Pool", vec![render_ui_reading("mana")]);
        let traits = vec![&host, &w1, &w2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "Referenced wrappers must attribute to the host authority: {:?}",
            result.errors
        );
    }

    #[test]
    fn shared_entity_transitive_wrapper_chain_collapses_to_root() {
        // host → child (via @trait.Child) → grandchild (via @trait.Grand):
        // all three read the shared entity; attribution collapses to the
        // single root → VALID.
        let host = trait_with_init_effects(
            "Root",
            "Pool",
            vec![render_ui_reading_with_refs("hp", &["Child"])],
        );
        let child = trait_with_init_effects(
            "Child",
            "Pool",
            vec![render_ui_reading_with_refs("hp", &["Grand"])],
        );
        let grand = trait_with_init_effects("Grand", "Pool", vec![render_ui_reading("hp")]);
        let traits = vec![&host, &child, &grand];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "Transitive wrapper chain must collapse to one root authority: {:?}",
            result.errors
        );
    }

    #[test]
    fn shared_entity_unreferenced_second_reader_still_rejected() {
        // Host authority + referenced wrapper are fine, but a genuinely
        // independent second reader (referenced from NO render tree) must
        // still trip SharedEntityRenderAuthority. Regression pin: the
        // attribution must never relax the strict single-authority rule.
        let host = trait_with_init_effects(
            "DeckView",
            "Pool",
            vec![render_ui_reading_with_refs("hp", &["InlineW1"])],
        );
        let w1 = trait_with_init_effects("InlineW1", "Pool", vec![render_ui_reading("hp")]);
        let rogue = trait_with_init_effects("RogueReader", "Pool", vec![render_ui_reading("hp")]);
        let traits = vec![&host, &w1, &rogue];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "Unreferenced second reader must still be rejected: {:?}",
            result.errors
        );
    }

    #[test]
    fn shared_entity_render_ref_cycle_terminates() {
        // Pure reference cycle: A references B, B references A, both read
        // the shared entity, nobody outside references either. Attribution
        // must terminate (no infinite loop); each cycle member attributes to
        // itself → two authorities → still rejected.
        let a = trait_with_init_effects(
            "CycleA",
            "Pool",
            vec![render_ui_reading_with_refs("hp", &["CycleB"])],
        );
        let b = trait_with_init_effects(
            "CycleB",
            "Pool",
            vec![render_ui_reading_with_refs("mana", &["CycleA"])],
        );
        let traits = vec![&a, &b];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result =
            validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs, &HashMap::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
            "A pure cycle has no root authority and must still be rejected: {:?}",
            result.errors
        );
    }

    // ---- A: tick guards -----------------------------------------------------

    #[test]
    fn tick_requires_runtime_warns_on_persistent_non_cron() {
        let t = trait_with_tick("Cooldown", "CooldownRecord", "100ms", vec![]);
        let mut defs = HashMap::new();
        defs.insert(
            "CooldownRecord".to_string(),
            entity("CooldownRecord", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_tick_guards(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TickRequiresRuntime),
            "Expected TickRequiresRuntime error: {:?}",
            result.errors
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn tick_cron_interval_exempt() {
        let t = trait_with_tick("Scheduled", "Job", "0 9 * * *", vec![]);
        let mut defs = HashMap::new();
        defs.insert(
            "Job".to_string(),
            entity("Job", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_tick_guards(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TickRequiresRuntime),
            "Cron interval must be exempt: {:?}",
            result.warnings
        );
    }

    #[test]
    fn tick_requires_runtime_passes_on_runtime_entity() {
        let t = trait_with_tick("Cooldown", "CooldownRecord", "100ms", vec![]);
        let mut defs = HashMap::new();
        defs.insert(
            "CooldownRecord".to_string(),
            entity("CooldownRecord", false, Some(Persistence::Runtime)),
        );
        let result = validate_trait_tick_guards(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TickRequiresRuntime),
            "Runtime entity must not trip TickRequiresRuntime: {:?}",
            result.warnings
        );
    }

    #[test]
    fn tick_async_effect_warns() {
        let async_call = arr(vec![s("llm/generate"), s("prompt")]);
        let t = trait_with_tick("Chatter", "ChatState", "1s", vec![async_call]);
        let mut defs = HashMap::new();
        defs.insert(
            "ChatState".to_string(),
            entity("ChatState", false, Some(Persistence::Runtime)),
        );
        let result = validate_trait_tick_guards(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TickAsyncEffect),
            "Expected TickAsyncEffect error: {:?}",
            result.errors
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn tick_async_effect_exempt_on_cron() {
        let async_call = arr(vec![s("llm/generate"), s("prompt")]);
        let t = trait_with_tick("Chatter", "ChatState", "0 9 * * *", vec![async_call]);
        let mut defs = HashMap::new();
        defs.insert(
            "ChatState".to_string(),
            entity("ChatState", false, Some(Persistence::Runtime)),
        );
        let result = validate_trait_tick_guards(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TickAsyncEffect),
            "Cron tick must be exempt from TickAsyncEffect: {:?}",
            result.warnings
        );
    }

    // ---- C: local trait dispatch contract ----------------------------------

    #[test]
    fn local_trait_touches_server_via_persist_errors() {
        let persist_effect = arr(vec![
            s("persist"),
            s("create"),
            s("Message"),
            SExpression::Object(std::collections::HashMap::new()),
        ]);
        let t = local_trait_with_guard("ChatComposer", "Message", None, vec![persist_effect]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitLocalTouchesServer),
            "Expected TraitLocalTouchesServer error: {:?}",
            result.errors
        );
    }

    #[test]
    fn local_trait_guard_on_user_errors() {
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let t = local_trait_with_guard("Composer", "Message", Some(guard), vec![]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitLocalTouchesServer),
            "Expected TraitLocalTouchesServer error for @user guard: {:?}",
            result.errors
        );
    }

    #[test]
    fn local_trait_with_no_server_effects_and_no_user_guard_passes() {
        let plain_guard = arr(vec![s("!="), s("@entity.activeChannel"), s("")]);
        let t = local_trait_with_guard(
            "ChatComposer",
            "Message",
            Some(plain_guard),
            vec![set_effect("draft")],
        );
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TraitLocalTouchesServer),
            "A plain precondition guard + client-only effect must pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn local_trait_on_non_shared_persistent_entity_errors() {
        let t = local_trait_with_guard("Composer", "Message", None, vec![set_effect("draft")]);
        let mut defs = HashMap::new();
        defs.insert(
            "Message".to_string(),
            entity("Message", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitLocalOnNonSharedPersistent),
            "Expected TraitLocalOnNonSharedPersistent error: {:?}",
            result.errors
        );
    }

    #[test]
    fn local_trait_on_shared_entity_passes_the_shape_check() {
        let t = local_trait_with_guard("Composer", "Message", None, vec![set_effect("draft")]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TraitLocalOnNonSharedPersistent),
            "[shared] entity must satisfy the local-trait shape check: {:?}",
            result.errors
        );
    }

    #[test]
    fn local_trait_on_runtime_entity_passes_the_shape_check() {
        let t = local_trait_with_guard("Composer", "Widget", None, vec![set_effect("draft")]);
        let mut defs = HashMap::new();
        defs.insert(
            "Widget".to_string(),
            entity("Widget", false, Some(Persistence::Runtime)),
        );
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::TraitLocalOnNonSharedPersistent),
            "[runtime] entity must satisfy the local-trait shape check: {:?}",
            result.errors
        );
    }

    #[test]
    fn non_local_trait_is_exempt_from_both_checks() {
        let persist_effect = arr(vec![
            s("persist"),
            s("create"),
            s("Message"),
            SExpression::Object(std::collections::HashMap::new()),
        ]);
        let t = trait_with_init_effects("Composer", "Message", vec![persist_effect]);
        let mut defs = HashMap::new();
        defs.insert(
            "Message".to_string(),
            entity("Message", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_local_dispatch(&t, "test", &defs);
        assert!(
            result.errors.is_empty(),
            "A non-local trait must be exempt from both Rule 8 checks: {:?}",
            result.errors
        );
    }

    // ---- D: set-column-without-row backstop --------------------------------

    fn entity_with_intrinsic_field(name: &str, intrinsic_field: &str) -> EntityDefinition {
        let mut e = entity(name, false, Some(Persistence::Persistent));
        e.fields.push(FieldDefinition {
            merged_from: None,
            name: Some(intrinsic_field.to_string()),
            field_type: FieldType::Boolean,
            required: None,
            primary_key: None,
            unique: None,
            relation: None,
            items: None,
            values: None,
            default: None,
            properties: None,
            intrinsic: Some(true),
            description: None,
            synonyms: None,
        });
        e
    }

    fn persist_create_effect(entity_name: &str) -> SExpression {
        arr(vec![
            s("persist"),
            s("create"),
            s(entity_name),
            SExpression::Object(std::collections::HashMap::new()),
        ])
    }

    #[test]
    fn set_column_without_row_and_no_persist_errors() {
        let t = trait_with_init_effects("DraftScratch", "Note", vec![set_effect("title")]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SetColumnWithoutRow),
            "Expected SetColumnWithoutRow error: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_from_fetch_success_typed_by_concrete_entity_name_passes() {
        // std-kflow's `GraphView` shape: a directly-authored organism, not a
        // generic library atom, so its fetch-success payload is typed by the
        // CONCRETE entity name (`data : Note`) rather than the unresolved
        // `@entity` sentinel a rebindable atom still carries. Same "whole
        // row" meaning, already resolved.
        let mut t = trait_with_init_effects(
            "NoteView",
            "Note",
            vec![arr(vec![
                s("set"),
                s("@entity.title"),
                s("@payload.data.title"),
            ])],
        );
        t.state_machine.transitions[0].event = "NoteLoaded".to_string();
        t.emits = Some(vec![EmitDefinition {
            event: "NoteLoaded".to_string(),
            event_id: None,
            description: None,
            synonyms: None,
            tier: None,
            scope: None,
            payload_schema: Some(vec![PayloadField {
                name: "data".to_string(),
                field_type: "Note".to_string(),
                required: None,
                description: None,
                properties: None,
                type_when: None,
                entity: None,
            }]),
            definer_knob: None,
            scope_overridden: None,
            payload_entity: None,
        }]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A field mirrored from a fetch-success payload typed by the trait's own concrete entity name must not trip SetColumnWithoutRow: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_from_fetch_success_stamped_object_entity_passes() {
        // The shape lowering ACTUALLY produces for `data : Note` — std-kflow's
        // real `GraphLoaded { data: Graph }`: the field flattens to
        // `field_type: "object"` with the entity identity moved to the
        // separate `PayloadField.entity` stamp, not left in `field_type`.
        // `event_payload_row_entity` (not the flat `per_event_payload` map)
        // is what has to see this.
        let mut t = trait_with_init_effects(
            "NoteView",
            "Note",
            vec![arr(vec![
                s("set"),
                s("@entity.title"),
                s("@payload.data.title"),
            ])],
        );
        t.state_machine.transitions[0].event = "NoteLoaded".to_string();
        t.emits = Some(vec![EmitDefinition {
            event: "NoteLoaded".to_string(),
            event_id: None,
            description: None,
            synonyms: None,
            tier: None,
            scope: None,
            payload_schema: Some(vec![PayloadField {
                name: "data".to_string(),
                field_type: "object".to_string(),
                required: None,
                description: None,
                properties: Some(vec![PayloadField {
                    name: "title".to_string(),
                    field_type: "string".to_string(),
                    required: None,
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                }]),
                type_when: None,
                entity: Some("Note".to_string()),
            }]),
            definer_knob: None,
            scope_overridden: None,
            payload_entity: None,
        }]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A field mirrored from a fetch-success payload stamped with the trait's own entity must not trip SetColumnWithoutRow: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_with_eventual_persist_create_passes() {
        let t = trait_with_init_effects(
            "DraftForm",
            "Note",
            vec![set_effect("title"), persist_create_effect("Note")],
        );
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A draft form that eventually persists must not trip SetColumnWithoutRow: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_on_shared_entity_persisted_by_a_sibling_trait_passes() {
        // ChatMessageAttachment's shape: sets a column on a [shared] entity
        // with NO persist call of its own; a sibling trait (the persistor)
        // is the one that creates the row against the same shared instance.
        let t = trait_with_init_effects("Attacher", "Message", vec![set_effect("attachment")]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let orbital_persisted: HashSet<&str> = ["Message"].into_iter().collect();
        let result =
            validate_trait_set_column_without_row(&t, "test", &defs, &orbital_persisted, &[]);
        assert!(
            result.errors.is_empty(),
            "A shared entity persisted by ANY trait in the orbital must exempt every co-mutator: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_on_non_shared_entity_ignores_sibling_persist() {
        // The isolation contract: a NON-shared entity gets a per-trait copy,
        // so a sibling's persist call can never see this trait's write — the
        // orbital-wide exemption must not apply here.
        let t = trait_with_init_effects("Attacher", "Note", vec![set_effect("title")]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let orbital_persisted: HashSet<&str> = ["Note"].into_iter().collect();
        let result =
            validate_trait_set_column_without_row(&t, "test", &defs, &orbital_persisted, &[]);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SetColumnWithoutRow),
            "A non-shared entity must not be exempted by a sibling's persist call: {:?}",
            result.errors
        );
    }

    /// `(set @entity.<field> @payload.<var>.<field>)` — the exact shape
    /// `restate_persistor_payload_var`/`set_field_from_payload_var` recognize.
    fn set_from_payload_effect(field: &str, var: &str, sub_field: &str) -> SExpression {
        arr(vec![
            s("set"),
            s(&format!("@entity.{field}")),
            s(&format!("@payload.{var}.{sub_field}")),
        ])
    }

    fn persist_whole_payload_effect(entity_name: &str, var: &str) -> SExpression {
        arr(vec![
            s("persist"),
            s("update"),
            s(entity_name),
            s(&format!("@payload.{var}")),
            SExpression::Object(std::collections::HashMap::new()),
        ])
    }

    fn restate_listen(source_trait: &str, event: &str, triggers: &str) -> ListenDefinition {
        ListenDefinition {
            event: event.to_string(),
            event_id: None,
            triggers: Some(triggers.to_string()),
            triggers_id: None,
            scope: None,
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            source: Some(ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
        }
    }

    #[test]
    fn set_column_landed_by_restate_persistor_sibling_passes() {
        // std-vendor-onboarding's shape: VendorOnboard.NEXT_STEP sets
        // @entity.businessName from @payload.data.businessName with NO
        // persist of its own; the restate route `VendorOnboard.NEXT_STEP ->
        // DO_UPDATE` forwards the SAME event's payload to VendorPersistor,
        // whose DO_UPDATE arm runs `persist update Vendor @payload.data`.
        // Note is deliberately NON-shared — this exemption is payload
        // provenance, not instance sharing.
        let mut source = trait_with_init_effects(
            "VendorOnboard",
            "Note",
            vec![set_from_payload_effect("title", "data", "title")],
        );
        source.state_machine.transitions[0].event = "NEXT_STEP".to_string();

        let mut persistor = trait_with_init_effects(
            "VendorPersistor",
            "Note",
            vec![persist_whole_payload_effect("Note", "data")],
        );
        persistor.state_machine.transitions[0].event = "DO_UPDATE".to_string();
        persistor.listens = Some(vec![restate_listen("VendorOnboard", "NEXT_STEP", "DO_UPDATE")]);

        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let siblings: Vec<&TraitDefinition> = vec![&source, &persistor];
        let result =
            validate_trait_set_column_without_row(&source, "test", &defs, &HashSet::new(), &siblings);
        assert!(
            result.errors.is_empty(),
            "A write drawn from the exact payload a restate-routed sibling persists must land: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_not_from_the_persisted_payload_var_still_errors() {
        // Same restate wiring, but this field's value comes from a
        // DIFFERENT payload key (`@payload.submittedAt`, not
        // `@payload.data.*`) — the sibling's `persist update Note
        // @payload.data` never carries it, so it must still error (this is
        // std-vendor-onboarding's real `submittedAt` gap, not a false
        // positive).
        let mut source = trait_with_init_effects(
            "VendorOnboard",
            "Note",
            vec![arr(vec![
                s("set"),
                s("@entity.submittedAt"),
                s("@payload.submittedAt"),
            ])],
        );
        source.state_machine.transitions[0].event = "SUBMIT".to_string();

        let mut persistor = trait_with_init_effects(
            "VendorPersistor",
            "Note",
            vec![persist_whole_payload_effect("Note", "data")],
        );
        persistor.state_machine.transitions[0].event = "DO_SUBMIT".to_string();
        persistor.listens = Some(vec![restate_listen("VendorOnboard", "SUBMIT", "DO_SUBMIT")]);

        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let siblings: Vec<&TraitDefinition> = vec![&source, &persistor];
        let result =
            validate_trait_set_column_without_row(&source, "test", &defs, &HashSet::new(), &siblings);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SetColumnWithoutRow),
            "A field drawn from a payload key the sibling's persist never touches must still error: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_scoping_a_fetch_filter_passes() {
        // std-lms's `MyProgressDisplay` shape: `(set @entity.studentId
        // @user.id)` then `(fetch Progress { filter: ["=", (object/get
        // @entity studentId), @user.id] })` — the field exists purely to
        // scope this trait's OWN read to the caller's row, never to land in
        // a persisted write.
        let filter_expr = arr(vec![
            s("="),
            arr(vec![s("object/get"), s("@entity"), s("studentId")]),
            s("@user.id"),
        ]);
        let fetch_effect = arr(vec![
            s("fetch"),
            s("Progress"),
            SExpression::Object(HashMap::from([("filter".to_string(), filter_expr)])),
        ]);
        let t = trait_with_init_effects(
            "MyProgressDisplay",
            "Progress",
            vec![
                arr(vec![s("set"), s("@entity.studentId"), s("@user.id")]),
                fetch_effect,
            ],
        );
        let mut defs = HashMap::new();
        defs.insert(
            "Progress".to_string(),
            entity("Progress", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A field set only to scope this transition's own fetch filter must not trip SetColumnWithoutRow: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_read_elsewhere_but_not_in_a_fetch_filter_still_errors() {
        // Same field reference shape, but the read is a plain guard, not a
        // fetch's `filter:` key — must NOT be exempted by the read-filter
        // check (only an actual scoped read counts).
        let t = trait_with_init_effects(
            "Reader",
            "Progress",
            vec![
                arr(vec![s("set"), s("@entity.studentId"), s("@user.id")]),
                arr(vec![
                    s("notify"),
                    s("info"),
                    arr(vec![s("object/get"), s("@entity"), s("studentId")]),
                ]),
            ],
        );
        let mut defs = HashMap::new();
        defs.insert(
            "Progress".to_string(),
            entity("Progress", false, Some(Persistence::Persistent)),
        );
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::SetColumnWithoutRow),
            "A field merely read outside a fetch filter must still error: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_with_entity_id_bound_passes() {
        let t = trait_with_init_effects(
            "EditForm",
            "Note",
            vec![set_effect("id"), set_effect("title")],
        );
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity("Note", false, Some(Persistence::Persistent)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A trait binding @entity.id has a row in scope and must not trip the rule: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_on_intrinsic_field_passes() {
        let t = trait_with_init_effects("Widget", "Note", vec![set_effect("isEditing")]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity_with_intrinsic_field("Note", "isEditing"));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "An @intrinsic field must never trip SetColumnWithoutRow: {:?}",
            result.errors
        );
    }

    #[test]
    fn set_column_on_runtime_entity_is_exempt() {
        let t = trait_with_init_effects("Widget", "Cursor", vec![set_effect("x")]);
        let mut defs = HashMap::new();
        defs.insert("Cursor".to_string(), entity("Cursor", false, Some(Persistence::Runtime)));
        let result = validate_trait_set_column_without_row(&t, "test", &defs, &HashSet::new(), &[]);
        assert!(
            result.errors.is_empty(),
            "A [runtime] entity has no row to land in and must be exempt: {:?}",
            result.errors
        );
    }

    // ---- Rule 2b: guard-authorization on an @intrinsic field ---------------

    #[test]
    fn guard_on_intrinsic_field_alongside_user_gating_persist_errors() {
        let guard = arr(vec![
            s("and"),
            arr(vec![s("=="), s("@entity.isEditing"), SExpression::Bool(true)]),
            arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]),
        ]);
        let persist_effect = arr(vec![
            s("persist"),
            s("update"),
            s("Note"),
            s("@entity"),
        ]);
        let t = trait_with_guard("NoteEditor", "Note", Some(guard), vec![persist_effect]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity_with_intrinsic_field("Note", "isEditing"));
        let result = validate_trait_guard_frame_field_authorization(&t, "test", &defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::GuardOnFrameFieldAuthorizes),
            "Expected GuardOnFrameFieldAuthorizes error: {:?}",
            result.errors
        );
    }

    #[test]
    fn guard_on_intrinsic_field_without_user_is_not_authorization() {
        // A plain precondition guard with no identity comparison — like
        // ChatComposer.SEND's `activeChannel != ""` — is not a hit.
        let guard = arr(vec![s("=="), s("@entity.isEditing"), SExpression::Bool(true)]);
        let persist_effect = arr(vec![s("persist"), s("update"), s("Note"), s("@entity")]);
        let t = trait_with_guard("NoteEditor", "Note", Some(guard), vec![persist_effect]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity_with_intrinsic_field("Note", "isEditing"));
        let result = validate_trait_guard_frame_field_authorization(&t, "test", &defs);
        assert!(
            result.errors.is_empty(),
            "A plain precondition guard must not be flagged as authorization: {:?}",
            result.errors
        );
    }

    #[test]
    fn guard_on_intrinsic_field_with_user_but_no_mutation_is_not_a_hit() {
        let guard = arr(vec![
            s("and"),
            arr(vec![s("=="), s("@entity.isEditing"), SExpression::Bool(true)]),
            arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]),
        ]);
        let t = trait_with_guard("NoteViewer", "Note", Some(guard), vec![set_effect("title")]);
        let mut defs = HashMap::new();
        defs.insert("Note".to_string(), entity_with_intrinsic_field("Note", "isEditing"));
        let result = validate_trait_guard_frame_field_authorization(&t, "test", &defs);
        assert!(
            result.errors.is_empty(),
            "No persist/call-service in the transition means no authorization is at stake: {:?}",
            result.errors
        );
    }

    // ---- Rule 4: ORB_AUTH_GUARD_CLIENT_ONLY ---------------------------------

    #[test]
    fn user_guard_on_local_trait_without_server_effect_does_not_warn() {
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let t = local_trait_with_guard("Composer", "Message", Some(guard), vec![set_effect("draft")]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_auth_guard_client_only(&t, "t", &defs);
        assert!(
            result.warnings.iter().all(|e| e.code != ErrorCode::AuthGuardClientOnly),
            "A local trait with no server-touching effect has nothing to protect: {:?}",
            result.warnings
        );
    }

    #[test]
    fn user_guard_on_runtime_entity_trait_without_server_effect_does_not_warn() {
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let t = trait_with_guard("Cursor", "Cursor", Some(guard), vec![set_effect("x")]);
        let mut defs = HashMap::new();
        defs.insert("Cursor".to_string(), entity("Cursor", false, Some(Persistence::Runtime)));
        let result = validate_trait_auth_guard_client_only(&t, "t", &defs);
        assert!(
            result.warnings.iter().all(|e| e.code != ErrorCode::AuthGuardClientOnly),
            "A [runtime]-bound trait with no server-touching effect has nothing to protect: {:?}",
            result.warnings
        );
    }

    #[test]
    fn user_guard_on_local_trait_with_server_effect_warns_optimistic_advance() {
        // A local trait CAN carry a server-bound effect (Rule 8's own
        // `local_trait_touches_server_via_persist_errors` fixture proves
        // that shape is possible, just separately flagged) — when it does,
        // Rule 4's message must describe the OPTIMISTIC-advance shape, not
        // claim the guard is inert, since the effect really is server-gated.
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let persist_effect = arr(vec![
            s("persist"),
            s("update"),
            s("Message"),
            SExpression::Object(std::collections::HashMap::new()),
        ]);
        let t = local_trait_with_guard("Composer", "Message", Some(guard), vec![persist_effect]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_auth_guard_client_only(&t, "test", &defs);
        let hit = result
            .warnings
            .iter()
            .find(|e| e.code == ErrorCode::AuthGuardClientOnly);
        assert!(hit.is_some(), "Expected AuthGuardClientOnly warning: {:?}", result.warnings);
        assert!(
            hit.unwrap().message.contains("advances optimistically")
                && hit.unwrap().message.contains("effects stay server-gated"),
            "A transition with a real server-touching effect must get the optimistic-advance message: {:?}",
            hit
        );
    }

    #[test]
    fn user_guard_on_runtime_entity_trait_with_nested_server_effect_warns_optimistic_advance() {
        // The server-bound effect is nested inside `do`/`if`, matching
        // `contains_server_bound_effect`'s own recursive-wrapper contract —
        // must still pick the optimistic-advance message, not the inert one.
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let nested_fetch = arr(vec![
            s("do"),
            arr(vec![
                s("fetch"),
                s("Cursor"),
                SExpression::Object(std::collections::HashMap::new()),
            ]),
        ]);
        let t = trait_with_guard("Cursor", "Cursor", Some(guard), vec![nested_fetch]);
        let mut defs = HashMap::new();
        defs.insert("Cursor".to_string(), entity("Cursor", false, Some(Persistence::Runtime)));
        let result = validate_trait_auth_guard_client_only(&t, "test", &defs);
        let hit = result
            .warnings
            .iter()
            .find(|e| e.code == ErrorCode::AuthGuardClientOnly);
        assert!(hit.is_some(), "Expected AuthGuardClientOnly warning: {:?}", result.warnings);
        assert!(
            hit.unwrap().message.contains("advances optimistically")
                && hit.unwrap().message.contains("effects stay server-gated"),
            "A nested server-touching effect must still get the optimistic-advance message: {:?}",
            hit
        );
    }

    #[test]
    fn user_guard_on_server_gated_trait_does_not_warn() {
        let guard = arr(vec![s("=="), s("@user.id"), s("@entity.ownerId")]);
        let t = trait_with_guard("Composer", "Message", Some(guard), vec![set_effect("draft")]);
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), entity("Message", true, Some(Persistence::Persistent)));
        let result = validate_trait_auth_guard_client_only(&t, "test", &defs);
        assert!(
            result.warnings.is_empty(),
            "A server-gated (non-local, non-runtime) trait must not warn: {:?}",
            result.warnings
        );
    }
}
