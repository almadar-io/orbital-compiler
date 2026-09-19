//! Entity reference inlining and field-merge overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::InlineError;
use crate::phases::validation::effect::{
    parse_fetch_include_literals, persist_entity_index, resolve_fetch_include_field,
};
use orbital_core::schema::{
    EntityDefinition, EntityRef, EntityRefExtends, FieldDefinition, SExpression, TraitDefinition,
    TraitReference,
};
use std::collections::HashSet;

/// Apply override fields from an `EntityRefExtends` to a resolved
/// `EntityDefinition`. Mirrors `apply_overrides_to_service` (Phase 3.5.G.A.4)
/// for entities. The merged entity carries the upstream fields with the
/// molecule's `fields` appended; on name collision the molecule's field
/// replaces the inherited one (the user wins). `name`, `persistence`, and
/// `collection` overrides directly replace the corresponding upstream value.
///
/// `FieldDefinition.name` is `Option<String>` (anonymous fields appear in
/// nested `properties`/`items` slots), so the dedup logic only compares
/// fields that have a name. Anonymous fields are passed through unchanged
/// from the upstream entity, and any anonymous fields the molecule supplies
/// are appended after the inherited ones. Inherited fields whose name
/// matches an extra field's name are dropped from the merged result before
/// the extras are appended, preserving the molecule's intent without
/// duplicating the field.
fn apply_overrides_to_entity(entity_def: &mut EntityDefinition, extends: &EntityRefExtends) {
    // Rename first so any subsequent debug output reflects the final name.
    if let Some(new_name) = &extends.name {
        entity_def.name = new_name.clone();
    }
    if let Some(p) = &extends.persistence {
        entity_def.persistence = Some(p.clone());
    }
    if let Some(c) = &extends.collection {
        entity_def.collection = Some(c.clone());
    }

    // Merge fields. Extras are appended after the inherited fields, with
    // collision-by-name resolved in favor of the extras (the user's override
    // wins). Anonymous fields on either side are passed through unchanged
    // because there is no key to dedup on.
    if let Some(extra_fields) = &extends.fields {
        let extra_names: HashSet<String> =
            extra_fields.iter().filter_map(|f| f.name.clone()).collect();

        let mut merged: Vec<FieldDefinition> =
            Vec::with_capacity(entity_def.fields.len() + extra_fields.len());
        for f in &entity_def.fields {
            match f.name.as_ref() {
                Some(fname) if extra_names.contains(fname) => {
                    // Drop the inherited field; the extras loop below
                    // will push the molecule's replacement.
                }
                _ => merged.push(f.clone()),
            }
        }
        for f in extra_fields {
            merged.push(f.clone());
        }
        entity_def.fields = merged;
    }
}

/// Auto-merge imported entity fields into the caller's entity.
///
/// For each field in the imported behavior's canonical entity, if the
/// caller's entity doesn't already have a field with that name, add it.
/// Caller's fields win on name collision (the caller can override the
/// type/default of a shared field like `id`). Anonymous fields (those
/// with `name: None`) from the import are skipped — there is no key to
/// dedup on and they could silently duplicate structure.
///
/// On an explicit rebind (`intrinsic_only == false`) a field only carries
/// when it's also in `referenced_fields` —
/// R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY: the composed trait's own
/// field usage decides what merges onto a differently-named host entity,
/// never the imported entity's whole field list (see
/// `collect_trait_referenced_entity_fields`). The no-rebind
/// `intrinsic_only` path is unaffected — `referenced_fields` is ignored
/// there, matching its existing "all intrinsic fields carry" contract.
pub(crate) fn merge_imported_entity_fields(
    caller_entity: &mut EntityDefinition,
    imported_entity: &EntityDefinition,
    // When true, carry ONLY fields marked `intrinsic` (machinery/view-state the
    // imported trait computes itself). Used for the no-rebind same-name
    // composition path (depth-propagation fix) where blindly merging every
    // imported DATA field would mask a genuinely-missing field. When false,
    // the imported field set is narrowed to `referenced_fields` instead (the
    // explicit-rebind behavior).
    intrinsic_only: bool,
    // Required DATA fields (the trait's `entity_contract.requires`). These are
    // never auto-merged: a required DATA read must be satisfied by the target
    // (present or remapped), so merging it from the source would silently mask a
    // missing field and defeat ORB_T_REBIND_MISSING_FIELDS.
    required_data: &HashSet<String>,
    // Field names the composed trait (or a hoisted synthetic child of it)
    // actually reads, writes, fetch-includes, or persists — see
    // `collect_trait_referenced_entity_fields`. Only consulted when
    // `!intrinsic_only`; ignored on the no-rebind path.
    referenced_fields: &HashSet<String>,
) {
    let existing_names: HashSet<String> = caller_entity
        .fields
        .iter()
        .filter_map(|f| f.name.clone())
        .collect();

    // The imported entity's SELF-referential relations target the imported
    // entity's own name (e.g. a thread variant's `replies : [CommentsLinearPost]`,
    // or a nested `focusedRow.replies : [EmailThreadedPost]`). Merging into the
    // caller's entity IS a linkedEntity rebind, so those self-references must be
    // retargeted to the caller entity or they dangle (ORB_E_INVALID_RELATION /
    // ORB_X_UNRESOLVED_RELATION_TARGET). Relations targeting OTHER entities are
    // left alone — only the self-reference (== imported_entity.name) is rewritten.
    let new_name = caller_entity.name.clone();
    let new_id = caller_entity.id.clone();
    let old_name = imported_entity.name.as_str();

    for field in &imported_entity.fields {
        if intrinsic_only && field.intrinsic != Some(true) {
            continue;
        }
        if let Some(ref fname) = field.name {
            // Never carry a required DATA field — unless it's intrinsic (an
            // intrinsic field is never required, but guard defensively).
            if required_data.contains(fname) && field.intrinsic != Some(true) {
                continue;
            }
            // Explicit rebind: the composed trait's own field usage gates
            // the merge (R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY).
            if !intrinsic_only && !referenced_fields.contains(fname) {
                continue;
            }
            if !existing_names.contains(fname) {
                let mut f = field.clone();
                rewrite_self_relation_target(&mut f, old_name, &new_name, new_id.as_deref());
                // Provenance: this field was ADDED to the host from the
                // imported entity, never authored by the host itself — its
                // `required: true` is the imported atom's own write contract,
                // never the host's `persist create` obligation. See
                // `required_create_field_names`.
                f.merged_from = Some(imported_entity.name.clone());
                caller_entity.fields.push(f);
            }
        }
    }
}

/// Every field of `imported_entity` that `trait_def` — or a hoisted
/// synthetic render child of it, recursively — actually reads, writes,
/// fetch-includes, or persists (R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY,
/// `docs/Almadar_Runtime_Gaps.md`). Four sources, unioned:
///
/// 1. `@entity.<field>` reads/writes across guards, transition effects, tick
///    effects, and config defaults — `TraitDefinition::referenced_entity_field_names`
///    (the canonical `orbital-core` walker; render-ui is itself an effect so
///    it's covered by the same pass, no separate scan needed).
/// 2. `fetch <imported_entity.name> { include: [...] }` relation names,
///    resolved the way `validate_fetch_include` resolves them
///    (`resolve_fetch_include_field` — the one matcher, reused not copied).
/// 3. `persist create/update <imported_entity.name> { ... }` data-literal
///    keys, read at the same array position `validate_persist` reads them
///    (`persist_entity_index`).
/// 4. The same three sets for every hoisted synthetic render child. Hoisting
///    happens in the lolo parser and the parent link isn't stored on the
///    resolved schema, so children are rediscovered structurally: walk
///    `trait_def`'s body for `@trait.<Sibling>` embeds
///    (`sibling_trait_refs_of`, the single owner of that walk) and recurse
///    into every sibling name that resolves to a real trait inside the same
///    imported behavior (`ctx.get_trait`) — a name that doesn't resolve
///    there isn't one of its children. `visited` guards a self-referential
///    embed cycle.
///
/// `entity_contract.requires` fields count as referenced by definition (a
/// required read is a read) even though `merge_imported_entity_fields`
/// separately excludes them from ever being auto-merged.
pub(crate) fn collect_trait_referenced_entity_fields(
    trait_def: &TraitDefinition,
    imported_entity: &EntityDefinition,
    alias: &str,
    ctx: &InlineContext,
    visited: &mut HashSet<String>,
) -> HashSet<String> {
    let mut fields = trait_def.referenced_entity_field_names();
    collect_fetch_persist_entity_fields(trait_def, imported_entity, &mut fields);
    if let Some(contract) = &trait_def.entity_contract {
        fields.extend(contract.requires.iter().cloned());
    }

    for sibling in super::sibling_trait_refs_of(trait_def) {
        if !visited.insert(sibling.clone()) {
            continue;
        }
        if let Ok(TraitReference::Inline(child_def)) = ctx.get_trait(alias, &sibling) {
            fields.extend(collect_trait_referenced_entity_fields(
                &child_def,
                imported_entity,
                alias,
                ctx,
                visited,
            ));
        }
    }
    fields
}

/// Walk every guard/effect in `trait_def`'s transitions, ticks, and
/// config-field defaults looking for `fetch`/`persist` calls that name
/// `entity_def` — parts 2+3 of `collect_trait_referenced_entity_fields`.
/// Scoped to `entity_def.name`: a trait that fetches or persists some OTHER
/// entity says nothing about which of THIS entity's fields it uses.
fn collect_fetch_persist_entity_fields(
    trait_def: &TraitDefinition,
    entity_def: &EntityDefinition,
    out: &mut HashSet<String>,
) {
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = &transition.guard {
            collect_fetch_persist_fields(guard, entity_def, out);
        }
        if let Some(effects) = &transition.effects {
            for effect in effects {
                collect_fetch_persist_fields(effect, entity_def, out);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            if let Some(guard) = &tick.guard {
                collect_fetch_persist_fields(guard, entity_def, out);
            }
            for effect in &tick.effects {
                collect_fetch_persist_fields(effect, entity_def, out);
            }
        }
    }
    if let Some(config) = &trait_def.config {
        for field in config.values() {
            if let Some(default) = &field.default {
                collect_fetch_persist_fields(default, entity_def, out);
            }
        }
    }
}

/// Leaf-level `fetch`/`persist` matcher, recursing through arrays/objects so
/// a call nested inside e.g. an `onSuccess` effect tree is still found.
fn collect_fetch_persist_fields(
    expr: &SExpression,
    entity_def: &EntityDefinition,
    out: &mut HashSet<String>,
) {
    if let SExpression::Array(arr) = expr {
        match arr.first() {
            Some(SExpression::String(op)) if op == "fetch" => {
                if let Some(SExpression::String(entity_name)) = arr.get(1) {
                    if entity_name == &entity_def.name {
                        if let Some(SExpression::Object(opts)) = arr.get(2) {
                            if let Some(include_expr) = opts.get("include") {
                                if let Some(literals) = parse_fetch_include_literals(include_expr) {
                                    for literal in &literals {
                                        if let Some(field_name) =
                                            resolve_fetch_include_field(entity_def, literal)
                                        {
                                            out.insert(field_name);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Some(SExpression::String(op)) if op == "persist" => {
                let entity_index = persist_entity_index(arr);
                if let Some(SExpression::String(entity_name)) = arr.get(entity_index) {
                    if entity_name == &entity_def.name {
                        if let Some(SExpression::Object(data_fields)) = arr.get(entity_index + 1) {
                            out.extend(data_fields.keys().cloned());
                        }
                    }
                }
            }
            _ => {}
        }
        for item in arr {
            collect_fetch_persist_fields(item, entity_def, out);
        }
    } else if let SExpression::Object(obj) = expr {
        for value in obj.values() {
            collect_fetch_persist_fields(value, entity_def, out);
        }
    }
}

/// Retarget a field's self-referential `relation.entity` (and its dual-carry
/// `relation.entity_id`) from `old`/imported-entity-id to `new`/`new_id`,
/// recursing into array `items` and object `properties` so nested relations
/// (e.g. an object field's `replies` relation) are rewritten too. Only relations
/// whose target equals `old` (the imported entity's own name) are changed. The
/// id sibling MUST move together with the name — leaving the imported entity's
/// id in place while renaming the target dangles a ledger-mismatched reference
/// (`ORB_ID_NAME_MISMATCH`: the id still resolves to the imported entity's own
/// ledger row, not the rebind target's).
fn rewrite_self_relation_target(
    field: &mut FieldDefinition,
    old: &str,
    new: &str,
    new_id: Option<&str>,
) {
    if let Some(rel) = field.relation.as_mut() {
        if rel.entity == old {
            rel.entity = new.to_string();
            rel.entity_id = new_id.map(str::to_string);
        }
    }
    if let Some(items) = field.items.as_mut() {
        rewrite_self_relation_target(items, old, new, new_id);
    }
    if let Some(props) = field.properties.as_mut() {
        for v in props.values_mut() {
            rewrite_self_relation_target(v, old, new, new_id);
        }
    }
}

/// Inline entity reference. Handles all three `EntityRef` variants:
///
/// - `Inline` — pass through unchanged.
/// - `Reference("Alias.entity")` — resolve via the import context and
///   substitute the upstream `Inline` entity.
/// - `Extends { reference, name?, fields?, .. }` — resolve via the import
///   context, clone the upstream `Inline` entity, then apply the rename /
///   field-merge / persistence / collection overrides via
///   `apply_overrides_to_entity`. The result is always a single
///   `EntityRef::Inline(merged_entity)` so downstream phases never see
///   `Extends`. This mirrors how `inline_traits` collapses
///   `TraitReference::Reference` to a merged `Inline`.
pub(crate) fn inline_entity(
    entity: &EntityRef,
    ctx: &InlineContext,
) -> Result<EntityRef, InlineError> {
    match entity {
        EntityRef::Inline(_) => Ok(entity.clone()),
        EntityRef::Reference(ref_str) => {
            let parsed = parse_reference(ref_str)?;
            match parsed {
                ParsedReference::Entity { alias } => ctx.get_entity(&alias),
                _ => Err(InlineError::InvalidReference(ref_str.clone())),
            }
        }
        EntityRef::Extends(extends) => {
            let parsed = parse_reference(&extends.reference)?;
            match parsed {
                ParsedReference::Entity { alias } => {
                    // V4: prefer the dual-carry `extendsId` when present —
                    // resolves to the same entity node the `extends` alias
                    // path would. Falls back to `get_entity` unchanged when
                    // absent or unmatched (pre-id imports).
                    let inherited = match extends
                        .extends_id
                        .as_deref()
                        .and_then(|id| ctx.get_entity_by_extends_id(&alias, id))
                    {
                        Some(e) => EntityRef::Inline(e),
                        None => ctx.get_entity(&alias)?,
                    };
                    match inherited {
                        EntityRef::Inline(mut entity_def) => {
                            apply_overrides_to_entity(&mut entity_def, extends);
                            Ok(EntityRef::Inline(entity_def))
                        }
                        // Defensive: `ctx.get_entity` is documented to
                        // return Inline for any successful lookup. If a
                        // future change relaxes that, surface the same
                        // EntityNotFound error so the molecule author
                        // sees a clear failure rather than a panic.
                        _ => Err(InlineError::EntityNotFound(alias)),
                    }
                }
                _ => Err(InlineError::InvalidReference(extends.reference.clone())),
            }
        }
    }
}
