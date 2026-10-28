//! Page reference inlining and call-site overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::rewrite::{rewrite_identifiers, stale_entity_rebind_subs};
use super::InlineError;
use orbital_core_base::schema::{
    ConfigField, PageAccess, PageDefinition, PageModifiers, PageRef, PageRefObject, TraitReference,
};

/// An import's explicit page modifiers over the upstream page's: each declared
/// one replaces it, absent ones inherit, and `public` over an upstream
/// `authenticated` is refused (`ORB_O_PAGE_ACCESS_WEAKENED`). Shared by page
/// references and whole-orbital `pages {}` remap entries.
pub(crate) fn apply_page_modifiers(
    page_def: &mut PageDefinition,
    mods: &PageModifiers,
    reference: &str,
    path: &str,
) -> Result<(), InlineError> {
    if mods.access == Some(PageAccess::Public) && page_def.access == Some(PageAccess::Authenticated)
    {
        return Err(InlineError::PageAccessWeakened {
            reference: reference.to_string(),
            path: path.to_string(),
        });
    }
    if mods.access.is_some() {
        page_def.access = mods.access;
    }
    if mods.indexing.is_some() {
        page_def.indexing = mods.indexing;
    }
    if mods.title.is_some() {
        page_def.title = mods.title.clone();
    }
    if mods.description.is_some() {
        page_def.description = mods.description.clone();
    }
    if mods.translation_of.is_some() {
        page_def.translation_of = mods.translation_of.clone();
    }
    if mods.label.is_some() {
        page_def.label = mods.label.clone();
    }
    if mods.icon.is_some() {
        page_def.icon = mods.icon.clone();
    }
    if mods.roles.is_some() {
        page_def.roles = mods.roles.clone();
    }
    Ok(())
}

/// Phase F.3: Apply override fields from a `PageRefObject` to a resolved
/// `PageDefinition`. Mirrors `apply_overrides_to_trait` for pages.
///
/// Argument handling:
///
/// - **`path`** — direct assignment to `page_def.path`. Phase A behavior.
/// - **`address`** — replaces the protocol page's address record at the call site.
/// - **`linked_entity`** — rebinds `page_def.primary_entity` and walks any
///   inline `TraitReference::Inline` entries in `page_def.traits` so their
///   own `linked_entity` and embedded SExpression bindings are rewritten
///   from the original entity to the caller's entity.
/// - **`traits`** — replaces `page_def.traits` entirely with the caller's
///   list. The caller takes full ownership of the page's trait set.
/// - **`access` / `indexing` / `title` / `description` / `label` / `icon` /
///   `roles`** — explicit page modifiers replace the upstream's; absent ones inherit it. `public` over an
///   upstream `authenticated` is refused (`ORB_O_PAGE_ACCESS_WEAKENED`).
fn apply_overrides_to_page(
    page_def: &mut PageDefinition,
    obj: &PageRefObject,
) -> Result<(), InlineError> {
    let mods = PageModifiers {
        access: obj.access,
        indexing: obj.indexing,
        title: obj.title.clone(),
        description: obj.description.clone(),
        translation_of: obj.translation_of.clone(),
        label: obj.label.clone(),
        icon: obj.icon.clone(),
        roles: obj.roles.clone(),
    };
    let path = obj.path.clone().unwrap_or_else(|| page_def.path.clone());
    let found = obj.protocol.as_deref().unwrap_or(orbital_core_base::protocols::ROUTE);
    if found != page_def.protocol_name() {
        return Err(InlineError::PageProtocolMismatch {
            reference: obj.reference.clone(),
            path,
            expected: page_def.protocol_name().to_string(),
            found: found.to_string(),
        });
    }
    apply_page_modifiers(page_def, &mods, &obj.reference, &path)?;
    if let Some(path_override) = &obj.path {
        page_def.path = path_override.clone();
    }
    if let Some(address_override) = &obj.address {
        page_def.address = Some(address_override.clone());
    }

    if let Some(new_entity) = &obj.linked_entity {
        let old_entity = page_def.primary_entity.clone();
        page_def.primary_entity = Some(new_entity.clone());

        // Build a single-entry substitution map for the rewrite walks
        // below. Build it eagerly even when `old_entity == new_entity` so
        // the rebound primary_entity matches what callers see.
        if let Some(orig) = old_entity {
            if &orig != new_entity {
                let mut entity_subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
                entity_subs.insert(orig, new_entity.clone());
                let event_subs: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();

                // Walk every inline trait the page declares so embedded
                // SExpression entity refs are rewritten. References (the
                // other TraitReference variant) are left alone — those are
                // resolved by the orbital-level inline pass and the
                // rewrite happens there.
                if let Some(traits) = page_def.traits.as_mut() {
                    for trait_ref in traits.iter_mut() {
                        if let TraitReference::Inline(inline_trait) = trait_ref {
                            // V4 leverage-ids: augment the page-primary rebind
                            // subs with any stale token name the trait's
                            // `entity_ref_ids` side-map ties to its bound entity
                            // id (recovers tokens that kept an old name after an
                            // entity rename). Empty side-map = `trait_subs`
                            // equals `entity_subs`, so the name path is unchanged.
                            let mut trait_subs = entity_subs.clone();
                            for (from, to) in stale_entity_rebind_subs(inline_trait, new_entity) {
                                trait_subs.entry(from).or_insert(to);
                            }
                            // Rebind the trait's linked_entity if it
                            // matched the original.
                            if let Some(le) = inline_trait.linked_entity.as_mut() {
                                if let Some(to) = trait_subs.get(le) {
                                    *le = to.clone();
                                }
                            }
                            for p in inline_trait.propositions_mut() {
                                rewrite_identifiers(p, &trait_subs, &event_subs);
                            }
                            for transition in inline_trait.state_machine.transitions.iter_mut() {
                                if let Some(guard) = transition.guard.as_mut() {
                                    rewrite_identifiers(guard, &trait_subs, &event_subs);
                                }
                                if let Some(effects) = transition.effects.as_mut() {
                                    for effect in effects.iter_mut() {
                                        rewrite_identifiers(effect, &trait_subs, &event_subs);
                                    }
                                }
                            }
                            if let Some(ticks) = inline_trait.ticks.as_mut() {
                                for tick in ticks.iter_mut() {
                                    if let Some(guard) = tick.guard.as_mut() {
                                        rewrite_identifiers(guard, &trait_subs, &event_subs);
                                    }
                                    for effect in tick.effects.iter_mut() {
                                        rewrite_identifiers(effect, &trait_subs, &event_subs);
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
    Ok(())
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
                                // V4: prefer the dual-carry `refId` when
                                // present — resolves to the same page node
                                // the alias.name path would. Falls back to
                                // `get_page` by name unchanged.
                                let inlined = match obj
                                    .ref_id
                                    .as_deref()
                                    .and_then(|id| ctx.get_page_by_ref_id(&alias, id))
                                {
                                    Some(p) => p,
                                    None => ctx.get_page(&alias, &name)?,
                                };
                                // Phase F.3: Apply path / linked_entity /
                                // traits overrides via the helper. The path
                                // override is the original Phase A behavior;
                                // linked_entity rebinds the page's entity
                                // (and rewrites @Entity bindings inside any
                                // inline traits the page declares); traits
                                // replaces the page's trait list entirely.
                                if let PageRef::Inline(mut page_def) = inlined {
                                    page_def.source_page =
                                        page_def.source_page.take().or_else(|| page_def.id.clone());
                                    apply_overrides_to_page(&mut page_def, obj)?;
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

/// Fold every `@config.<knob>` read inside a page's `address` to the value the
/// orbital's or the app's `config {}` supplies, along the same rungs a trait's
/// forwarded config walks. A knob no rung supplies stays a live `@config` read.
pub(crate) fn resolve_page_addresses(
    pages: &mut Option<Vec<PageRef>>,
    orbital_config: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
    schema_config: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
) {
    for page_ref in pages.iter_mut().flatten() {
        let PageRef::Inline(page_def) = page_ref else {
            continue;
        };
        let Some(address) = page_def.address.as_ref() else {
            continue;
        };
        if let Some(folded) =
            super::r#trait::forwarded_expression(address, orbital_config, schema_config)
        {
            page_def.address = Some(folded);
        }
    }
}
