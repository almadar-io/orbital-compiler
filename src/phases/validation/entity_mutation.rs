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
    EntityDefinition, OrbitalSchema, Persistence, SExpression, StringOrNumber, TraitDefinition,
    TraitReference,
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

        let result = validate_orbital_entity_mutations(
            &inline_traits,
            &orbital_path,
            &ctx.entity_definitions,
        );
        ctx.merge(result);
    }
}

fn validate_orbital_entity_mutations(
    traits: &[&TraitDefinition],
    orbital_path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

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

            let has_reader = renderers
                .get(entity_name)
                .map(|r| !r.is_empty())
                .unwrap_or(false);
            let message = if has_reader {
                format!(
                    "Entity '{}' (runtime, not [shared]) is mutated by {} distinct traits {:?} and read by a render-ui — each mutator gets an ISOLATED per-trait copy, so the render never sees the other traits' writes; this is provably wrong unless the entity is meant to be one shared instance",
                    entity_name, all_mutators.len(), all_mutators
                )
            } else {
                format!(
                    "Entity '{}' (runtime, not [shared]) is mutated by {} distinct traits {:?} but none of them render-ui reads it — likely legitimate isolated per-trait scratch state, flagged informationally",
                    entity_name, all_mutators.len(), all_mutators
                )
            };
            result.add_error(
                ValidationError::phase_f_warning(ErrorCode::EntitySplitMutation, path.as_str(), message)
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
                    ValidationError::phase_f_warning(
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
                    ValidationError::phase_f_warning(
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
        FieldDefinition, FieldType, StateDefinition, StateMachine, StringOrArray, TickDefinition,
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Expected EntitySplitMutation warning: errors={:?} warnings={:?}",
            result.errors,
            result.warnings
        );
        assert!(result.errors.is_empty(), "Must be a warning, not an error");
    }

    #[test]
    fn split_mutation_warning_without_render_ui_reader() {
        let t1 = trait_with_init_effects("Mover", "Blob", vec![set_effect("x")]);
        let t2 = trait_with_init_effects("Scorer", "Blob", vec![set_effect("y")]);
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert(
            "Blob".to_string(),
            entity("Blob", false, Some(Persistence::Runtime)),
        );
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::EntitySplitMutation),
            "Expected EntitySplitMutation warning even without a reader: {:?}",
            result.warnings
        );
        assert!(result.errors.is_empty());
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::TickRequiresRuntime),
            "Expected TickRequiresRuntime warning: {:?}",
            result.warnings
        );
        assert!(result.errors.is_empty());
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
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::TickAsyncEffect),
            "Expected TickAsyncEffect warning: {:?}",
            result.warnings
        );
        assert!(result.errors.is_empty());
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
