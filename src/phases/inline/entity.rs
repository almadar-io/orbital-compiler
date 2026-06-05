//! Entity reference inlining and field-merge overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::InlineError;
use orbital_core::schema::{EntityDefinition, EntityRef, EntityRefExtends, FieldDefinition};
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
pub(crate) fn merge_imported_entity_fields(
    caller_entity: &mut EntityDefinition,
    imported_entity: &EntityDefinition,
    // When true, carry ONLY fields marked `intrinsic` (machinery/view-state the
    // imported trait computes itself). Used for the no-rebind same-name
    // composition path (depth-propagation fix) where blindly merging every
    // imported DATA field would mask a genuinely-missing field. When false the
    // full imported field set is merged (the long-standing explicit-rebind
    // behavior — unchanged for unmarked fields).
    intrinsic_only: bool,
    // Required DATA fields (the trait's `entity_contract.requires`). These are
    // never auto-merged: a required DATA read must be satisfied by the target
    // (present or remapped), so merging it from the source would silently mask a
    // missing field and defeat ORB_T_REBIND_MISSING_FIELDS.
    required_data: &HashSet<String>,
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
            if !existing_names.contains(fname) {
                let mut f = field.clone();
                rewrite_self_relation_target(&mut f, old_name, &new_name);
                caller_entity.fields.push(f);
            }
        }
    }
}

/// Retarget a field's self-referential `relation.entity` from `old` to `new`,
/// recursing into array `items` and object `properties` so nested relations
/// (e.g. an object field's `replies` relation) are rewritten too. Only relations
/// whose target equals `old` (the imported entity's own name) are changed.
fn rewrite_self_relation_target(field: &mut FieldDefinition, old: &str, new: &str) {
    if let Some(rel) = field.relation.as_mut() {
        if rel.entity == old {
            rel.entity = new.to_string();
        }
    }
    if let Some(items) = field.items.as_mut() {
        rewrite_self_relation_target(items, old, new);
    }
    if let Some(props) = field.properties.as_mut() {
        for v in props.values_mut() {
            rewrite_self_relation_target(v, old, new);
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
                    let inherited = ctx.get_entity(&alias)?;
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
