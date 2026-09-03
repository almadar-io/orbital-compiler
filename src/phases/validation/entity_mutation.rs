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
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, Orbital, OrbitalSchema, PageRef, Persistence, SExpression, StringOrNumber,
    TraitDefinition, TraitReference,
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
fn collect_render_ui_readers(traits: &[&TraitDefinition]) -> HashMap<String, HashSet<String>> {
    // Direct readers: (trait name, linked entity) pairs whose render-ui
    // effects read @entity.*.
    let mut direct: Vec<(String, String)> = Vec::new();
    // Reverse render-reference graph: target trait name → trait names whose
    // render-ui trees reference it via `@trait.<target>`.
    let mut referenced_by: HashMap<String, HashSet<String>> = HashMap::new();

    for trait_def in traits {
        let mut reads_entity = false;
        let mut refs: HashSet<String> = HashSet::new();
        for transition in &trait_def.state_machine.transitions {
            if let Some(effects) = &transition.effects {
                for effect in effects {
                    if !is_render_ui_effect(effect) {
                        continue;
                    }
                    if render_ui_effect_reads_entity(effect) {
                        reads_entity = true;
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
                        reads_entity = true;
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

        if reads_entity {
            if let Some(linked) = &trait_def.linked_entity {
                direct.push((trait_def.name.clone(), linked.clone()));
            }
        }
    }

    let mut readers: HashMap<String, HashSet<String>> = HashMap::new();
    for (name, linked) in direct {
        for root in resolve_render_roots(&name, &referenced_by) {
            readers.entry(linked.clone()).or_default().insert(root);
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
            // authority is allowed per shared entity.
            if let Some(entity_renderers) = renderers.get(entity_name) {
                if entity_renderers.len() > 1 {
                    let mut names: Vec<String> = entity_renderers.iter().cloned().collect();
                    names.sort();
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::SharedEntityRenderAuthority,
                            path.as_str(),
                            format!(
                                "Shared entity '{}' is render-ui-read by {} traits: {:?} — exactly one render authority is allowed per shared entity",
                                entity_name, names.len(), names
                            ),
                        )
                        .with_suggestion(
                            "Keep a single trait's render-ui reading this shared entity; route the others through it",
                        ),
                    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EntityRef, FieldDefinition, FieldType, PageDefinition, StateDefinition, StateMachine,
        StringOrArray, TickDefinition, TransitionDefinition,
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
}
