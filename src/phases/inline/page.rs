//! Page reference inlining and call-site overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::rewrite::rewrite_identifiers;
use super::InlineError;
use orbital_core::schema::{PageDefinition, PageRef, PageRefObject, TraitReference};
use std::collections::HashMap;

/// Walk every page's `traits[]` array and apply a `name:` trait-rename map.
///
/// Each entry in `pages[].traits[]` is a `TraitReference`. When the entry is
/// a `Reference` whose `reference` field is a bare trait name (not a dotted
/// `Alias.traits.X` form) AND that name matches a key in `rename_map`, the
/// `reference` is rewritten to the new name. Inline trait entries on pages
/// are left alone — they don't carry the cross-orbital staleness shape.
///
/// Why this exists: when a molecule renames an imported atom's trait via
/// `{ref: "Modal.traits.ModalRecordModal", name: "ArticleAddItem"}`, the
/// imported atom's page (resolved into the molecule via inline_pages) still
/// carries `traits: [{ref: "ModalRecordModal"}]` — the page hasn't seen the
/// rename. Without this rewrite, validate fires `ORB_P_INVALID_TRAIT_REF`
/// because no trait named `ModalRecordModal` exists in the resolved orbital.
pub(crate) fn rewrite_page_trait_refs(
    pages: &mut Option<Vec<PageRef>>,
    rename_map: &HashMap<String, String>,
) {
    let Some(page_list) = pages else { return };
    for page_ref in page_list.iter_mut() {
        let PageRef::Inline(page_def) = page_ref else {
            continue;
        };
        let Some(traits) = page_def.traits.as_mut() else {
            continue;
        };
        for trait_ref in traits.iter_mut() {
            if let TraitReference::Reference {
                reference: Some(ref mut ref_str),
                ..
            } = trait_ref
            {
                // Only rewrite bare trait-name refs (no `.traits.` segment).
                // Cross-alias refs like "Modal.traits.X" were already resolved
                // by the time inline_pages ran; they shouldn't carry old
                // names through this layer.
                if !ref_str.contains('.') {
                    if let Some(new_name) = rename_map.get(ref_str) {
                        *ref_str = new_name.clone();
                    }
                }
            }
        }
    }
}

/// Phase F.3: Apply override fields from a `PageRefObject` to a resolved
/// `PageDefinition`. Mirrors `apply_overrides_to_trait` for pages.
///
/// Argument handling:
///
/// - **`path`** — direct assignment to `page_def.path`. Phase A behavior.
/// - **`linked_entity`** — rebinds `page_def.primary_entity` and walks any
///   inline `TraitReference::Inline` entries in `page_def.traits` so their
///   own `linked_entity` and embedded SExpression bindings are rewritten
///   from the original entity to the caller's entity.
/// - **`traits`** — replaces `page_def.traits` entirely with the caller's
///   list. The caller takes full ownership of the page's trait set.
fn apply_overrides_to_page(page_def: &mut PageDefinition, obj: &PageRefObject) {
    if let Some(path_override) = &obj.path {
        page_def.path = path_override.clone();
    }

    if let Some(new_entity) = &obj.linked_entity {
        let old_entity = page_def.primary_entity.clone();
        page_def.primary_entity = Some(new_entity.clone());

        // Build a single-entry substitution map for the rewrite walks
        // below. Build it eagerly even when `old_entity == new_entity` so
        // the rebound primary_entity matches what callers see.
        if let Some(orig) = old_entity {
            if &orig != new_entity {
                let mut entity_subs: HashMap<String, String> = HashMap::new();
                entity_subs.insert(orig, new_entity.clone());
                let event_subs: HashMap<String, String> = HashMap::new();

                // Walk every inline trait the page declares so embedded
                // SExpression entity refs are rewritten. References (the
                // other TraitReference variant) are left alone — those are
                // resolved by the orbital-level inline pass and the
                // rewrite happens there.
                if let Some(traits) = page_def.traits.as_mut() {
                    for trait_ref in traits.iter_mut() {
                        if let TraitReference::Inline(inline_trait) = trait_ref {
                            // Rebind the trait's linked_entity if it
                            // matched the original.
                            if let Some(le) = inline_trait.linked_entity.as_mut() {
                                if let Some(to) = entity_subs.get(le) {
                                    *le = to.clone();
                                }
                            }
                            for transition in inline_trait.state_machine.transitions.iter_mut() {
                                if let Some(guard) = transition.guard.as_mut() {
                                    rewrite_identifiers(guard, &entity_subs, &event_subs);
                                }
                                if let Some(effects) = transition.effects.as_mut() {
                                    for effect in effects.iter_mut() {
                                        rewrite_identifiers(effect, &entity_subs, &event_subs);
                                    }
                                }
                            }
                            if let Some(ticks) = inline_trait.ticks.as_mut() {
                                for tick in ticks.iter_mut() {
                                    if let Some(guard) = tick.guard.as_mut() {
                                        rewrite_identifiers(guard, &entity_subs, &event_subs);
                                    }
                                    for effect in tick.effects.iter_mut() {
                                        rewrite_identifiers(effect, &entity_subs, &event_subs);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(traits_override) = &obj.traits {
        page_def.traits = Some(traits_override.clone());
    }
}

/// Inline page references
pub(crate) fn inline_pages(
    pages: &Option<Vec<PageRef>>,
    ctx: &InlineContext,
) -> Result<Option<Vec<PageRef>>, InlineError> {
    match pages {
        None => Ok(None),
        Some(page_refs) => {
            let mut result = Vec::new();

            for page_ref in page_refs {
                match page_ref {
                    PageRef::Inline(_) => {
                        result.push(page_ref.clone());
                    }
                    PageRef::Object(obj) => {
                        let parsed = parse_reference(&obj.reference)?;
                        match parsed {
                            ParsedReference::Page { alias, name } => {
                                let inlined = ctx.get_page(&alias, &name)?;
                                // Phase F.3: Apply path / linked_entity /
                                // traits overrides via the helper. The path
                                // override is the original Phase A behavior;
                                // linked_entity rebinds the page's entity
                                // (and rewrites @Entity bindings inside any
                                // inline traits the page declares); traits
                                // replaces the page's trait list entirely.
                                if let PageRef::Inline(mut page_def) = inlined {
                                    apply_overrides_to_page(&mut page_def, obj);
                                    result.push(PageRef::Inline(page_def));
                                } else {
                                    // Still a reference (loader returned
                                    // an unresolved form). Pass through —
                                    // a later pass can resolve it. The
                                    // override fields stay attached to the
                                    // original object form.
                                    result.push(page_ref.clone());
                                }
                            }
                            _ => return Err(InlineError::InvalidReference(obj.reference.clone())),
                        }
                    }
                    PageRef::Reference(ref_str) => {
                        let parsed = parse_reference(ref_str)?;
                        match parsed {
                            ParsedReference::Page { alias, name } => {
                                let inlined = ctx.get_page(&alias, &name)?;
                                result.push(inlined);
                            }
                            _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                        }
                    }
                }
            }

            Ok(Some(result))
        }
    }
}
