//! Entity reference inlining and field-merge overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::InlineError;
use crate::phases::validation::effect::{
    parse_fetch_include_literals, persist_entity_index, resolve_fetch_include_field,
};
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, EntityRefExtends, FieldDefinition, Orbital, SExpression,
    TraitDefinition, TraitReference,
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

/// Locate the orbital (within `orbitals`, primary checked before
/// auxiliaries, first match wins) whose entity is named `target`, and merge
/// `imported`'s fields into it via [`merge_imported_entity_fields`]. Returns
/// whether a target was found.
///
/// This is the ONE §52 rebind-merge landing-site resolver: called with a
/// single caller orbital (`std::slice::from_mut`) to try the LOCAL landing
/// first, then — if that fails — again with every orbital in the schema
/// once all of them have finished inlining (so every Gap #22 no-rebind
/// auxiliary is registered and every sibling orbital's primary entity is
/// known). There is no other landing site: a target this can't find is not
/// merged anywhere, and `ORB_T_REBIND_TARGET_UNRESOLVED`
/// (`validation::trait_validator::validate_entity_rebind`) reports it — the
/// retired behavior silently merged onto the caller's own primary entity
/// instead.
pub(crate) fn merge_into_named_entity(
    orbitals: &mut [Orbital],
    target: &str,
    imported: &EntityDefinition,
    intrinsic_only: bool,
    required_data: &HashSet<String>,
    referenced_fields: &HashSet<String>,
) -> bool {
    for orbital in orbitals.iter_mut() {
        if let EntityRef::Inline(entity) = &mut orbital.entity {
            if entity.name == target {
                merge_imported_entity_fields(
                    entity,
                    imported,
                    intrinsic_only,
                    required_data,
                    referenced_fields,
                );
                return true;
            }
        }
        if let Some(aux_list) = orbital.auxiliary_entities.as_mut() {
            for aux in aux_list.iter_mut() {
                if let EntityRef::Inline(entity) = aux {
                    if entity.name == target {
                        merge_imported_entity_fields(
                            entity,
                            imported,
                            intrinsic_only,
                            required_data,
                            referenced_fields,
                        );
                        return true;
                    }
                }
            }
        }
    }
    false
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
    let mut fields = trait_references_entity_fields(trait_def, imported_entity);
    if let Some(contract) = &trait_def.entity_contract {
        fields.extend(contract.requires.iter().cloned());
    }

    for (sibling, _) in
        super::sibling_trait_refs_of(trait_def, alias, &trait_def.name, ctx, &HashSet::new())
    {
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

/// Parts 1+2+3 of the field-usage definition for a SINGLE trait (no
/// recursion into hoisted children, no `entity_contract` — those are the
/// caller's concern): `@entity.<field>` reads/writes
/// (`TraitDefinition::referenced_entity_field_names`) unioned with
/// `fetch`/`persist` references against `entity_def`
/// (`collect_fetch_persist_entity_fields`). This is the ONE shared primitive
/// both `collect_trait_referenced_entity_fields` (pre-inline merge, unions it
/// over a composed trait + its hoisted children via `ctx`) and
/// `phases::validation::entity_mutation`'s `ORB_E_MERGED_FIELD_UNREFERENCED`
/// rule (post-inline, unions it over every trait of the final orbital bound
/// to the entity) build on — never two definitions of "referenced".
pub(crate) fn trait_references_entity_fields(
    trait_def: &TraitDefinition,
    entity_def: &EntityDefinition,
) -> HashSet<String> {
    let mut fields = trait_def.referenced_entity_field_names();
    collect_fetch_persist_entity_fields(trait_def, entity_def, &mut fields);
    fields
}

/// Fields of `entity_def` that `trait_def` references, given whether
/// `entity_def` is `trait_def`'s EFFECTIVE bound entity (`is_own_entity`,
/// supplied by the caller). Own-entity: parts 1-4 — `@entity.<field>`
/// reads/writes (`trait_references_entity_fields`, entity-agnostic by
/// construction: a `@entity.<field>` token stays the same string across any
/// rebind rename, so it's correct for the caller to say "yes, own entity"
/// even when `trait_def.linked_entity` itself still names the SOURCE
/// package's pre-rebind binding) plus `entity_contract.requires`. Not
/// own-entity: parts 2-4 only (`collect_fetch_persist_entity_fields` — a
/// fetch/persist/render-ui-pattern target naming `entity_def` literally) —
/// UNLESS `entity_def` is `trait_def`'s own RAW binding (a self-reference
/// `apply_overrides_to_trait`'s rewrite retargets under an active rebind,
/// e.g. `std-wiki-page`'s `WikiHistory` — bound to its OWN `WikiPageRevision`
/// — doing a self-refetch `(persist update WikiPageRevision …)`; std-wiki's
/// `-> WikiRevision` rebind retargets that literal token like every other
/// one in the body, so it is never evidence `WikiPageRevision` itself is
/// still needed), which contributes nothing.
///
/// THE ONE field-usage definition both `ORB_E_MERGED_FIELD_UNREFERENCED`
/// (`validation::entity_mutation::validate_all_merged_field_usage`) and the
/// §52 travel-copy filter (`inline_orbital`'s Gap #22 aux collection) build
/// on.
pub(crate) fn trait_referenced_fields_of_entity(
    trait_def: &TraitDefinition,
    entity_def: &EntityDefinition,
    is_own_entity: bool,
) -> HashSet<String> {
    if is_own_entity {
        let mut fields = trait_references_entity_fields(trait_def, entity_def);
        if let Some(contract) = &trait_def.entity_contract {
            fields.extend(contract.requires.iter().cloned());
        }
        fields
    } else if trait_def.linked_entity.as_deref() == Some(entity_def.name.as_str()) {
        HashSet::new()
    } else {
        let mut fields = HashSet::new();
        let mut matched = false;
        collect_fetch_persist_entity_fields_matched(
            trait_def,
            entity_def,
            &mut fields,
            &mut matched,
        );
        if matched && fields.is_empty() {
            // A bare `fetch X {emit:…}`/`persist … X @entity` (no
            // `include`/enumerated data) loads or writes the WHOLE row —
            // every field, not some subset we can name — so nothing can be
            // narrowed away. Every field name survives rather than none.
            fields = entity_def
                .fields
                .iter()
                .filter_map(|f| f.name.clone())
                .collect();
        }
        fields
    }
}

/// [`trait_referenced_fields_of_entity`], unioned over `trait_def` and every
/// hoisted synthetic render child of it, recursively via
/// `sibling_trait_refs_of` — a child's OWN binding to `candidate` counts as
/// own-entity for ITSELF regardless of `trait_def`'s. Returns `(references,
/// fields)`: `references` is "does this trait (or a child) reference
/// `candidate` at all" — true whenever `candidate` IS some level's own
/// binding, even one with zero field-level accesses (a trivial child), the
/// check the Gap #22 aux-travel decision uses — while `fields` is what
/// survives when `candidate`'s merged fields are filtered for travel
/// (narrower than `references`: a bare own-binding with no field accesses
/// contributes no fields, only the travel signal).
pub(crate) fn trait_referenced_fields_of_entity_recursive(
    trait_def: &TraitDefinition,
    candidate: &EntityDefinition,
    is_own_entity: bool,
    alias: &str,
    ctx: &InlineContext,
    visited: &mut HashSet<String>,
) -> (bool, HashSet<String>) {
    let mut fields = trait_referenced_fields_of_entity(trait_def, candidate, is_own_entity);
    let mut references = is_own_entity || !fields.is_empty();
    for (sibling, _) in
        super::sibling_trait_refs_of(trait_def, alias, &trait_def.name, ctx, &HashSet::new())
    {
        if !visited.insert(sibling.clone()) {
            continue;
        }
        if let Ok(TraitReference::Inline(child_def)) = ctx.get_trait(alias, &sibling) {
            let child_is_own = child_def.linked_entity.as_deref() == Some(candidate.name.as_str());
            let (child_references, child_fields) = trait_referenced_fields_of_entity_recursive(
                &child_def,
                candidate,
                child_is_own,
                alias,
                ctx,
                visited,
            );
            references = references || child_references;
            fields.extend(child_fields);
        }
    }
    (references, fields)
}

/// Walk every guard/effect in `trait_def`'s transitions, ticks, and
/// config-field defaults looking for `fetch`/`persist` calls and render-ui
/// pattern-contract nodes that name `entity_def` — parts 2+3+4 of
/// `collect_trait_referenced_entity_fields`. Scoped to `entity_def.name`: a
/// trait that fetches, persists, or renders against some OTHER entity says
/// nothing about which of THIS entity's fields it uses.
///
/// `entity_def` need NOT be `trait_def`'s own `linked_entity` — this is
/// also the cross-entity primitive `validate_all_merged_field_usage` uses
/// to find a merged-field reference living in a trait bound to a DIFFERENT
/// entity (e.g. `WikiRevisionCapture -> WikiPage` doing
/// `(persist create WikiPageRevision {...})`). `trait_def.linked_entity` is
/// threaded through separately as the render-ui fallback target (an absent
/// literal `entity:` key defaults to the trait's OWN binding, never to
/// whatever `entity_def` the caller happens to be checking).
pub(crate) fn collect_fetch_persist_entity_fields(
    trait_def: &TraitDefinition,
    entity_def: &EntityDefinition,
    out: &mut HashSet<String>,
) {
    let mut matched = false;
    collect_fetch_persist_entity_fields_matched(trait_def, entity_def, out, &mut matched);
}

/// Same walk as [`collect_fetch_persist_entity_fields`], additionally
/// setting `matched` when a `fetch`/`persist` call names `entity_def` at
/// all — even with no `include`/data fields (`fetch Entity { emit: {...} }`,
/// the common shape for a whole-row load with nothing to narrow, e.g.
/// std-notify-on-event's `fetch NotificationPreference { emit: {...} }` or
/// std-data-import's `fetch ImportRow { emit: {...} }`). `out` alone can't
/// answer "is the entity referenced AT ALL": it's field-precise by design
/// (`ORB_E_MERGED_FIELD_UNREFERENCED` wants exact field names), so a
/// legitimate whole-row fetch with nothing to include looks identical to no
/// reference at all. `matched` is the entity-EXISTENCE half of the SAME tree
/// walk (used by `trait_references_entity_by_name`) — never a second walker
/// over the same tree. The render-ui arm doesn't set `matched` independently
/// of `out` (a node whose contract genuinely requires zero fields is
/// indistinguishable from a non-match there) — a narrower, pre-existing
/// limitation of `required_entity_fields_for_node` itself, not new here.
pub(crate) fn collect_fetch_persist_entity_fields_matched(
    trait_def: &TraitDefinition,
    entity_def: &EntityDefinition,
    out: &mut HashSet<String>,
    matched: &mut bool,
) {
    let linked_entity = trait_def.linked_entity.as_deref();
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = &transition.guard {
            collect_fetch_persist_fields(guard, entity_def, linked_entity, out, matched);
        }
        if let Some(effects) = &transition.effects {
            for effect in effects {
                collect_fetch_persist_fields(effect, entity_def, linked_entity, out, matched);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            if let Some(guard) = &tick.guard {
                collect_fetch_persist_fields(guard, entity_def, linked_entity, out, matched);
            }
            for effect in &tick.effects {
                collect_fetch_persist_fields(effect, entity_def, linked_entity, out, matched);
            }
        }
    }
    if let Some(config) = &trait_def.config {
        for field in config.values() {
            if let Some(default) = &field.default {
                collect_fetch_persist_fields(default, entity_def, linked_entity, out, matched);
            }
        }
    }
}

/// Leaf-level `fetch`/`persist`/render-ui-pattern-contract matcher,
/// recursing through arrays/objects so a call or node nested inside e.g. an
/// `onSuccess` effect tree or a `children:` list is still found. The
/// render-ui arm reuses `required_entity_fields_for_node` (part 1's sibling
/// reference kind — a pattern's `requiredFields` contract is a reference an
/// `@entity.<field>` grep never sees) so this ONE recursive walk finds
/// fetch/persist targets AND pattern-contract fields in the same pass —
/// never a second walker over the same tree. `linked_entity` is the OWNING
/// trait's own bound entity (for a render-ui node's absent-`entity`-key
/// fallback), independent of `entity_def` (the entity we're checking
/// references AGAINST — see `collect_fetch_persist_entity_fields`).
fn collect_fetch_persist_fields(
    expr: &SExpression,
    entity_def: &EntityDefinition,
    linked_entity: Option<&str>,
    out: &mut HashSet<String>,
    matched: &mut bool,
) {
    if let SExpression::Object(obj) = expr {
        if let Some(SExpression::String(pattern_type)) = obj.get("type") {
            let fields =
                crate::phases::validation::render_ui::pattern::required_entity_fields_for_node(
                    pattern_type,
                    obj,
                    &entity_def.name,
                    linked_entity,
                );
            if !fields.is_empty() {
                *matched = true;
            }
            out.extend(fields);
        }
    }
    if let SExpression::Array(arr) = expr {
        match arr.first() {
            Some(SExpression::String(op)) if op == "fetch" => {
                if let Some(SExpression::String(entity_name)) = arr.get(1) {
                    if entity_name == &entity_def.name {
                        *matched = true;
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
            Some(SExpression::String(op))
                if op == "persist"
                    && matches!(arr.get(1), Some(SExpression::String(a)) if a == "batch") =>
            {
                if let Some(ops) = arr.get(2) {
                    for w in orbital_core_base::persist_batch::batch_writes(ops) {
                        if w.entity == entity_def.name {
                            *matched = true;
                            if let Some(SExpression::Object(data_fields)) = w.data {
                                out.extend(data_fields.keys().cloned());
                            }
                        }
                    }
                }
            }
            Some(SExpression::String(op)) if op == "persist" => {
                let entity_index = persist_entity_index(arr);
                if let Some(SExpression::String(entity_name)) = arr.get(entity_index) {
                    if entity_name == &entity_def.name {
                        *matched = true;
                        if let Some(SExpression::Object(data_fields)) = arr.get(entity_index + 1) {
                            out.extend(data_fields.keys().cloned());
                        }
                    }
                }
            }
            _ => {}
        }
        for item in arr {
            collect_fetch_persist_fields(item, entity_def, linked_entity, out, matched);
        }
    } else if let SExpression::Object(obj) = expr {
        for value in obj.values() {
            collect_fetch_persist_fields(value, entity_def, linked_entity, out, matched);
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
    if let Some(projection) = field.projected_from.as_mut() {
        if projection.type_name == old {
            projection.type_name = new.to_string();
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

#[cfg(test)]
mod render_ui_pattern_contract_reference_tests {
    use super::*;
    use orbital_core_base::schema::{
        FieldDefinition, FieldType, Persistence, StateDefinition, StateMachine, StringOrArray,
        TransitionDefinition,
    };

    fn entity(name: &str) -> EntityDefinition {
        EntityDefinition {
            timestamps: None,
            soft_delete: None,
            description: None,
            visual_prompt: None,
            asset_ref: None,
            store: None,
            shared: false,
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
            persistence: Some(Persistence::Persistent),
            instances: None,
            fields: vec![FieldDefinition {
                nullable: None,
                additional_properties: None,
                mock: None,
                merged_from: None,
                name: Some("id".to_string()),
                field_type: FieldType::String,
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
            }],
            id: None,
            invariants: None,
        }
    }

    fn trait_with_render_ui(linked_entity: &str, node: SExpression) -> TraitDefinition {
        let render_effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            node,
        ]);
        TraitDefinition {
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "TestTrait".to_string(),
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
                    effects: Some(vec![render_effect]),
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

    fn calendar_grid_node() -> SExpression {
        let mut obj = orbital_core_base::schema::SObject::new();
        obj.insert(
            "type".to_string(),
            SExpression::String("calendar-grid".to_string()),
        );
        // No literal "entity" key — defaults to the trait's own linked_entity,
        // matching `ui-calendar-grid.lolo`'s real render-ui node.
        obj.insert(
            "events".to_string(),
            SExpression::String("@entity".to_string()),
        );
        SExpression::Object(obj)
    }

    /// PASS: a render-ui node whose pattern (`calendar-grid`) declares
    /// `requiredFields` on a prop bound (whole-entity) to this trait's own
    /// entity — the pattern's contract is a reference `@entity.<field>`
    /// literal-grepping never sees, so it must count as referenced.
    #[test]
    fn render_ui_pattern_contract_field_counts_as_referenced() {
        let e = entity("Schedule");
        let t = trait_with_render_ui("Schedule", calendar_grid_node());
        let fields = trait_references_entity_fields(&t, &e);
        assert!(
            fields.contains("title") && fields.contains("startTime"),
            "calendar-grid's requiredFields (title, startTime) must be referenced: {:?}",
            fields
        );
    }

    /// FAIL (control): the SAME pattern node, but bound to a DIFFERENT
    /// entity name via a literal `entity: "Other"` — the requiredFields
    /// apply to `Other`, not to the entity under test, so nothing is added.
    #[test]
    fn render_ui_pattern_contract_field_not_referenced_for_a_different_bound_entity() {
        let e = entity("Schedule");
        let mut obj = match calendar_grid_node() {
            SExpression::Object(o) => o,
            _ => unreachable!(),
        };
        obj.insert(
            "entity".to_string(),
            SExpression::String("Other".to_string()),
        );
        let t = trait_with_render_ui("Schedule", SExpression::Object(obj));
        let fields = trait_references_entity_fields(&t, &e);
        assert!(
            !fields.contains("title") && !fields.contains("startTime"),
            "a node bound to a DIFFERENT entity must not contribute fields to this one: {:?}",
            fields
        );
    }
}
