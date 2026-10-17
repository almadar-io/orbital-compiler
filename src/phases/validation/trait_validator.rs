//! Trait Validation
//!
//! Validates trait references, categories, state machines, and transitions.
//! Ported from `builder/packages/shared/src/orbitals/validation/trait-validator.ts`.

use super::{
    is_std_trait, render_ui::is_render_ui_effect, trait_path, ErrorCode, ValidationContext,
    ValidationError, ValidationResult, VALID_TRAIT_CATEGORIES,
};
use orbital_core_base::schema::{
    EntityDefinition, Orbital, OrbitalSchema, Persistence, SExpression, StateMachine,
    StringOrArray, TraitCategory, TraitDefinition, TraitReference, TransitionDefinition,
};
use std::collections::{HashMap, HashSet};

/// Validate all traits in a schema
pub fn validate_all_traits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate trait references in orbitals
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let path = trait_path(i, &orbital.name, j, trait_ref);
            let result =
                validate_trait_reference(trait_ref, &path, &ctx.trait_names, &ctx.entity_names);
            ctx.merge(result);
        }
    }

    // Validate `events` rename maps on surviving trait references against the
    // referenced trait's known event set. The inline phase raises
    // `InlineError::EventRenameUnknownKey` for `uses`-resolved references, but a
    // reference that survives inline unchanged (no `ref:` string, or an
    // un-inlined named reference) carries an unchecked `events` map — raw `.orb`
    // / agent output can author a bogus rename key here. This pass closes that
    // gap with `ErrorCode::EventRenameUnknownKey`.
    let trait_events = build_trait_event_sets(schema);
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let path = trait_path(i, &orbital.name, j, trait_ref);
            ctx.merge(validate_event_renames(trait_ref, &path, &trait_events));
        }
    }

    // Check persistent entities read by interaction traits are fetched on INIT
    // by at least one trait in the orbital (entity-scoped, not per-trait: the
    // datasource idiom loads an entity once; sibling render traits are pure
    // consumers and must not each carry a duplicate fetch).
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let result = validate_orbital_init_fetch(orbital, i, &ctx.entity_definitions);
        ctx.merge(result);
    }

    // Enforce `@rebindable`: a trait imported from an atom and rebound to a
    // different entity via `linkedEntity` is only legal when the atom's
    // binding opted in (`-> @rebindable Entity`); the rebind target must also
    // provide the trait's contract `requires` fields. Runs on the resolved
    // (post-inline) schema where the marker + contract survive on the cloned
    // Inline trait and `source_entity_definition` holds the atom's original
    // entity. See docs/Almadar_Rabit_Gaps.md (RABIT-STD-1 follow-up).
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_entity_rebind(
                    trait_def,
                    &path,
                    &ctx.entity_definitions,
                    &ctx.entity_names,
                );
                ctx.merge(result);
            }
        }
    }

    // Validate trait uniqueness across orbitals
    let uniqueness_result = validate_trait_uniqueness(schema);
    ctx.merge(uniqueness_result);
}

/// Enforce the `@rebindable` opt-in + inferred field contract on a resolved
/// (post-inline) trait. A trait is a "rebind" when it was imported from an
/// atom (`source_behavior` set) and its `linked_entity` differs from the
/// atom's own entity (`source_entity_definition.name`). Only then do the
/// rules apply:
///   0. the target must actually name an entity — primary or auxiliary,
///      this orbital or a sibling's — somewhere in the schema (else
///      `ORB_T_REBIND_TARGET_UNRESOLVED`);
///   1. the atom's binding must be `entity_rebindable` (else `ORB_T_ENTITY_NOT_REBINDABLE`);
///   2. the target entity must have every field in `entity_contract.requires`
///      (else `ORB_T_REBIND_MISSING_FIELDS`). `provides` are NOT required —
///      the trait provisions those itself.
fn validate_entity_rebind(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    entity_names: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Only imported traits can be rebinds. A trait authored directly on the
    // orbital (no source_behavior) declaring `linkedEntity` is a definition,
    // not a rebind, and is exempt.
    let Some(source) = &trait_def.source_behavior else {
        return result;
    };
    let Some(linked) = &trait_def.linked_entity else {
        return result;
    };
    let source_entity = trait_def
        .source_entity_definition
        .as_ref()
        .map(|e| e.name.as_str());
    let is_rebind = source_entity.map(|n| n != linked.as_str()).unwrap_or(false);
    if !is_rebind {
        return result;
    }

    // §52 hard stop, before anything else: the rebind target must actually
    // exist somewhere in the schema. `entity_names` is the SAME schema-wide
    // existence set (built by `collect_names`, primary + Gap #22 auxiliary,
    // every orbital) the inline-phase merge's own schema-wide resolver
    // (`phases::inline::entity::merge_into_named_entity`) searches — a name
    // absent from both has no field-merge landing site and never will.
    // Retired 2026-09-19: this used to fall back to silently merging onto
    // the caller's own primary entity instead of erroring.
    if !entity_names.contains(linked.as_str()) {
        result.add_error(
            ValidationError::error(
                ErrorCode::RebindTargetUnresolved,
                format!("{}.linkedEntity", path),
                format!(
                    "Trait '{}' (from {}) rebinds to '{}', which names no primary or auxiliary entity anywhere in this schema; the merge has no target to land its fields on.",
                    trait_def.name, source.behavior, linked
                ),
            )
            .with_suggestion(
                "Fix the linkedEntity rebind target's spelling, or declare/import the entity it should name.",
            ),
        );
        return result;
    }

    if trait_def.entity_rebindable != Some(true) {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitEntityNotRebindable,
                format!("{}.linkedEntity", path),
                format!(
                    "Trait '{}' (from {}) has a fixed entity binding and cannot be rebound to '{}'; the atom's binding is not marked `@rebindable`.",
                    trait_def.name, source.behavior, linked
                ),
            )
            .with_suggestion(
                "Mark the atom trait's binding `-> @rebindable Entity` in its .lolo source, or remove the linkedEntity rebind.",
            ),
        );
        return result;
    }

    if let Some(contract) = &trait_def.entity_contract {
        if !contract.requires.is_empty() {
            if let Some(target) = entity_definitions.get(linked.as_str()) {
                let have: HashSet<&str> = target
                    .fields
                    .iter()
                    .filter_map(|f| f.name.as_deref())
                    .collect();
                let missing: Vec<String> = contract
                    .requires
                    .iter()
                    .filter(|r| !have.contains(r.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::RebindMissingEntityFields,
                            format!("{}.linkedEntity", path),
                            format!(
                                "Entity '{}' is missing fields required by trait '{}': {}",
                                linked,
                                trait_def.name,
                                missing.join(", ")
                            ),
                        )
                        .with_suggestion(
                            "Add the missing fields to the entity, or rebind to an entity that already has them.",
                        ),
                    );
                }
            }
        }
    }

    result
}

/// Validate a trait reference (inline or external)
pub fn validate_trait_reference(
    trait_ref: &TraitReference,
    path: &str,
    known_traits: &HashSet<String>,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match trait_ref {
        TraitReference::Inline(trait_def) => {
            // Validate inline trait definition
            result.merge(validate_trait_definition(trait_def, path, known_entities));
        }
        TraitReference::Reference {
            reference,
            name,
            linked_entity,
            ..
        } => {
            // Get the trait name from reference or name field
            let trait_name = reference.as_ref().or(name.as_ref());

            if let Some(name) = trait_name {
                // Check if it's a known trait (custom or std library)
                if !known_traits.contains(name) && !is_std_trait(name) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitUndefined,
                            path,
                            format!("Trait '{}' is not defined", name),
                        )
                        .with_suggestion(
                            "Define traits inline within orbitals or use a std library trait",
                        ),
                    );
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidFormat,
                        path,
                        "Trait reference must have 'ref' or 'name' property",
                    )
                    .with_suggestion("Add ref: 'TraitName' to reference a defined trait"),
                );
            }

            // Validate linked entity if specified
            if let Some(linked) = linked_entity {
                if !known_entities.contains(linked) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EntityInvalidRelation,
                            format!("{}.linkedEntity", path),
                            format!("Linked entity '{}' does not exist", linked),
                        )
                        .with_suggestion("Ensure the linked entity is defined in the schema"),
                    );
                }
            }
        }
    }

    result
}

/// Build a `trait name -> declared event-key set` map from every INLINE trait in
/// the schema. The event set is the union of declared `emits`, transition
/// triggers, and `state_machine.events[].key` — the same union the inline phase
/// validates rename source keys against.
fn build_trait_event_sets(schema: &OrbitalSchema) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let entry = map.entry(trait_def.name.clone()).or_default();
                if let Some(emits) = &trait_def.emits {
                    for e in emits {
                        entry.insert(e.event.clone());
                    }
                }
                for t in &trait_def.state_machine.transitions {
                    entry.insert(t.event.clone());
                }
                if let Some(events) = &trait_def.state_machine.events {
                    for e in events {
                        entry.insert(e.key.clone());
                    }
                }
                // Effect emit-maps + render-ui event props — the same union the
                // inline phase's RULE 3 checks rename source keys against, so
                // the two checks can never disagree on what counts as known.
                entry.extend(super::cross_orbital::collect_effect_produced_events(
                    trait_def,
                ));
            }
        }
    }
    map
}

/// Validate the `events` rename map on a `TraitReference::Reference` against the
/// referenced trait's known event set. Each `from` key that names no declared
/// event silently no-ops the rename at runtime — surface it as
/// `ORB_T_EVENT_RENAME_UNKNOWN_KEY` instead. Skips silently when the referenced
/// trait's event set is unknown (external / std trait not present inline in this
/// schema) — those are covered by the inline phase when resolved.
fn validate_event_renames(
    trait_ref: &TraitReference,
    path: &str,
    trait_events: &HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let TraitReference::Reference {
        reference,
        name,
        events: Some(events),
        ..
    } = trait_ref
    else {
        return result;
    };

    // Resolve the referenced trait name: the `ref:`'s trailing `.Member`
    // segment, else the `name` field.
    let target_name = reference
        .as_deref()
        .and_then(|r| r.rsplit('.').next())
        .or(name.as_deref());
    let Some(target_name) = target_name else {
        return result;
    };
    let Some(known) = trait_events.get(target_name) else {
        // Unknown target event set (external/std) — defer to the inline phase.
        return result;
    };

    for from in events.keys() {
        if from.is_empty() {
            continue;
        }
        if !known.contains(from) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EventRenameUnknownKey,
                    path,
                    format!(
                        "events rename key '{}' does not name any event declared by trait '{}';                          the rename silently no-ops.",
                        from, target_name
                    ),
                )
                .with_suggestion(
                    "Use an event key the referenced trait declares (an emit, a transition                      trigger, or a stateMachine.events entry).",
                ),
            );
        }
    }

    result
}

/// Validate a full trait definition
pub fn validate_trait_definition(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Trait must have a name
    if trait_def.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitUndefined,
                format!("{}.name", path),
                "Trait name is required",
            )
            .with_suggestion("Add a descriptive name for the trait"),
        );
    }

    // Validate trait category if specified
    if let Some(category) = &trait_def.category {
        let category_str = category_to_string(category);
        if !VALID_TRAIT_CATEGORIES.contains(&category_str.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitInvalidCategory,
                    format!("{}.category", path),
                    format!("Invalid trait category: '{}'", category_str),
                )
                .with_valid_values(
                    VALID_TRAIT_CATEGORIES
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
            );
        }
    }

    // Validate linked entity if specified
    if let Some(linked) = &trait_def.linked_entity {
        if !known_entities.contains(linked) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidRelation,
                    format!("{}.linkedEntity", path),
                    format!("Linked entity '{}' does not exist", linked),
                )
                .with_suggestion("Ensure the linked entity is defined in the schema"),
            );
        }
    }

    // Validate state machine
    let sm_result = validate_state_machine(&trait_def.state_machine, path, trait_def);
    result.merge(sm_result);

    // Validate render_ui effects for interaction traits
    if is_interaction_trait(trait_def) {
        let has_render_ui = has_render_ui_effects(&trait_def.state_machine);
        if !has_render_ui {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitMissingRenderUI,
                    path,
                    format!(
                        "Interaction trait '{}' should have render_ui effects",
                        trait_def.name
                    ),
                )
                .with_suggestion("Add render_ui effects to transitions to render UI components"),
            );
        }
    }

    // Validate `trait`-typed config field defaults — must be `@trait.<TraitName>`
    // strings, or `Null` (the reserved "unbound" value — owner ruling
    // 2026-09-13: a `trait`-typed config knob may be `none`, which lowers to
    // `Null`; see `canonicalize_unbound_trait_default`/
    // `canonicalize_unbound_trait_override`). Catches malformed defaults at
    // the field declaration so authors get a precise error rather than a
    // downstream `ORB_RUI_INVALID_PATTERN` failure when the config flows
    // into render-ui children.
    if let Some(config) = &trait_def.config {
        for (field_name, cf) in config.iter() {
            if cf.field_type != "trait" {
                continue;
            }
            let Some(default) = &cf.default else { continue };
            match default {
                SExpression::Null => {}
                SExpression::Object(_)
                    if orbital_core_base::schema::behavior_value::is_trait_value(
                        &orbital_core_base::value::Value::from(default.clone()),
                    ) => {}
                SExpression::String(s) => {
                    if !is_trait_ref_string(s) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::BindingTraitInvalidFormat,
                                format!("{}.config.{}.default", path, field_name),
                                format!(
                                    "trait-typed config field '{}' has invalid default '{}' — must be `@trait.<TraitName>` or `none`",
                                    field_name, s
                                ),
                            )
                            .with_suggestion(
                                "Set the default to a `@trait.PascalCaseName` reference, to `none` (unbound), or remove the default and require the consumer to override.",
                            ),
                        );
                    }
                }
                _ => {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingTraitInvalidFormat,
                            format!("{}.config.{}.default", path, field_name),
                            format!(
                                "trait-typed config field '{}' default must be a string `@trait.<TraitName>` reference, a trait value, or `none`",
                                field_name
                            ),
                        ),
                    );
                }
            }
        }
    }

    result.merge(validate_unbound_trait_knob_embeds(trait_def, path));

    result
}

/// `ORB_RENDER_UNBOUND_TRAIT_KNOB_EMBEDDED`: a render-ui payload embeds
/// `@config.<knob>` (as a child, or inside a `children` entry) where `<knob>`
/// is a `trait`-typed config field whose resolved value at this call site is
/// `none` (`Null` — see `canonicalize_unbound_trait_default`), unless the
/// render sits in the THEN-branch of a `(if @config.<knob> ...)` guard in the
/// same effect list — structurally: the condition is exactly that knob's own
/// binding string, `@config.<knob>`. An unguarded embed of an unbound trait
/// renders nothing (a dangling `@config.<knob>` string reaches the render
/// substrate, per the closed-circuit rule that no wire may terminate in
/// nothing) — the fix is the same `(if @config.<knob> (render-ui ...))`
/// wrap `tools/almadar-pattern-sync/src/commands/lolo-service.ts` generates.
fn validate_unbound_trait_knob_embeds(trait_def: &TraitDefinition, path: &str) -> ValidationResult {
    let unbound_knobs: HashSet<&str> = trait_def
        .config
        .as_ref()
        .map(|config| {
            config
                .iter()
                .filter(|(_, cf)| {
                    cf.field_type == "trait" && matches!(cf.default, Some(SExpression::Null))
                })
                .map(|(name, _)| name.as_str())
                .collect()
        })
        .unwrap_or_default();
    validate_unbound_trait_embeds(trait_def, path, "@config.", &unbound_knobs)
}

/// The entity-field twin of [`validate_unbound_trait_knob_embeds`]: a
/// `trait`- or `behavior`-typed entity field can hold no value (`none`, or a row never
/// bound), so every render-ui embed of `@entity.<field>` must sit under
/// `(if @entity.<field> …)`.
pub(crate) fn validate_unbound_trait_field_embeds(
    trait_def: &TraitDefinition,
    entity: &orbital_core_base::schema::EntityDefinition,
    path: &str,
) -> ValidationResult {
    let trait_fields: HashSet<&str> = entity
        .fields
        .iter()
        .filter(|f| {
            matches!(
                f.field_type,
                orbital_core_base::schema::FieldType::Trait | orbital_core_base::schema::FieldType::Behavior
            )
        })
        .filter_map(|f| f.name.as_deref())
        .collect();
    validate_unbound_trait_embeds(trait_def, path, "@entity.", &trait_fields)
}

/// `ORB_RENDER_ORBITAL_VALUE_EMBEDDED`: a render-ui payload embeds
/// `@entity.<field>` where `<field>` is `orbital`-typed. An orbital value is an
/// import body held as data; only a trait value renders, so no guard makes the
/// embed legal.
pub(crate) fn validate_orbital_field_embeds(
    trait_def: &TraitDefinition,
    entity: &orbital_core_base::schema::EntityDefinition,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let orbital_fields: HashSet<&str> = entity
        .fields
        .iter()
        .filter(|f| f.field_type == orbital_core_base::schema::FieldType::Orbital)
        .filter_map(|f| f.name.as_deref())
        .collect();
    if orbital_fields.is_empty() {
        return result;
    }

    fn scan(
        expr: &SExpression,
        fields: &HashSet<&str>,
        effect_path: &str,
        result: &mut ValidationResult,
    ) {
        match expr {
            SExpression::String(s) => {
                if let Some(field) = s.strip_prefix("@entity.") {
                    if fields.contains(field) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::RenderOrbitalValueEmbedded,
                                effect_path,
                                format!("render-ui embeds '@entity.{field}', an orbital value — only trait values render"),
                            )
                            .with_suggestion(
                                "Bind a trait value to render, or evaluate the orbital value into a program with `program/eval`",
                            ),
                        );
                    }
                }
            }
            SExpression::Array(arr) => arr
                .iter()
                .for_each(|item| scan(item, fields, effect_path, result)),
            SExpression::Object(obj) => obj
                .values()
                .for_each(|v| scan(v, fields, effect_path, result)),
            _ => {}
        }
    }

    fn walk(
        expr: &SExpression,
        fields: &HashSet<&str>,
        effect_path: &str,
        result: &mut ValidationResult,
    ) {
        let SExpression::Array(arr) = expr else {
            return;
        };
        if is_render_ui_effect(expr) {
            arr.iter()
                .for_each(|item| scan(item, fields, effect_path, result));
            return;
        }
        arr.iter()
            .for_each(|item| walk(item, fields, effect_path, result));
    }

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let Some(effects) = &transition.effects else {
            continue;
        };
        for (j, effect) in effects.iter().enumerate() {
            let effect_path = format!("{path}.stateMachine.transitions[{i}].effects[{j}]");
            walk(effect, &orbital_fields, &effect_path, &mut result);
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{path}.ticks[{i}].effects[{j}]");
                walk(effect, &orbital_fields, &effect_path, &mut result);
            }
        }
    }
    result
}

fn validate_unbound_trait_embeds(
    trait_def: &TraitDefinition,
    path: &str,
    prefix: &'static str,
    unbound_knobs: &HashSet<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if unbound_knobs.is_empty() {
        return result;
    }

    fn walk(
        expr: &SExpression,
        prefix: &'static str,
        unbound_knobs: &HashSet<&str>,
        effect_path: &str,
        result: &mut ValidationResult,
    ) {
        let SExpression::Array(arr) = expr else {
            return;
        };
        // The sanctioned guard shapes: `["if", "@config.<knob>", then, else]`
        // and the one-armed effect `["when", "@config.<knob>", then]`, where
        // `<knob>` is one of this trait's unbound-eligible knobs. The
        // then-branch is exempt for THAT knob (still walked, so an unrelated
        // knob embedded inside it is still caught); the else-branch (if any)
        // is walked normally, same as any other node.
        // The condition is the value's own binding, or its explicit presence test `(validate/required <binding>)`.
        let presence = |cond: &SExpression| -> Option<String> {
            match cond {
                SExpression::String(s) => Some(s.clone()),
                SExpression::Array(call) => match call.as_slice() {
                    [SExpression::String(op), SExpression::String(s)] if op == "validate/required" => {
                        Some(s.clone())
                    }
                    _ => None,
                },
                _ => None,
            }
        };
        if let [SExpression::String(op), cond_expr, then_branch, rest @ ..] = arr.as_slice() {
            let cond = presence(cond_expr).unwrap_or_default();
            if op == "if" || op == "when" {
                if let Some(knob) = cond.strip_prefix(prefix) {
                    if unbound_knobs.contains(knob) {
                        // Exempt ONLY this knob for the then-branch — an
                        // unrelated unbound knob embedded inside it must
                        // still be caught.
                        let mut exempt = unbound_knobs.clone();
                        exempt.remove(knob);
                        walk(then_branch, prefix, &exempt, effect_path, result);
                        if let Some(else_branch) = rest.first() {
                            walk(else_branch, prefix, unbound_knobs, effect_path, result);
                        }
                        return;
                    }
                }
            }
        }
        if is_render_ui_effect(expr) {
            for item in arr {
                check_render_payload_for_unbound_embeds(
                    item,
                    prefix,
                    unbound_knobs,
                    effect_path,
                    result,
                );
            }
            return;
        }
        for item in arr {
            walk(item, prefix, unbound_knobs, effect_path, result);
        }
    }

    /// Once inside a render-ui effect's own subtree (not gated by the
    /// sanctioned `if` guard above), any `@config.<knob>` string for an
    /// unbound-eligible knob is a dangling embed — flag it and keep
    /// descending so multiple embeds in one payload are all reported.
    fn check_render_payload_for_unbound_embeds(
        expr: &SExpression,
        prefix: &'static str,
        unbound_knobs: &HashSet<&str>,
        effect_path: &str,
        result: &mut ValidationResult,
    ) {
        match expr {
            SExpression::String(s) => {
                if let Some(knob) = s.strip_prefix(prefix) {
                    if unbound_knobs.contains(knob) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::RenderUnboundTraitKnobEmbedded,
                                effect_path,
                                format!(
                                    "render-ui embeds '{}{}', a trait- or behavior-typed value that may be `none` (unbound) — an unguarded embed \
                                     renders nothing",
                                    prefix, knob
                                ),
                            )
                            .with_suggestion(format!(
                                "Wrap this render-ui in `(if {}{} (render-ui ...))` so the \
                                 unbound case renders nothing instead of a dangling embed",
                                prefix, knob
                            )),
                        );
                    }
                }
            }
            SExpression::Array(arr) => {
                // An in-payload `if`/`when` condition picks a branch and is never
                // embedded; a condition that is exactly the knob guards its
                // then-branch for that knob, as the effect-level guard does.
                if let [SExpression::String(op), cond, then_branch, rest @ ..] = arr.as_slice() {
                    if op == "if" || op == "when" {
                        let guarded = match cond {
                            SExpression::String(c) => {
                                c.strip_prefix(prefix).filter(|k| unbound_knobs.contains(k))
                            }
                            _ => None,
                        };
                        let mut exempt = unbound_knobs.clone();
                        if let Some(knob) = guarded {
                            exempt.remove(knob);
                        }
                        check_render_payload_for_unbound_embeds(
                            then_branch,
                            prefix,
                            &exempt,
                            effect_path,
                            result,
                        );
                        for else_branch in rest {
                            check_render_payload_for_unbound_embeds(
                                else_branch,
                                prefix,
                                unbound_knobs,
                                effect_path,
                                result,
                            );
                        }
                        return;
                    }
                }
                for item in arr {
                    check_render_payload_for_unbound_embeds(
                        item,
                        prefix,
                        unbound_knobs,
                        effect_path,
                        result,
                    );
                }
            }
            SExpression::Object(obj) => {
                for (_, v) in obj {
                    check_render_payload_for_unbound_embeds(
                        v,
                        prefix,
                        unbound_knobs,
                        effect_path,
                        result,
                    );
                }
            }
            _ => {}
        }
    }

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let Some(effects) = &transition.effects else {
            continue;
        };
        for (j, effect) in effects.iter().enumerate() {
            let effect_path = format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
            walk(effect, prefix, unbound_knobs, &effect_path, &mut result);
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                walk(effect, prefix, unbound_knobs, &effect_path, &mut result);
            }
        }
    }

    result
}

/// `^@trait\.[A-Z][a-zA-Z0-9]*$` — accepted shape for trait references.
fn is_trait_ref_string(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("@trait.") else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Validate a state machine
pub fn validate_state_machine(
    sm: &StateMachine,
    path: &str,
    trait_def: &TraitDefinition,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let sm_path = format!("{}.stateMachine", path);

    // Must have at least one state
    if sm.states.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitNoInitialState,
                format!("{}.states", sm_path),
                "State machine must have at least one state",
            )
            .with_suggestion("Add at least one state to the state machine"),
        );
        return result;
    }

    // Collect state names and check for duplicates
    let mut state_names: HashSet<&str> = HashSet::new();
    let mut initial_states: Vec<&str> = Vec::new();

    for (i, state) in sm.states.iter().enumerate() {
        let state_path = format!("{}.states[{}]", sm_path, i);

        // Check for duplicate state names
        if !state_names.insert(&state.name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitDuplicateState,
                    state_path,
                    format!("Duplicate state name: '{}'", state.name),
                )
                .with_suggestion("Each state must have a unique name"),
            );
        }

        // Track initial states
        if state.is_initial == Some(true) {
            initial_states.push(&state.name);
        }
    }

    // Check for exactly one initial state
    if initial_states.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitNoInitialState,
                format!("{}.states", sm_path),
                "State machine must have an initial state",
            )
            .with_suggestion("Mark one state with isInitial: true"),
        );
    } else if initial_states.len() > 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitMultipleInitialStates,
                format!("{}.states", sm_path),
                format!("Multiple initial states found: {:?}", initial_states),
            )
            .with_suggestion("Only one state should be marked as initial"),
        );
    }

    // An event with no name can never be emitted, listened for or dispatched.
    let empty_events = sm
        .events
        .iter()
        .flatten()
        .enumerate()
        .filter(|(_, e)| e.key.is_empty())
        .map(|(i, _)| format!("{}.events[{}].key", sm_path, i))
        .chain(
            trait_def
                .emits
                .iter()
                .flatten()
                .enumerate()
                .filter(|(_, e)| e.event.is_empty())
                .map(|(i, _)| format!("{}.emits[{}].event", path, i)),
        )
        .chain(
            sm.transitions
                .iter()
                .enumerate()
                .filter(|(_, t)| t.event.is_empty())
                .map(|(i, _)| format!("{}.transitions[{}].event", sm_path, i)),
        );
    for empty_path in empty_events {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitEmptyEventName,
                empty_path,
                "Event name is empty",
            )
            .with_suggestion(
                "Name the event, or remove it; an unset optional event knob declares no event",
            ),
        );
    }

    // Collect event names (using key field)
    let event_names: HashSet<&str> = sm
        .events
        .as_ref()
        .map(|events| events.iter().map(|e| e.key.as_str()).collect())
        .unwrap_or_default();

    // Validate transitions
    let transitions_result =
        validate_transitions(&sm.transitions, &sm_path, &state_names, &event_names);
    result.merge(transitions_result);

    // Check for INIT transition on initial state (for interaction traits)
    if is_interaction_trait(trait_def) && !initial_states.is_empty() {
        let has_init = sm
            .transitions
            .iter()
            .any(|t| t.event == "INIT" && matches_state(&t.from, initial_states[0]));

        if !has_init {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitMissingInitTransition,
                    format!("{}.transitions", sm_path),
                    format!(
                        "Interaction trait should have INIT transition on initial state '{}'",
                        initial_states[0]
                    ),
                )
                .with_suggestion("Add a transition: { from: 'initialState', to: 'initialState', event: 'INIT', effects: [...] }"),
            );
        }
    }

    // A declared external input is a complete command: the trait accepts it
    // from its initial (resting) state, so an outside client never has to
    // drive a UI-shaped flow to reach it.
    if let (Some(events), Some(initial)) = (sm.events.as_ref(), initial_states.first()) {
        for (i, ev) in events.iter().enumerate() {
            if ev.external != Some(true) {
                continue;
            }
            let handled = sm
                .transitions
                .iter()
                .any(|t| t.event == ev.key && matches_state(&t.from, initial));
            if !handled {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitExternalInputNotResting,
                        format!("{}.events[{}]", sm_path, i),
                        format!(
                            "External input '{}' is not accepted from the initial state '{}'",
                            ev.key, initial
                        ),
                    )
                    .with_suggestion(
                        "Declare as external only a complete command the trait handles from its initial state (e.g. a persistor's DO_CREATE), or add that transition",
                    ),
                );
            }
        }
    }

    // Reachability: every declared state must be reachable from the initial
    // state via some transition path. Mirrors the verifier's BFS exclusions —
    // skip `from == "*"` wildcards (they don't define real topology edges) and
    // skip the boot `INIT` self-loop on the initial state. Dead states surface
    // only as a generic "uncovered" in the verifier today; catch them here.
    if !initial_states.is_empty() {
        let initial = initial_states[0];
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for transition in &sm.transitions {
            // Skip ONLY the boot INIT *self-loop* on the initial state (it adds
            // no reachability). A `loading --INIT--> loaded` boot transition that
            // moves to a DIFFERENT state IS a real auto-fired reachability edge
            // (the runtime fires INIT on mount), so it must be followed — the
            // walker skips re-firing it, but for reachability `loaded` is reached.
            if transition.event == "INIT"
                && matches_state(&transition.from, initial)
                && transition.to.as_str() == initial
            {
                continue;
            }
            let froms: Vec<&str> = match &transition.from {
                StringOrArray::Single(s) => vec![s.as_str()],
                StringOrArray::Multiple(states) => states.iter().map(|s| s.as_str()).collect(),
            };
            for from in froms {
                if from == "*" {
                    continue;
                }
                adjacency
                    .entry(from)
                    .or_default()
                    .push(transition.to.as_str());
            }
        }

        let mut reached: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&str> = vec![initial];
        reached.insert(initial);
        while let Some(state) = queue.pop() {
            if let Some(targets) = adjacency.get(state) {
                for &target in targets {
                    if reached.insert(target) {
                        queue.push(target);
                    }
                }
            }
        }

        for state in &sm.states {
            if !reached.contains(state.name.as_str()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitStateUnreachable,
                        format!("{}.states", sm_path),
                        format!(
                            "State '{}' is unreachable from the initial state '{}' via any transition path",
                            state.name, initial
                        ),
                    )
                    .with_suggestion(
                        "Add a transition that reaches this state, or remove the dead state",
                    ),
                );
            }
        }
    }

    result
}

/// Validate transitions
pub fn validate_transitions(
    transitions: &[TransitionDefinition],
    path: &str,
    state_names: &HashSet<&str>,
    event_names: &HashSet<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Track seen transitions for duplicate detection
    let mut seen_transitions: HashMap<(&str, &str), Vec<usize>> = HashMap::new();

    for (i, transition) in transitions.iter().enumerate() {
        let trans_path = format!("{}.transitions[{}]", path, i);

        // Validate 'from' states
        match &transition.from {
            StringOrArray::Single(state) => {
                if state != "*" && !state_names.contains(state.as_str()) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitInvalidTransition,
                            format!("{}.from", trans_path),
                            format!("Source state '{}' does not exist", state),
                        )
                        .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
                    );
                }

                // Track for duplicate detection
                let key = (state.as_str(), transition.event.as_str());
                seen_transitions.entry(key).or_default().push(i);
            }
            StringOrArray::Multiple(states) => {
                for state in states {
                    if state != "*" && !state_names.contains(state.as_str()) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidTransition,
                                format!("{}.from", trans_path),
                                format!("Source state '{}' does not exist", state),
                            )
                            .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
                        );
                    }
                }
            }
        }

        // Validate 'to' state
        if !state_names.contains(transition.to.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitInvalidTransition,
                    format!("{}.to", trans_path),
                    format!("Target state '{}' does not exist", transition.to),
                )
                .with_valid_values(state_names.iter().map(|s| s.to_string()).collect()),
            );
        }

        // Validate event is defined (if events are declared)
        if !event_names.is_empty()
            && transition.event != "INIT"
            && !event_names.contains(transition.event.as_str())
        {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitUndefinedEvent,
                    format!("{}.event", trans_path),
                    format!(
                        "Event '{}' is not defined in events array",
                        transition.event
                    ),
                )
                .with_suggestion("Add the event to the stateMachine.events array"),
            );
        }
    }

    // Check for duplicate transitions (same from/event without guards)
    for ((from, event), indices) in seen_transitions {
        if indices.len() > 1 {
            // Check if all have guards
            let guarded: Vec<_> = indices
                .iter()
                .filter(|&&i| transitions[i].guard.is_some())
                .collect();

            if guarded.len() < indices.len() - 1 {
                // More than one unguarded transition
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitDuplicateTransition,
                        format!("{}.transitions", path),
                        format!(
                            "Duplicate transitions from '{}' on event '{}' without guards",
                            from, event
                        ),
                    )
                    .with_suggestion("Add guard conditions to distinguish transitions"),
                );
            } else if indices.iter().enumerate().any(|(n, &a)| {
                indices[n + 1..].iter().any(|&b| {
                    transitions[a].guard.is_some() && transitions[a].guard == transitions[b].guard
                })
            }) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitDuplicateTransition,
                        format!("{}.transitions", path),
                        format!(
                            "Duplicate transitions from '{}' on event '{}' with the same guard",
                            from, event
                        ),
                    )
                    .with_suggestion("Remove the repeated arm, or give each arm its own guard"),
                );
            }
        }
    }

    result
}

/// Validate trait uniqueness across orbitals
pub fn validate_trait_uniqueness(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut trait_to_orbital: HashMap<&str, Vec<&str>> = HashMap::new();

    // Collect all inline trait names with their orbitals
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                trait_to_orbital
                    .entry(&trait_def.name)
                    .or_default()
                    .push(&orbital.name);
            }
        }
    }

    // Check for duplicates (same trait name in multiple orbitals)
    for (trait_name, orbitals) in trait_to_orbital {
        if orbitals.len() > 1 {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitDuplicateName,
                    "orbitals",
                    format!(
                        "Trait '{}' is defined in multiple orbitals: {:?}",
                        trait_name, orbitals
                    ),
                )
                .with_suggestion(
                    "Use unique trait names per orbital or define traits inline within orbitals",
                ),
            );
        }
    }

    result
}

/// Check if a trait is an interaction trait
fn is_interaction_trait(trait_def: &TraitDefinition) -> bool {
    match &trait_def.category {
        Some(TraitCategory::Interaction) => true,
        None => true, // Default is interaction
        _ => false,
    }
}

/// Check if state machine has render_ui effects. Looks through `if`
/// wrappers — a conditional whose branches render still renders.

fn has_render_ui_effects(sm: &StateMachine) -> bool {
    fn renders(expr: &SExpression) -> bool {
        if is_render_ui_effect(expr) {
            return true;
        }
        if let SExpression::Array(arr) = expr {
            if let Some(SExpression::String(op)) = arr.first() {
                if op == "if" {
                    return arr.iter().skip(2).any(renders);
                }
            }
        }
        false
    }
    for transition in &sm.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if renders(effect) {
                    return true;
                }
            }
        }
    }
    false
}

/// Entity names fetched (`["fetch", E]` / `["ref", E]`) by INIT transitions
/// leaving the state machine's initial state.
fn init_fetch_targets(sm: &StateMachine) -> Vec<&str> {
    let Some(initial) = sm
        .states
        .iter()
        .find(|s| s.is_initial.unwrap_or(false))
        .map(|s| s.name.as_str())
    else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for t in &sm.transitions {
        if t.event != "INIT" || !matches_state(&t.from, initial) {
            continue;
        }
        let Some(effs) = &t.effects else { continue };
        for eff in effs {
            if let SExpression::Array(arr) = eff {
                if arr.len() >= 2 {
                    if let (SExpression::String(op), SExpression::String(entity)) =
                        (&arr[0], &arr[1])
                    {
                        if op == "fetch" || op == "ref" {
                            targets.push(entity.as_str());
                        }
                    }
                }
            }
        }
    }
    targets
}

/// Whether `expr` holds an `@entity` binding — the whole row or any path into it.
fn mentions_entity_binding(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) => {
            s == "@entity" || s.starts_with("@entity.") || s.starts_with("@entity[")
        }
        SExpression::Array(items) => items.iter().any(mentions_entity_binding),
        SExpression::Object(map) => map.values().any(mentions_entity_binding),
        _ => false,
    }
}

/// Whether a trait reads or writes its bound entity anywhere: guards, effects, ticks.
fn trait_reads_its_entity(trait_def: &TraitDefinition) -> bool {
    let transitions = trait_def.state_machine.transitions.iter().any(|t| {
        t.guard.as_ref().is_some_and(mentions_entity_binding)
            || t.effects.iter().flatten().any(mentions_entity_binding)
    });
    let ticks = trait_def.ticks.iter().flatten().any(|tick| {
        tick.guard.as_ref().is_some_and(mentions_entity_binding)
            || tick.effects.iter().any(mentions_entity_binding)
    });
    transitions || ticks
}

/// ORB_T_INIT_MISSING_FETCH, scoped per (orbital, persistent entity): fires
/// once when interaction traits read a persistent entity but NO trait in the
/// orbital fetches it on INIT — i.e. the orbital has no datasource for data
/// its UI displays. A single fetch anywhere in the orbital satisfies every
/// consumer; requiring one per trait would mandate N duplicate fetches.
fn validate_orbital_init_fetch(
    orbital: &Orbital,
    orbital_index: usize,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut fetched: HashSet<&str> = HashSet::new();
    // (entity, first consumer trait index) in trait order → deterministic output
    let mut consumers: Vec<(&str, usize)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for (j, trait_ref) in orbital.traits.iter().enumerate() {
        let TraitReference::Inline(trait_def) = trait_ref else {
            continue;
        };
        for target in init_fetch_targets(&trait_def.state_machine) {
            fetched.insert(target);
        }
        if !is_interaction_trait(trait_def) {
            continue;
        }
        let Some(linked) = &trait_def.linked_entity else {
            continue;
        };
        if !trait_reads_its_entity(trait_def) {
            continue;
        }
        let is_persistent = entity_definitions
            .get(linked)
            .map(|e| matches!(e.persistence, None | Some(Persistence::Persistent)))
            .unwrap_or(false);
        if !is_persistent {
            continue;
        }
        // Same applicability as the original per-trait rule: only traits with
        // a resolvable initial state participate.
        let has_initial = trait_def
            .state_machine
            .states
            .iter()
            .any(|s| s.is_initial.unwrap_or(false));
        if !has_initial {
            continue;
        }
        if seen.insert(linked.as_str()) {
            consumers.push((linked.as_str(), j));
        }
    }

    for (entity, j) in consumers {
        if fetched.contains(entity) {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitInitMissingFetch,
                format!(
                    "orbitals[{}].traits[{}].stateMachine.transitions",
                    orbital_index, j
                ),
                format!(
                    "no trait in this orbital fetches persistent entity \"{}\" on INIT — interaction traits read it but nothing loads it",
                    entity
                ),
            )
            .with_suggestion(format!(
                "Add [\"fetch\", \"{}\"] to one datasource trait's INIT transition; sibling render traits read the loaded data and do not each need their own fetch",
                entity
            )),
        );
    }
    result
}

/// Check if a transition 'from' matches a state name
fn matches_state(from: &StringOrArray, state: &str) -> bool {
    match from {
        StringOrArray::Single(s) => s == state || s == "*",
        StringOrArray::Multiple(states) => states.iter().any(|s| s == state || s == "*"),
    }
}

/// Convert TraitCategory to string
fn category_to_string(category: &TraitCategory) -> String {
    match category {
        TraitCategory::Interaction => "interaction".to_string(),
        TraitCategory::Integration => "integration".to_string(),
        TraitCategory::Lifecycle => "lifecycle".to_string(),
        TraitCategory::GameCore => "game-core".to_string(),
        TraitCategory::GameEntity => "game-entity".to_string(),
        TraitCategory::GameUi => "game-ui".to_string(),
    }
}

#[cfg(test)]
mod empty_event_name_tests {
    use super::*;

    fn trait_with(events: serde_json::Value, emits: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Calendar",
            "emits": emits,
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": events,
                "transitions": []
            }
        }))
        .expect("trait deserializes")
    }

    fn codes(td: &TraitDefinition) -> Vec<String> {
        validate_state_machine(&td.state_machine, "traits[0]", td)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitEmptyEventName)
            .map(|e| e.path.clone())
            .collect()
    }

    #[test]
    fn an_empty_event_key_or_emit_name_is_an_error() {
        let td = trait_with(
            serde_json::json!([ { "key": "", "name": "on event click" } ]),
            serde_json::json!([ { "event": "", "scope": "external" } ]),
        );
        assert_eq!(
            codes(&td),
            vec![
                "traits[0].stateMachine.events[0].key",
                "traits[0].emits[0].event"
            ]
        );
    }

    #[test]
    fn control_named_events_pass() {
        let td = trait_with(
            serde_json::json!([ { "key": "OPEN", "name": "open" } ]),
            serde_json::json!([ { "event": "OPEN", "scope": "external" } ]),
        );
        assert!(codes(&td).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        EntityFieldContract, EventDefinition, FieldDefinition, FieldType, SourceBehaviorMetadata,
        StateDefinition,
    };

    fn create_valid_state_machine() -> StateMachine {
        StateMachine {
            states: vec![
                StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
                StateDefinition {
                    name: "active".to_string(),
                    is_initial: None,
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                },
            ],
            events: Some(vec![
                EventDefinition {
                    id: None,
                    key: "INIT".to_string(),
                    name: Some("Initialize".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    external: None,
                    listened: None,
                    inbound: None,
                },
                EventDefinition {
                    id: None,
                    key: "START".to_string(),
                    name: Some("Start".to_string()),
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    external: None,
                    listened: None,
                    inbound: None,
                },
            ]),
            transitions: vec![
                // INIT self-transition with a render-ui effect — required for
                // Interaction traits to pass the validator (TraitMissingInitTransition +
                // TraitMissingRenderUI errors fire otherwise).
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![orbital_core_base::schema::SExpression::Array(vec![
                        orbital_core_base::schema::SExpression::String("render-ui".to_string()),
                        orbital_core_base::schema::SExpression::String("main".to_string()),
                        orbital_core_base::schema::SExpression::Object({
                            let mut m = orbital_core_base::schema::SObject::new();
                            m.insert(
                                "type".to_string(),
                                orbital_core_base::schema::SExpression::String("box".to_string()),
                            );
                            m
                        }),
                    ])]),
                },
                TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "active".to_string(),
                    event: "START".to_string(),
                    guard: None,
                    effects: None,
                },
            ],
        }
    }

    fn create_valid_trait() -> TraitDefinition {
        TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "TestTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: Some(TraitCategory::Interaction),
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: create_valid_state_machine(),
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

    #[test]
    fn test_valid_trait() {
        let trait_def = create_valid_trait();
        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        // May have warnings but no errors
        assert!(
            result.errors.iter().all(|e| e.is_warning()),
            "Unexpected errors: {:?}",
            result.errors
        );
    }

    // =========================================================================
    // `trait`-typed config knob `none` (owner ruling 2026-09-13)
    // =========================================================================
    mod unbound_trait_knob {
        use super::*;
        use orbital_core_base::schema::ConfigField;

        fn s(v: &str) -> SExpression {
            SExpression::String(v.to_string())
        }

        fn obj(pairs: &[(&str, SExpression)]) -> SExpression {
            SExpression::Object(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            )
        }

        fn trait_typed_config_field(default: SExpression) -> ConfigField {
            let mut f: ConfigField = serde_json::from_value(serde_json::json!({ "type": "trait" }))
                .expect("config field fixture");
            f.default = Some(default);
            f
        }

        fn uitrait_render_effect() -> SExpression {
            SExpression::Array(vec![
                s("render-ui"),
                s("main"),
                obj(&[
                    ("type", s("stack")),
                    ("children", SExpression::Array(vec![s("@config.uiTrait")])),
                ]),
            ])
        }

        /// Adds a `SHOW -> idle` transition whose sole effect is `render_effect`
        /// to `create_valid_trait()`'s state machine, and sets its `config` to
        /// a single `uiTrait : trait` knob with the given default.
        fn trait_with_uitrait(default: SExpression, render_effect: SExpression) -> TraitDefinition {
            let mut trait_def = create_valid_trait();
            // Replace the fixture's INIT render-ui effect (transitions[0], the
            // idle->idle self-transition `create_valid_state_machine` already
            // declares) rather than adding a new event, so no extra
            // `stateMachine.events` entry is needed.
            trait_def.state_machine.transitions[0].effects = Some(vec![render_effect]);
            trait_def.config = Some(HashMap::from([(
                "uiTrait".to_string(),
                trait_typed_config_field(default),
            )]));
            trait_def
        }

        fn has_code(result: &ValidationResult, code: ErrorCode) -> bool {
            result.errors.iter().any(|e| e.code == code)
        }

        /// `uiTrait : trait = none` (`Null`) is a legal default — the
        /// trait-typed-default check (`ORB_BINDING_TRAIT_INVALID_FORMAT`) must
        /// not fire on it.
        #[test]
        fn null_default_is_a_valid_trait_typed_default() {
            let trait_def = trait_with_uitrait(SExpression::Null, uitrait_render_effect());
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::BindingTraitInvalidFormat),
                "Null must be accepted as the unbound sentinel: {:?}",
                result.errors
            );
        }

        /// An unbound (`none`/`Null`) trait knob embedded in a render-ui with
        /// no `(if @config.uiTrait ...)` guard renders nothing — must error.
        #[test]
        fn unguarded_embed_of_unbound_knob_errors() {
            let trait_def = trait_with_uitrait(SExpression::Null, uitrait_render_effect());
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "an unguarded embed of an unbound trait knob must error: {:?}",
                result.errors
            );
        }

        /// The same embed, wrapped in `(if @config.uiTrait (render-ui ...))`
        /// in the same effect list, is the sanctioned guard — silent.
        #[test]
        fn guarded_embed_of_unbound_knob_is_silent() {
            let guarded =
                SExpression::Array(vec![s("if"), s("@config.uiTrait"), uitrait_render_effect()]);
            let trait_def = trait_with_uitrait(SExpression::Null, guarded);
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "a guarded embed must not error: {:?}",
                result.errors
            );
        }

        fn render_with_prop(prop: SExpression) -> SExpression {
            SExpression::Array(vec![
                s("render-ui"),
                s("main"),
                obj(&[("type", s("chat-bar")), ("placeholder", prop)]),
            ])
        }

        fn render_with_children(child: SExpression) -> SExpression {
            SExpression::Array(vec![
                s("render-ui"),
                s("main"),
                obj(&[
                    ("type", s("stack")),
                    ("children", SExpression::Array(vec![child])),
                ]),
            ])
        }

        /// The knob as an in-payload `if` condition picks a value; it is never embedded.
        #[test]
        fn an_in_payload_condition_on_the_knob_is_silent() {
            let cond = SExpression::Array(vec![s("if"), s("@config.uiTrait"), s("Edit"), s("New")]);
            let trait_def = trait_with_uitrait(SExpression::Null, render_with_prop(cond));
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "{:?}",
                result.errors
            );
        }

        #[test]
        fn a_comparison_inside_an_in_payload_condition_is_silent() {
            let cmp = SExpression::Array(vec![s("="), s("@config.uiTrait"), SExpression::Null]);
            let cond = SExpression::Array(vec![s("if"), cmp, s("New"), s("Edit")]);
            let trait_def = trait_with_uitrait(SExpression::Null, render_with_prop(cond));
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "{:?}",
                result.errors
            );
        }

        #[test]
        fn an_in_payload_guard_exempts_its_then_branch() {
            let guarded = SExpression::Array(vec![
                s("if"),
                s("@config.uiTrait"),
                s("@config.uiTrait"),
                SExpression::Null,
            ]);
            let trait_def = trait_with_uitrait(SExpression::Null, render_with_children(guarded));
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "{:?}",
                result.errors
            );
        }

        #[test]
        fn control_the_else_branch_of_an_in_payload_guard_is_still_checked() {
            let inverted = SExpression::Array(vec![
                s("if"),
                s("@config.uiTrait"),
                SExpression::Null,
                s("@config.uiTrait"),
            ]);
            let trait_def = trait_with_uitrait(SExpression::Null, render_with_children(inverted));
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "{:?}",
                result.errors
            );
        }

        #[test]
        fn control_a_then_branch_under_another_condition_is_still_checked() {
            let other = SExpression::Array(vec![
                s("if"),
                s("@entity.ready"),
                s("@config.uiTrait"),
                SExpression::Null,
            ]);
            let trait_def = trait_with_uitrait(SExpression::Null, render_with_children(other));
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "{:?}",
                result.errors
            );
        }

        /// A BOUND trait knob (`@trait.DefaultForm`, never `none`) embedded
        /// with no guard is the ordinary, pre-existing shape — never flagged
        /// by the new rule regardless of guard.
        #[test]
        fn bound_knob_embed_is_never_flagged() {
            let trait_def = trait_with_uitrait(s("@trait.DefaultForm"), uitrait_render_effect());
            let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
            assert!(
                !has_code(&result, ErrorCode::RenderUnboundTraitKnobEmbedded),
                "a bound knob's embed must never be flagged: {:?}",
                result.errors
            );
        }
    }

    #[test]
    fn test_trait_no_initial_state() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states[0].is_initial = None;

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitNoInitialState));
    }

    #[test]
    fn test_trait_multiple_initial_states() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states[1].is_initial = Some(true);

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitMultipleInitialStates));
    }

    #[test]
    fn test_trait_state_unreachable_fires() {
        // 3 states: idle (initial) -> active via START; `orphan` has no
        // incoming transition, so it is unreachable from the initial state.
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states.push(StateDefinition {
            name: "orphan".to_string(),
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        });

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitStateUnreachable
                    && e.message.contains("orphan")),
            "Expected ORB_T_STATE_UNREACHABLE for 'orphan', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_trait_state_all_reachable_no_fire() {
        // idle (initial) -> active is fully reachable; no unreachable state.
        let trait_def = create_valid_trait();
        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitStateUnreachable),
            "Did not expect ORB_T_STATE_UNREACHABLE, got: {:?}",
            result.errors
        );
    }

    // ---- @rebindable enforcement (validate_entity_rebind) ----

    fn make_entity(name: &str, fields: &[&str]) -> EntityDefinition {
        EntityDefinition {
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
            name: name.to_string(),
            collection: None,
            persistence: None,
            fields: fields
                .iter()
                .map(|f| FieldDefinition {
                    mock: None,
                    merged_from: None,
                    name: Some(f.to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: None,
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                    projected_from: None,
                })
                .collect(),
            instances: None,
            id: None,
        }
    }

    /// A trait imported from std-browse (atom entity `BrowseItem`) and rebound
    /// to `Listing`.
    fn rebound_trait(rebindable: Option<bool>, requires: &[&str]) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.source_behavior = Some(SourceBehaviorMetadata {
            behavior: "std/behaviors/std-browse".to_string(),
            alias: "Browse".to_string(),
            original_name: "BrowseItemBrowse".to_string(),
        });
        t.source_entity_definition = Some(make_entity("BrowseItem", &[]));
        t.linked_entity = Some("Listing".to_string());
        t.entity_rebindable = rebindable;
        if !requires.is_empty() {
            t.entity_contract = Some(EntityFieldContract {
                requires: requires.iter().map(|s| s.to_string()).collect(),
                provides: vec![],
            });
        }
        t
    }

    /// The rebind target name alone, registered as existing in the schema —
    /// what the other rebind rules (rebindable / required-fields) assume so
    /// their own case is isolated from the new §52 existence check.
    fn listing_only_names() -> HashSet<String> {
        HashSet::from(["Listing".to_string()])
    }

    #[test]
    fn test_rebind_not_rebindable_rejected() {
        let t = rebound_trait(None, &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new(), &listing_only_names());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEntityNotRebindable));
    }

    #[test]
    fn test_rebind_rebindable_allowed() {
        let t = rebound_trait(Some(true), &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new(), &listing_only_names());
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_rebind_missing_required_field_rejected() {
        let t = rebound_trait(Some(true), &["sku"]);
        let mut defs = HashMap::new();
        defs.insert("Listing".to_string(), make_entity("Listing", &["title"])); // no `sku`
        let result = validate_entity_rebind(&t, "test", &defs, &listing_only_names());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RebindMissingEntityFields));
    }

    #[test]
    fn test_rebind_required_field_present_ok() {
        let t = rebound_trait(Some(true), &["sku"]);
        let mut defs = HashMap::new();
        defs.insert(
            "Listing".to_string(),
            make_entity("Listing", &["sku", "title"]),
        );
        let result = validate_entity_rebind(&t, "test", &defs, &listing_only_names());
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_inline_authored_binding_exempt() {
        // No source_behavior = authored directly on the orbital, not a rebind.
        let mut t = create_valid_trait();
        t.linked_entity = Some("Listing".to_string());
        t.entity_rebindable = None;
        let result = validate_entity_rebind(&t, "test", &HashMap::new(), &HashSet::new());
        assert!(result.errors.is_empty(), "Unexpected: {:?}", result.errors);
    }

    #[test]
    fn test_rebind_target_unresolved_when_name_not_in_schema() {
        // §52: `linkedEntity` names something that is not any orbital's
        // primary or auxiliary entity anywhere in the schema — the retired
        // fallback used to silently merge onto the caller's own primary
        // entity instead of reporting this.
        let t = rebound_trait(Some(true), &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new(), &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RebindTargetUnresolved));
        // Fires INSTEAD of (before) the rebindable/required-fields checks —
        // there is no target to check those against.
        assert!(!result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEntityNotRebindable));
    }

    #[test]
    fn test_rebind_target_present_in_schema_not_unresolved() {
        // Contrast/guard-rail: the SAME target name, registered as existing,
        // must not trip the new check (the exclusion above is scoped to a
        // truly absent name, not a blanket rejection of rebinds).
        let t = rebound_trait(Some(true), &[]);
        let result = validate_entity_rebind(&t, "test", &HashMap::new(), &listing_only_names());
        assert!(!result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RebindTargetUnresolved));
    }

    #[test]
    fn test_trait_duplicate_state() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.states.push(StateDefinition {
            name: "idle".to_string(), // Duplicate
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        });

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitDuplicateState));
    }

    #[test]
    fn test_trait_invalid_transition_target() {
        let mut trait_def = create_valid_trait();
        trait_def.state_machine.transitions[0].to = "nonexistent".to_string();

        let result = validate_trait_definition(&trait_def, "test", &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitInvalidTransition));
    }

    #[test]
    fn test_trait_undefined_reference() {
        let trait_ref = TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some("NonExistentTrait".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        };

        let result = validate_trait_reference(&trait_ref, "test", &HashSet::new(), &HashSet::new());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitUndefined));
    }

    #[test]
    fn test_std_trait_reference() {
        let trait_ref = TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: Some("std/Form".to_string()),
            name: None,
            linked_entity: None,
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        };

        let result = validate_trait_reference(&trait_ref, "test", &HashSet::new(), &HashSet::new());
        assert!(result.is_valid());
    }

    fn event_ref(
        reference: Option<&str>,
        name: Option<&str>,
        renames: &[(&str, &str)],
    ) -> TraitReference {
        let mut events = HashMap::new();
        for (k, v) in renames {
            events.insert(k.to_string(), v.to_string());
        }
        TraitReference::Reference {
            local: false,
            type_args: None,
            id: None,
            reference: reference.map(str::to_string),
            name: name.map(str::to_string),
            linked_entity: None,
            config: None,
            events: Some(events),
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        }
    }

    fn known_events() -> HashMap<String, HashSet<String>> {
        let mut m = HashMap::new();
        m.insert(
            "TestTrait".to_string(),
            ["INIT", "START"].into_iter().map(str::to_string).collect(),
        );
        m
    }

    #[test]
    fn test_event_rename_unknown_key_via_ref() {
        // `Alias.traits.TestTrait` resolves to TestTrait; BOGUS isn't declared.
        let r = event_ref(Some("Alias.traits.TestTrait"), None, &[("BOGUS", "X")]);
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EventRenameUnknownKey));
    }

    #[test]
    fn test_event_rename_unknown_key_via_name() {
        let r = event_ref(None, Some("TestTrait"), &[("NOPE", "Y")]);
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EventRenameUnknownKey));
    }

    #[test]
    fn test_event_rename_known_key_passes() {
        let r = event_ref(
            Some("Alias.traits.TestTrait"),
            None,
            &[("INIT", "ADD"), ("START", "GO")],
        );
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result.is_valid());
    }

    #[test]
    fn test_event_rename_unknown_target_trait_skips() {
        // Target trait not in the event-set map (external/std) — defer to inline.
        let r = event_ref(
            Some("Alias.traits.ExternalTrait"),
            None,
            &[("WHATEVER", "Z")],
        );
        let result = validate_event_renames(&r, "test", &known_events());
        assert!(result.is_valid());
    }

    // ---- entity-scoped INIT fetch (validate_orbital_init_fetch) ----

    fn make_orbital(traits: Vec<TraitDefinition>) -> Orbital {
        Orbital {
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: orbital_core_base::schema::EntityRef::Inline(make_entity("Item", &["id"])),
            auxiliary_entities: None,
            traits: traits.into_iter().map(TraitReference::Inline).collect(),
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            os_grants: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        }
    }

    fn with_render(mut t: TraitDefinition, prop: &str) -> TraitDefinition {
        let mut props = orbital_core_base::schema::SObject::new();
        props.insert("type".to_string(), SExpression::String("stack".to_string()));
        props.insert(
            "children".to_string(),
            SExpression::String(prop.to_string()),
        );
        t.state_machine.transitions[0]
            .effects
            .get_or_insert_with(Vec::new)
            .push(SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String("main".to_string()),
                SExpression::Object(props),
            ]));
        t
    }

    fn make_consumer(name: &str) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.name = name.to_string();
        t.linked_entity = Some("Item".to_string());
        with_render(t, "@entity.name")
    }

    fn init_fetch_errors(orbital: &Orbital) -> usize {
        validate_orbital_init_fetch(orbital, 0, &entity_defs())
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitInitMissingFetch)
            .count()
    }

    #[test]
    fn test_init_fetch_ignores_a_bound_trait_that_never_reads_its_entity() {
        // A layout bound to the orbital's entity (explicitly, or by the host
        // default) that only composes children displays no row of it.
        let mut layout = create_valid_trait();
        layout.name = "Layout".to_string();
        layout.linked_entity = Some("Item".to_string());
        let layout = with_render(layout, "@trait.Child");
        assert_eq!(init_fetch_errors(&make_orbital(vec![layout])), 0);
    }

    #[test]
    fn test_init_fetch_counts_a_whole_entity_read() {
        let mut whole = create_valid_trait();
        whole.name = "Whole".to_string();
        whole.linked_entity = Some("Item".to_string());
        let whole = with_render(whole, "@entity");
        assert_eq!(init_fetch_errors(&make_orbital(vec![whole])), 1);
    }

    fn make_datasource(name: &str) -> TraitDefinition {
        let mut t = create_valid_trait();
        t.name = name.to_string();
        t.category = Some(TraitCategory::Lifecycle);
        t.linked_entity = Some("Item".to_string());
        t.state_machine.transitions[0]
            .effects
            .get_or_insert_with(Vec::new)
            .push(SExpression::Array(vec![
                SExpression::String("fetch".to_string()),
                SExpression::String("Item".to_string()),
            ]));
        t
    }

    fn entity_defs() -> HashMap<String, EntityDefinition> {
        let mut m = HashMap::new();
        m.insert("Item".to_string(), make_entity("Item", &["id"]));
        m
    }

    #[test]
    fn test_init_fetch_fires_once_per_entity_when_nothing_fetches() {
        let orbital = make_orbital(vec![make_consumer("ViewA"), make_consumer("ViewB")]);
        let result = validate_orbital_init_fetch(&orbital, 0, &entity_defs());
        let fetch_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitInitMissingFetch)
            .collect();
        assert_eq!(
            fetch_errors.len(),
            1,
            "expected ONE entity-scoped error, got: {:?}",
            fetch_errors
        );
        assert!(fetch_errors[0].message.contains("Item"));
    }

    #[test]
    fn test_init_fetch_satisfied_by_sibling_datasource() {
        let orbital = make_orbital(vec![
            make_consumer("ViewA"),
            make_consumer("ViewB"),
            make_datasource("DataSource"),
        ]);
        let result = validate_orbital_init_fetch(&orbital, 0, &entity_defs());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitInitMissingFetch),
            "sibling datasource fetch must satisfy every consumer, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_init_fetch_skips_runtime_entities() {
        let mut defs = entity_defs();
        defs.get_mut("Item").unwrap().persistence = Some(Persistence::Runtime);
        let orbital = make_orbital(vec![make_consumer("ViewA")]);
        let result = validate_orbital_init_fetch(&orbital, 0, &defs);
        assert!(result
            .errors
            .iter()
            .all(|e| e.code != ErrorCode::TraitInitMissingFetch));
    }
}

#[cfg(test)]
mod trait_field_embed_tests {
    use super::*;
    use orbital_core_base::schema::EntityDefinition;

    fn entity() -> EntityDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Panel",
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "view", "type": "trait" }
            ]
        }))
        .expect("entity")
    }

    fn trait_with(effect: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Hold",
            "linkedEntity": "Panel",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "GO" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [effect] }]
            }
        }))
        .expect("trait")
    }

    fn embed() -> serde_json::Value {
        serde_json::json!(["render-ui", "main", { "type": "stack", "children": ["@entity.view"] }])
    }

    fn fires(effect: serde_json::Value) -> bool {
        validate_unbound_trait_field_embeds(&trait_with(effect), &entity(), "t")
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUnboundTraitKnobEmbedded)
    }

    #[test]
    fn an_unguarded_embed_of_a_trait_typed_entity_field_errors() {
        assert!(fires(embed()));
    }

    #[test]
    fn control_the_same_embed_under_an_if_on_the_field_is_clean() {
        assert!(!fires(serde_json::json!(["if", "@entity.view", embed()])));
    }

    #[test]
    fn control_a_guard_on_a_different_field_does_not_exempt_it() {
        assert!(fires(serde_json::json!(["if", "@entity.id", embed()])));
    }

    #[test]
    fn the_one_armed_when_effect_is_a_guard_too() {
        assert!(!fires(serde_json::json!(["when", "@entity.view", embed()])));
        assert!(fires(serde_json::json!(["when", "@entity.id", embed()])));
    }

    fn orbital_entity() -> EntityDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Panel",
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "app", "type": "orbital" }
            ]
        }))
        .expect("entity")
    }

    fn orbital_fires(effect: serde_json::Value) -> bool {
        validate_orbital_field_embeds(&trait_with(effect), &orbital_entity(), "t")
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderOrbitalValueEmbedded)
    }

    #[test]
    fn embedding_an_orbital_typed_field_errors_even_under_a_guard() {
        let embed = serde_json::json!(["render-ui", "main", { "type": "stack", "children": ["@entity.app"] }]);
        assert!(orbital_fires(embed.clone()));
        assert!(orbital_fires(serde_json::json!([
            "when",
            "@entity.app",
            embed
        ])));
    }

    #[test]
    fn control_reading_an_orbital_value_outside_render_ui_is_clean() {
        assert!(!orbital_fires(serde_json::json!([
            "set",
            "@entity.app",
            null
        ])));
        assert!(!orbital_fires(serde_json::json!(
            ["render-ui", "main", { "type": "typography", "content": "@entity.id" }]
        )));
    }

    #[test]
    fn control_embedding_a_non_trait_field_is_never_flagged() {
        assert!(!fires(serde_json::json!(
            ["render-ui", "main", { "type": "stack", "children": ["@entity.id"] }]
        )));
    }

    #[test]
    fn a_trait_value_object_is_a_valid_trait_typed_config_default() {
        let mut t = trait_with(serde_json::json!(["log", "x"]));
        let mut cf: orbital_core_base::schema::ConfigField =
            serde_json::from_value(serde_json::json!({ "type": "trait" })).expect("cf");
        cf.default = Some(
            serde_json::from_value(
                serde_json::json!({ "behavior": "std/std-kanban", "trait": "KanbanBoard" }),
            )
            .expect("default"),
        );
        t.config = Some(HashMap::from([("view".to_string(), cf)]));
        let r = validate_trait_definition(&t, "t", &HashSet::new());
        assert!(
            !r.errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingTraitInvalidFormat),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn control_any_other_object_is_not_a_trait_typed_default() {
        let mut t = trait_with(serde_json::json!(["log", "x"]));
        let mut cf: orbital_core_base::schema::ConfigField =
            serde_json::from_value(serde_json::json!({ "type": "trait" })).expect("cf");
        cf.default =
            Some(serde_json::from_value(serde_json::json!({ "label": "x" })).expect("default"));
        t.config = Some(HashMap::from([("view".to_string(), cf)]));
        let r = validate_trait_definition(&t, "t", &HashSet::new());
        assert!(r
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingTraitInvalidFormat));
    }
}

#[cfg(test)]
mod behavior_field_embed_tests {
    //! A `behavior`-typed field can be `none` like a `trait` field, so its
    //! render embed (which runs the program in place) must be guarded too.
    use super::*;
    use orbital_core_base::schema::EntityDefinition;

    fn entity() -> EntityDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Workspace",
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "app", "type": "behavior" }
            ]
        }))
        .expect("entity")
    }

    fn trait_with(effect: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Preview",
            "linkedEntity": "Workspace",
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "events": [{ "key": "GO" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [effect] }]
            }
        }))
        .expect("trait")
    }

    fn embed() -> serde_json::Value {
        serde_json::json!(["render-ui", "main", { "type": "stack", "children": ["@entity.app"] }])
    }

    fn fires(effect: serde_json::Value) -> bool {
        validate_unbound_trait_field_embeds(&trait_with(effect), &entity(), "t")
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUnboundTraitKnobEmbedded)
    }

    #[test]
    fn an_unguarded_embed_of_a_behavior_field_errors() {
        assert!(fires(embed()));
    }

    #[test]
    fn control_guarded_by_its_own_field_is_clean() {
        assert!(!fires(serde_json::json!(["when", "@entity.app", embed()])));
        assert!(!fires(serde_json::json!(["if", "@entity.app", embed()])));
    }

    #[test]
    fn a_presence_test_of_its_own_field_is_a_guard_too() {
        // A trait- or behavior-typed value is not a boolean, so the presence test is explicit.
        let present = serde_json::json!(["validate/required", "@entity.app"]);
        assert!(!fires(serde_json::json!(["when", present.clone(), embed()])));
        assert!(!fires(serde_json::json!(["if", present, embed()])));
        let other = serde_json::json!(["validate/required", "@entity.id"]);
        assert!(fires(serde_json::json!(["when", other, embed()])), "control: another field's test does not exempt it");
    }

    #[test]
    fn control_a_guard_on_another_field_does_not_exempt_it() {
        assert!(fires(serde_json::json!(["when", "@entity.id", embed()])));
    }

    #[test]
    fn a_behavior_field_is_not_an_orbital_embed_error() {
        assert!(
            validate_orbital_field_embeds(&trait_with(embed()), &entity(), "t")
                .errors
                .is_empty()
        );
    }

    fn transitions_from(json: serde_json::Value) -> Vec<TransitionDefinition> {
        serde_json::from_value(json).expect("transitions deserialize")
    }

    fn duplicate_hits(json: serde_json::Value) -> usize {
        let states: HashSet<&str> = HashSet::from(["running", "paused"]);
        let events: HashSet<&str> = HashSet::new();
        validate_transitions(&transitions_from(json), "t", &states, &events)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitDuplicateTransition)
            .count()
    }

    #[test]
    fn identical_guarded_arms_are_duplicates() {
        let arm = serde_json::json!({ "from": "paused", "to": "paused", "event": "INIT", "guard": ["not", "@config.autoStart"] });
        assert_eq!(duplicate_hits(serde_json::json!([arm.clone(), arm])), 1);
    }

    #[test]
    fn control_arms_with_different_guards_are_not_duplicates() {
        assert_eq!(
            duplicate_hits(serde_json::json!([
                { "from": "paused", "to": "paused", "event": "INIT", "guard": ["not", "@config.autoStart"] },
                { "from": "paused", "to": "running", "event": "INIT", "guard": "@config.autoStart" }
            ])),
            0
        );
    }

    #[test]
    fn control_one_guarded_and_one_fallback_arm_are_not_duplicates() {
        assert_eq!(
            duplicate_hits(serde_json::json!([
                { "from": "paused", "to": "paused", "event": "INIT", "guard": ["not", "@config.autoStart"] },
                { "from": "paused", "to": "running", "event": "INIT" }
            ])),
            0
        );
    }
}
