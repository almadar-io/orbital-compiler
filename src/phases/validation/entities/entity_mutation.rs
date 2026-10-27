//! Entity Mutation
//!
//! Multi-trait entity-mutation rules (`SharedEntityRenderAuthority`,
//! `ORB_X_MIXED_RESIDENCE`). The non-`Cron`
//! tick backstop (`ORB_TICK_REQUIRES_RUNTIME`, `ORB_TICK_ASYNC_EFFECT`) is
//! owned by the std lint `std-orb-lint-tick-requires-runtime`. Runs POST-INLINE (after `uses` imports resolve)
//! so the fully composed trait set and each entity's `shared` /
//! `persistence` flags are visible.
//!
//! `EntityDefinition::shared` means several traits deliberately co-mutate ONE
//! entity instance; `[shared]` traits double-rendering one slot are reported.

use super::render_ui::is_render_ui_effect;
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::runs_on::Residence;
use orbital_core_base::schema::{
    EntityDefinition, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

// ============================================================================
// Shared aggregation primitive (also consumed by binding.rs's group-aware
// setter check, Task C) — the single owner of "who mutates this field",
// built on top of `SExpression::collect_entity_field_refs`. No second
// field-ref walker.
// ============================================================================

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

    let renderers = collect_render_ui_readers(traits);

    for entity_name in renderers.keys() {
        let Some(entity_def) = entity_definitions.get(entity_name) else {
            continue;
        };
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
        }
    }

    result
}

/// The entities a tree's data effects (`fetch`/`persist`, by `ENTITY_ARGS`) target, at any depth.
fn data_effect_entities<'a>(expr: &'a SExpression, out: &mut Vec<&'a str>) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(op) = arr.first().and_then(|h| h.as_str()) {
                let batch_ops = (op == "persist")
                    .then(|| orbital_core_base::persist_batch::batch_ops_arg(&arr[1..]))
                    .flatten();
                if let Some(ops) = batch_ops {
                    out.extend(orbital_core_base::persist_batch::batch_entities(ops));
                } else if let Some(entity) = orbital_core_base::runs_on::entity_arg(op)
                    .and_then(|i| arr.get(i + 1))
                    .and_then(|a| a.as_str())
                    .filter(|e| !e.starts_with('@'))
                {
                    out.push(entity);
                }
            }
            arr.iter().for_each(|e| data_effect_entities(e, out));
        }
        SExpression::Object(obj) => obj.values().for_each(|e| data_effect_entities(e, out)),
        _ => {}
    }
}

/// `ORB_X_MIXED_RESIDENCE`: one transition's data effects reach both
/// browser-stored and server-stored entities. No single executor holds both,
/// so the transition must be split (an emit + listen) — Runtime Spec 7.2.
pub fn validate_all_mixed_residence(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let residence = Residence::of_entities(ctx.entity_definitions.values());
    for (trait_def, path) in super::inline_traits(schema) {
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            let mut entities = Vec::new();
            for effect in transition.effects.iter().flatten() {
                data_effect_entities(effect, &mut entities);
            }
            let (browser, server): (Vec<&str>, Vec<&str>) =
                entities.into_iter().partition(|e| residence.is_client(e));
            if browser.is_empty() || server.is_empty() {
                continue;
            }
            ctx.merge({
                let mut result = ValidationResult::new();
                result.add_error(
                    ValidationError::error(
                        ErrorCode::CrossMixedResidence,
                        format!("{}.stateMachine.transitions[{}].effects", path, i),
                        format!(
                            "trait {:?} transition '{}' reads or writes browser-stored {:?} and server-stored {:?} in one transition — no single executor holds both",
                            trait_def.name, transition.event, browser, server
                        ),
                    )
                    .with_suggestion("Split the transition: do the browser-side work here and emit an event a trait on the server-stored entity listens to."),
                );
                result
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        FieldDefinition, FieldType, Persistence, StateDefinition, StateMachine, StringOrArray,
        TransitionDefinition,
    };

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string().into())
    }

    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }

    fn entity(name: &str, shared: bool, persistence: Option<Persistence>) -> EntityDefinition {
        EntityDefinition {
            timestamps: None,
            soft_delete: None,
            description: None,
            visual_prompt: None,
            asset_ref: None,
            store: None,
            shared,
            identity: false,
            browser: false,
            locale_instances: None,
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
                nullable: None,
                additional_properties: None,
                mock: None,
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
                projected_from: None,
                record: None,
                derived: None,
            }],
            id: None,
            invariants: None,
            indexes: None,
        }
    }

    fn render_ui_reading(field: &str) -> SExpression {
        arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = orbital_core_base::schema::SObject::new();
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
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
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
                    holds: None,
                }],
                events: None,
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(effects),
                    ensures: None,
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            source_declaration: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
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

    fn render_ui_reading_slot(field: &str, slot: &str) -> SExpression {
        arr(vec![
            s("render-ui"),
            s(slot),
            SExpression::Object({
                let mut m = orbital_core_base::schema::SObject::new();
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
        let t1 = trait_with_init_effects(
            "Reader1",
            "Pool",
            vec![render_ui_reading_slot("hp", "main")],
        );
        let t2 = trait_with_init_effects(
            "Reader2",
            "Pool",
            vec![render_ui_reading_slot("hp", "sidebar")],
        );
        let traits = vec![&t1, &t2];
        let mut defs = HashMap::new();
        defs.insert("Pool".to_string(), entity("Pool", true, None));
        let result = validate_orbital_entity_mutations(&traits, "orbitals[0]", &defs);
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
                let mut m = orbital_core_base::schema::SObject::new();
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

    // ---- C: local trait dispatch contract ----------------------------------
}
